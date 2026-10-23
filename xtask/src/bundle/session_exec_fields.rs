//! What `0.23.0` adds for design 27: the session start publishes every exec field exec start does.
//!
//! `PipeSessionStartInput.exec` is the wire crate's `ExecStartInput`, so a session start decodes
//! every member an exec start decodes, and the daemon admits and the driver applies each of them on
//! both routes. Bundles up to `0.22.0` left `scratch`, `measurements` and `workspace_access` off the
//! session-start `exec` object, so a bundle-only client could not request them on a session.
//!
//! The rule is checked for the class, not for the three instances: the session-start `exec` object
//! declares exactly the exec-start property set, and every shared member is the same schema except
//! those [`narrowed`] names, which a session restricts on purpose. The object's own
//! `additionalProperties` is exec start's, and its `required` list is exec start's plus
//! [`SESSION_REQUIRES`] (amendment 1). A future exec-start field left off the session schema fails
//! here, in `0.23.0` and in every later bundle ([`applies_to`]).
//!
//! The refusals the three members raise are published on both starts and held to the wire crate's
//! [`wire::EXEC_FIELD_REFUSALS`] (amendment 2), and the wire type refuses the values the schema
//! refuses (amendment 3).

use std::collections::BTreeSet;

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const EXEC_START: &str = "schemas/inputs/exec-start.json";
const SESSION_START: &str = "schemas/inputs/pipe-session-start.json";
const SESSION_EXEC: &str = "/properties/exec";

/// The three members design 27 adds to the session start.
const ADDED: [&str; 3] = ["scratch", "measurements", "workspace_access"];

/// What a session's exec requires beyond exec start: a session is always leased
/// (`validate_pipe_session_input` refuses one without `lease_ttl_ms`).
const SESSION_REQUIRES: [&str; 1] = ["lease_ttl_ms"];

/// The first bundle the class check binds.
const FIRST: (u64, u64, u64) = (0, 23, 0);

/// The schema a member a session narrows must have there, or `None` for a shared member: a
/// session's exec never waits.
fn narrowed(name: &str) -> Option<Value> {
    (name == "wait").then(|| json!({ "const": false }))
}

/// Whether `version` is `0.23.0` or later, so design 27's class check binds it.
pub(super) fn applies_to(version: &str) -> bool {
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => (major, minor, patch) >= FIRST,
        _ => false,
    }
}

/// The class check, for `0.23.0` and every later bundle.
pub(super) fn check(released: &Tree, failures: &mut Vec<String>) {
    check_members(released, failures);
    check_object(released, failures);
    check_refusals(released, failures);
    check_wire_admits(failures);
}

/// What only the bundle that introduced the change states: its client-visible change.
pub(super) fn check_introduction(released: &Tree, failures: &mut Vec<String>) {
    check_client_visible_change(released, failures);
}

fn names(list: Option<&Value>) -> BTreeSet<String> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

/// The session's exec object is as closed as exec start and requires what exec start does, plus
/// the lease.
fn check_object(released: &Tree, failures: &mut Vec<String>) {
    let (Some(exec), Some(session)) = (
        json_at(released, EXEC_START, failures),
        json_at(released, SESSION_START, failures),
    ) else {
        return;
    };
    let Some(session) = session.pointer(SESSION_EXEC) else {
        failures.push(format!("{SESSION_START}: no exec object"));
        return;
    };
    if session.get("additionalProperties") != exec.get("additionalProperties") {
        failures.push(format!(
            "{SESSION_START}: exec additionalProperties is not exec start's ({})",
            exec.get("additionalProperties").unwrap_or(&Value::Null)
        ));
    }
    let mut expected = names(exec.get("required"));
    expected.extend(SESSION_REQUIRES.iter().map(|name| (*name).to_owned()));
    let required = names(session.get("required"));
    for missing in expected.difference(&required) {
        failures.push(format!(
            "{SESSION_START}: exec does not require {missing}, which a session start must name"
        ));
    }
    for extra in required.difference(&expected) {
        failures.push(format!(
            "{SESSION_START}: exec requires {extra}, which exec start leaves optional"
        ));
    }
    let common = "schemas/common.json";
    if let Some(document) = json_at(released, common, failures)
        && document.pointer("/$defs/storage-limit/properties/max_bytes/multipleOf")
            != Some(&json!(wire::STORAGE_QUOTA_BYTES_UNIT))
    {
        failures.push(format!(
            "{common}: storage-limit max_bytes is not a multiple of the wire unit {}",
            wire::STORAGE_QUOTA_BYTES_UNIT
        ));
    }
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

/// Each start publishes exactly the wire crate's refusals on each of the three members.
fn check_refusals(released: &Tree, failures: &mut Vec<String>) {
    for (path, object) in [(EXEC_START, ""), (SESSION_START, SESSION_EXEC)] {
        let Some(document) = json_at(released, path, failures) else {
            continue;
        };
        for member in ADDED {
            let expected: BTreeSet<(String, u64, String)> = wire::EXEC_FIELD_REFUSALS
                .iter()
                .filter(|row| row.member == member)
                .map(|row| {
                    (
                        row.code.to_owned(),
                        u64::from(row.status),
                        row.class.to_owned(),
                    )
                })
                .collect();
            let published = declared(
                document.pointer(&format!("{object}/properties/{member}/x-b10x-refusals")),
            );
            for (code, status, class) in expected.difference(&published) {
                failures.push(format!(
                    "{path}: {member} does not declare {code} ({status}, {class})"
                ));
            }
            for (code, status, class) in published.difference(&expected) {
                failures.push(format!(
                    "{path}: {member} declares {code} ({status}, {class}), which is not a refusal \
                     {member} raises"
                ));
            }
        }
    }
}

fn properties(document: &Value, pointer: &str) -> Option<serde_json::Map<String, Value>> {
    document
        .pointer(&format!("{pointer}/properties"))
        .and_then(Value::as_object)
        .cloned()
}

/// The session's exec object has exactly exec start's members, each the same schema.
fn check_members(released: &Tree, failures: &mut Vec<String>) {
    let (Some(exec), Some(session)) = (
        json_at(released, EXEC_START, failures),
        json_at(released, SESSION_START, failures),
    ) else {
        return;
    };
    let Some(exec) = properties(&exec, "") else {
        failures.push(format!("{EXEC_START}: no properties"));
        return;
    };
    let Some(session) = properties(&session, SESSION_EXEC) else {
        failures.push(format!("{SESSION_START}: no exec properties"));
        return;
    };
    let exec_names: BTreeSet<&String> = exec.keys().collect();
    let session_names: BTreeSet<&String> = session.keys().collect();
    for missing in exec_names.difference(&session_names) {
        failures.push(format!(
            "{SESSION_START}: exec lacks {missing}, which exec start publishes and a session \
             start admits"
        ));
    }
    for extra in session_names.difference(&exec_names) {
        failures.push(format!(
            "{SESSION_START}: exec declares {extra}, which exec start does not"
        ));
    }
    for (name, schema) in &session {
        if let Some(expected) = narrowed(name) {
            if *schema != expected {
                failures.push(format!("{SESSION_START}: exec {name} must be {expected}"));
            }
            continue;
        }
        if exec.get(name).is_some_and(|published| published != schema) {
            failures.push(format!(
                "{SESSION_START}: exec {name} is not the schema exec start publishes"
            ));
        }
    }
}

/// The wire crate's own session type admits each added member, so publishing it states what the
/// daemon decodes rather than a request it would refuse as unknown.
fn check_wire_admits(failures: &mut Vec<String>) {
    let session = json!({
        "exec": {
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
            "lease_ttl_ms": 60_000,
            "scratch": { "max_bytes": 1_048_576, "max_inodes": 16 },
            "measurements": ["resource-usage"],
            "workspace_access": { "mode": "read-only" }
        },
        "input_limit_bytes": 1024,
        "frame_limit_bytes": 1024,
        "queued_frames": 1
    });
    // Amendment 3: a value every published start schema refuses is one the wire type refuses too,
    // so the daemon answers it `request.schema-invalid` rather than running something else.
    for (field, value) in [
        ("scratch", Value::Null),
        ("measurements", json!(["resource-usage", "resource-usage"])),
    ] {
        let mut refused = session.clone();
        refused["exec"][field] = value.clone();
        if serde_json::from_value::<wire::PipeSessionStartInput>(refused).is_ok() {
            failures.push(format!(
                "the wire crate's session start admits exec.{field} = {value}, which the \
                 published schema refuses"
            ));
        }
    }
    let misaligned = wire::StorageLimit {
        max_bytes: wire::MIN_STORAGE_QUOTA_BYTES + 1,
        max_inodes: wire::MIN_STORAGE_QUOTA_INODES,
    };
    if misaligned.within_contract_bounds() {
        failures.push(format!(
            "the wire crate admits a storage max_bytes off the {}-byte unit the schema states",
            wire::STORAGE_QUOTA_BYTES_UNIT
        ));
    }
    match serde_json::from_value::<wire::PipeSessionStartInput>(session) {
        Ok(start)
            if start.exec.scratch.is_some()
                && !start.exec.measurements.is_empty()
                && !start.exec.workspace_access.is_read_write() => {}
        Ok(_) => failures.push(format!(
            "the wire crate's session start drops one of {ADDED:?}"
        )),
        Err(error) => failures.push(format!(
            "the wire crate's session start refuses one of {ADDED:?}: {error}"
        )),
    }
}

fn check_client_visible_change(released: &Tree, failures: &mut Vec<String>) {
    let path = "compatibility.json";
    let Some(compatibility) = json_at(released, path, failures) else {
        return;
    };
    let stated = compatibility
        .get("client_visible_changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|change| {
            change.contains("design 27")
                && change.contains("session")
                && ADDED.iter().all(|field| change.contains(field))
        });
    if !stated {
        failures.push(format!(
            "{path}: client_visible_changes must state, in one entry citing design 27, that the \
             session start publishes {ADDED:?}"
        ));
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
        super::check(tree, &mut failures);
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
            "not reported: {named}: {failures:?}"
        );
    }

    /// Acceptance 1, the bundle half: the session start publishes every exec-start field.
    #[test]
    fn the_session_start_publishes_every_exec_start_field() {
        let failures = failures_of(&released(VERSION));
        assert!(failures.is_empty(), "{failures:?}");
    }

    /// The predecessor is the defect this exists for, so the check must name all three there.
    #[test]
    fn the_predecessor_lacks_all_three() {
        let failures = failures_of(&released("0.22.0"));
        for field in super::ADDED {
            assert_reported(&failures, &format!("exec lacks {field}, which exec start"));
        }
    }

    /// The class: any exec-start member dropped from the session start is reported by name.
    #[test]
    fn any_exec_start_member_missing_from_the_session_is_reported() {
        for field in ["workdir", "capsule", "secret_slots", "workspace_access"] {
            let tree = edited(super::SESSION_START, |document| {
                document["properties"]["exec"]["properties"]
                    .as_object_mut()
                    .expect("object")
                    .remove(field);
            });
            assert_reported(&failures_of(&tree), &format!("exec lacks {field}"));
        }
    }

    /// A shared member that drifts from exec start, or a narrowing that is lost, is reported.
    #[test]
    fn a_drifted_or_unnarrowed_member_is_reported() {
        let tree = edited(super::SESSION_START, |document| {
            document["properties"]["exec"]["properties"]["measurements"]["maxItems"] = 2.into();
        });
        assert_reported(
            &failures_of(&tree),
            "exec measurements is not the schema exec start publishes",
        );
        let tree = edited(super::SESSION_START, |document| {
            document["properties"]["exec"]["properties"]["wait"] =
                serde_json::json!({ "type": "boolean" });
        });
        assert_reported(&failures_of(&tree), "exec wait must be");
        let tree = edited(super::SESSION_START, |document| {
            document["properties"]["exec"]["properties"]["unheard_of"] =
                serde_json::json!({ "type": "boolean" });
        });
        assert_reported(
            &failures_of(&tree),
            "exec declares unheard_of, which exec start does not",
        );
    }

    /// Amendment 1: the object's own `required` list and `additionalProperties` are compared too.
    #[test]
    fn a_changed_required_list_or_an_open_object_is_reported() {
        let tree = edited(super::SESSION_START, |document| {
            document["properties"]["exec"]["required"]
                .as_array_mut()
                .expect("required list")
                .push("workspace_access".into());
        });
        assert_reported(&failures_of(&tree), "exec requires workspace_access");
        let tree = edited(super::SESSION_START, |document| {
            document["properties"]["exec"]["required"]
                .as_array_mut()
                .expect("required list")
                .retain(|name| name != "argv");
        });
        assert_reported(&failures_of(&tree), "exec does not require argv");
        let tree = edited(super::SESSION_START, |document| {
            document["properties"]["exec"]["required"]
                .as_array_mut()
                .expect("required list")
                .retain(|name| name != "lease_ttl_ms");
        });
        assert_reported(&failures_of(&tree), "exec does not require lease_ttl_ms");
        let tree = edited(super::SESSION_START, |document| {
            document["properties"]["exec"]["additionalProperties"] = true.into();
        });
        assert_reported(
            &failures_of(&tree),
            "exec additionalProperties is not exec start's",
        );
    }

    /// Amendment 2: each refusal the three members raise is published on both starts.
    #[test]
    fn the_refusals_of_the_three_members_are_published_on_both_starts() {
        let tree = released(VERSION);
        for (path, prefix) in [
            (super::EXEC_START, ""),
            (super::SESSION_START, "/properties/exec"),
        ] {
            let document: serde_json::Value =
                serde_json::from_slice(tree.get(path).expect("present")).expect("parses");
            for (member, code) in [
                ("measurements", "exec.metrics-unserved"),
                ("scratch", "exec.scratch-quota-unserved"),
                ("scratch", "exec.scratch-quota-limit"),
                ("workspace_access", "exec.workspace-access-invalid"),
                ("workspace_access", "exec.workspace-scoped-write-unserved"),
                ("workspace_access", "exec.workspace-subtree-absent"),
                ("workspace_access", "exec.workspace-subtree-symlink"),
                ("workspace_access", "exec.workspace-subtree-not-directory"),
                ("workspace_access", "exec.workspace-subtree-changed"),
            ] {
                let rows = document
                    .pointer(&format!("{prefix}/properties/{member}/x-b10x-refusals"))
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    rows.iter().any(|row| row["code"] == code),
                    "{path}: {member} does not publish {code}"
                );
            }
        }
    }

    /// A member that loses a published refusal, or gains one it never raises, is reported.
    #[test]
    fn a_missing_or_invented_refusal_is_reported() {
        let tree = edited(super::EXEC_START, |document| {
            document["properties"]["scratch"]
                .as_object_mut()
                .expect("object")
                .remove("x-b10x-refusals");
        });
        assert_reported(
            &failures_of(&tree),
            "scratch does not declare exec.scratch-quota-unserved (501, unserved)",
        );
        let tree = edited(super::SESSION_START, |document| {
            document["properties"]["exec"]["properties"]["measurements"]["x-b10x-refusals"] = serde_json::json!([
                { "class": "unserved", "code": "exec.metrics-unserved", "status": 501 },
                { "class": "refused", "code": "exec.metrics-invented", "status": 422 }
            ]);
        });
        assert_reported(
            &failures_of(&tree),
            "measurements declares exec.metrics-invented (422, refused)",
        );
    }

    /// Amendment 1: the class check binds every bundle from `0.23.0` on, not `0.23.0` alone.
    #[test]
    fn every_later_bundle_runs_the_class_check() {
        let mut failures = Vec::new();
        super::super::check_additions("0.24.0", &released("0.22.0"), &mut failures);
        assert_reported(&failures, "exec lacks scratch, which exec start");
        let mut failures = Vec::new();
        super::super::check_additions("1.0.0", &released("0.22.0"), &mut failures);
        assert_reported(&failures, "exec lacks scratch, which exec start");
    }

    #[test]
    fn an_unstated_client_visible_change_is_reported() {
        let tree = edited("compatibility.json", |document| {
            document["client_visible_changes"]
                .as_array_mut()
                .expect("array")
                .retain(|change| {
                    !change
                        .as_str()
                        .is_some_and(|text| text.contains("design 27"))
                });
        });
        assert_reported(
            &failures_of(&tree),
            "client_visible_changes must state, in one entry citing design 27",
        );
    }
}
