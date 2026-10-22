//! What `0.22.0` exists for (ADR 0038): writable roots outside `/workspace`, each a host root the
//! operator declared by name or a Git checkout workspace of the caller.
//!
//! Every value this check expects comes from `substrate_wire`: the most writable roots a start may
//! name, the mount byte bound, the reserved mounts, both fact names and each new refusal's code,
//! status, class and message. A bound that moves in the crate and not in the bundle, or a refusal
//! the crate emits that no vector answers with its exact message, fails here. The bundle cannot
//! bind these through a `{"$wire": …}` marker: `render_v2.rs` is hashed by every bundle it rendered
//! (AGENTS.md § *The gate*), so the binding lives in this file, which no bundle hashes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

/// The two start inputs that carry an exec start: the exec itself and the session that embeds it.
const STARTS: [(&str, &str); 2] = [
    ("schemas/inputs/exec-start.json", ""),
    ("schemas/inputs/pipe-session-start.json", "/properties/exec"),
];

const LIST_DEFINITION: &str = "/$defs/writable-roots";
const MOUNT_DEFINITION: &str = "/$defs/writable-root-mount";
const APPLIED_DEFINITION: &str = "/$defs/applied-writable-roots";

pub(super) fn check(version: &str, released: &Tree, failures: &mut Vec<String>) {
    check_definitions(released, failures);
    check_starts(released, failures);
    check_facts(released, failures);
    check_applied_record(released, failures);
    check_vectors(version, released, failures);
    check_client_visible_changes(released, failures);
}

fn class_of(class: wire::ErrorClass) -> String {
    serde_json::to_value(class)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `(code, status, class)` for each row of the wire refusal table.
fn table() -> BTreeSet<(String, u64, String)> {
    wire::WRITABLE_ROOT_REFUSALS
        .iter()
        .map(|refusal| {
            (
                refusal.code.to_owned(),
                u64::from(refusal.status),
                class_of(refusal.class),
            )
        })
        .collect()
}

/// `(code, status, class)` for each row an `x-b10x-refusals` annotation declares.
fn declared(
    annotation: Option<&Value>,
    path: &str,
    failures: &mut Vec<String>,
) -> BTreeSet<(String, u64, String)> {
    let mut rows = BTreeSet::new();
    for row in annotation.and_then(Value::as_array).into_iter().flatten() {
        let (Some(code), Some(status), Some(class)) = (
            row.get("code").and_then(Value::as_str),
            row.get("status").and_then(Value::as_u64),
            row.get("class").and_then(Value::as_str),
        ) else {
            failures.push(format!(
                "{path}: a declared refusal lacks code, status or class"
            ));
            continue;
        };
        rows.insert((code.to_owned(), status, class.to_owned()));
    }
    rows
}

/// The list's bound, capabilities and refusals, the mount's form and reserved paths, and each
/// entry's two closed kinds are the wire crate's.
fn check_definitions(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/common.json";
    let Some(common) = json_at(released, path, failures) else {
        return;
    };
    let Some(list) = common.pointer(LIST_DEFINITION) else {
        failures.push(format!("{path}: no writable-roots definition"));
        return;
    };
    if list.get("maxItems") != Some(&json!(wire::MAX_WRITABLE_ROOTS)) {
        failures.push(format!(
            "{path}: writable-roots maxItems must be the wire bound {}",
            wire::MAX_WRITABLE_ROOTS
        ));
    }
    if list.get("x-b10x-requires-capability")
        != Some(&json!([
            wire::EXEC_WRITABLE_ROOTS_HOST_FACT,
            wire::EXEC_WRITABLE_ROOTS_GIT_FACT
        ]))
    {
        failures.push(format!(
            "{path}: writable-roots does not require {} and {}",
            wire::EXEC_WRITABLE_ROOTS_HOST_FACT,
            wire::EXEC_WRITABLE_ROOTS_GIT_FACT
        ));
    }
    let expected = table();
    let published = declared(list.get("x-b10x-refusals"), path, failures);
    for (code, status, class) in expected.difference(&published) {
        failures.push(format!(
            "{path}: writable-roots does not declare {code} ({status}, {class})"
        ));
    }
    for (code, status, class) in published.difference(&expected) {
        failures.push(format!(
            "{path}: writable-roots declares {code} ({status}, {class}), which is not a refusal \
             the wire crate emits there"
        ));
    }
    check_mount(&common, path, failures);
    check_kinds(&common, path, failures);
}

/// The mount is the host-absolute-path form, bounded by the wire's byte bound, and names every
/// reserved path the wire refuses other than `/`, which the form itself excludes.
fn check_mount(common: &Value, path: &str, failures: &mut Vec<String>) {
    let Some(mount) = common.pointer(MOUNT_DEFINITION) else {
        failures.push(format!("{path}: no writable-root-mount definition"));
        return;
    };
    if mount.get("$ref") != Some(&json!("#/$defs/host-absolute-path")) {
        failures.push(format!(
            "{path}: writable-root-mount is not the host-absolute-path form"
        ));
    }
    if common.pointer("/$defs/host-absolute-path/maxLength")
        != Some(&json!(wire::MAX_WRITABLE_ROOT_MOUNT_BYTES))
    {
        failures.push(format!(
            "{path}: host-absolute-path maxLength must be the wire bound {}",
            wire::MAX_WRITABLE_ROOT_MOUNT_BYTES
        ));
    }
    let listed: BTreeSet<&str> = mount
        .get("x-b10x-not-equal-beneath-or-above")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    for reserved in wire::WRITABLE_ROOT_RESERVED_MOUNTS {
        if reserved != "/" && !listed.contains(reserved) {
            failures.push(format!(
                "{path}: writable-root-mount does not reserve {reserved}"
            ));
        }
    }
    check_pattern_agrees(common, path, failures);
}

/// The host-absolute-path pattern and the wire crate's mount rule accept and refuse the same
/// paths. Every candidate is outside each reserved path, so only the form decides it.
fn check_pattern_agrees(common: &Value, path: &str, failures: &mut Vec<String>) {
    let Some(pattern) = common
        .pointer("/$defs/host-absolute-path/pattern")
        .and_then(Value::as_str)
    else {
        failures.push(format!("{path}: host-absolute-path has no pattern"));
        return;
    };
    let Ok(validator) = jsonschema::validator_for(&json!({ "type": "string", "pattern": pattern }))
    else {
        failures.push(format!(
            "{path}: host-absolute-path pattern does not compile"
        ));
        return;
    };
    for candidate in [
        "/opt",
        "/opt/cache",
        "/opt/a b/c",
        "/opt/..x",
        "/opt/.hidden",
        "/",
        "",
        "opt",
        "opt/cache",
        "/opt/",
        "//opt",
        "/opt//cache",
        "/opt/.",
        "/opt/./cache",
        "/opt/..",
        "/opt/../cache",
        "/opt/a\u{0}b",
    ] {
        let schema_accepts = validator.is_valid(&json!(candidate));
        let wire_accepts = !matches!(
            wire::validate_writable_roots(&start_with(&json!([
                { "host": "cache", "mount": candidate }
            ]))),
            Err(wire::WireValidationError::WritableRootMountInvalid)
        );
        if schema_accepts != wire_accepts {
            failures.push(format!(
                "{path}: writable-root-mount form {} {candidate:?}, which the wire crate {}",
                if schema_accepts { "accepts" } else { "refuses" },
                if wire_accepts { "accepts" } else { "refuses" },
            ));
        }
    }
}

/// Each entry is exactly a closed `{host, mount}` or `{workspace, mount}`, as the wire union is.
fn check_kinds(common: &Value, path: &str, failures: &mut Vec<String>) {
    let kinds: Vec<BTreeSet<String>> = common
        .pointer("/$defs/writable-root/oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|kind| kind.get("additionalProperties") == Some(&json!(false)))
        .map(|kind| {
            kind.get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .collect();
    for (kind, members) in [("host", ["host", "mount"]), ("git", ["workspace", "mount"])] {
        let members: BTreeSet<String> = members.into_iter().map(str::to_owned).collect();
        if !kinds.contains(&members) {
            failures.push(format!(
                "{path}: writable-root has no closed {kind} kind requiring {members:?}"
            ));
        }
    }
    if kinds.len() != 2 {
        failures.push(format!(
            "{path}: writable-root must be exactly two closed kinds, not {}",
            kinds.len()
        ));
    }
    for (entry, kind) in [
        (json!({ "host": "cache", "mount": "/opt/cache" }), "host"),
        (
            json!({ "workspace": "ws_checkout", "mount": "/opt/checkout" }),
            "git",
        ),
    ] {
        if serde_json::from_value::<wire::WritableRoot>(entry).is_err() {
            failures.push(format!("the wire crate does not admit the {kind} kind"));
        }
    }
}

/// The smallest exec start that names `writable_roots`.
fn start_with(roots: &Value) -> wire::ExecStartInput {
    serde_json::from_value(json!({
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
        "wait": false,
        "writable_roots": roots
    }))
    .expect("the smallest exec start parses")
}

/// Both starts carry `writable_roots` as the shared definition.
fn check_starts(released: &Tree, failures: &mut Vec<String>) {
    for (path, exec) in STARTS {
        let Some(input) = json_at(released, path, failures) else {
            continue;
        };
        let Some(start) = input.pointer(exec) else {
            failures.push(format!("{path}: no exec start at {exec}"));
            continue;
        };
        if start.pointer("/properties/writable_roots/$ref")
            != Some(&json!(format!("../common.json#{LIST_DEFINITION}")))
        {
            failures.push(format!(
                "{path}: writable_roots is not the shared definition"
            ));
        }
    }
}

/// The host fact lists root names; the Git fact is the constant true; both say what absence means.
fn check_facts(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/capability.json";
    let Some(capability) = json_at(released, path, failures) else {
        return;
    };
    let facts = capability.pointer("/properties/facts/properties");
    let host = facts.and_then(|facts| facts.get(wire::EXEC_WRITABLE_ROOTS_HOST_FACT));
    if host.and_then(|fact| fact.get("type")) != Some(&json!("array"))
        || host.and_then(|fact| fact.pointer("/items/pattern"))
            != Some(&json!("^[A-Za-z0-9_][A-Za-z0-9_-]*$"))
    {
        failures.push(format!(
            "{path}: {} must be published as a list of root names",
            wire::EXEC_WRITABLE_ROOTS_HOST_FACT
        ));
    }
    let git = facts.and_then(|facts| facts.get(wire::EXEC_WRITABLE_ROOTS_GIT_FACT));
    if git.and_then(|fact| fact.get("const")) != Some(&json!(true)) {
        failures.push(format!(
            "{path}: {} must be published as the constant true",
            wire::EXEC_WRITABLE_ROOTS_GIT_FACT
        ));
    }
    for (name, fact) in [
        (wire::EXEC_WRITABLE_ROOTS_HOST_FACT, host),
        (wire::EXEC_WRITABLE_ROOTS_GIT_FACT, git),
    ] {
        if fact
            .and_then(|fact| fact.get("x-b10x-absent-means"))
            .is_none()
        {
            failures.push(format!("{path}: {name} must say what its absence means"));
        }
    }
}

/// Every applied shape of every exec branch may list its bound writable roots, each with its
/// kind, its name or workspace id, the resolved host path and its mount.
fn check_applied_record(released: &Tree, failures: &mut Vec<String>) {
    let common_path = "schemas/common.json";
    if let Some(common) = json_at(released, common_path, failures) {
        match common.pointer(APPLIED_DEFINITION) {
            None => failures.push(format!(
                "{common_path}: no applied-writable-roots definition"
            )),
            Some(applied) => {
                if applied.get("maxItems") != Some(&json!(wire::MAX_WRITABLE_ROOTS)) {
                    failures.push(format!(
                        "{common_path}: applied-writable-roots maxItems must be the wire bound {}",
                        wire::MAX_WRITABLE_ROOTS
                    ));
                }
            }
        }
    }
    let path = "schemas/resource.json";
    let Some(resource) = json_at(released, path, failures) else {
        return;
    };
    let mut applied_shapes = 0;
    for (index, branch) in resource
        .pointer("/$defs/exec/oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        for applied in branch
            .pointer("/properties/applied/oneOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            applied_shapes += 1;
            if applied.pointer("/properties/writable_roots/$ref")
                != Some(&json!(format!("common.json#{APPLIED_DEFINITION}")))
            {
                failures.push(format!(
                    "{path}: exec branch {index} has an applied shape without writable_roots"
                ));
            }
        }
    }
    if applied_shapes == 0 {
        failures.push(format!("{path}: no exec branch has an applied shape"));
    }
}

/// Every new refusal is answered by a vector with the wire's exact status, class and the message
/// the wire renders for that vector's own request against that vector's own facts; and the
/// applied record is shown by a vector whose result validates against the exec result schema and
/// parses as the wire's applied writable roots.
fn check_vectors(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let validator = super::workdir_apertures::exec_result_validator(version, released, failures);
    let expected: BTreeMap<&str, &wire::ExecStartRefusal> = wire::WRITABLE_ROOT_REFUSALS
        .iter()
        .map(|refusal| (refusal.code, refusal))
        .collect();
    let mut answered: BTreeSet<&str> = BTreeSet::new();
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
        let Some(response) = vector.pointer("/expected/response") else {
            continue;
        };
        if let Some(error) = response.pointer("/body/error") {
            let code = error.get("code").and_then(Value::as_str).unwrap_or("");
            let Some((code, refusal)) = expected.get_key_value(code) else {
                continue;
            };
            let class = class_of(refusal.class);
            let rendered = rendered_message(&vector);
            let observed = (
                response.get("status").and_then(Value::as_u64),
                error.get("class").and_then(Value::as_str),
                error.get("message").and_then(Value::as_str),
            );
            if rendered.as_ref().map(|(rendered, _)| rendered.as_str()) != Some(code)
                || observed
                    != (
                        Some(u64::from(refusal.status)),
                        Some(class.as_str()),
                        rendered.as_ref().map(|(_, message)| message.as_str()),
                    )
            {
                failures.push(format!(
                    "{path}: {code} must answer {} {class} with the message the wire renders for \
                     its request",
                    refusal.status
                ));
                continue;
            }
            answered.insert(code);
        } else if let Some(result) = response.pointer("/body/result") {
            let Some(roots) = result.pointer("/applied/writable_roots") else {
                continue;
            };
            with_applied = true;
            if serde_json::from_value::<Vec<wire::AppliedWritableRoot>>(roots.clone()).is_err() {
                failures.push(format!(
                    "{path}: the applied writable_roots do not parse as the wire record"
                ));
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
    }
    for code in expected.keys() {
        if !answered.contains(code) {
            failures.push(format!("vectors: no exec.start vector answers {code}"));
        }
    }
    if !with_applied {
        failures.push("vectors: no exec.start vector shows applied writable roots".to_owned());
    }
}

/// The code and message the wire crate renders for a vector's request against its setup's facts.
fn rendered_message(vector: &Value) -> Option<(String, String)> {
    let input = vector.pointer("/action/request/body/input")?;
    let input: wire::ExecStartInput = serde_json::from_value(input.clone()).ok()?;
    let facts = vector
        .get("setup")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|step| step.pointer("/state/facts"))?;
    let mut capability = wire::CapabilityFacts::default();
    if let Some(names) = facts.get(wire::EXEC_WRITABLE_ROOTS_HOST_FACT) {
        capability.exec_writable_roots_host = serde_json::from_value(names.clone()).ok()?;
    }
    if let Some(git) = facts.get(wire::EXEC_WRITABLE_ROOTS_GIT_FACT) {
        capability.exec_writable_roots_git = serde_json::from_value(git.clone()).ok()?;
    }
    let error = wire::admit_writable_roots(&input, &capability).err()?;
    let (refusal, message) = wire::writable_root_refusal(&error)?;
    Some((refusal.code.to_owned(), message))
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
    let bound = wire::MAX_WRITABLE_ROOTS.to_string();
    let named = [
        wire::EXEC_WRITABLE_ROOTS_HOST_FACT,
        wire::EXEC_WRITABLE_ROOTS_GIT_FACT,
        "writable_roots",
        "host_path",
        "0.21.0",
        bound.as_str(),
    ]
    .into_iter()
    .chain(
        wire::WRITABLE_ROOT_REFUSALS
            .iter()
            .map(|refusal| refusal.code),
    );
    for named in named {
        if !changes.iter().any(|change| change.contains(named)) {
            failures.push(format!("{path}: client_visible_changes must state {named}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::repo;

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
        super::super::check_additions("0.22.0", tree, &mut failures);
        failures
    }

    fn edited(path: &str, edit: impl FnOnce(&mut serde_json::Value)) -> super::Tree {
        let mut tree = released("0.22.0");
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
    fn the_successor_carries_writable_roots() {
        let failures = failures_of(&released("0.22.0"));
        assert!(failures.is_empty(), "{failures:?}");
    }

    /// A check that passes on the tree that predates the change proves nothing about the tree that
    /// carries it.
    #[test]
    fn the_predecessor_carries_none_of_it() {
        let failures = failures_of(&released("0.21.0"));
        for named in [
            "schemas/common.json: no writable-roots definition",
            "schemas/inputs/exec-start.json: writable_roots is not the shared definition",
            "schemas/inputs/pipe-session-start.json: writable_roots is not the shared definition",
            "exec.writable-roots.host must be published as a list of root names",
            "exec.writable-roots.git must be published as the constant true",
            "no applied-writable-roots definition",
            "has an applied shape without writable_roots",
            "no exec.start vector answers exec.writable-roots-host-unserved",
            "no exec.start vector answers exec.writable-root-mount-overlap",
            "no exec.start vector shows applied writable roots",
            "client_visible_changes must state exec.writable-roots.git",
        ] {
            assert_reported(&failures, named);
        }
    }

    /// Each bound is the wire constant: one moved in the bundle alone is reported by name.
    #[test]
    fn a_bound_that_is_not_the_wire_constant_is_reported() {
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["writable-roots"]["maxItems"] = 9.into();
        });
        assert_reported(
            &failures_of(&tree),
            "writable-roots maxItems must be the wire bound 8",
        );
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["applied-writable-roots"]["maxItems"] = 16.into();
        });
        assert_reported(
            &failures_of(&tree),
            "applied-writable-roots maxItems must be the wire bound 8",
        );
    }

    /// A reserved path the wire refuses and the schema forgets is reported by name.
    #[test]
    fn a_reserved_mount_the_schema_forgets_is_reported() {
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["writable-root-mount"]["x-b10x-not-equal-beneath-or-above"]
                .as_array_mut()
                .expect("array")
                .retain(|path| path != "/etc");
        });
        assert_reported(
            &failures_of(&tree),
            "writable-root-mount does not reserve /etc",
        );
    }

    /// A mount form looser than the wire rule is reported with the path it disagrees on.
    #[test]
    fn a_mount_form_looser_than_the_wire_rule_is_reported() {
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["host-absolute-path"]["pattern"] = "^/.*$".into();
        });
        assert_reported(
            &failures_of(&tree),
            "writable-root-mount form accepts \"/opt/../cache\", which the wire crate refuses",
        );
    }

    /// An open entry, or a third kind, is not the wire's closed union.
    #[test]
    fn an_open_or_extra_kind_is_reported() {
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["writable-root"]["oneOf"][0]
                .as_object_mut()
                .expect("a kind")
                .remove("additionalProperties");
        });
        assert_reported(&failures_of(&tree), "writable-root has no closed host kind");
    }

    /// The refusal declarations are exactly the wire table, status and class included.
    #[test]
    fn refusal_declarations_are_exactly_the_wire_table() {
        let tree = edited("schemas/common.json", |document| {
            let refusals = document["$defs"]["writable-roots"]["x-b10x-refusals"]
                .as_array_mut()
                .expect("array");
            refusals.retain(|refusal| refusal["code"] != "exec.writable-roots-git-unserved");
            refusals.push(serde_json::json!({
                "class": "refused",
                "code": "exec.writable-roots-git-unserved",
                "status": 422
            }));
        });
        let failures = failures_of(&tree);
        assert_reported(
            &failures,
            "writable-roots does not declare exec.writable-roots-git-unserved (501, unserved)",
        );
        assert_reported(
            &failures,
            "writable-roots declares exec.writable-roots-git-unserved (422, refused)",
        );
    }

    /// Every refusal is answered by a vector with the wire's status, class and the message it
    /// renders for that request, the root's name included.
    #[test]
    fn a_refusal_no_vector_answers_or_answers_wrongly_is_reported() {
        let mut tree = released("0.22.0");
        tree.remove("vectors/http/writable-roots-nine-is-refused.json");
        assert_reported(
            &failures_of(&tree),
            "no exec.start vector answers exec.writable-roots-exceeded",
        );
        let tree = edited(
            "vectors/http/writable-host-root-fact-absent-is-unserved.json",
            |document| {
                document["expected"]["response"]["body"]["error"]["message"] =
                    "Writable host root build is not served by this host.".into();
            },
        );
        assert_reported(
            &failures_of(&tree),
            "exec.writable-roots-host-unserved must answer 501 unserved with the message the wire \
             renders for its request",
        );
        let tree = edited(
            "vectors/http/writable-host-root-undeclared-is-unserved.json",
            |document| {
                document["setup"][0]["state"]["facts"]["exec.writable-roots.host"] =
                    serde_json::json!(["build"]);
            },
        );
        assert_reported(
            &failures_of(&tree),
            "exec.writable-roots-host-unserved must answer 501 unserved",
        );
    }

    /// A success result is held to the exec result schema and the wire record: an applied host
    /// root without its host path is neither.
    #[test]
    fn a_vector_result_outside_its_schema_is_reported() {
        let tree = edited(
            "vectors/http/writable-roots-applied-are-observed.json",
            |document| {
                document["expected"]["response"]["body"]["result"]["applied"]["writable_roots"][0]
                    .as_object_mut()
                    .expect("an applied root")
                    .remove("host_path");
            },
        );
        let failures = failures_of(&tree);
        assert_reported(
            &failures,
            "the exec.start result does not validate against its result schema",
        );
        assert_reported(
            &failures,
            "the applied writable_roots do not parse as the wire record",
        );
    }
}
