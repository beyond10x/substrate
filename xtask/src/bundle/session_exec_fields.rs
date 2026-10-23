//! What `0.23.0` adds for design 27: the session start publishes every exec field exec start does.
//!
//! `PipeSessionStartInput.exec` is the wire crate's `ExecStartInput`, so a session start decodes
//! every member an exec start decodes, and the daemon admits and the driver applies each of them on
//! both routes. Bundles up to `0.22.0` left `scratch`, `measurements` and `workspace_access` off the
//! session-start `exec` object, so a bundle-only client could not request them on a session.
//!
//! The rule is checked for the class, not for the three instances: the session-start `exec` object
//! declares exactly the exec-start property set, and every shared member is the same schema except
//! those [`narrowed`] names, which a session restricts on purpose. A future exec-start field left off the
//! session schema fails here.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const EXEC_START: &str = "schemas/inputs/exec-start.json";
const SESSION_START: &str = "schemas/inputs/pipe-session-start.json";
const SESSION_EXEC: &str = "/properties/exec";

/// The three members design 27 adds to the session start.
const ADDED: [&str; 3] = ["scratch", "measurements", "workspace_access"];

/// The schema a member a session narrows must have there, or `None` for a shared member: a
/// session's exec never waits.
fn narrowed(name: &str) -> Option<Value> {
    (name == "wait").then(|| json!({ "const": false }))
}

pub(super) fn check(released: &Tree, failures: &mut Vec<String>) {
    check_members(released, failures);
    check_wire_admits(failures);
    check_client_visible_change(released, failures);
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
