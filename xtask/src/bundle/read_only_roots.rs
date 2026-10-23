//! What `0.23.0` exists for (ADR 0010): the declared read-only roots every start could already
//! name in the wire crate, published at last in the bundle and the capability snapshot.
//!
//! Every value this check expects comes from `substrate_wire`: the most roots a start may name,
//! the fact name, the mounts substrate owns, and each refusal a root can raise. The schema is held
//! to the wire crate's own rule by asking both the same questions: a start the wire crate admits is
//! one the published exec-start schema accepts, and a list over `MAX_READ_ONLY_ROOTS` is one it
//! rejects. The bundle cannot bind these through a `{"$wire": …}` marker that `render_v2.rs` does
//! not already know: that file is hashed by every bundle it rendered (AGENTS.md § *The gate*), so
//! the binding lives here, which no bundle hashes.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

/// The two start inputs that carry an exec start: the exec itself and the session that embeds it.
const STARTS: [(&str, &str); 2] = [
    ("schemas/inputs/exec-start.json", ""),
    ("schemas/inputs/pipe-session-start.json", "/properties/exec"),
];

const LIST_DEFINITION: &str = "/$defs/read-only-roots";
const ENTRY_DEFINITION: &str = "/$defs/read-only-root";
const PATH_DEFINITION: &str = "/$defs/read-only-root-path";
const START_INPUT: &str = "schemas/inputs/exec-start.json";

/// The reserved-list entry that states the wire rule two declared roots may not nest.
const OTHER_ROOTS: &str = "every-other-declared-read-only-root-mount";

/// What a snapshot without the fact means, exactly what the daemon then answers
/// (`check_read_only_roots` in `crates/substrate-daemon/src/app/operations.rs`), and nothing else.
const ABSENT_MEANS: &str =
    "a-start-naming-a-root-is-refused-exec-sandbox-unavailable-before-dispatch";

/// Every optional member the wire crate's `AppliedConfinement` serializes beside the required
/// five; each closed applied shape of an exec record has to admit all of them.
const APPLIED_MEMBERS: [&str; 6] = [
    "workspace_access",
    "read_only_roots",
    "read_only_files",
    "secret_slots",
    "workdir",
    "writable_roots",
];

/// The address every root refusal names.
const ADDRESS: &str = "read_only_roots";

pub(super) fn check(version: &str, released: &Tree, failures: &mut Vec<String>) {
    check_definitions(released, failures);
    check_starts(released, failures);
    check_start_schema_agrees(version, released, failures);
    check_facts(released, failures);
    check_applied_record(version, released, failures);
    check_vectors(version, released, failures);
    check_refusal_vectors(released, failures);
    check_client_visible_changes(released, failures);
}

/// Every mount substrate owns, which a root may not equal, lie beneath or contain.
fn owned_mounts() -> impl Iterator<Item = &'static str> {
    wire::RESERVED_MOUNTS.into_iter().chain([
        wire::EXECUTION_CAPSULE_MOUNT,
        wire::EXEC_SCRATCH_MOUNT,
        wire::APERTURE_HOSTS_PATH,
        wire::APERTURE_CA_BUNDLE_PATH,
    ])
}

/// `(code, status, class)` for each row an `x-b10x-refusals` annotation declares.
fn declared(annotation: Option<&Value>) -> BTreeSet<(String, u64, String)> {
    annotation
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                row.get("code")?.as_str()?.to_owned(),
                row.get("status")?.as_u64()?,
                row.get("class")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// The list's bound, capability and refusals, and each entry's closed `{host_path, mount}` shape
/// with every owned mount reserved, are the wire crate's.
fn check_definitions(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/common.json";
    let Some(common) = json_at(released, path, failures) else {
        return;
    };
    let Some(list) = common.pointer(LIST_DEFINITION) else {
        failures.push(format!("{path}: no read-only-roots definition"));
        return;
    };
    if list.get("maxItems") != Some(&json!(wire::MAX_READ_ONLY_ROOTS)) {
        failures.push(format!(
            "{path}: read-only-roots maxItems must be the wire bound {}",
            wire::MAX_READ_ONLY_ROOTS
        ));
    }
    if list.pointer("/items/$ref") != Some(&json!(format!("#{ENTRY_DEFINITION}"))) {
        failures.push(format!(
            "{path}: read-only-roots items are not the read-only-root definition"
        ));
    }
    if list.get("x-b10x-requires-capability") != Some(&json!(wire::EXEC_READ_ONLY_ROOTS_FACT)) {
        failures.push(format!(
            "{path}: read-only-roots does not require {}",
            wire::EXEC_READ_ONLY_ROOTS_FACT
        ));
    }
    let expected: BTreeSet<(String, u64, String)> = wire::READ_ONLY_ROOT_REFUSAL_CODES
        .iter()
        .map(|code| ((*code).to_owned(), 422, "refused".to_owned()))
        .collect();
    let published = declared(list.get("x-b10x-refusals"));
    for (code, status, class) in expected.difference(&published) {
        failures.push(format!(
            "{path}: read-only-roots does not declare {code} ({status}, {class})"
        ));
    }
    for (code, status, class) in published.difference(&expected) {
        failures.push(format!(
            "{path}: read-only-roots declares {code} ({status}, {class}), which is not a refusal \
             a declared root raises"
        ));
    }
    let Some(entry) = common.pointer(ENTRY_DEFINITION) else {
        failures.push(format!("{path}: no read-only-root definition"));
        return;
    };
    if entry.get("additionalProperties") != Some(&json!(false))
        || entry.get("required") != Some(&json!(["host_path", "mount"]))
    {
        failures.push(format!(
            "{path}: read-only-root is not the closed {{host_path, mount}} shape"
        ));
    }
    for member in ["host_path", "mount"] {
        if entry.pointer(&format!("/properties/{member}/$ref"))
            != Some(&json!(format!("#{PATH_DEFINITION}")))
        {
            failures.push(format!(
                "{path}: read-only-root {member} is not the read-only-root-path form"
            ));
        }
    }
    let listed: BTreeSet<&str> = entry
        .pointer("/properties/mount/x-b10x-not-equal-beneath-or-above")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    for owned in owned_mounts() {
        if !listed.contains(owned) {
            failures.push(format!(
                "{path}: read-only-root mount does not reserve {owned}"
            ));
        }
    }
    // The other way round: a mount the bundle reserves and the wire crate admits tells a client a
    // start is refused that the daemon serves.
    let owned: BTreeSet<&str> = owned_mounts().collect();
    for reserved in &listed {
        if *reserved != OTHER_ROOTS && !owned.contains(reserved) {
            failures.push(format!(
                "{path}: read-only-root mount reserves {reserved}, which the wire crate admits"
            ));
        }
    }
    // `validate_read_only_roots` refuses a mount named twice and two that nest; the bundle states
    // the one as `x-b10x-unique` and the other as the sibling entry of the reserved list.
    if list.get("x-b10x-unique") != Some(&json!("mount")) {
        failures.push(format!(
            "{path}: read-only-roots does not state x-b10x-unique: mount"
        ));
    }
    if !listed.contains(OTHER_ROOTS) {
        failures.push(format!(
            "{path}: read-only-root mount does not state the overlapping-mount rule \
             ({OTHER_ROOTS})"
        ));
    }
}

/// Both starts carry `read_only_roots` as the shared definition.
fn check_starts(released: &Tree, failures: &mut Vec<String>) {
    for (path, exec) in STARTS {
        let Some(input) = json_at(released, path, failures) else {
            continue;
        };
        let Some(start) = input.pointer(exec) else {
            failures.push(format!("{path}: no exec start at {exec}"));
            continue;
        };
        if start.pointer("/properties/read_only_roots/$ref")
            != Some(&json!(format!("../common.json#{LIST_DEFINITION}")))
        {
            failures.push(format!(
                "{path}: read_only_roots is not the shared definition"
            ));
        }
    }
}

/// The smallest exec start, as JSON, naming `roots` when there are any.
fn start_json(roots: Option<Value>) -> Value {
    let mut start = json!({
        "workspace": "ws_check",
        "argv": ["/usr/bin/true"],
        "env": { "allow": [], "set": {} },
        "sandbox": {
            "capability_snapshot": format!("sha256:{}", "0".repeat(64)),
            "profile": "workspace",
            "network": "none",
            "require": true
        },
        "limits": {
            "timeout_ms": 1000,
            "output_bytes": 1024,
            "processes": 1,
            "memory_bytes": 1_048_576,
            "cpu_millis": 1000
        },
        "wait": false
    });
    if let Some(roots) = roots {
        start["read_only_roots"] = roots;
    }
    start
}

/// `count` distinct roots the wire crate admits, each at its own mount.
fn roots(count: usize) -> Value {
    (0..count)
        .map(|index| json!({ "host_path": format!("/srv/root{index}"), "mount": format!("/opt/root{index}") }))
        .collect()
}

/// Whether the wire crate admits a start naming `roots`, taking its own type and its own rule.
fn wire_admits(roots: &Value) -> bool {
    serde_json::from_value::<wire::ExecStartInput>(start_json(Some(roots.clone())))
        .is_ok_and(|start| wire::validate_read_only_roots(&start.read_only_roots).is_ok())
}

/// A validator for one published schema, against the bundle's own registry.
fn validator(
    version: &str,
    released: &Tree,
    target: &str,
    failures: &mut Vec<String>,
) -> Option<jsonschema::Validator> {
    let mut registry = jsonschema::Registry::new();
    for (path, bytes) in released {
        if !path.starts_with("schemas/") {
            continue;
        }
        let Ok(mut document) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        if let Some(object) = document.as_object_mut() {
            object.remove("$id");
        }
        match registry.add(super::resource_uri(version, path), document) {
            Ok(next) => registry = next,
            Err(error) => {
                failures.push(format!("{path}: cannot register: {error}"));
                return None;
            }
        }
    }
    let registry = registry
        .prepare()
        .map_err(|error| failures.push(format!("bundle schemas do not form a registry: {error}")))
        .ok()?;
    let reference = json!({ "$ref": super::resource_uri(version, target) });
    jsonschema::draft202012::options()
        .with_registry(&registry)
        .build(&reference)
        .map_err(|error| failures.push(format!("{target}: does not compile: {error}")))
        .ok()
}

/// The published exec-start schema and the wire crate answer the same: up to the bound, every
/// path form either admits, and over the bound both refuse.
///
/// Only the lexical half is the schema's to answer. A mount that collides with an owned one, or
/// two that overlap, are rules no JSON Schema keyword states; the annotations name them and the
/// candidates below keep them out of the comparison.
fn check_start_schema_agrees(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let Some(schema) = validator(version, released, START_INPUT, failures) else {
        return;
    };
    if !schema.is_valid(&start_json(None)) {
        failures.push(format!(
            "{START_INPUT}: the smallest start without read_only_roots is rejected"
        ));
        return;
    }
    let bound = usize::try_from(wire::MAX_READ_ONLY_ROOTS).unwrap_or(usize::MAX);
    for count in [0, 1, bound, bound + 1] {
        let list = roots(count);
        let schema_accepts = schema.is_valid(&start_json(Some(list.clone())));
        let wire_accepts = wire_admits(&list);
        if schema_accepts != wire_accepts {
            failures.push(format!(
                "{START_INPUT}: a start naming {count} read-only roots is {} by the schema and {} \
                 by the wire crate",
                if schema_accepts {
                    "accepted"
                } else {
                    "rejected"
                },
                if wire_accepts { "admitted" } else { "refused" },
            ));
        }
    }
    let deep = "/d".repeat(wire::MAX_PATH_DEPTH);
    let too_deep = "/d".repeat(wire::MAX_PATH_DEPTH + 1);
    let long_component = format!("/{}", "c".repeat(300));
    for candidate in [
        "/srv",
        "/srv/toolchain",
        "/srv/a b/c",
        "/srv/..x",
        "/srv/.hidden",
        deep.as_str(),
        too_deep.as_str(),
        long_component.as_str(),
        "/",
        "",
        "srv",
        "/srv/",
        "//srv",
        "/srv//toolchain",
        "/srv/.",
        "/srv/./toolchain",
        "/srv/..",
        "/srv/../toolchain",
        "/srv/a\u{0}b",
    ] {
        for (member, list) in [
            (
                "host_path",
                json!([{ "host_path": candidate, "mount": "/opt/toolchain" }]),
            ),
            (
                "mount",
                json!([{ "host_path": "/srv/toolchain", "mount": format!("/opt{candidate}") }]),
            ),
        ] {
            let schema_accepts = schema.is_valid(&start_json(Some(list.clone())));
            let wire_accepts = wire_admits(&list);
            if schema_accepts != wire_accepts {
                failures.push(format!(
                    "{START_INPUT}: read-only-root {member} form {} {:?}, which the wire crate {}",
                    if schema_accepts { "accepts" } else { "refuses" },
                    list[0][member].as_str().unwrap_or_default(),
                    if wire_accepts { "admits" } else { "refuses" },
                ));
            }
        }
    }
}

/// The fact states the served bound, and says what its absence means.
fn check_facts(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/capability.json";
    let Some(capability) = json_at(released, path, failures) else {
        return;
    };
    let fact = capability
        .pointer("/properties/facts/properties")
        .and_then(|facts| facts.get(wire::EXEC_READ_ONLY_ROOTS_FACT));
    let published = serde_json::to_value(wire::ReadOnlyRootFacts {
        max_roots: wire::MAX_READ_ONLY_ROOTS,
    })
    .unwrap_or(Value::Null);
    if fact.and_then(|fact| fact.pointer("/properties/max_roots/const"))
        != published.get("max_roots")
        || fact.and_then(|fact| fact.get("required")) != Some(&json!(["max_roots"]))
        || fact.and_then(|fact| fact.get("additionalProperties")) != Some(&json!(false))
    {
        failures.push(format!(
            "{path}: {} must be published as the closed {{max_roots: {}}}",
            wire::EXEC_READ_ONLY_ROOTS_FACT,
            wire::MAX_READ_ONLY_ROOTS
        ));
    }
    if fact.and_then(|fact| fact.get("x-b10x-absent-means")) != Some(&json!(ABSENT_MEANS)) {
        failures.push(format!(
            "{path}: {} absence must mean {ABSENT_MEANS}, the one answer the daemon gives",
            wire::EXEC_READ_ONLY_ROOTS_FACT
        ));
    }
    let vector = "schemas/vector.json";
    if let Some(document) = json_at(released, vector, failures) {
        let mut named = false;
        visit(&document, &mut |value| {
            named |= value
                .get("properties")
                .and_then(|properties| properties.get(wire::EXEC_READ_ONLY_ROOTS_FACT))
                .is_some()
                && value.pointer("/properties/exec.namespaces").is_some();
        });
        if !named {
            failures.push(format!(
                "{vector}: a vector's machine facts cannot carry {}",
                wire::EXEC_READ_ONLY_ROOTS_FACT
            ));
        }
    }
}

fn visit(value: &Value, each: &mut impl FnMut(&Value)) {
    each(value);
    match value {
        Value::Array(items) => items.iter().for_each(|item| visit(item, each)),
        Value::Object(members) => members.values().for_each(|member| visit(member, each)),
        _ => {}
    }
}

/// Every closed object shape beneath `value`, through any depth of `oneOf`.
fn closed_shapes<'a>(name: &str, value: &'a Value, out: &mut Vec<(String, &'a Value)>) {
    if value.get("additionalProperties") == Some(&json!(false)) {
        out.push((name.to_owned(), value));
    }
    for (index, branch) in value
        .get("oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        closed_shapes(&format!("{name}.{index}"), branch, out);
    }
}

/// The exec result schemas a wire exec record has to validate against.
const EXEC_RESULTS: [&str; 2] = [
    "schemas/results/exec-get.json",
    "schemas/results/exec-start.json",
];

/// Every state an exec record publishes.
const EXEC_STATES: [&str; 5] = ["running", "exited", "cancelled", "expired", "unknown"];

/// An applied network the wire emits: none, one aperture, or the design-26 list at its bound.
fn applied_network(kind: &str) -> Value {
    let aperture = |index: u32| {
        json!({
            "bytes": { "from_destination": 7, "to_destination": 9 },
            "destination": format!("203.0.113.{}:443", index + 1),
            "mechanism": "loopback-forwarder",
            "mode": "aperture",
            "name": format!("aperture{index}"),
        })
    };
    match kind {
        "aperture" => {
            let mut one = aperture(0);
            one["max_bytes"] = json!(1_048_576);
            one
        }
        "list" => Value::Array(
            (0..wire::MAX_EGRESS_APERTURES)
                .map(|index| {
                    let mut entry = aperture(index);
                    entry["listen"] = json!(format!("127.0.0.{}:443", index + 1));
                    entry["max_bytes"] = json!(4096);
                    entry
                })
                .collect(),
        ),
        _ => json!("none"),
    }
}

/// An `AppliedConfinement` with every optional member present, each list at its wire bound.
fn applied_at_bounds(capsule: bool, network: &str) -> Value {
    let roots: Vec<Value> = (1..=wire::MAX_READ_ONLY_ROOTS)
        .map(|i| json!({ "host_path": format!("/srv/root{i}"), "mount": format!("/opt/root{i}") }))
        .collect();
    let files: Vec<Value> = (1..=wire::MAX_READ_ONLY_FILES)
        .map(|i| json!({ "host_path": format!("/etc/file{i}"), "mount": format!("/etc/file{i}") }))
        .collect();
    let slots: Vec<Value> = (0..wire::MAX_SECRET_SLOTS)
        .map(|i| {
            let fd = if i == 0 {
                wire::MIN_SECRET_SLOT_FD
            } else {
                wire::MAX_SECRET_SLOT_FD - i + 1
            };
            json!({ "slot": format!("slot_{i}"), "fd": fd })
        })
        .collect();
    let writable: Vec<Value> = (0..wire::MAX_WRITABLE_ROOTS)
        .map(|i| {
            if i % 2 == 0 {
                json!({
                    "kind": "host", "name": format!("cache{i}"),
                    "host_path": format!("/srv/cache{i}"), "mount": format!("/cache{i}"),
                })
            } else {
                json!({
                    "kind": "git", "workspace": format!("ws_second{i}"),
                    "host_path": format!("/srv/ws/second{i}"), "mount": format!("/src{i}"),
                })
            }
        })
        .collect();
    let mut record = json!({
        "capability_snapshot": format!("sha256:{}", "7".repeat(64)),
        "cgroup": "substrate/vector",
        "filesystem": if capsule {
            "workspace-rw-capsule-ro-system-ro"
        } else {
            "workspace-rw-system-ro"
        },
        "network": applied_network(network),
        "profile": "workspace",
        "workspace_access": { "mode": "scoped", "writable_subtrees": ["artifacts"] },
        "read_only_roots": roots,
        "read_only_files": files,
        "secret_slots": slots,
        "workdir": "/opt/root1/src",
        "writable_roots": writable,
    });
    if capsule {
        record["capsule"] = json!({
            "manifest_sha256": "a".repeat(64),
            "entrypoint": "/runtime/bin/tool",
            "mount": "/runtime",
            "file_count": 3,
            "total_bytes": 4096,
        });
    }
    record
}

/// The exec record the wire crate serializes in `state` around [`applied_at_bounds`]: parsed into
/// `substrate_wire::Exec` and serialized back, so it is a record the daemon can emit. `Err` names a
/// fixture the wire type does not reproduce, which would make the comparison prove nothing.
fn wire_exec_record(state: &str, capsule: bool, network: &str) -> Result<Value, String> {
    let (exit, refusal) = match state {
        "exited" => (json!({ "code": 0, "signal": null }), None),
        "cancelled" => (
            json!({ "code": null, "signal": "KILL" }),
            Some(json!({
                "class": "exhausted",
                "code": "exec.aperture-byte-limit",
                "message": "The declared aperture byte ceiling was reached.",
            })),
        ),
        "unknown" => (
            Value::Null,
            Some(json!({
                "class": "refused",
                "code": "exec.workdir-missing",
                "message": "The working directory was not there.",
            })),
        ),
        _ => (Value::Null, None),
    };
    let mut record = json!({
        "id": "ex_vector",
        "kind": "exec",
        "workspace": "ws_vector",
        "state": state,
        "observed_at": "2026-08-13T12:00:00Z",
        "requested": {
            "capability_snapshot": format!("sha256:{}", "7".repeat(64)),
            "network": "none",
            "profile": "workspace",
            "require": true,
        },
        "applied": applied_at_bounds(capsule, network),
        "exit": exit,
    });
    if let Some(refusal) = refusal {
        record["refusal"] = refusal;
    }
    let exec: wire::Exec =
        serde_json::from_value(record.clone()).map_err(|error| error.to_string())?;
    let emitted = serde_json::to_value(&exec).map_err(|error| error.to_string())?;
    if emitted == record {
        Ok(emitted)
    } else {
        Err(format!("the wire serializes {emitted}"))
    }
}

/// What the doc of [`check_applied_record`] promises, asked of the published result schemas: an
/// exec record carrying every member `AppliedConfinement` serializes, each at its wire bound, in
/// every state, with and without a capsule and for each network form, validates.
fn check_wire_exec_records(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let validators: Vec<(&str, jsonschema::Validator)> = EXEC_RESULTS
        .into_iter()
        .filter_map(|target| {
            validator(version, released, target, failures).map(|schema| (target, schema))
        })
        .collect();
    for state in EXEC_STATES {
        for capsule in [false, true] {
            for network in ["none", "aperture", "list"] {
                let name = format!("{state}, capsule {capsule}, network {network}");
                let record = match wire_exec_record(state, capsule, network) {
                    Ok(record) => record,
                    Err(error) => {
                        failures.push(format!(
                            "the {name} exec record is not one the wire emits: {error}"
                        ));
                        continue;
                    }
                };
                for (target, schema) in &validators {
                    if let Err(error) = schema.validate(&record) {
                        failures.push(format!(
                            "{target}: rejects the wire exec record ({name}) with every applied \
                             member at its bound: {error} at {}",
                            error.instance_path()
                        ));
                    }
                }
            }
        }
    }
}

/// Every closed applied confinement shape lists the bound roots, bounded by the wire's bound, and
/// admits every member the wire's `AppliedConfinement` emits, in every exec state. A closed shape
/// that omits one rejects the record of a start that used it; one that names it with a narrower
/// shape than the wire's is caught by [`check_wire_exec_records`].
fn check_applied_record(version: &str, released: &Tree, failures: &mut Vec<String>) {
    check_wire_exec_records(version, released, failures);
    let path = "schemas/resource.json";
    let Some(resource) = json_at(released, path, failures) else {
        return;
    };
    let mut shapes: Vec<(String, &Value)> = Vec::new();
    if let Some(applied) = resource.pointer("/$defs/confinement-applied") {
        closed_shapes("confinement-applied", applied, &mut shapes);
    }
    for (index, branch) in resource
        .pointer("/$defs/exec/oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(applied) = branch.pointer("/properties/applied") {
            closed_shapes(&format!("exec branch {index}"), applied, &mut shapes);
        }
    }
    if shapes.is_empty() {
        failures.push(format!("{path}: no applied confinement shape"));
    }
    for (name, shape) in &shapes {
        for member in APPLIED_MEMBERS {
            if shape.pointer(&format!("/properties/{member}")).is_none() {
                failures.push(format!(
                    "{path}: {name} is a closed applied shape without {member}, which the wire's \
                     AppliedConfinement emits"
                ));
            }
        }
    }
    for (name, shape) in shapes {
        let member = shape.pointer("/properties/read_only_roots");
        if member.and_then(|member| member.pointer("/items/$ref"))
            != Some(&json!(format!("common.json#{ENTRY_DEFINITION}")))
            || member.and_then(|member| member.get("maxItems"))
                != Some(&json!(wire::MAX_READ_ONLY_ROOTS))
        {
            failures.push(format!(
                "{path}: {name} has an applied shape without read_only_roots bounded by {}",
                wire::MAX_READ_ONLY_ROOTS
            ));
        }
    }
}

/// A vector shows the applied record: a start naming roots whose result validates against the
/// exec result schema and lists exactly the roots the request declared, as the wire's record.
fn check_vectors(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let validator = super::workdir_apertures::exec_result_validator(version, released, failures);
    let mut with_applied = false;
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        if vector.pointer("/action/operation") != Some(&json!("exec.start")) {
            continue;
        }
        let Some(result) = vector.pointer("/expected/response/body/result") else {
            continue;
        };
        let Some(applied) = result.pointer("/applied/read_only_roots") else {
            continue;
        };
        with_applied = true;
        let declared = vector.pointer("/action/request/body/input/read_only_roots");
        match serde_json::from_value::<Vec<wire::ReadOnlyRoot>>(applied.clone()) {
            Err(_) => failures.push(format!(
                "{path}: the applied read_only_roots do not parse as the wire record"
            )),
            Ok(bound) => {
                if declared != Some(applied) || wire::validate_read_only_roots(&bound).is_err() {
                    failures.push(format!(
                        "{path}: the applied read_only_roots are not exactly the roots declared"
                    ));
                }
            }
        }
        if let Some(validator) = &validator
            && let Err(error) = validator.validate(result)
        {
            failures.push(format!(
                "{path}: the exec.start result does not validate against its result schema: \
                 {error}"
            ));
        }
    }
    if !with_applied {
        failures.push("vectors: no exec.start vector shows applied read-only roots".to_owned());
    }
}

/// ADR 0010 § *Consequences*: the refusals need vectors for path escape, mount collision, a
/// non-directory and the bound. Each published root refusal is answered by an `exec.start` vector,
/// a 422 `refused` at `read_only_roots`; the lexical ones between them break each of the three
/// rules the wire crate states, judged by the wire crate's own validation of the declared list,
/// and the host ones declare a list the wire crate admits, so only the host can refuse it.
fn check_refusal_vectors(released: &Tree, failures: &mut Vec<String>) {
    let mut answered: BTreeSet<&str> = BTreeSet::new();
    let mut lexical: BTreeSet<&'static str> = BTreeSet::new();
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        if vector.pointer("/action/operation") != Some(&json!("exec.start")) {
            continue;
        }
        let Some(code) = vector
            .pointer("/expected/response/body/error/code")
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(code) = wire::READ_ONLY_ROOT_REFUSAL_CODES
            .into_iter()
            .find(|known| *known == code)
        else {
            continue;
        };
        answered.insert(code);
        let error = vector.pointer("/expected/response/body/error");
        if vector.pointer("/expected/response/status") != Some(&json!(422))
            || error.and_then(|error| error.get("class")) != Some(&json!("refused"))
            || error.and_then(|error| error.get("address")) != Some(&json!(ADDRESS))
        {
            failures.push(format!(
                "{path}: {code} must be answered 422 refused at {ADDRESS}"
            ));
        }
        let declared = vector
            .pointer("/action/request/body/input/read_only_roots")
            .cloned()
            .and_then(|roots| serde_json::from_value::<Vec<wire::ReadOnlyRoot>>(roots).ok());
        let Some(declared) = declared else {
            failures.push(format!(
                "{path}: {code} is answered for a start that declares no read-only roots"
            ));
            continue;
        };
        let validated = wire::validate_read_only_roots(&declared);
        let message = match (code, &validated) {
            (wire::READ_ONLY_ROOT_INVALID, Err(error)) => {
                Some(wire::read_only_root_invalid_message(error))
            }
            (wire::READ_ONLY_ROOT_ABSENT, _) => {
                Some(wire::READ_ONLY_ROOT_ABSENT_MESSAGE.to_owned())
            }
            (wire::READ_ONLY_ROOT_NOT_A_DIRECTORY, _) => {
                Some(wire::READ_ONLY_ROOT_NOT_A_DIRECTORY_MESSAGE.to_owned())
            }
            _ => None,
        };
        if let Some(message) = message
            && error.and_then(|error| error.get("message")) != Some(&json!(message))
        {
            failures.push(format!(
                "{path}: the {code} message is not the one the daemon or host sends, {message:?}"
            ));
        }
        match (code == wire::READ_ONLY_ROOT_INVALID, validated) {
            (true, Err(wire::WireValidationError::InvalidReadOnlyRootBounds)) => {
                lexical.insert("the bound");
            }
            (true, Err(wire::WireValidationError::InvalidReadOnlyRootPath)) => {
                lexical.insert("path escape");
            }
            (true, Err(wire::WireValidationError::ReservedReadOnlyRootMount)) => {
                lexical.insert("mount collision");
            }
            (true, Err(_)) | (false, Ok(())) => {}
            (true, Ok(())) => failures.push(format!(
                "{path}: {code} is answered for roots the wire crate admits"
            )),
            (false, Err(error)) => failures.push(format!(
                "{path}: {code} is a host answer for roots the wire crate already refuses: {error}"
            )),
        }
    }
    for code in wire::READ_ONLY_ROOT_REFUSAL_CODES {
        if !answered.contains(code) {
            failures.push(format!(
                "vectors: no exec.start vector answers the read-only-root refusal {code}"
            ));
        }
    }
    for rule in ["path escape", "mount collision", "the bound"] {
        if !lexical.contains(rule) {
            failures.push(format!(
                "vectors: no {} vector breaks the read-only-root rule {rule}",
                wire::READ_ONLY_ROOT_INVALID
            ));
        }
    }
}

fn check_client_visible_changes(released: &Tree, failures: &mut Vec<String>) {
    let path = "compatibility.json";
    let Some(compatibility) = json_at(released, path, failures) else {
        return;
    };
    let changes: Vec<&str> = compatibility
        .get("client_visible_changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let bound = wire::MAX_READ_ONLY_ROOTS.to_string();
    let named = [
        wire::EXEC_READ_ONLY_ROOTS_FACT,
        "read_only_roots",
        "max_roots",
        "0.22.0",
        bound.as_str(),
    ]
    .into_iter()
    .chain(wire::READ_ONLY_ROOT_REFUSAL_CODES);
    for named in named {
        if !changes.iter().any(|change| change.contains(named)) {
            failures.push(format!("{path}: client_visible_changes must state {named}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::repo;

    const VERSION: &str = "0.23.0";

    fn released(version: &str) -> super::Tree {
        super::super::tree_of(
            &repo::root()
                .expect("root")
                .join("contracts/substrate-wire")
                .join(version),
        )
        .expect("the bundle reads")
    }

    fn failures_of(tree: &super::Tree) -> Vec<String> {
        let mut failures = Vec::new();
        super::super::check_additions(VERSION, tree, &mut failures);
        failures
    }

    fn edited(path: &str, edit: impl FnOnce(&mut serde_json::Value)) -> super::Tree {
        let mut tree = released(VERSION);
        let bytes = tree.get(path).expect("the document is in the bundle");
        let mut document: serde_json::Value = serde_json::from_slice(bytes).expect("parses");
        edit(&mut document);
        tree.insert(
            path.to_owned(),
            serde_json::to_vec(&document).expect("serializes"),
        );
        tree
    }

    fn assert_reported(failures: &[String], named: &str) {
        assert!(
            failures.iter().any(|failure| failure.contains(named)),
            "not reported: {named}: {failures:?}"
        );
    }

    #[test]
    fn the_successor_carries_read_only_roots() {
        let failures = failures_of(&released(VERSION));
        assert!(failures.is_empty(), "{failures:?}");
    }

    /// A check that passes on the tree that predates the change proves nothing about the tree that
    /// carries it.
    #[test]
    fn the_predecessor_carries_none_of_it() {
        let failures = failures_of(&released("0.22.0"));
        for named in [
            "schemas/common.json: no read-only-roots definition",
            "schemas/inputs/exec-start.json: read_only_roots is not the shared definition",
            "schemas/inputs/pipe-session-start.json: read_only_roots is not the shared definition",
            "exec.read-only-roots must be published as the closed {max_roots: 4}",
            "a vector's machine facts cannot carry exec.read-only-roots",
            "has an applied shape without read_only_roots bounded by 4",
            "no exec.start vector shows applied read-only roots",
            "client_visible_changes must state exec.read-only-roots",
        ] {
            assert_reported(&failures, named);
        }
    }

    /// Acceptance 1: the published start rejects a list longer than the wire bound, which the
    /// wire crate refuses too. One bound moved in the bundle alone is reported by count.
    #[test]
    fn a_start_naming_more_roots_than_the_wire_bound_is_rejected() {
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["read-only-roots"]["maxItems"] = 5.into();
        });
        let failures = failures_of(&tree);
        assert_reported(
            &failures,
            "read-only-roots maxItems must be the wire bound 4",
        );
        assert_reported(
            &failures,
            "a start naming 5 read-only roots is accepted by the schema and refused by the wire \
             crate",
        );
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["read-only-roots"]["maxItems"] = 3.into();
        });
        assert_reported(
            &failures_of(&tree),
            "a start naming 4 read-only roots is rejected by the schema and admitted by the wire \
             crate",
        );
    }

    /// A path form stricter or looser than the wire rule is reported with the path it disagrees on.
    #[test]
    fn a_path_form_that_is_not_the_wire_rule_is_reported() {
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["read-only-root-path"]["pattern"] = "^/.*$".into();
        });
        assert_reported(
            &failures_of(&tree),
            "read-only-root host_path form accepts \"/srv/../toolchain\", which the wire crate \
             refuses",
        );
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["read-only-root-path"]["maxLength"] = 255.into();
        });
        assert_reported(&failures_of(&tree), "read-only-root host_path form refuses");
    }

    /// An owned mount the schema forgets, or a refusal it adds or drops, is reported by name.
    #[test]
    fn a_reserved_mount_or_refusal_the_schema_misstates_is_reported() {
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["read-only-root"]["properties"]["mount"]
                ["x-b10x-not-equal-beneath-or-above"]
                .as_array_mut()
                .expect("array")
                .retain(|path| path != "/runtime");
        });
        assert_reported(
            &failures_of(&tree),
            "read-only-root mount does not reserve /runtime",
        );
        for code in super::wire::READ_ONLY_ROOT_REFUSAL_CODES {
            let tree = edited("schemas/common.json", |document| {
                document["$defs"]["read-only-roots"]["x-b10x-refusals"]
                    .as_array_mut()
                    .expect("array")
                    .retain(|row| row["code"] != code);
            });
            assert_reported(
                &failures_of(&tree),
                &format!("read-only-roots does not declare {code} (422, refused)"),
            );
        }
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["read-only-roots"]["x-b10x-refusals"]
                .as_array_mut()
                .expect("array")
                .push(serde_json::json!({
                    "class": "unserved",
                    "code": "exec.read-only-roots-unserved",
                    "status": 501
                }));
        });
        assert_reported(
            &failures_of(&tree),
            "read-only-roots declares exec.read-only-roots-unserved (501, unserved)",
        );
    }

    /// Acceptance 2: the fact's bound is the wire constant.
    #[test]
    fn a_fact_bound_that_is_not_the_wire_constant_is_reported() {
        let tree = edited("schemas/capability.json", |document| {
            document["properties"]["facts"]["properties"]["exec.read-only-roots"]["properties"]["max_roots"]
                ["const"] = 8.into();
        });
        assert_reported(
            &failures_of(&tree),
            "exec.read-only-roots must be published as the closed {max_roots: 4}",
        );
    }

    /// Acceptance 3: an applied shape that cannot list the roots, or a vector whose applied record
    /// is not the declared list, is reported.
    #[test]
    fn an_applied_record_that_drops_a_root_is_reported() {
        let tree = edited("schemas/resource.json", |document| {
            document["$defs"]["exec"]["oneOf"][1]["properties"]["applied"]["oneOf"][0]
                ["properties"]
                .as_object_mut()
                .expect("properties")
                .remove("read_only_roots");
        });
        assert_reported(
            &failures_of(&tree),
            "exec branch 1.0 has an applied shape without read_only_roots",
        );
        let tree = edited(
            "vectors/http/read-only-roots-applied-are-observed.json",
            |document| {
                document["expected"]["response"]["body"]["result"]["applied"]["read_only_roots"]
                    .as_array_mut()
                    .expect("an applied list")
                    .pop();
            },
        );
        assert_reported(
            &failures_of(&tree),
            "the applied read_only_roots are not exactly the roots declared",
        );
        let tree = edited(
            "vectors/http/read-only-roots-applied-are-observed.json",
            |document| {
                for pointer in [
                    "/expected/response/body/result/applied/read_only_roots/0",
                    "/action/request/body/input/read_only_roots/0",
                ] {
                    document
                        .pointer_mut(pointer)
                        .and_then(serde_json::Value::as_object_mut)
                        .expect("a root")
                        .insert("writable".to_owned(), true.into());
                }
            },
        );
        let failures = failures_of(&tree);
        assert_reported(
            &failures,
            "the applied read_only_roots do not parse as the wire record",
        );
        assert_reported(
            &failures,
            "the exec.start result does not validate against its result schema",
        );
    }

    /// The reserved list holds the owned mounts and nothing the wire admits, and the duplicate and
    /// overlap rules are both still stated.
    #[test]
    fn a_reserved_mount_the_wire_admits_or_a_dropped_mount_rule_is_reported() {
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["read-only-root"]["properties"]["mount"]
                ["x-b10x-not-equal-beneath-or-above"]
                .as_array_mut()
                .expect("array")
                .insert(0, "/opt".into());
        });
        assert_reported(
            &failures_of(&tree),
            "read-only-root mount reserves /opt, which the wire crate admits",
        );
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["read-only-roots"]
                .as_object_mut()
                .expect("list")
                .remove("x-b10x-unique");
        });
        assert_reported(
            &failures_of(&tree),
            "read-only-roots does not state x-b10x-unique: mount",
        );
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["read-only-root"]["properties"]["mount"]
                ["x-b10x-not-equal-beneath-or-above"]
                .as_array_mut()
                .expect("array")
                .retain(|entry| entry != super::OTHER_ROOTS);
        });
        assert_reported(
            &failures_of(&tree),
            "read-only-root mount does not state the overlapping-mount rule",
        );
    }

    /// The fact's absence names the one answer the daemon gives, and no other.
    #[test]
    fn an_absence_that_is_not_the_daemon_answer_is_reported() {
        let tree = edited("schemas/capability.json", |document| {
            document["properties"]["facts"]["properties"]["exec.read-only-roots"]["x-b10x-absent-means"] =
                "every-declared-read-only-root-is-unserved-by-name".into();
        });
        assert_reported(
            &failures_of(&tree),
            "exec.read-only-roots absence must mean a-start-naming-a-root-is-refused-exec-sandbox-\
             unavailable-before-dispatch",
        );
    }

    /// A closed applied shape in any exec state that drops a member the wire emits is reported,
    /// through the nested `oneOf` of the expired and unknown states too.
    #[test]
    fn an_applied_shape_without_an_emitted_member_is_reported() {
        for (branch, pointer) in [
            (
                "exec branch 1.0",
                "/$defs/exec/oneOf/1/properties/applied/oneOf/0/properties",
            ),
            (
                "exec branch 3.0.1",
                "/$defs/exec/oneOf/3/properties/applied/oneOf/0/oneOf/1/properties",
            ),
        ] {
            for member in super::APPLIED_MEMBERS {
                let tree = edited("schemas/resource.json", |document| {
                    document
                        .pointer_mut(pointer)
                        .and_then(serde_json::Value::as_object_mut)
                        .expect("properties")
                        .remove(member);
                });
                assert_reported(
                    &failures_of(&tree),
                    &format!("{branch} is a closed applied shape without {member}"),
                );
            }
        }
    }

    /// ADR 0010's refusal vectors: each code is answered, the lexical ones break each rule, and a
    /// host answer for a list the wire already refuses is reported.
    #[test]
    fn a_missing_or_misdirected_refusal_vector_is_reported() {
        for (file, named) in [
            (
                "vectors/http/read-only-root-absent-is-refused.json",
                "no exec.start vector answers the read-only-root refusal exec.read-only-root-absent",
            ),
            (
                "vectors/http/read-only-root-not-a-directory-is-refused.json",
                "no exec.start vector answers the read-only-root refusal \
                 exec.read-only-root-not-a-directory",
            ),
            (
                "vectors/http/read-only-roots-five-is-refused.json",
                "no exec.read-only-root-invalid vector breaks the read-only-root rule the bound",
            ),
            (
                "vectors/http/read-only-root-path-escape-is-refused.json",
                "no exec.read-only-root-invalid vector breaks the read-only-root rule path escape",
            ),
            (
                "vectors/http/read-only-root-mount-collision-is-refused.json",
                "no exec.read-only-root-invalid vector breaks the read-only-root rule mount \
                 collision",
            ),
        ] {
            let mut tree = released(VERSION);
            tree.remove(file).expect("the vector is in the bundle");
            assert_reported(&failures_of(&tree), named);
        }
        let tree = edited(
            "vectors/http/read-only-root-absent-is-refused.json",
            |document| {
                document["action"]["request"]["body"]["input"]["read_only_roots"][0]["mount"] =
                    "/workspace/toolchain".into();
            },
        );
        assert_reported(
            &failures_of(&tree),
            "exec.read-only-root-absent is a host answer for roots the wire crate already refuses",
        );
        let tree = edited(
            "vectors/http/read-only-root-path-escape-is-refused.json",
            |document| {
                document["expected"]["response"]["status"] = 501.into();
            },
        );
        assert_reported(
            &failures_of(&tree),
            "exec.read-only-root-invalid must be answered 422 refused at read_only_roots",
        );
    }
}
