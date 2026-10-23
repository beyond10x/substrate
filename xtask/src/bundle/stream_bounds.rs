//! What `0.23.0` adds for ADR 0042: the caps, lifetime and refusal of every upgraded stream
//! family, published as capability facts.
//!
//! The event stream, the metrics stream and the session attachment each refused a caller over its
//! cap with a named `429`, but the caps and the codes were stated only in daemon source and in a
//! guide (`story:the-metrics-stream-cap-is-machine-readable`). Since ADR 0042 these caps are also
//! what bounds an upgraded socket, because the transport hands its slot over at the upgrade, so a
//! client needs them as bytes the wire publishes.
//!
//! Every value this check expects comes from `substrate_wire`: the fact names, the caps, the
//! lifetime and the refusal codes, read through the very fact types the host probe publishes. The
//! bundle states them as literals, because `render_v2.rs` is hashed by every bundle it rendered and
//! cannot learn a new `{"$wire": …}` binding (AGENTS.md § *The gate*); the binding lives here,
//! which no bundle hashes.

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const CAPABILITY: &str = "schemas/capability.json";
const FACTS: &str = "/properties/facts";

/// The three facts, each with the value the wire crate publishes for it.
fn published() -> [(&'static str, Value); 3] {
    [
        (
            wire::EVENTS_STREAM_BOUNDS_FACT,
            serde_json::to_value(wire::StreamBoundsFacts::events()).unwrap_or(Value::Null),
        ),
        (
            wire::METRICS_STREAM_BOUNDS_FACT,
            serde_json::to_value(wire::StreamBoundsFacts::metrics()).unwrap_or(Value::Null),
        ),
        (
            wire::SESSIONS_ATTACHMENT_BOUNDS_FACT,
            serde_json::to_value(wire::AttachmentBoundsFacts::sessions()).unwrap_or(Value::Null),
        ),
    ]
}

/// The class check, for `0.23.0` and every later bundle: each fact stays published as the closed
/// object of the wire crate's values.
pub(super) fn check(released: &Tree, failures: &mut Vec<String>) {
    let Some(capability) = json_at(released, CAPABILITY, failures) else {
        return;
    };
    let Some(facts) = capability.pointer(FACTS) else {
        failures.push(format!("{CAPABILITY}: no facts object"));
        return;
    };
    for (name, value) in published() {
        check_fact(name, &value, facts, failures);
    }
}

/// What only the bundle that introduced the facts states: its client-visible change.
pub(super) fn check_introduction(released: &Tree, failures: &mut Vec<String>) {
    let path = "compatibility.json";
    let Some(compatibility) = json_at(released, path, failures) else {
        return;
    };
    let names = published().map(|(name, _)| name);
    let stated = compatibility
        .get("client_visible_changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|change| {
            change.contains("ADR 0042") && names.iter().all(|name| change.contains(name))
        });
    if !stated {
        failures.push(format!(
            "{path}: client_visible_changes must state, in one entry citing ADR 0042, that the \
             snapshot may publish {names:?}"
        ));
    }
}

/// One fact: declared, optional, closed, every member required, and every member the `const` the
/// wire crate publishes.
fn check_fact(name: &str, value: &Value, facts: &Value, failures: &mut Vec<String>) {
    let Some(fact) = facts
        .pointer("/properties")
        .and_then(|facts| facts.get(name))
    else {
        failures.push(format!("{CAPABILITY}: the {name} fact is not declared"));
        return;
    };
    if facts
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| required.contains(&json!(name)))
    {
        failures.push(format!(
            "{CAPABILITY}: {name} must stay optional, because a host publishes it only beside the \
             stream it bounds"
        ));
    }
    let Some(members) = value.as_object() else {
        failures.push(format!(
            "{name}: the wire crate's fact does not serialize as an object"
        ));
        return;
    };
    if fact.get("type") != Some(&json!("object"))
        || fact.get("additionalProperties") != Some(&json!(false))
    {
        failures.push(format!("{CAPABILITY}: {name} must be a closed object"));
    }
    let mut required = fact
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    required.sort_unstable();
    let mut expected = members.keys().map(String::as_str).collect::<Vec<_>>();
    expected.sort_unstable();
    if required != expected {
        failures.push(format!(
            "{CAPABILITY}: {name} must require exactly {expected:?}, it requires {required:?}"
        ));
    }
    let declared = fact
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            let mut keys = properties.keys().map(String::as_str).collect::<Vec<_>>();
            keys.sort_unstable();
            keys
        })
        .unwrap_or_default();
    if declared != expected {
        failures.push(format!(
            "{CAPABILITY}: {name} must declare exactly {expected:?}, it declares {declared:?}"
        ));
    }
    for (member, wire_value) in members {
        let stated = fact.pointer(&format!("/properties/{member}/const"));
        if stated != Some(wire_value) {
            failures.push(format!(
                "{CAPABILITY}: {name} {member} must be the const {wire_value}, the value \
                 substrate_wire publishes, not {stated:?}"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

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

    fn fact(document: &mut serde_json::Value, name: &str) -> serde_json::Value {
        document
            .pointer_mut("/properties/facts/properties")
            .and_then(|facts| facts.get_mut(name))
            .expect("the fact is declared")
            .take()
    }

    /// Acceptance, the bundle half: `0.23.0` publishes all three facts as the wire values.
    #[test]
    fn the_bundle_publishes_every_stream_bound_as_the_wire_value() {
        let failures = failures_of(&released(VERSION));
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn the_predecessor_publishes_none_of_them() {
        let failures = failures_of(&released("0.22.0"));
        for name in [
            "events.stream-bounds",
            "metrics.stream-bounds",
            "sessions.attachment-bounds",
        ] {
            assert_reported(&failures, &format!("the {name} fact is not declared"));
        }
    }

    #[test]
    fn a_cap_or_code_that_drifts_from_the_wire_crate_is_reported() {
        let tree = edited(super::CAPABILITY, |document| {
            let mut metrics = fact(document, "metrics.stream-bounds");
            metrics["properties"]["global"] = json!({ "const": 128 });
            *document
                .pointer_mut("/properties/facts/properties/metrics.stream-bounds")
                .expect("declared") = metrics;
            let mut events = fact(document, "events.stream-bounds");
            events["properties"]["refusal"] = json!({ "const": "event.capacity" });
            *document
                .pointer_mut("/properties/facts/properties/events.stream-bounds")
                .expect("declared") = events;
        });
        let failures = failures_of(&tree);
        assert_reported(
            &failures,
            "metrics.stream-bounds global must be the const 64",
        );
        assert_reported(
            &failures,
            "events.stream-bounds refusal must be the const \"event.stream-capacity\"",
        );
    }

    #[test]
    fn an_open_or_incomplete_fact_is_reported() {
        let tree = edited(super::CAPABILITY, |document| {
            let mut sessions = fact(document, "sessions.attachment-bounds");
            sessions["additionalProperties"] = json!(true);
            sessions["required"] = json!(["global"]);
            *document
                .pointer_mut("/properties/facts/properties/sessions.attachment-bounds")
                .expect("declared") = sessions;
        });
        let failures = failures_of(&tree);
        assert_reported(
            &failures,
            "sessions.attachment-bounds must be a closed object",
        );
        assert_reported(&failures, "sessions.attachment-bounds must require exactly");
    }

    #[test]
    fn a_required_fact_is_reported() {
        let tree = edited(super::CAPABILITY, |document| {
            document
                .pointer_mut("/properties/facts/required")
                .and_then(serde_json::Value::as_array_mut)
                .expect("required list")
                .push(json!("metrics.stream-bounds"));
        });
        assert_reported(
            &failures_of(&tree),
            "metrics.stream-bounds must stay optional",
        );
    }

    #[test]
    fn a_missing_client_visible_change_is_reported() {
        let tree = edited("compatibility.json", |document| {
            let changes = document["client_visible_changes"]
                .as_array_mut()
                .expect("changes");
            changes.retain(|change| {
                !change
                    .as_str()
                    .is_some_and(|text| text.contains("ADR 0042"))
            });
        });
        assert_reported(
            &failures_of(&tree),
            "client_visible_changes must state, in one entry citing ADR 0042",
        );
    }
}
