//! What `0.29.0` exists for (design 37 § 2 to § 6, § 8 and § 9, amended 2026-11-25): an exec
//! start and a session start carry an executable allow-list and deny-list, the applied record
//! echoes them, the capability snapshot publishes the deny-list's property and names why a fact
//! was withheld, and every refusal a start that names a list can earn is published with a vector.
//!
//! The bundle adds no route. Both members stand in the start's `sandbox` and in the exec record's
//! requested and applied `sandbox`, each a `$ref` to one definition of `common.json`. Their bounds
//! (`MAX_EXEC_SUBPATHS`, `MAX_EXEC_DENIED_PATHS`, `MAX_EXEC_LIST_ENTRY_BYTES`) and the withheld
//! bound (`MAX_WITHHELD_FACTS`) are literals in the bundle, because `render_v2.rs` is hashed by
//! every bundle it rendered and cannot learn a new `{"$wire": …}` binding (AGENTS.md § *The
//! gate*). This module binds each literal to the wire crate, which no bundle hashes.
//!
//! Each refusal vector is held to its row of `substrate_wire::EXEC_LIST_REFUSALS`: code, status,
//! address, and the message where the row or `exec_lists_fact_refusal` gives one. The messages of
//! rows 6 to 9 are the host driver's and hold a path the request sent; row 9's names the cause's
//! wire word and never a file-system type, so no vector of `0.29.0` depends on the types the host
//! serves today.
//!
//! The module binds `0.29.0` only ([`binds`]): the vector of row 2 is that bundle's, so a later
//! bundle can give a present and empty allow-list a meaning (design 37 § 3).

use std::collections::BTreeSet;

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const VERSION: &str = "0.29.0";
const PREDECESSOR: &str = "0.28.0";
/// The routes of `0.28.0`, which `0.29.0` preserves and adds none to.
const ROUTES_OF_0_28_0: u64 = 45;

const BUNDLE: &str = "bundle.json";
const CAPABILITY: &str = "schemas/capability.json";
const COMMON: &str = "schemas/common.json";
const COMPATIBILITY: &str = "compatibility.json";
const COVERAGE: &str = "coverage.json";
const RESOURCE: &str = "schemas/resource.json";
const PROBE: &str = "vectors/http/machine-probe.json";

/// The coverage requirement every refusal vector of a list is evidence of.
const REQUIREMENT: &str = "exec.executable-lists-refusals";
/// The coverage requirement of the vector that holds a start without the lists to its hash.
const COMPATIBLE_REQUIREMENT: &str = "exec.executable-lists-compatible";
/// The vector that replays a start without the lists under its `0.28.0` request hash.
const COMPATIBLE: &str = "vectors/http/exec-lists-absent-keeps-the-request-hash.json";
/// The address an exec start is hashed at.
const EXEC_ADDRESS: &str = "/v1/execs";

/// The two starts whose sandbox carries the lists, and where each states it.
const STARTS: [(&str, &str); 2] = [
    ("schemas/inputs/exec-start.json", "/properties/sandbox"),
    (
        "schemas/inputs/pipe-session-start.json",
        "/properties/exec/properties/sandbox",
    ),
];

/// The file-system types no vector message of `0.29.0` names (design 37 § 6, amended): the three
/// the host served when the design was written, the one design 35 § 3.14's probe adds, and the
/// one it refuses by name.
const FILE_SYSTEM_TYPES: [&str; 5] = ["ext4", "overlay", "tmpfs", "xfs", "btrfs"];

/// The three limits design 35 states and design 37 § 9 requires the reference text to carry, as
/// the reference text words them: § 7, § 8.2 and § 8.5.
const LIMITS: [&str; 3] = [
    "not what an admitted interpreter or loader then runs",
    "A deny-list is exact only where the run cannot write",
    "another hard link to a denied file beneath a listed path runs",
];

/// One of the two members.
struct Member {
    name: &'static str,
    definition: &'static str,
    bound: usize,
    fact: &'static str,
    /// The rows of design 37 § 6 a start that names this member can earn, by number.
    rows: &'static [usize],
}

/// Row 5 is answered at `mode` for a start with an allow-list, and row 8 at `argv` for a program
/// a deny entry holds; every other row is listed on each member whose address it is answered at.
const MEMBERS: [Member; 2] = [
    Member {
        name: "exec_subpaths",
        definition: "exec-subpaths",
        bound: wire::MAX_EXEC_SUBPATHS,
        fact: wire::CONFINEMENT_EXEC_SUBPATHS_FACT,
        rows: &[2, 4, 5, 6, 7, 9],
    },
    Member {
        name: "exec_denied_paths",
        definition: "exec-denied-paths",
        bound: wire::MAX_EXEC_DENIED_PATHS,
        fact: wire::CONFINEMENT_EXEC_DENIED_PATHS_FACT,
        rows: &[3, 4, 6, 7, 8, 9],
    },
];

/// Row `number` of design 37 § 6, `2` to `9`.
fn row(number: usize) -> wire::ExecListRefusal {
    wire::EXEC_LIST_REFUSALS[number - 2]
}

/// The class a row is published with: `unserved` at 501, `refused` at 422.
fn class_of(refusal: &wire::ExecListRefusal) -> &'static str {
    if refusal.status == wire::UnservedRefusal::STATUS {
        wire::UnservedRefusal::CLASS
    } else {
        "refused"
    }
}

/// Whether `version` is the bundle that introduced the lists: `0.29.0` only.
pub(super) fn binds(version: &str) -> bool {
    version == VERSION
}

/// Whether the lists bind `version`: `0.29.0` and every later bundle, which carries them on.
pub(super) fn applies_to(version: &str) -> bool {
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => {
            (major, minor, patch) >= (0, 29, 0)
        }
        _ => false,
    }
}

/// Every check of the additions, for `0.29.0` and every later bundle.
pub(super) fn check(version: &str, released: &Tree, failures: &mut Vec<String>) {
    check_definitions(released, failures);
    check_starts(released, failures);
    check_record(version, released, failures);
    check_snapshot(version, released, failures);
    check_requires(released, failures);
    check_refusal_vectors(released, failures);
    check_file_system_types(released, failures);
    check_compatible_vector(released, failures);
    check_coverage(released, failures);
}

/// What only `0.29.0` states: its predecessor, its route counts and its client-visible changes.
pub(super) fn check_introduction(released: &Tree, failures: &mut Vec<String>) {
    if let Some(bundle) = json_at(released, BUNDLE, failures) {
        let routes = json_at(released, "operations.json", failures)
            .and_then(|operations| {
                operations
                    .get("operations")
                    .and_then(Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or_default() as u64;
        for (member, expected) in [
            ("predecessor", json!(PREDECESSOR)),
            ("kind", json!("additive-v1")),
            ("adds_routes", json!(0)),
            ("preserves_routes", json!(ROUTES_OF_0_28_0)),
        ] {
            let stated = bundle.pointer(&format!("/compatibility/{member}"));
            if stated != Some(&expected) {
                failures.push(format!(
                    "{BUNDLE}: compatibility.{member} is {}, and {VERSION} states {expected}",
                    stated.unwrap_or(&Value::Null)
                ));
            }
        }
        if routes != ROUTES_OF_0_28_0 {
            failures.push(format!(
                "operations.json: {routes} routes, and {VERSION} serves the {ROUTES_OF_0_28_0} of \
                 {PREDECESSOR}"
            ));
        }
    }
    let Some(compatibility) = json_at(released, COMPATIBILITY, failures) else {
        return;
    };
    let changes: Vec<&str> = compatibility
        .get("client_visible_changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let routes = format!("the {ROUTES_OF_0_28_0} routes of {PREDECESSOR}");
    let mut named = vec![
        "design 37".to_owned(),
        routes,
        wire::EXEC_SUBPATHS_ADDRESS.to_owned(),
        wire::EXEC_DENIED_PATHS_ADDRESS.to_owned(),
        wire::CONFINEMENT_EXEC_DENIED_PATHS_FACT.to_owned(),
        "withheld".to_owned(),
    ];
    named.extend(
        wire::EXEC_LIST_REFUSALS
            .iter()
            .map(|refusal| format!("{} ({})", refusal.code, refusal.status)),
    );
    named.extend(LIMITS.iter().map(|limit| (*limit).to_owned()));
    for named in named {
        if !changes.iter().any(|change| change.contains(&named)) {
            failures.push(format!(
                "{COMPATIBILITY}: client_visible_changes must state {named}"
            ));
        }
    }
}

/// `released` without the vectors this module holds that answer `confinement.fact-unserved` at a
/// list's address (row 4). Design 30's check of that code counts the one vector it published at
/// `sandbox.requires.facts`; design 37 § 6 gives the code two more addresses, and the vectors at
/// them are this module's.
pub(super) fn without_list_fact_vectors(released: &Tree) -> Tree {
    let listed = row(4);
    released
        .iter()
        .filter(|(path, bytes)| {
            if !path.starts_with("vectors/") {
                return true;
            }
            let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
                return true;
            };
            let error = vector.pointer("/expected/response/body/error");
            let code = error.and_then(|error| error.get("code"));
            let address = error
                .and_then(|error| error.get("address"))
                .and_then(Value::as_str);
            !(code == Some(&json!(listed.code))
                && address.is_some_and(|address| listed.addresses.contains(&address)))
        })
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect()
}

/// The `x-b10x-refusals` a member publishes: one row of class, code and status for each row of
/// design 37 § 6 it can earn, in the table's order.
fn expected_refusals(member: &Member) -> Value {
    Value::Array(
        member
            .rows
            .iter()
            .map(|number| {
                let refusal = row(*number);
                json!({
                    "class": class_of(&refusal),
                    "code": refusal.code,
                    "status": refusal.status,
                })
            })
            .collect(),
    )
}

/// Each member's one definition: an array of at most its bound, no `minItems` (a present and
/// empty allow-list is row 2, and an empty deny-list reads as an absent one), unique entries of
/// 1 to `MAX_EXEC_LIST_ENTRY_BYTES` characters, the capability it requires, its refusals, and the
/// three limits of design 35 in its reference text.
fn check_definitions(released: &Tree, failures: &mut Vec<String>) {
    let Some(common) = json_at(released, COMMON, failures) else {
        return;
    };
    for member in &MEMBERS {
        let Some(definition) = common.pointer(&format!("/$defs/{}", member.definition)) else {
            failures.push(format!(
                "{COMMON}: no {} definition for {}",
                member.definition, member.name
            ));
            continue;
        };
        let at = format!("{COMMON}: {}", member.definition);
        if definition.get("type") != Some(&json!("array")) {
            failures.push(format!("{at} is not an array"));
        }
        let stated = definition.get("maxItems");
        if stated != Some(&json!(member.bound)) {
            failures.push(format!(
                "{at} maxItems is {}, and the wire bound of {} is {}",
                stated.unwrap_or(&Value::Null),
                member.name,
                member.bound
            ));
        }
        if definition.get("minItems").is_some() {
            failures.push(format!(
                "{at} states minItems: a present and empty allow-list is refused by name and an \
                 empty deny-list reads as an absent one"
            ));
        }
        if definition.get("uniqueItems") != Some(&json!(true)) {
            failures.push(format!("{at} does not state uniqueItems"));
        }
        let items = definition.get("items");
        let entry = json!({
            "maxLength": wire::MAX_EXEC_LIST_ENTRY_BYTES,
            "minLength": 1,
            "type": "string",
        });
        if items != Some(&entry) {
            failures.push(format!(
                "{at} items is {}, and an entry is {entry}: minLength 1 and \
                 substrate_wire::MAX_EXEC_LIST_ENTRY_BYTES ({})",
                items.unwrap_or(&Value::Null),
                wire::MAX_EXEC_LIST_ENTRY_BYTES
            ));
        }
        let capability = definition.get("x-b10x-requires-capability");
        if capability != Some(&json!(member.fact)) {
            failures.push(format!(
                "{at} x-b10x-requires-capability is {}, and {} requires {}",
                capability.unwrap_or(&Value::Null),
                member.name,
                member.fact
            ));
        }
        let refusals = expected_refusals(member);
        if definition.get("x-b10x-refusals") != Some(&refusals) {
            failures.push(format!(
                "{at} x-b10x-refusals must be exactly {refusals}, the rows of design 37 § 6 it \
                 can earn"
            ));
        }
        let description = definition
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for limit in LIMITS {
            if !description.contains(limit) {
                failures.push(format!(
                    "{at} reference text does not state the limit of design 35: {limit:?}"
                ));
            }
        }
    }
}

/// Both starts carry both members as optional members of their sandbox, each its definition.
fn check_starts(released: &Tree, failures: &mut Vec<String>) {
    for (path, pointer) in STARTS {
        let Some(start) = json_at(released, path, failures) else {
            continue;
        };
        let Some(sandbox) = start.pointer(pointer) else {
            failures.push(format!("{path}: no sandbox"));
            continue;
        };
        for member in &MEMBERS {
            let reference = json!({
                "$ref": format!("../common.json#/$defs/{}", member.definition)
            });
            let stated = sandbox.pointer(&format!("/properties/{}", member.name));
            if stated != Some(&reference) {
                failures.push(format!(
                    "{path}: sandbox.{} is {}, and must be {reference}",
                    member.name,
                    stated.unwrap_or(&Value::Null)
                ));
            }
            if sandbox
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|required| required.contains(&json!(member.name)))
            {
                failures.push(format!("{path}: sandbox.{} must be optional", member.name));
            }
        }
    }
}

/// Every object schema in `value` that declares all of `members`, keyed by its JSON pointer.
fn shapes_with<'a>(
    value: &'a Value,
    members: &[&str],
    at: &str,
    found: &mut Vec<(String, &'a Value)>,
) {
    match value {
        Value::Object(object) => {
            if object
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|properties| {
                    members
                        .iter()
                        .all(|member| properties.contains_key(*member))
                })
            {
                found.push((at.to_owned(), value));
            }
            for (key, child) in object {
                shapes_with(child, members, &format!("{at}/{key}"), found);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                shapes_with(child, members, &format!("{at}/{index}"), found);
            }
        }
        _ => {}
    }
}

/// Every requested sandbox (it alone states `require`) and every applied one (it alone states
/// `cgroup`) of the exec record carries both members, and a record the wire crate emits with both
/// lists on both validates against the exec result schema.
fn check_record(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let Some(resource) = json_at(released, RESOURCE, failures) else {
        return;
    };
    let mut shapes = Vec::new();
    for (kind, members) in [
        ("requested", ["capability_snapshot", "require"]),
        ("applied", ["capability_snapshot", "cgroup"]),
    ] {
        let mut found = Vec::new();
        shapes_with(&resource, &members, "", &mut found);
        if found.is_empty() {
            failures.push(format!("{RESOURCE}: no {kind} confinement shape"));
        }
        shapes.extend(found.into_iter().map(|(at, shape)| (kind, at, shape)));
    }
    for (kind, at, shape) in shapes {
        for member in &MEMBERS {
            let reference = json!({
                "$ref": format!("common.json#/$defs/{}", member.definition)
            });
            let stated = shape.pointer(&format!("/properties/{}", member.name));
            if stated.map(without_description) != Some(reference.clone()) {
                failures.push(format!(
                    "{RESOURCE}: the {kind} sandbox at {at} does not carry {} as {reference}",
                    member.name
                ));
            }
            if shape
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|required| required.contains(&json!(member.name)))
            {
                failures.push(format!(
                    "{RESOURCE}: the {kind} sandbox at {at} requires {}",
                    member.name
                ));
            }
        }
    }
    let record = match wire_record() {
        Ok(record) => record,
        Err(error) => {
            failures.push(format!(
                "the exec record with both lists is not one the wire emits: {error}"
            ));
            return;
        }
    };
    let Some(validator) =
        super::workdir_apertures::exec_result_validator(version, released, failures)
    else {
        return;
    };
    if let Err(error) = validator.validate(&record) {
        failures.push(format!(
            "schemas/results/exec-start.json: rejects the wire exec record with both lists: \
             {error} at {}",
            error.instance_path()
        ));
    }
}

/// A schema without its `description`, so a member may say what it echoes.
fn without_description(schema: &Value) -> Value {
    let mut schema = schema.clone();
    if let Some(object) = schema.as_object_mut() {
        object.remove("description");
    }
    schema
}

/// A running exec record whose start named both lists, requested and applied: parsed into
/// `substrate_wire::Exec` and serialized back, so it is a record the daemon can emit.
fn wire_record() -> Result<Value, String> {
    let snapshot = format!("sha256:{}", "7".repeat(64));
    let allow = json!(["/usr/bin", "tools"]);
    let deny = json!(["tools/unsafe"]);
    let record = json!({
        "id": "ex_vector",
        "kind": "exec",
        "workspace": "ws_vector",
        "state": "running",
        "observed_at": "2026-11-25T12:00:00Z",
        "requested": {
            "capability_snapshot": snapshot,
            "network": "none",
            "profile": "workspace",
            "require": true,
            "exec_subpaths": allow,
            "exec_denied_paths": deny,
        },
        "applied": {
            "capability_snapshot": snapshot,
            "filesystem": "workspace-rw-system-ro",
            "network": "none",
            "profile": "workspace",
            "exec_subpaths": allow,
            "exec_denied_paths": deny,
        },
        "exit": null,
    });
    let exec: wire::Exec =
        serde_json::from_value(record.clone()).map_err(|error| error.to_string())?;
    let emitted = serde_json::to_value(&exec).map_err(|error| error.to_string())?;
    if emitted == record {
        Ok(emitted)
    } else {
        Err(format!("the wire serializes {emitted}"))
    }
}

/// The snapshot's optional `withheld`, exactly: a list of 1 to `MAX_WITHHELD_FACTS` distinct
/// closed entries whose three members are the wire crate's enumerations in their order.
fn expected_withheld() -> Value {
    let facts: Vec<&str> = wire::WithheldFactName::ALL
        .iter()
        .map(|fact| fact.fact())
        .collect();
    let causes: Vec<&str> = wire::WithheldCause::ALL
        .iter()
        .map(|cause| cause.name())
        .collect();
    let addresses: Vec<&str> = wire::WithheldAddress::ALL
        .iter()
        .map(|address| address.name())
        .collect();
    json!({
        "items": {
            "additionalProperties": false,
            "properties": {
                "address": { "enum": addresses },
                "cause": { "enum": causes },
                "fact": { "enum": facts },
            },
            "required": ["fact", "cause", "address"],
            "type": "object",
        },
        "maxItems": wire::MAX_WITHHELD_FACTS,
        "minItems": 1,
        "type": "array",
        "uniqueItems": true,
    })
}

/// Every entry `withheld` can hold, each once, in the order the wire writes them.
fn every_withheld_entry() -> Vec<wire::WithheldFact> {
    let mut entries = Vec::new();
    for fact in wire::WithheldFactName::ALL {
        for cause in wire::WithheldCause::ALL {
            for address in wire::WithheldAddress::ALL {
                entries.push(wire::WithheldFact {
                    fact,
                    cause,
                    address,
                });
            }
        }
    }
    wire::withheld_facts(entries)
}

/// The deny-list's fact and the member `withheld` (design 37 § 5 and § 8), held to the wire
/// crate: the snapshot the probe publishes validates with every entry `withheld` can hold, and
/// is refused with a duplicate entry or an entry that names a path.
fn check_snapshot(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let Some(capability) = json_at(released, CAPABILITY, failures) else {
        return;
    };
    let fact = wire::CONFINEMENT_EXEC_DENIED_PATHS_FACT;
    if capability
        .pointer(&format!("/properties/facts/properties/{fact}"))
        .is_none()
    {
        failures.push(format!("{CAPABILITY}: {fact} is not declared"));
    }
    let product = wire::WithheldFactName::ALL.len()
        * wire::WithheldCause::ALL.len()
        * wire::WithheldAddress::ALL.len();
    if product != wire::MAX_WITHHELD_FACTS {
        failures.push(format!(
            "substrate_wire::MAX_WITHHELD_FACTS is {}, and the three enumerations admit {product} \
             entries",
            wire::MAX_WITHHELD_FACTS
        ));
    }
    let Some(withheld) = capability.pointer("/properties/withheld") else {
        failures.push(format!("{CAPABILITY}: withheld is not declared"));
        return;
    };
    let expected = expected_withheld();
    if without_description(withheld) != expected {
        failures.push(format!(
            "{CAPABILITY}: withheld must be exactly {expected}, the wire crate's three \
             enumerations in their order"
        ));
    }
    if capability
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| required.contains(&json!("withheld")))
    {
        failures.push(format!("{CAPABILITY}: withheld must be optional"));
    }
    let Some(probe) = json_at(released, PROBE, failures) else {
        return;
    };
    let Some(snapshot) = probe.pointer("/expected/response/body/result") else {
        failures.push(format!("{PROBE}: no capability snapshot result"));
        return;
    };
    let mut full = snapshot.clone();
    full["withheld"] = serde_json::to_value(every_withheld_entry()).unwrap_or(Value::Null);
    match serde_json::from_value::<wire::CapabilitySnapshot>(full.clone())
        .map_err(|error| error.to_string())
        .and_then(|parsed| serde_json::to_value(parsed).map_err(|error| error.to_string()))
    {
        Ok(emitted) if emitted == full => {}
        Ok(emitted) => failures.push(format!(
            "the wire crate writes the snapshot with every withheld entry as {emitted}"
        )),
        Err(error) => failures.push(format!(
            "the wire crate refuses the snapshot with every withheld entry: {error}"
        )),
    }
    let Some(schema) = super::read_only_roots::validator(version, released, CAPABILITY, failures)
    else {
        return;
    };
    if let Err(error) = schema.validate(&full) {
        failures.push(format!(
            "{CAPABILITY}: rejects a snapshot with every withheld entry: {error} at {}",
            error.instance_path()
        ));
    }
    let entry = full["withheld"][0].clone();
    let mut twice = snapshot.clone();
    twice["withheld"] = json!([entry, entry]);
    if schema.is_valid(&twice) {
        failures.push(format!(
            "{CAPABILITY}: accepts a withheld entry written twice"
        ));
    }
    let mut pathed = snapshot.clone();
    let mut with_path = entry;
    with_path["path"] = json!("/srv/workspaces");
    pathed["withheld"] = json!([with_path]);
    if schema.is_valid(&pathed) {
        failures.push(format!(
            "{CAPABILITY}: accepts a withheld entry that names a path"
        ));
    }
}

/// `sandbox.requires.facts` admits the eight names of `ConfinementProperty::ALL`, in its order.
fn check_requires(released: &Tree, failures: &mut Vec<String>) {
    let Some(common) = json_at(released, COMMON, failures) else {
        return;
    };
    let words: Vec<&str> = wire::ConfinementProperty::ALL
        .iter()
        .map(|property| property.name())
        .collect();
    let stated = common.pointer("/$defs/confinement-requires/properties/facts/items/enum");
    if stated != Some(&json!(words)) {
        failures.push(format!(
            "{COMMON}: sandbox.requires.facts admits {}, and ConfinementProperty::ALL is {words:?}",
            stated.unwrap_or(&Value::Null)
        ));
    }
}

/// The start a vector makes, as the wire crate reads it, and its raw `sandbox`.
struct Start {
    exec: wire::ExecStartInput,
    operation: String,
    mode: Option<String>,
}

/// The raw `sandbox` of the start a vector makes, on an exec start or a session's exec.
fn raw_sandbox(vector: &Value) -> Option<&Value> {
    vector
        .pointer("/action/request/body/input/sandbox")
        .or_else(|| vector.pointer("/action/request/body/input/exec/sandbox"))
}

/// Whether the vector's start names either list.
fn names_a_list(vector: &Value) -> bool {
    raw_sandbox(vector).is_some_and(|sandbox| {
        MEMBERS
            .iter()
            .any(|member| sandbox.get(member.name).is_some())
    })
}

fn start_of(vector: &Value) -> Result<Start, String> {
    let operation = vector
        .pointer("/action/operation")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let input = vector
        .pointer("/action/request/body/input")
        .cloned()
        .ok_or("has no request input")?;
    let refused = |error: serde_json::Error| {
        format!("is answered for a start the wire does not admit: {error}")
    };
    let (exec, mode) = match operation.as_str() {
        "exec.start" => (
            serde_json::from_value::<wire::ExecStartInput>(input).map_err(refused)?,
            None,
        ),
        "session.start" => {
            let mode = input.get("mode").and_then(Value::as_str).map(str::to_owned);
            (
                serde_json::from_value::<wire::PipeSessionStartInput>(input)
                    .map_err(refused)?
                    .exec,
                mode,
            )
        }
        other => return Err(format!("is answered for {other}, which starts no exec")),
    };
    Ok(Start {
        exec,
        operation,
        mode,
    })
}

/// The facts of the vector's one machine, which states the snapshot the start names.
fn machine_facts(vector: &Value, start: &Start) -> Result<wire::CapabilityFacts, String> {
    let machines: Vec<&Value> = vector
        .get("setup")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|step| step.get("kind") == Some(&json!("machine")))
        .map(|step| step.get("state").unwrap_or(&Value::Null))
        .collect();
    let [machine] = machines.as_slice() else {
        return Err(format!(
            "sets up {} machines; exactly one states the snapshot the start names",
            machines.len()
        ));
    };
    if machine.get("snapshot") != Some(&json!(start.exec.sandbox.capability_snapshot)) {
        return Err("names a snapshot its machine does not hold".to_owned());
    }
    serde_json::from_value(machine.get("facts").cloned().unwrap_or(Value::Null))
        .map_err(|error| format!("sets up machine facts the wire crate does not read: {error}"))
}

/// Whether the vector states `actual` `equals` `expected` among its postconditions.
fn states(vector: &Value, actual: &str, expected: &Value) -> bool {
    vector
        .get("postconditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|condition| {
            condition.get("actual") == Some(&json!(actual))
                && condition.get("operator") == Some(&json!("equals"))
                && condition.get("expected") == Some(expected)
        })
}

/// What the request sent at `address`: the program for `argv`, the member's entries for a list.
fn sent_at<'a>(start: &'a Start, address: &str) -> Vec<&'a str> {
    let sandbox = &start.exec.sandbox;
    if address == "argv" {
        start
            .exec
            .argv
            .first()
            .map(String::as_str)
            .into_iter()
            .collect()
    } else if address == wire::EXEC_SUBPATHS_ADDRESS {
        sandbox
            .exec_subpaths
            .iter()
            .flatten()
            .map(String::as_str)
            .collect()
    } else if address == wire::EXEC_DENIED_PATHS_ADDRESS {
        sandbox
            .exec_denied_paths
            .iter()
            .map(String::as_str)
            .collect()
    } else {
        Vec::new()
    }
}

/// The path of the view an entry or the program names: a relative entry lies beneath the
/// workspace mount.
fn view_path(text: &str) -> String {
    if text.starts_with('/') {
        text.to_owned()
    } else {
        format!("{}/{text}", wire::EXEC_WORKSPACE_MOUNT)
    }
}

/// The message of row 9 on the wire for the place of `path` (design 37 § 6, amended): the path
/// of the view and the cause's wire word, never a file-system type.
fn place_unserved_message(path: &str) -> String {
    format!(
        "This host cannot hold the run to its executable list: cannot establish the place of \
         {path}: {}.",
        wire::WithheldCause::FileSystemUnlisted.name()
    )
}

/// One refusal vector against row `number`: its form, its message and why no earlier row
/// answers its start. Returns the address it is answered at.
fn check_row_vector(
    path: &str,
    vector: &Value,
    number: usize,
    released: &Tree,
    failures: &mut Vec<String>,
) -> String {
    let refusal = row(number);
    let code = refusal.code;
    let error = vector.pointer("/expected/response/body/error");
    let status = vector.pointer("/expected/response/status");
    if status != Some(&json!(refusal.status)) {
        failures.push(format!(
            "{path}: {code} answers {}, and row {number} is {}",
            status.unwrap_or(&Value::Null),
            refusal.status
        ));
    }
    let address = error
        .and_then(|error| error.get("address"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !refusal.addresses.contains(&address.as_str()) {
        failures.push(format!(
            "{path}: {code} is answered at {address:?}, and row {number} is answered at {:?}",
            refusal.addresses
        ));
    }
    for (member, expected) in [
        ("class", json!(class_of(&refusal))),
        ("retriable", json!(false)),
        (
            "operation",
            vector
                .pointer("/action/request/body/op")
                .cloned()
                .unwrap_or(Value::Null),
        ),
    ] {
        let stated = error.and_then(|error| error.get(member));
        if stated != Some(&expected) {
            failures.push(format!(
                "{path}: {code} {member} is {}, and row {number} publishes {expected}",
                stated.unwrap_or(&Value::Null)
            ));
        }
    }
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let start = match start_of(vector) {
        Ok(start) => start,
        Err(reason) => {
            failures.push(format!("{path}: {code} {reason}"));
            return address;
        }
    };
    if let Err(error) = wire::validate_exec_lists(&start.exec.sandbox) {
        failures.push(format!(
            "{path}: {code} is answered for lists the wire refuses as request.schema-invalid: \
             {error}"
        ));
    }
    let facts = match machine_facts(vector, &start) {
        Ok(facts) => facts,
        Err(reason) => {
            failures.push(format!("{path}: {code} {reason}"));
            return address;
        }
    };
    let answer = Answer {
        path,
        number,
        message,
        address: &address,
    };
    check_cause(&answer, &start, &facts, failures);
    check_recorded(path, vector, number, &start, released, failures);
    address
}

/// What one refusal vector answers: its path, its row, its message and its address.
struct Answer<'a> {
    path: &'a str,
    number: usize,
    message: &'a str,
    address: &'a str,
}

/// Why the row answers the vector's start and no earlier row does, and its message.
fn check_cause(
    answer: &Answer<'_>,
    start: &Start,
    facts: &wire::CapabilityFacts,
    failures: &mut Vec<String>,
) {
    let Answer {
        path,
        number,
        message,
        address,
    } = *answer;
    let refusal = row(number);
    let code = refusal.code;
    let sandbox = &start.exec.sandbox;
    let allow = sandbox.exec_subpaths.as_deref();
    let fixed = |failures: &mut Vec<String>| {
        if Some(message) != refusal.message {
            failures.push(format!(
                "{path}: {code} message is {message:?}, and row {number} is {:?}",
                refusal.message.unwrap_or_default()
            ));
        }
    };
    match number {
        2 => {
            if !allow.is_some_and(<[String]>::is_empty) {
                failures.push(format!(
                    "{path}: {code} is answered for a start whose allow-list is not present and \
                     empty"
                ));
            }
            fixed(failures);
        }
        3 => {
            if allow.is_some() || sandbox.exec_denied_paths.is_empty() {
                failures.push(format!(
                    "{path}: {code} is answered for a start that is not a deny-list without an \
                     allow-list"
                ));
            }
            fixed(failures);
        }
        4 => {
            if allow.is_some_and(<[String]>::is_empty)
                || (allow.is_none() && !sandbox.exec_denied_paths.is_empty())
            {
                failures.push(format!(
                    "{path}: {code} is answered where row 2 or row 3 refuses the start first"
                ));
            }
            match wire::exec_lists_fact_refusal(sandbox, facts) {
                None => failures.push(format!(
                    "{path}: {code} is answered for lists the machine proves"
                )),
                Some((computed, at, expected)) => {
                    if computed != refusal || at != address || expected != message {
                        failures.push(format!(
                            "{path}: {code} must be answered at {at} with {expected:?}, which \
                             exec_lists_fact_refusal computes from the request and the machine"
                        ));
                    }
                }
            }
        }
        _ => {
            if allow.is_none_or(<[String]>::is_empty) {
                failures.push(format!(
                    "{path}: {code} is answered for a start without an allow-list, which the \
                     daemon refuses before the driver"
                ));
            }
            if wire::exec_lists_fact_refusal(sandbox, facts).is_some() {
                failures.push(format!(
                    "{path}: {code} is answered where the machine does not prove a list's fact, \
                     which row 4 refuses first"
                ));
            }
            check_driver_message(path, number, message, start, address, failures);
            if number == 5 {
                fixed(failures);
            }
        }
    }
}

/// Rows 5 to 9: row 5 is a `pty` session's; the messages of rows 6 to 8 hold what the request
/// sent at the address, and row 9's is the wire form for the place of an entry it sent.
fn check_driver_message(
    path: &str,
    number: usize,
    message: &str,
    start: &Start,
    address: &str,
    failures: &mut Vec<String>,
) {
    let code = row(number).code;
    let sent = sent_at(start, address);
    match number {
        5 => {
            if start.operation != "session.start" || start.mode.as_deref() != Some("pty") {
                failures.push(format!(
                    "{path}: {code} is answered for a start that is not a pty session"
                ));
            }
        }
        6..=8 => {
            if !sent
                .iter()
                .any(|text| message.contains(&format!("{text:?}")))
            {
                failures.push(format!(
                    "{path}: {code} message {message:?} does not hold what the request sent at \
                     {address}: {sent:?}"
                ));
            }
        }
        _ => {
            let expected: Vec<String> = sent
                .iter()
                .map(|text| place_unserved_message(&view_path(text)))
                .collect();
            if !expected.iter().any(|expected| expected == message) {
                failures.push(format!(
                    "{path}: {code} message {message:?} is not the wire form of row 9 for what \
                     the request sent at {address}: one of {expected:?}"
                ));
            }
        }
    }
}

/// Design 37 D23. Rows 2 to 4 are the daemon's: it answers before it accepts the operation, which
/// is recorded as refused, and dispatches nothing. Rows 5 to 9 are the driver's: the operation was
/// accepted, the refusal is its error, its recorded state is terminal, and nothing started. Row
/// 9's vector is a design vector, never one the manifest selects for execution: no lane has a
/// place it refuses.
fn check_recorded(
    path: &str,
    vector: &Value,
    number: usize,
    start: &Start,
    released: &Tree,
    failures: &mut Vec<String>,
) {
    let code = row(number).code;
    let recorded = if number <= 4 { "refused" } else { "terminal" };
    let mut stated = vec![("/probes/operation_state", json!(recorded))];
    if number <= 4 {
        stated.push(("/driver/dispatch_count", json!(0)));
        stated.push(("/probes/operation_row_count_delta", json!(1)));
    } else if start.operation == "session.start" {
        stated.push(("/probes/pipe_session_started", json!(false)));
    } else {
        stated.push(("/probes/program_started", json!(false)));
    }
    for (actual, expected) in stated {
        if !states(vector, actual, &expected) {
            failures.push(format!(
                "{path}: {code} must state {actual} equals {expected}"
            ));
        }
    }
    let covers = vector.get("covers").and_then(Value::as_array);
    if !covers.is_some_and(|covers| covers.contains(&json!(REQUIREMENT))) {
        failures.push(format!("{path}: {code} does not cover {REQUIREMENT}"));
    }
    if number == 9 {
        let mut ignored = Vec::new();
        let conformance = json_at(released, BUNDLE, &mut ignored)
            .and_then(|bundle| bundle.get("conformance").cloned())
            .unwrap_or(Value::Null);
        let listed = |list: &str| {
            conformance
                .get(list)
                .and_then(Value::as_array)
                .is_some_and(|paths| paths.contains(&json!(path)))
        };
        if !listed("design_vectors") || listed("executable_vectors") {
            failures.push(format!(
                "{path}: {code} must be a design vector, not one the manifest selects for \
                 execution: no lane has a place it refuses"
            ));
        }
    }
}

/// One vector per row of `EXEC_LIST_REFUSALS` whose start names a list, two for row 4 (one per
/// address), each held to its row.
fn check_refusal_vectors(released: &Tree, failures: &mut Vec<String>) {
    let mut found: Vec<Vec<String>> = vec![Vec::new(); wire::EXEC_LIST_REFUSALS.len()];
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        let code = vector.pointer("/expected/response/body/error/code");
        let Some(index) = wire::EXEC_LIST_REFUSALS
            .iter()
            .position(|refusal| code == Some(&json!(refusal.code)))
        else {
            continue;
        };
        if !names_a_list(&vector) {
            continue;
        }
        found[index].push(check_row_vector(
            path,
            &vector,
            index + 2,
            released,
            failures,
        ));
    }
    for (index, addresses) in found.iter().enumerate() {
        let number = index + 2;
        let refusal = row(number);
        let published = if number == 4 {
            let mut sorted: Vec<&str> = addresses.iter().map(String::as_str).collect();
            sorted.sort_unstable();
            let mut wanted = refusal.addresses.to_vec();
            wanted.sort_unstable();
            sorted == wanted
        } else {
            addresses.len() == 1
        };
        if !published {
            failures.push(format!(
                "vectors: row {number} ({}) is answered by {} vectors at {addresses:?}, and the \
                 bundle publishes {}",
                refusal.code,
                addresses.len(),
                if number == 4 {
                    "one per address of the two lists"
                } else {
                    "exactly one"
                }
            ));
        }
    }
}

/// Every string at a `message` key in `value`.
fn messages<'a>(value: &'a Value, found: &mut Vec<&'a str>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if key == "message"
                    && let Some(message) = child.as_str()
                {
                    found.push(message);
                }
                messages(child, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|child| messages(child, found)),
        _ => {}
    }
}

/// No vector message of `0.29.0` names a file-system type (design 37 § 6, amended): the bundle
/// does not depend on the types the host serves.
fn check_file_system_types(released: &Tree, failures: &mut Vec<String>) {
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        let mut found = Vec::new();
        messages(&vector, &mut found);
        for message in found {
            for name in FILE_SYSTEM_TYPES {
                if message.contains(name) {
                    failures.push(format!(
                        "{path}: a message names the file-system type {name}: {message:?}"
                    ));
                }
            }
        }
    }
}

/// The vector that replays a start without the lists (design 37 § 10): the start is what the wire
/// crate writes back byte for byte, so it hashes as on `0.28.0`, and the stored row of that hash
/// answers it as a replay, not as `operation.request-conflict`.
fn check_compatible_vector(released: &Tree, failures: &mut Vec<String>) {
    let Some(vector) = json_at(released, COMPATIBLE, failures) else {
        return;
    };
    if vector.pointer("/action/operation") != Some(&json!("exec.start")) {
        failures.push(format!("{COMPATIBLE}: does not start an exec"));
    }
    let input = vector
        .pointer("/action/request/body/input")
        .cloned()
        .unwrap_or(Value::Null);
    if names_a_list(&vector) {
        failures.push(format!("{COMPATIBLE}: the start names a list"));
    }
    match serde_json::from_value::<wire::ExecStartInput>(input.clone())
        .map_err(|error| error.to_string())
        .and_then(|start| serde_json::to_value(start).map_err(|error| error.to_string()))
    {
        Ok(written) if written == input => {}
        Ok(written) => failures.push(format!(
            "{COMPATIBLE}: the wire crate writes the start back as {written}"
        )),
        Err(error) => failures.push(format!(
            "{COMPATIBLE}: the wire crate does not admit the start: {error}"
        )),
    }
    let hash =
        wire::canonical_request_hash_v2("POST", EXEC_ADDRESS, &input, None).unwrap_or_default();
    let record = vector
        .get("setup")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|step| step.get("kind") == Some(&json!("operation")))
        .find_map(|step| step.pointer("/state/record"));
    let Some(record) = record else {
        failures.push(format!("{COMPATIBLE}: no stored operation row"));
        return;
    };
    let operation = vector.pointer("/action/request/body/op");
    for (member, expected) in [
        ("request_hash", json!(hash)),
        ("operation", operation.cloned().unwrap_or(Value::Null)),
        ("operation_kind", json!("exec.start")),
        ("state", json!("terminal")),
    ] {
        let stated = record.get(member);
        if stated != Some(&expected) {
            failures.push(format!(
                "{COMPATIBLE}: the stored row's {member} is {}, and the start's is {expected}",
                stated.unwrap_or(&Value::Null)
            ));
        }
    }
    let body = vector.pointer("/expected/response/body");
    let replayed = match record.pointer("/outcome/kind").and_then(Value::as_str) {
        Some("success") => {
            body.and_then(|body| body.get("result")) == record.pointer("/outcome/result")
                && body.and_then(|body| body.get("operation")) == operation
        }
        Some("error") => {
            let mut error = record
                .pointer("/outcome/error")
                .cloned()
                .unwrap_or(Value::Null);
            if let (Some(object), Some(operation)) = (error.as_object_mut(), operation) {
                object.insert("operation".to_owned(), operation.clone());
            }
            body.and_then(|body| body.get("error")) == Some(&error)
        }
        _ => false,
    };
    if !replayed {
        failures.push(format!(
            "{COMPATIBLE}: the answer is not the stored row's outcome replayed"
        ));
    }
    for (actual, expected) in [
        ("/driver/dispatch_count", json!(0)),
        ("/probes/operation_row_count_delta", json!(0)),
        ("/probes/original_record_changed", json!(false)),
    ] {
        if !states(&vector, actual, &expected) {
            failures.push(format!(
                "{COMPATIBLE}: must state {actual} equals {expected}"
            ));
        }
    }
}

/// Both requirements cite design 37, and each refusal vector and the compatibility vector is
/// evidence of its own.
fn check_coverage(released: &Tree, failures: &mut Vec<String>) {
    let Some(coverage) = json_at(released, COVERAGE, failures) else {
        return;
    };
    let evidence_of = |id: &str| -> Option<BTreeSet<String>> {
        let requirement = coverage
            .get("requirements")
            .and_then(Value::as_array)?
            .iter()
            .find(|requirement| requirement.get("id") == Some(&json!(id)))?;
        if !requirement
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|source| source.contains("Design 37"))
        {
            return Some(BTreeSet::new());
        }
        Some(
            requirement
                .get("evidence")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|evidence| evidence.get("id").and_then(Value::as_str))
                .map(str::to_owned)
                .collect(),
        )
    };
    let id_of = |path: &str| -> String {
        released
            .get(path)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .and_then(|vector| vector.get("id").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default()
    };
    let mut held = vec![(COMPATIBLE_REQUIREMENT, id_of(COMPATIBLE))];
    for (path, bytes) in released {
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        let code = vector.pointer("/expected/response/body/error/code");
        if path.starts_with("vectors/")
            && names_a_list(&vector)
            && wire::EXEC_LIST_REFUSALS
                .iter()
                .any(|refusal| code == Some(&json!(refusal.code)))
        {
            held.push((REQUIREMENT, id_of(path)));
        }
    }
    for (requirement, id) in held {
        match evidence_of(requirement) {
            None => failures.push(format!("{COVERAGE}: no requirement {requirement}")),
            Some(evidence) if !evidence.contains(&id) => failures.push(format!(
                "{COVERAGE}: {requirement} does not cite Design 37 or list the vector {id}"
            )),
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use substrate_wire as wire;

    use crate::repo;

    const VERSION: &str = super::VERSION;
    const EMPTY: &str = "vectors/http/exec-lists-empty-allow-list-is-refused.json";
    const ALLOW_UNSERVED: &str = "vectors/http/exec-lists-allow-list-fact-unserved.json";
    const DENY_UNSERVED: &str = "vectors/http/exec-lists-deny-list-fact-unserved.json";
    const UNREPRESENTABLE: &str = "vectors/http/exec-lists-entry-unrepresentable-is-refused.json";
    const UNDECLARED: &str = "vectors/http/exec-lists-entry-undeclared-is-refused.json";
    const UNLISTED: &str = "vectors/http/exec-lists-denied-program-is-refused.json";
    const PLACE: &str = "vectors/http/exec-lists-place-unserved.json";
    const PTY: &str = "vectors/http/exec-lists-pty-session-is-unserved.json";
    const SESSION: &str = "schemas/inputs/pipe-session-start.json";

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
        super::check(VERSION, tree, &mut failures);
        super::check_introduction(tree, &mut failures);
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
            "expected a failure naming {named:?}, got {failures:#?}"
        );
    }

    /// `MEMBERS` lists a row on a member where the wire table answers the row at the member's
    /// address, and nowhere else but the two rows its own comment names: row 5, answered at
    /// `mode` for a start with an allow-list, and row 8, answered at `argv` for a deny entry.
    #[test]
    fn the_rows_of_each_member_are_the_wire_tables_addresses() {
        for (member, elsewhere) in super::MEMBERS.iter().zip([5, 8]) {
            let address = format!("sandbox.{}", member.name);
            let mut rows: Vec<usize> = (2..=9)
                .filter(|number| {
                    *number == elsewhere
                        || super::row(*number).addresses.contains(&address.as_str())
                })
                .collect();
            rows.sort_unstable();
            assert_eq!(member.rows, rows, "{}", member.name);
        }
    }

    #[test]
    fn the_module_binds_0_29_0_only() {
        assert!(super::binds("0.29.0"));
        for version in ["0.28.0", "0.30.0", "1.0.0", "0.29"] {
            assert!(!super::binds(version), "{version}");
        }
    }

    #[test]
    fn the_successor_publishes_the_executable_lists() {
        let failures = failures_of(&released(VERSION));
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// The whole additions check of the version, design 30's check of `confinement.fact-unserved`
    /// among it, passes: the two vectors of row 4 are this module's and not counted there.
    #[test]
    fn the_additions_of_0_29_0_pass_with_two_more_addresses_of_the_fact_refusal() {
        let tree = released(VERSION);
        let mut failures = Vec::new();
        super::super::check_additions(VERSION, &tree, &mut failures);
        assert!(failures.is_empty(), "{failures:#?}");
        let mut counted = Vec::new();
        super::super::confinement_facts::check(VERSION, &tree, &mut counted);
        assert_reported(
            &counted,
            "confinement.fact-unserved must be answered by exactly one vector, found 3",
        );
        // A vector of the fact refusal at an address of neither list is still counted there.
        let tree = edited(ALLOW_UNSERVED, |vector| {
            vector["expected"]["response"]["body"]["error"]["address"] = json!("sandbox");
        });
        let mut failures = Vec::new();
        super::super::check_additions(VERSION, &tree, &mut failures);
        assert_reported(
            &failures,
            "confinement.fact-unserved must be answered by exactly one vector, found 2",
        );
    }

    /// `0.28.0` declares none of it, so every check reports.
    #[test]
    fn the_predecessor_declares_none_of_it() {
        let mut failures = Vec::new();
        super::check(
            super::PREDECESSOR,
            &released(super::PREDECESSOR),
            &mut failures,
        );
        for named in [
            "no exec-subpaths definition for exec_subpaths",
            "no exec-denied-paths definition for exec_denied_paths",
            "exec-start.json: sandbox.exec_subpaths is null",
            "pipe-session-start.json: sandbox.exec_denied_paths is null",
            "does not carry exec_subpaths",
            "rejects the wire exec record with both lists",
            "confinement.exec-denied-paths is not declared",
            "withheld is not declared",
            "sandbox.requires.facts admits",
            "row 2 (exec.sandbox-exec-subpaths-empty) is answered by 0 vectors",
            "row 4 (confinement.fact-unserved) is answered by 0 vectors",
            "row 9 (exec.sandbox-place-unserved) is answered by 0 vectors",
            "exec-lists-absent-keeps-the-request-hash.json: absent from the bundle",
        ] {
            assert_reported(&failures, named);
        }
        let mut failures = Vec::new();
        super::check_introduction(&released(super::PREDECESSOR), &mut failures);
        assert_reported(&failures, "compatibility.predecessor is \"0.27.0\"");
    }

    #[test]
    fn a_bound_off_its_wire_constant_is_reported() {
        for (definition, pointer, value, named) in [
            (
                "exec-subpaths",
                "maxItems",
                json!(wire::MAX_EXEC_SUBPATHS + 1),
                "exec-subpaths maxItems is 65",
            ),
            (
                "exec-denied-paths",
                "maxItems",
                json!(wire::MAX_EXEC_DENIED_PATHS - 1),
                "exec-denied-paths maxItems is 63",
            ),
        ] {
            let tree = edited("schemas/common.json", |common| {
                common["$defs"][definition][pointer] = value;
            });
            assert_reported(&failures_of(&tree), named);
        }
        for definition in ["exec-subpaths", "exec-denied-paths"] {
            for (member, value) in [
                ("maxLength", json!(wire::MAX_EXEC_LIST_ENTRY_BYTES + 1)),
                ("minLength", json!(0)),
            ] {
                let tree = edited("schemas/common.json", |common| {
                    common["$defs"][definition]["items"][member] = value;
                });
                assert_reported(&failures_of(&tree), &format!("{definition} items is"));
            }
            let tree = edited("schemas/common.json", |common| {
                common["$defs"][definition]
                    .as_object_mut()
                    .expect("a definition")
                    .remove("uniqueItems");
            });
            assert_reported(
                &failures_of(&tree),
                &format!("{definition} does not state uniqueItems"),
            );
        }
    }

    #[test]
    fn a_minimum_on_the_allow_list_is_reported() {
        let tree = edited("schemas/common.json", |common| {
            common["$defs"]["exec-subpaths"]["minItems"] = json!(1);
        });
        assert_reported(&failures_of(&tree), "exec-subpaths states minItems");
    }

    #[test]
    fn a_start_without_a_member_is_reported() {
        let tree = edited(SESSION, |start| {
            start["properties"]["exec"]["properties"]["sandbox"]["properties"]
                .as_object_mut()
                .expect("properties")
                .remove("exec_denied_paths");
        });
        assert_reported(
            &failures_of(&tree),
            "pipe-session-start.json: sandbox.exec_denied_paths is null",
        );
        let tree = edited("schemas/inputs/exec-start.json", |start| {
            start["properties"]["sandbox"]["required"]
                .as_array_mut()
                .expect("required")
                .push(json!("exec_subpaths"));
        });
        assert_reported(
            &failures_of(&tree),
            "exec-start.json: sandbox.exec_subpaths must be optional",
        );
    }

    #[test]
    fn a_member_without_its_capability_refusals_or_limits_is_reported() {
        let tree = edited("schemas/common.json", |common| {
            common["$defs"]["exec-denied-paths"]["x-b10x-requires-capability"] =
                json!(wire::CONFINEMENT_EXEC_SUBPATHS_FACT);
        });
        assert_reported(
            &failures_of(&tree),
            "exec-denied-paths x-b10x-requires-capability is",
        );
        let tree = edited("schemas/common.json", |common| {
            common["$defs"]["exec-subpaths"]["x-b10x-refusals"][0]["status"] = json!(400);
        });
        assert_reported(&failures_of(&tree), "exec-subpaths x-b10x-refusals must be");
        let tree = edited("schemas/common.json", |common| {
            common["$defs"]["exec-denied-paths"]["x-b10x-refusals"]
                .as_array_mut()
                .expect("refusals")
                .pop();
        });
        assert_reported(
            &failures_of(&tree),
            "exec-denied-paths x-b10x-refusals must be",
        );
        let tree = edited("schemas/common.json", |common| {
            common["$defs"]["exec-subpaths"]["description"] = json!("The allow-list.");
        });
        let failures = failures_of(&tree);
        for limit in super::LIMITS {
            assert_reported(&failures, limit);
        }
    }

    /// Removes `member` from every `properties` object beneath `value`.
    fn strip_member(value: &mut serde_json::Value, member: &str) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(serde_json::Value::Object(properties)) = object.get_mut("properties") {
                    properties.remove(member);
                }
                for child in object.values_mut() {
                    strip_member(child, member);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    strip_member(child, member);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn an_applied_or_requested_shape_without_a_member_is_reported() {
        let tree = edited("schemas/resource.json", |resource| {
            resource["$defs"]["exec"]["oneOf"][2]["properties"]["applied"]["oneOf"][1]
                ["properties"]
                .as_object_mut()
                .expect("properties")
                .remove("exec_subpaths");
        });
        assert_reported(&failures_of(&tree), "the applied sandbox at");
        let tree = edited("schemas/resource.json", |resource| {
            strip_member(resource, "exec_subpaths");
        });
        assert_reported(
            &failures_of(&tree),
            "rejects the wire exec record with both lists",
        );
        let tree = edited("schemas/resource.json", |resource| {
            resource["$defs"]["confinement-request"]["properties"]
                .as_object_mut()
                .expect("properties")
                .remove("exec_denied_paths");
        });
        assert_reported(&failures_of(&tree), "the requested sandbox at");
    }

    #[test]
    fn the_deny_list_fact_and_withheld_are_held_to_the_wire() {
        let tree = edited("schemas/capability.json", |capability| {
            capability["properties"]["facts"]["properties"]
                .as_object_mut()
                .expect("facts")
                .remove("confinement.exec-denied-paths");
        });
        assert_reported(
            &failures_of(&tree),
            "confinement.exec-denied-paths is not declared",
        );
        for (pointer, edit) in [
            (
                "/items/properties/cause/enum",
                json!(["mount-id-unreported"]),
            ),
            (
                "/items/properties/address/enum",
                json!([
                    "config.workspace_root",
                    "config.exec_gate",
                    "config.writable_roots"
                ]),
            ),
            (
                "/items/properties/fact/enum",
                json!(["confinement.exec-subpaths"]),
            ),
            ("/maxItems", json!(13)),
            ("/uniqueItems", json!(false)),
            ("/items/additionalProperties", json!(true)),
        ] {
            let tree = edited("schemas/capability.json", |capability| {
                *capability["properties"]["withheld"]
                    .pointer_mut(pointer)
                    .expect("the member") = edit;
            });
            assert_reported(&failures_of(&tree), "withheld must be exactly");
        }
        let tree = edited("schemas/capability.json", |capability| {
            capability["required"]
                .as_array_mut()
                .expect("required")
                .push(json!("withheld"));
        });
        assert_reported(&failures_of(&tree), "withheld must be optional");
    }

    #[test]
    fn the_eight_property_names_stand_in_their_order() {
        let tree = edited("schemas/common.json", |common| {
            common["$defs"]["confinement-requires"]["properties"]["facts"]["items"]["enum"]
                .as_array_mut()
                .expect("enum")
                .swap(5, 6);
        });
        assert_reported(&failures_of(&tree), "sandbox.requires.facts admits");
    }

    #[test]
    fn each_row_has_one_vector_and_row_four_two() {
        let mut tree = released(VERSION);
        tree.remove(DENY_UNSERVED);
        assert_reported(
            &failures_of(&tree),
            "row 4 (confinement.fact-unserved) is answered by 1 vectors",
        );
        let mut tree = released(VERSION);
        tree.remove(PLACE);
        assert_reported(
            &failures_of(&tree),
            "row 9 (exec.sandbox-place-unserved) is answered by 0 vectors",
        );
        let mut tree = released(VERSION);
        let copy = tree.get(UNDECLARED).expect("the vector").clone();
        tree.insert(
            "vectors/http/a-second-undeclared-entry.json".to_owned(),
            copy,
        );
        assert_reported(
            &failures_of(&tree),
            "row 7 (exec.sandbox-exec-subpath-undeclared) is answered by 2 vectors",
        );
        let tree = edited(ALLOW_UNSERVED, |vector| {
            vector["action"]["request"]["body"]["input"]["sandbox"]["exec_denied_paths"] =
                json!(["/usr/bin/false"]);
            vector["expected"]["response"]["body"]["error"]["address"] =
                json!(wire::EXEC_DENIED_PATHS_ADDRESS);
        });
        assert_reported(
            &failures_of(&tree),
            "must be answered at sandbox.exec_subpaths",
        );
    }

    #[test]
    fn a_vector_off_its_row_is_reported() {
        let tree = edited(EMPTY, |vector| {
            vector["expected"]["response"]["status"] = json!(400);
        });
        assert_reported(&failures_of(&tree), "answers 400, and row 2 is 422");
        let tree = edited(EMPTY, |vector| {
            vector["expected"]["response"]["body"]["error"]["message"] = json!("Empty.");
        });
        assert_reported(&failures_of(&tree), "message is \"Empty.\", and row 2 is");
        let tree = edited(EMPTY, |vector| {
            vector["action"]["request"]["body"]["input"]["sandbox"]["exec_subpaths"] =
                json!(["/usr/bin"]);
        });
        assert_reported(
            &failures_of(&tree),
            "whose allow-list is not present and empty",
        );
        let tree = edited(ALLOW_UNSERVED, |vector| {
            vector["expected"]["response"]["body"]["error"]["class"] = json!("refused");
        });
        assert_reported(
            &failures_of(&tree),
            "class is \"refused\", and row 4 publishes",
        );
        let tree = edited(ALLOW_UNSERVED, |vector| {
            vector["setup"][0]["state"]["facts"]["confinement.exec-subpaths"] = json!(true);
        });
        assert_reported(
            &failures_of(&tree),
            "is answered for lists the machine proves",
        );
        let tree = edited(UNREPRESENTABLE, |vector| {
            vector["expected"]["response"]["body"]["error"]["message"] =
                json!("An entry is not normal.");
        });
        assert_reported(&failures_of(&tree), "does not hold what the request sent");
        let tree = edited(UNLISTED, |vector| {
            vector["expected"]["response"]["body"]["error"]["address"] =
                json!(wire::EXEC_SUBPATHS_ADDRESS);
        });
        assert_reported(
            &failures_of(&tree),
            "is answered at \"sandbox.exec_subpaths\"",
        );
        let tree = edited(PLACE, |vector| {
            vector["expected"]["response"]["body"]["error"]["message"] =
                json!("This host cannot hold the run to its executable list.");
        });
        assert_reported(&failures_of(&tree), "is not the wire form of row 9");
        // D23: a driver's row states `terminal` and a daemon's row `refused`, never the other.
        for (path, other, stated) in [
            (PLACE, "refused", "terminal"),
            (EMPTY, "terminal", "refused"),
        ] {
            let tree = edited(path, |vector| {
                for condition in vector["postconditions"].as_array_mut().expect("conditions") {
                    if condition["actual"] == "/probes/operation_state" {
                        condition["expected"] = json!(other);
                    }
                }
            });
            assert_reported(
                &failures_of(&tree),
                &format!("must state /probes/operation_state equals \"{stated}\""),
            );
        }
        let tree = edited(PLACE, |vector| {
            vector["setup"][0]["state"]["facts"]
                .as_object_mut()
                .expect("facts")
                .remove("confinement.exec-denied-paths");
            vector["action"]["request"]["body"]["input"]["sandbox"]["exec_denied_paths"] =
                json!(["tools/unsafe"]);
        });
        assert_reported(&failures_of(&tree), "which row 4 refuses first");
        let tree = edited(PTY, |vector| {
            vector["action"]["request"]["body"]["input"]["mode"] = json!("pipes");
            vector["action"]["request"]["body"]["input"]
                .as_object_mut()
                .expect("input")
                .remove("window");
        });
        assert_reported(&failures_of(&tree), "is not a pty session");
    }

    #[test]
    fn the_place_vector_is_never_selected_for_execution() {
        let tree = edited("bundle.json", |bundle| {
            bundle["conformance"]["executable_vectors"]
                .as_array_mut()
                .expect("executable vectors")
                .push(json!(PLACE));
        });
        assert_reported(&failures_of(&tree), "must be a design vector");
    }

    #[test]
    fn a_message_that_names_a_file_system_type_is_refused() {
        for name in super::FILE_SYSTEM_TYPES {
            let tree = edited(PLACE, |vector| {
                vector["expected"]["response"]["body"]["error"]["message"] = json!(format!(
                    "This host cannot hold the run to its executable list: cannot establish the \
                     place of /workspace/tools: the type {name} is not listed."
                ));
            });
            assert_reported(
                &failures_of(&tree),
                &format!("a message names the file-system type {name}"),
            );
        }
        // Every vector of the bundle, not only the list's.
        let tree = edited("vectors/http/sandbox-unavailable.json", |vector| {
            vector["expected"]["response"]["body"]["error"]["message"] =
                json!("Required host confinement is not available on btrfs.");
        });
        assert_reported(
            &failures_of(&tree),
            "sandbox-unavailable.json: a message names the file-system type btrfs",
        );
    }

    /// The start the compatibility vector replays is a `0.28.0` vector's start, byte for byte,
    /// and its stored row holds the hash a `0.28.0` daemon computed for it.
    #[test]
    fn a_start_without_the_lists_keeps_its_0_28_0_request_hash() {
        let published = released(super::PREDECESSOR);
        let before: serde_json::Value = serde_json::from_slice(
            published
                .get("vectors/http/exec-start.json")
                .expect("the 0.28.0 vector"),
        )
        .expect("parses");
        let tree = released(VERSION);
        let after: serde_json::Value =
            serde_json::from_slice(tree.get(super::COMPATIBLE).expect("the vector"))
                .expect("parses");
        let input = &before["action"]["request"]["body"]["input"];
        assert_eq!(&after["action"]["request"]["body"]["input"], input);
        assert_eq!(
            after["setup"][0]["state"]["record"]["request_hash"],
            json!(wire::canonical_request_hash_v2("POST", "/v1/execs", input, None).expect("hash"))
        );
        let tree = edited(super::COMPATIBLE, |vector| {
            vector["setup"][0]["state"]["record"]["request_hash"] = json!("4".repeat(64));
        });
        assert_reported(&failures_of(&tree), "the stored row's request_hash is");
        let tree = edited(super::COMPATIBLE, |vector| {
            vector["action"]["request"]["body"]["input"]["sandbox"]["exec_denied_paths"] =
                json!([]);
        });
        let failures = failures_of(&tree);
        assert_reported(&failures, "the start names a list");
        assert_reported(&failures, "the wire crate writes the start back as");
        let tree = edited(super::COMPATIBLE, |vector| {
            vector["expected"]["response"]["body"]["result"]["state"] = json!("exited");
        });
        assert_reported(&failures_of(&tree), "not the stored row's outcome replayed");
    }

    #[test]
    fn a_vector_outside_its_requirement_is_reported() {
        let tree = edited("coverage.json", |coverage| {
            for requirement in coverage["requirements"]
                .as_array_mut()
                .expect("requirements")
            {
                if requirement["id"] == super::REQUIREMENT {
                    requirement["evidence"]
                        .as_array_mut()
                        .expect("evidence")
                        .pop();
                }
            }
        });
        assert_reported(
            &failures_of(&tree),
            "exec.executable-lists-refusals does not cite",
        );
    }

    #[test]
    fn the_route_counts_and_changes_are_stated() {
        let tree = edited("bundle.json", |bundle| {
            bundle["compatibility"]["preserves_routes"] = json!(44);
        });
        assert_reported(&failures_of(&tree), "compatibility.preserves_routes is 44");
        let tree = edited("compatibility.json", |compatibility| {
            compatibility["client_visible_changes"] = json!(["0.29.0 adds no route."]);
        });
        let failures = failures_of(&tree);
        for named in [
            "the 45 routes of 0.28.0",
            "sandbox.exec_subpaths",
            "confinement.exec-denied-paths",
            "withheld",
            "exec.sandbox-exec-subpaths-empty (422)",
            "exec.sandbox-place-unserved (501)",
            super::LIMITS[1],
        ] {
            assert_reported(&failures, named);
        }
    }
}
