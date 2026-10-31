//! What `0.26.0` exists for (ADR 0048, design 30 § 4.3 to § 4.5 and § 7): one optional fact per
//! confinement family and per mechanism-neutral property, the optional `sandbox.requires` on both
//! starts, its echo and the optional `cgroup` on the applied record, and two refusals a start
//! meets when its snapshot does not serve or prove what it requires.
//!
//! Every value this check expects comes from `substrate_wire`: the enums' order and words, each
//! fact name, each refusal's operation, code and address, and each refusal message, which the
//! check computes with `confinement_requires_refusal` from the vector's own request and machine.
//! The bundle states them as literals, because `render_v2.rs` is hashed by every bundle it
//! rendered and cannot learn a new `{"$wire": …}` binding (AGENTS.md § *The gate*); the binding
//! lives here, which no bundle hashes.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const CAPABILITY: &str = "schemas/capability.json";
const COMMON: &str = "schemas/common.json";
const RESOURCE: &str = "schemas/resource.json";
const DEFINITION: &str = "confinement-requires";
const PUBLISHED_ONLY_FROM: &str = "a-verified-mechanism-never-configuration";

/// The two starts whose sandbox carries `requires`, and where each states it.
const STARTS: [(&str, &str); 2] = [
    ("schemas/inputs/exec-start.json", "/properties/sandbox"),
    (
        "schemas/inputs/pipe-session-start.json",
        "/properties/exec/properties/sandbox",
    ),
];

/// The first bundle that publishes design 30; every later one keeps it.
const FIRST: (u64, u64, u64) = (0, 26, 0);

pub(super) fn applies_to(version: &str) -> bool {
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => (major, minor, patch) >= FIRST,
        _ => false,
    }
}

/// Every family word and fact name, in ADR 0044 decision 1's order. The match does not compile
/// when a variant is added, so `ConfinementFamily::ALL` cannot fall behind unrevisited.
fn families() -> Vec<(&'static str, &'static str)> {
    wire::ConfinementFamily::ALL
        .into_iter()
        .map(|family| {
            let fact = match family {
                wire::ConfinementFamily::ProcessRun => wire::CONFINEMENT_FAMILY_PROCESS_RUN_FACT,
                wire::ConfinementFamily::ProcessWatch => {
                    wire::CONFINEMENT_FAMILY_PROCESS_WATCH_FACT
                }
                wire::ConfinementFamily::OutputRead => wire::CONFINEMENT_FAMILY_OUTPUT_READ_FACT,
                wire::ConfinementFamily::Wait => wire::CONFINEMENT_FAMILY_WAIT_FACT,
                wire::ConfinementFamily::Workspace => wire::CONFINEMENT_FAMILY_WORKSPACE_FACT,
                wire::ConfinementFamily::FilesystemView => {
                    wire::CONFINEMENT_FAMILY_FILESYSTEM_VIEW_FACT
                }
                wire::ConfinementFamily::NetworkEgress => {
                    wire::CONFINEMENT_FAMILY_NETWORK_EGRESS_FACT
                }
                wire::ConfinementFamily::NetworkIngress => {
                    wire::CONFINEMENT_FAMILY_NETWORK_INGRESS_FACT
                }
                wire::ConfinementFamily::Limits => wire::CONFINEMENT_FAMILY_LIMITS_FACT,
                wire::ConfinementFamily::Audit => wire::CONFINEMENT_FAMILY_AUDIT_FACT,
            };
            (family.name(), fact)
        })
        .collect()
}

/// Every property word and fact name, in the `Fact` order of `spec/confinement/`.
fn properties() -> Vec<(&'static str, &'static str)> {
    wire::ConfinementProperty::ALL
        .into_iter()
        .map(|property| {
            let fact = match property {
                wire::ConfinementProperty::PathsAreHostPaths => {
                    wire::CONFINEMENT_PATHS_ARE_HOST_PATHS_FACT
                }
                wire::ConfinementProperty::WholeTreeKill => wire::CONFINEMENT_WHOLE_TREE_KILL_FACT,
                wire::ConfinementProperty::IpcSocketAllowlist => {
                    wire::CONFINEMENT_IPC_SOCKET_ALLOWLIST_FACT
                }
                wire::ConfinementProperty::IpcSignalsContained => {
                    wire::CONFINEMENT_IPC_SIGNALS_CONTAINED_FACT
                }
                wire::ConfinementProperty::SealedPaths => wire::CONFINEMENT_SEALED_PATHS_FACT,
                wire::ConfinementProperty::ExecSubpaths => wire::CONFINEMENT_EXEC_SUBPATHS_FACT,
                wire::ConfinementProperty::PrivateTmpdir => wire::CONFINEMENT_PRIVATE_TMPDIR_FACT,
            };
            (property.name(), fact)
        })
        .collect()
}

/// The class check, for `0.26.0` and every later bundle.
pub(super) fn check(version: &str, released: &Tree, failures: &mut Vec<String>) {
    check_facts(released, failures);
    check_requires(released, failures);
    check_starts(released, failures);
    check_record(version, released, failures);
    for refusal in wire::CONFINEMENT_UNSERVED_REFUSALS {
        check_refusal_vector(released, &refusal, failures);
    }
}

/// What only the bundle that introduced design 30 states: its client-visible changes.
pub(super) fn check_introduction(released: &Tree, failures: &mut Vec<String>) {
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
    let mut named = vec!["0.25.0", "sandbox.requires", "echoes requires", "cgroup"];
    named.extend(families().into_iter().map(|(_, fact)| fact));
    named.extend(properties().into_iter().map(|(_, fact)| fact));
    named.extend(wire::CONFINEMENT_UNSERVED_REFUSALS.map(|refusal| refusal.code));
    for named in named {
        if !changes.iter().any(|change| change.contains(named)) {
            failures.push(format!("{path}: client_visible_changes must state {named}"));
        }
    }
}

/// Design 30 § 4.3: each family and property fact is an optional `const: true`, published only
/// from a verified mechanism, and the document names no other `confinement.family.` fact and no
/// other `confinement.` fact beside the neutral `confinement.no-egress`.
fn check_facts(released: &Tree, failures: &mut Vec<String>) {
    let Some(capability) = json_at(released, CAPABILITY, failures) else {
        return;
    };
    let published = capability.pointer("/properties/facts/properties");
    let required = capability
        .pointer("/properties/facts/required")
        .and_then(Value::as_array);
    let expected: Vec<&str> = families()
        .into_iter()
        .chain(properties())
        .map(|(_, fact)| fact)
        .collect();
    for name in &expected {
        let Some(fact) = published.and_then(|facts| facts.get(*name)) else {
            failures.push(format!("{CAPABILITY}: facts do not declare {name}"));
            continue;
        };
        if fact.get("const") != Some(&json!(true)) {
            failures.push(format!(
                "{CAPABILITY}: {name} must be const true, never false"
            ));
        }
        if fact.get("x-b10x-published-only-from") != Some(&json!(PUBLISHED_ONLY_FROM)) {
            failures.push(format!(
                "{CAPABILITY}: {name} must be published only from a verified mechanism"
            ));
        }
        if required.is_some_and(|required| required.contains(&json!(name))) {
            failures.push(format!("{CAPABILITY}: {name} must be optional"));
        }
    }
    for name in published
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|facts| facts.keys())
        .filter(|name| name.starts_with("confinement."))
    {
        if name != wire::CONFINEMENT_NO_EGRESS_FACT && !expected.contains(&name.as_str()) {
            failures.push(format!(
                "{CAPABILITY}: {name} is published and the wire crate names no such fact"
            ));
        }
    }
}

/// Design 30 § 4.4: the closed `requires` object, each list exactly the wire enum in its order,
/// unique and bounded by its size, the object non-empty, and its two refusals annotated as the
/// wire rows are.
fn check_requires(released: &Tree, failures: &mut Vec<String>) {
    let Some(common) = json_at(released, COMMON, failures) else {
        return;
    };
    let Some(definition) = common.pointer(&format!("/$defs/{DEFINITION}")) else {
        failures.push(format!("{COMMON}: no {DEFINITION} definition"));
        return;
    };
    if definition.get("additionalProperties") != Some(&json!(false))
        || definition.get("minProperties") != Some(&json!(1))
        || definition.get("type") != Some(&json!("object"))
    {
        failures.push(format!(
            "{COMMON}: {DEFINITION} must be a closed, non-empty object"
        ));
    }
    let members: BTreeSet<&str> = definition
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|members| members.keys())
        .map(String::as_str)
        .collect();
    if members != BTreeSet::from(["families", "facts"]) {
        failures.push(format!(
            "{COMMON}: {DEFINITION} must have exactly families and facts, has {members:?}"
        ));
    }
    for (member, words) in [
        (
            "families",
            families()
                .into_iter()
                .map(|(word, _)| word)
                .collect::<Vec<_>>(),
        ),
        (
            "facts",
            properties().into_iter().map(|(word, _)| word).collect(),
        ),
    ] {
        let list = definition.pointer(&format!("/properties/{member}"));
        let expected = json!({
            "items": { "enum": words },
            "maxItems": words.len(),
            "minItems": 1,
            "type": "array",
            "uniqueItems": true,
        });
        if list != Some(&expected) {
            failures.push(format!(
                "{COMMON}: {DEFINITION}.{member} must be exactly {expected}, the wire enum in \
                 its order"
            ));
        }
    }
    let expected: Vec<Value> = wire::CONFINEMENT_UNSERVED_REFUSALS
        .iter()
        .map(|refusal| {
            json!({
                "class": wire::UnservedRefusal::CLASS,
                "code": refusal.code,
                "status": wire::UnservedRefusal::STATUS,
            })
        })
        .collect();
    if definition.get("x-b10x-refusals") != Some(&Value::Array(expected.clone())) {
        failures.push(format!(
            "{COMMON}: {DEFINITION} must annotate exactly the refusals {expected:?}"
        ));
    }
}

/// Both starts carry `requires` as an optional member of their sandbox.
fn check_starts(released: &Tree, failures: &mut Vec<String>) {
    let reference = json!(format!("../common.json#/$defs/{DEFINITION}"));
    for (path, sandbox) in STARTS {
        let Some(start) = json_at(released, path, failures) else {
            continue;
        };
        let Some(sandbox) = start.pointer(sandbox) else {
            failures.push(format!("{path}: no sandbox"));
            continue;
        };
        if sandbox.pointer("/properties/requires/$ref") != Some(&reference) {
            failures.push(format!("{path}: sandbox.requires must be {reference}"));
        }
        if sandbox
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| required.contains(&json!("requires")))
        {
            failures.push(format!("{path}: sandbox.requires must be optional"));
        }
    }
}

/// Every object schema that declares all of `members`, keyed by its JSON pointer, so each `oneOf`
/// branch and each inline copy is checked on its own.
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

/// The applied shapes: each states the applied cgroup.
const APPLIED: [&str; 2] = ["capability_snapshot", "cgroup"];

/// The requested shapes: each echoes the start's sandbox, which alone states `require`.
const REQUESTED: [&str; 2] = ["capability_snapshot", "require"];

/// Design 30 § 4.4 and § 4.5 on the exec record: every requested sandbox admits `requires`, every
/// applied shape echoes it and no longer requires `cgroup`, and a record the wire crate emits with
/// both, and without a cgroup, validates against the exec result schema.
fn check_record(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let Some(resource) = json_at(released, RESOURCE, failures) else {
        return;
    };
    let reference = json!(format!("common.json#/$defs/{DEFINITION}"));
    let mut applied = Vec::new();
    shapes_with(&resource, &APPLIED, "", &mut applied);
    let mut requested = Vec::new();
    shapes_with(&resource, &REQUESTED, "", &mut requested);
    if applied.is_empty() || requested.is_empty() {
        failures.push(format!(
            "{RESOURCE}: no applied or requested confinement shape"
        ));
    }
    for (at, shape) in applied.iter().chain(&requested) {
        if shape.pointer("/properties/requires/$ref") != Some(&reference) {
            failures.push(format!(
                "{RESOURCE}: {at} does not carry requires as {reference}"
            ));
        }
    }
    for (at, shape) in &applied {
        if shape
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| required.contains(&json!("cgroup")))
        {
            failures.push(format!("{RESOURCE}: {at} still requires cgroup"));
        }
    }
    let record = match wire_record() {
        Ok(record) => record,
        Err(error) => {
            failures.push(format!(
                "the requires exec record is not one the wire emits: {error}"
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
            "schemas/results/exec-start.json: rejects the wire exec record with requires and \
             without cgroup: {error} at {}",
            error.instance_path()
        ));
    }
}

/// A running exec record whose start required every family and property and whose applied record
/// echoes that and names no cgroup: parsed into `substrate_wire::Exec` and serialized back, so it
/// is a record the daemon can emit.
fn wire_record() -> Result<Value, String> {
    let requires = json!({
        "families": families().into_iter().map(|(word, _)| word).collect::<Vec<_>>(),
        "facts": properties().into_iter().map(|(word, _)| word).collect::<Vec<_>>(),
    });
    let snapshot = format!("sha256:{}", "7".repeat(64));
    let record = json!({
        "id": "ex_vector",
        "kind": "exec",
        "workspace": "ws_vector",
        "state": "running",
        "observed_at": "2026-10-31T12:00:00Z",
        "requested": {
            "capability_snapshot": snapshot,
            "network": "none",
            "profile": "workspace",
            "require": true,
            "requires": requires,
        },
        "applied": {
            "capability_snapshot": snapshot,
            "filesystem": "workspace-rw-system-ro",
            "network": "none",
            "profile": "workspace",
            "requires": requires,
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

/// One vector answers exactly the row: the operation, 501, `unserved`, the code, the address, the
/// message `confinement_requires_refusal` computes from the vector's request and machine, not
/// retriable, recorded as refused and never dispatched.
fn check_refusal_vector(
    released: &Tree,
    refusal: &wire::UnservedRefusal,
    failures: &mut Vec<String>,
) {
    let code = refusal.code;
    let mut found = 0_usize;
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        let error = vector.pointer("/expected/response/body/error");
        if error.and_then(|error| error.get("code")) != Some(&json!(code)) {
            continue;
        }
        found += 1;
        if vector.pointer("/action/operation") != Some(&json!(refusal.operation)) {
            failures.push(format!(
                "{path}: {code} is answered for {}, not {}",
                vector.pointer("/action/operation").unwrap_or(&Value::Null),
                refusal.operation
            ));
        }
        if vector.pointer("/expected/response/status")
            != Some(&json!(wire::UnservedRefusal::STATUS))
        {
            failures.push(format!(
                "{path}: {code} must be answered {}",
                wire::UnservedRefusal::STATUS
            ));
        }
        let message = match computed(&vector) {
            Ok((row, message)) if row == *refusal => message,
            Ok((row, _)) => {
                failures.push(format!(
                    "{path}: {code} is answered where the wire refuses {}",
                    row.code
                ));
                continue;
            }
            Err(reason) => {
                failures.push(format!("{path}: {code} {reason}"));
                continue;
            }
        };
        let mut expected = json!({
            "class": wire::UnservedRefusal::CLASS,
            "code": code,
            "message": message,
            "retriable": false,
        });
        if let Some(address) = refusal.address {
            expected["address"] = json!(address);
        }
        if let (Some(operation), Some(object)) = (
            error.and_then(|error| error.get("operation")),
            expected.as_object_mut(),
        ) {
            object.insert("operation".to_owned(), operation.clone());
        }
        if error != Some(&expected) {
            failures.push(format!(
                "{path}: {code} must be answered exactly {expected}"
            ));
        }
        for (actual, value, what) in [
            (
                "/driver/dispatch_count",
                json!(0),
                "that nothing was dispatched",
            ),
            (
                "/probes/operation_state",
                json!("refused"),
                "that it was recorded as refused",
            ),
            (
                "/probes/operation_row_count_delta",
                json!(1),
                "that one operation row was recorded",
            ),
        ] {
            let stated = vector
                .get("postconditions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|condition| {
                    condition.get("actual") == Some(&json!(actual))
                        && condition.get("operator") == Some(&json!("equals"))
                        && condition.get("expected") == Some(&value)
                });
            if !stated {
                failures.push(format!("{path}: {code} must state {what}"));
            }
        }
    }
    if found != 1 {
        failures.push(format!(
            "vectors: {code} must be answered by exactly one vector, found {found}"
        ));
    }
}

/// What the wire crate answers for the vector: its start parsed as `ExecStartInput` with a valid
/// `requires`, against the facts of its one machine, whose snapshot the start names.
fn computed(vector: &Value) -> Result<(wire::UnservedRefusal, String), String> {
    let input = vector
        .pointer("/action/request/body/input")
        .cloned()
        .ok_or("has no request input")?;
    let start: wire::ExecStartInput = serde_json::from_value(input).map_err(|error| {
        format!("is answered for a request the wire crate does not admit: {error}")
    })?;
    let requires = start
        .sandbox
        .requires
        .as_ref()
        .ok_or("is answered for a start without sandbox.requires")?;
    wire::validate_confinement_requires(requires).map_err(|error| {
        format!("is answered for a requires the wire crate refuses as malformed: {error}")
    })?;
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
    if machine.get("snapshot") != Some(&json!(start.sandbox.capability_snapshot)) {
        return Err("names a snapshot its machine does not hold".to_owned());
    }
    let facts: wire::CapabilityFacts = serde_json::from_value(
        machine.get("facts").cloned().unwrap_or(Value::Null),
    )
    .map_err(|error| format!("sets up machine facts the wire crate does not read: {error}"))?;
    wire::confinement_requires_refusal(requires, &facts)
        .ok_or_else(|| "is answered for a start its machine serves".to_owned())
}

#[cfg(test)]
mod tests {
    use crate::repo;

    const VERSION: &str = "0.26.0";
    const FAMILY: &str = "vectors/http/confinement-family-unserved-by-the-snapshot.json";
    const FACT: &str = "vectors/http/confinement-fact-unserved-by-the-snapshot.json";

    fn tree(version: &str) -> super::Tree {
        super::super::tree_of(
            &repo::root()
                .expect("root")
                .join("contracts/substrate-wire")
                .join(version),
        )
        .expect("the bundle reads")
    }

    fn released() -> super::Tree {
        tree(VERSION)
    }

    fn failures_of(tree: &super::Tree) -> Vec<String> {
        let mut failures = Vec::new();
        super::super::check_additions(VERSION, tree, &mut failures);
        failures
    }

    fn edited(path: &str, edit: impl FnOnce(&mut serde_json::Value)) -> super::Tree {
        let mut tree = released();
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

    fn machine_facts(vector: &mut serde_json::Value) -> &mut serde_json::Value {
        vector["setup"]
            .as_array_mut()
            .expect("setup")
            .iter_mut()
            .find(|step| step["kind"] == "machine")
            .map(|step| &mut step["state"]["facts"])
            .expect("a machine")
    }

    #[test]
    fn the_successor_carries_the_confinement_facts() {
        let failures = failures_of(&released());
        assert!(failures.is_empty(), "{failures:?}");
    }

    /// A check that passes on the tree that predates the change proves nothing: `0.25.0` names no
    /// family or property fact, no `requires`, still requires `cgroup` and publishes neither code.
    #[test]
    fn the_predecessor_does_not_carry_them() {
        let mut failures = Vec::new();
        super::check("0.25.0", &tree("0.25.0"), &mut failures);
        assert_reported(
            &failures,
            "facts do not declare confinement.family.process-run",
        );
        assert_reported(&failures, "facts do not declare confinement.private-tmpdir");
        assert_reported(&failures, "no confinement-requires definition");
        assert_reported(&failures, "sandbox.requires must be");
        assert_reported(&failures, "still requires cgroup");
        for code in substrate_wire::CONFINEMENT_UNSERVED_REFUSALS.map(|refusal| refusal.code) {
            assert_reported(
                &failures,
                &format!("{code} must be answered by exactly one vector, found 0"),
            );
        }
    }

    #[test]
    fn a_fact_that_can_be_false_is_refused() {
        let tree = edited(super::CAPABILITY, |capability| {
            capability["properties"]["facts"]["properties"]["confinement.family.limits"] =
                serde_json::json!({ "type": "boolean" });
        });
        assert_reported(
            &failures_of(&tree),
            "confinement.family.limits must be const true",
        );
    }

    #[test]
    fn a_required_fact_is_refused() {
        let tree = edited(super::CAPABILITY, |capability| {
            capability["properties"]["facts"]["required"]
                .as_array_mut()
                .expect("required")
                .push("confinement.whole-tree-kill".into());
        });
        assert_reported(
            &failures_of(&tree),
            "confinement.whole-tree-kill must be optional",
        );
    }

    #[test]
    fn a_fact_the_wire_does_not_name_is_refused() {
        let tree = edited(super::CAPABILITY, |capability| {
            capability["properties"]["facts"]["properties"]["confinement.family.gpu"] =
                serde_json::json!({ "const": true });
        });
        assert_reported(
            &failures_of(&tree),
            "confinement.family.gpu is published and the wire crate names no such fact",
        );
    }

    #[test]
    fn a_family_enum_out_of_order_is_refused() {
        let tree = edited(super::COMMON, |common| {
            common["$defs"]["confinement-requires"]["properties"]["families"]["items"]["enum"]
                .as_array_mut()
                .expect("enum")
                .swap(0, 1);
        });
        assert_reported(
            &failures_of(&tree),
            "confinement-requires.families must be exactly",
        );
    }

    #[test]
    fn a_property_enum_missing_a_word_is_refused() {
        let tree = edited(super::COMMON, |common| {
            let list = &mut common["$defs"]["confinement-requires"]["properties"]["facts"];
            list["items"]["enum"].as_array_mut().expect("enum").pop();
            list["maxItems"] = 6.into();
        });
        assert_reported(
            &failures_of(&tree),
            "confinement-requires.facts must be exactly",
        );
    }

    #[test]
    fn an_open_requires_object_is_refused() {
        let tree = edited(super::COMMON, |common| {
            common["$defs"]["confinement-requires"]
                .as_object_mut()
                .expect("object")
                .remove("additionalProperties");
        });
        assert_reported(&failures_of(&tree), "must be a closed, non-empty object");
    }

    #[test]
    fn a_requires_annotated_with_another_status_is_refused() {
        let tree = edited(super::COMMON, |common| {
            common["$defs"]["confinement-requires"]["x-b10x-refusals"][1]["status"] = 422.into();
        });
        assert_reported(&failures_of(&tree), "must annotate exactly the refusals");
    }

    #[test]
    fn a_session_start_without_requires_is_refused() {
        let tree = edited(super::STARTS[1].0, |start| {
            start["properties"]["exec"]["properties"]["sandbox"]["properties"]
                .as_object_mut()
                .expect("properties")
                .remove("requires");
        });
        assert_reported(
            &failures_of(&tree),
            "pipe-session-start.json: sandbox.requires must be",
        );
    }

    /// Every closed shape that declares `cgroup` requires it again, as `0.25.0` did.
    fn require_cgroup(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                if object
                    .get("properties")
                    .is_some_and(|properties| properties.get("cgroup").is_some())
                    && let Some(required) =
                        object.get_mut("required").and_then(|r| r.as_array_mut())
                {
                    required.push("cgroup".into());
                }
                object.values_mut().for_each(require_cgroup);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(require_cgroup),
            _ => {}
        }
    }

    #[test]
    fn an_applied_shape_that_still_requires_cgroup_is_refused() {
        let tree = edited(super::RESOURCE, require_cgroup);
        let failures = failures_of(&tree);
        assert_reported(&failures, "still requires cgroup");
        assert_reported(
            &failures,
            "rejects the wire exec record with requires and without cgroup",
        );
    }

    #[test]
    fn an_applied_shape_without_the_echo_is_refused() {
        let tree = edited(super::RESOURCE, |resource| {
            resource["$defs"]["exec"]["oneOf"][1]["properties"]["applied"]["oneOf"][0]
                ["properties"]
                .as_object_mut()
                .expect("properties")
                .remove("requires");
        });
        assert_reported(&failures_of(&tree), "does not carry requires");
    }

    #[test]
    fn a_message_naming_another_family_is_refused() {
        let tree = edited(FAMILY, |vector| {
            vector["expected"]["response"]["body"]["error"]["message"] =
                "Confinement family process-run is not served by this driver.".into();
        });
        assert_reported(
            &failures_of(&tree),
            "confinement.family-unserved must be answered exactly",
        );
    }

    #[test]
    fn a_refusal_for_a_family_the_machine_serves_is_refused() {
        let tree = edited(FAMILY, |vector| {
            machine_facts(vector)["confinement.family.network-ingress"] = true.into();
        });
        assert_reported(
            &failures_of(&tree),
            "confinement.family-unserved is answered for a start its machine serves",
        );
    }

    #[test]
    fn a_fact_refusal_where_a_family_is_unserved_first_is_refused() {
        let tree = edited(FACT, |vector| {
            machine_facts(vector)
                .as_object_mut()
                .expect("facts")
                .remove("confinement.family.limits");
        });
        assert_reported(
            &failures_of(&tree),
            "confinement.fact-unserved is answered where the wire refuses confinement.family-unserved",
        );
    }

    #[test]
    fn a_duplicate_requirement_is_refused() {
        let tree = edited(FACT, |vector| {
            vector["action"]["request"]["body"]["input"]["sandbox"]["requires"]["facts"] =
                serde_json::json!(["sealed-paths", "sealed-paths"]);
        });
        assert_reported(
            &failures_of(&tree),
            "is answered for a requires the wire crate refuses as malformed",
        );
    }

    #[test]
    fn an_unknown_requirement_is_refused() {
        let tree = edited(FAMILY, |vector| {
            vector["action"]["request"]["body"]["input"]["sandbox"]["requires"]["families"] =
                serde_json::json!(["gpu"]);
        });
        assert_reported(
            &failures_of(&tree),
            "is answered for a request the wire crate does not admit",
        );
    }

    #[test]
    fn a_vector_naming_a_snapshot_its_machine_does_not_hold_is_refused() {
        let tree = edited(FACT, |vector| {
            vector["action"]["request"]["body"]["input"]["sandbox"]["capability_snapshot"] =
                format!("sha256:{}", "8".repeat(64)).into();
        });
        assert_reported(
            &failures_of(&tree),
            "names a snapshot its machine does not hold",
        );
    }

    #[test]
    fn a_refusal_that_is_not_recorded_is_refused() {
        let tree = edited(FAMILY, |vector| {
            vector["postconditions"]
                .as_array_mut()
                .expect("conditions")
                .retain(|condition| condition["actual"] != "/probes/operation_state");
        });
        assert_reported(
            &failures_of(&tree),
            "confinement.family-unserved must state that it was recorded as refused",
        );
    }

    #[test]
    fn a_dispatched_refusal_is_refused() {
        let tree = edited(FACT, |vector| {
            for condition in vector["postconditions"].as_array_mut().expect("conditions") {
                if condition["actual"] == "/driver/dispatch_count" {
                    condition["expected"] = 1.into();
                }
            }
        });
        assert_reported(
            &failures_of(&tree),
            "confinement.fact-unserved must state that nothing was dispatched",
        );
    }

    #[test]
    fn an_address_other_than_the_rows_is_refused() {
        let tree = edited(FACT, |vector| {
            vector["expected"]["response"]["body"]["error"]["address"] = "sandbox".into();
        });
        assert_reported(
            &failures_of(&tree),
            "confinement.fact-unserved must be answered exactly",
        );
    }

    #[test]
    fn a_refusal_answered_by_another_status_is_refused() {
        let tree = edited(FAMILY, |vector| {
            vector["expected"]["response"]["status"] = 422.into();
        });
        assert_reported(&failures_of(&tree), "must be answered 501");
    }

    #[test]
    fn a_missing_vector_is_refused() {
        let mut tree = released();
        tree.remove(FACT);
        assert_reported(
            &failures_of(&tree),
            "confinement.fact-unserved must be answered by exactly one vector, found 0",
        );
    }

    #[test]
    fn a_compatibility_block_that_omits_a_change_is_refused() {
        for named in [
            "confinement.family.audit",
            "confinement.fact-unserved",
            "cgroup",
        ] {
            let tree = edited("compatibility.json", |compatibility| {
                for change in compatibility["client_visible_changes"]
                    .as_array_mut()
                    .expect("changes")
                {
                    let text = change.as_str().expect("text").replace(named, "x");
                    *change = text.into();
                }
            });
            assert_reported(
                &failures_of(&tree),
                &format!("client_visible_changes must state {named}"),
            );
        }
    }
}
