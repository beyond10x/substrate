//! What `0.24.0` exists for (ADR 0036 S12): the per-process event stream and the seven
//! `process.*` variants of the one event union.
//!
//! Every value this check expects comes from `substrate_wire`: the transitions, the line and
//! readers bounds, and the refusal's code, status, class and message. The bundle states them as
//! literals, because `render_v2.rs` is hashed by every bundle it rendered and cannot learn a new
//! `{"$wire": …}` binding (AGENTS.md § *The gate*); the binding lives here, which no bundle hashes.

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const ROUTE: &str = "process.events";
const PATH: &str = "/v1/processes/{process_id}/events";
const INPUT: &str = "schemas/inputs/process-events.json";
const RESULT: &str = "schemas/results/process-events.json";
const FRAME: &str = "schemas/process-stream-item.json";

/// The first bundle that publishes S12; every later one keeps it.
const FIRST: (u64, u64, u64) = (0, 24, 0);

pub(super) fn applies_to(version: &str) -> bool {
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => (major, minor, patch) >= FIRST,
        _ => false,
    }
}

/// The class check, for `0.24.0` and every later bundle.
pub(super) fn check(released: &Tree, failures: &mut Vec<String>) {
    check_route(released, failures);
    check_event_variant(released, failures);
    check_protocol(released, failures);
    check_frame(released, failures);
    check_refusal(released, failures);
    check_cursor(released, failures);
}

/// Cursors the daemon's reader and the published pattern must judge alike: the canonical form,
/// and a mutation of each of its parts, its separators and its length.
fn cursor_corpus() -> Vec<String> {
    let valid = wire::ProcessEventCursor {
        process: "proc_01JPROCVECTOR000000000001".to_owned(),
        source: "scope_ab-C_9".to_owned(),
        generation: 7,
        seq: 0,
        offset: 340,
    }
    .to_string();
    let mut corpus = vec![
        valid.clone(),
        "pe1.proc_a.scope_b.1.0.0".to_owned(),
        "pe1.proc_a.scope_b.18446744073709551615.18446744073709551615.18446744073709551615"
            .to_owned(),
        "pe1.proc_a.scope_b.18446744073709551616.0.0".to_owned(),
        String::new(),
        "pe1.".to_owned(),
        format!("{valid}.0"),
        format!("{valid}\n"),
        format!(" {valid}"),
        valid.replacen("pe1.", "pe2.", 1),
        valid.replacen("pe1.", "PE1.", 1),
        valid.replacen("proc_", "", 1),
        valid.replacen("proc_01JPROCVECTOR000000000001", "proc_", 1),
        valid.replacen("proc_01JPROCVECTOR000000000001", "proc_0-1", 1),
        valid.replacen("proc_01JPROCVECTOR000000000001.", "", 1),
        valid.replacen("scope_ab-C_9", "scope_", 1),
        valid.replacen("scope_ab-C_9", "scope_a.b", 1),
        valid.replacen("scope_ab-C_9", "scope_a+b", 1),
        valid.replacen("scope_ab-C_9", "other", 1),
        valid.replacen(".7.", ".0.", 1),
        valid.replacen(".7.", ".07.", 1),
        valid.replacen(".7.", "..", 1),
        valid.replacen(".7.0.", ".7.00.", 1),
        valid.replacen(".7.0.", ".7.-1.", 1),
        valid.replacen(".340", ".0340", 1),
        valid.replacen(".340", ".", 1),
        valid.replacen(".340", ".3a", 1),
        // Exactly at the length bound, and one character past it.
        format!(
            "pe1.proc_{}.scope_b.1.0.0",
            "a".repeat(wire::MAX_PROCESS_EVENT_CURSOR_CHARS - 23)
        ),
        format!(
            "pe1.proc_{}.scope_b.1.0.0",
            "a".repeat(wire::MAX_PROCESS_EVENT_CURSOR_CHARS - 22)
        ),
    ];
    corpus.sort();
    corpus
}

/// The cursor the bundle publishes is exactly the one the daemon reads: `process-event-cursor`'s
/// pattern and `maxLength` admit a corpus string exactly when `ProcessEventCursor::parse` reads
/// it, and its `maxLength` is the wire crate's bound. A cursor the pattern admits and the daemon
/// refuses, or the reverse, answers a client the schema promised otherwise.
fn check_cursor(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/common.json";
    let Some(common) = json_at(released, path, failures) else {
        return;
    };
    let Some(definition) = common.pointer("/$defs/process-event-cursor") else {
        failures.push(format!("{path}: no process-event-cursor definition"));
        return;
    };
    if definition.get("maxLength") != Some(&json!(wire::MAX_PROCESS_EVENT_CURSOR_CHARS)) {
        failures.push(format!(
            "{path}: process-event-cursor maxLength must be the constant {}",
            wire::MAX_PROCESS_EVENT_CURSOR_CHARS
        ));
    }
    let Some(pattern) = definition.get("pattern").and_then(Value::as_str) else {
        failures.push(format!("{path}: process-event-cursor states no pattern"));
        return;
    };
    let compiled = match fancy_regex::Regex::new(pattern) {
        Ok(compiled) => compiled,
        Err(error) => {
            failures.push(format!(
                "{path}: the process-event-cursor pattern does not compile: {error}"
            ));
            return;
        }
    };
    let max = definition
        .get("maxLength")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    for cursor in cursor_corpus() {
        let published = compiled.is_match(&cursor).unwrap_or(false)
            && u64::try_from(cursor.chars().count()).unwrap_or(u64::MAX) <= max;
        let read = wire::ProcessEventCursor::parse(&cursor).is_some();
        if published != read {
            failures.push(format!(
                "{path}: process-event-cursor {} {cursor:?} and the daemon's reader {} it",
                if published { "admits" } else { "refuses" },
                if read { "reads" } else { "refuses" },
            ));
        }
    }
}

/// What only the bundle that introduced S12 states: its client-visible change.
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
    let mut named = vec![PATH, wire::PROCESS_EVENTS_READERS_EXCEEDED, "0.23.0"];
    named.extend(wire::PROCESS_EVENT_TRANSITIONS);
    for named in named {
        if !changes.iter().any(|change| change.contains(named)) {
            failures.push(format!("{path}: client_visible_changes must state {named}"));
        }
    }
}

/// The route: a query-bound read under the `process` scope, direct-callable as every upgraded
/// stream is, and gated by no capability predicate, like `process.logs.get`.
fn check_route(released: &Tree, failures: &mut Vec<String>) {
    let Some(registry) = json_at(released, "operations.json", failures) else {
        return;
    };
    let declared: Vec<&Value> = registry
        .get("operations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|operation| operation.get("id") == Some(&json!(ROUTE)))
        .collect();
    let [declared] = declared.as_slice() else {
        failures.push(format!(
            "operations.json: {ROUTE} must be declared exactly once, found {}",
            declared.len()
        ));
        return;
    };
    for (member, expected) in [
        ("method", json!("GET")),
        ("path", json!(PATH)),
        ("api_major", json!(1)),
        ("idempotency", json!("none")),
        ("input_binding", json!("query")),
        ("required_scope", json!("process")),
        ("exposure", json!("callable-direct")),
        ("risk", json!("read")),
        ("capability_predicates", json!([])),
        ("input_schema", json!(INPUT)),
        ("result_schema", json!(RESULT)),
    ] {
        if declared.get(member) != Some(&expected) {
            failures.push(format!(
                "operations.json: {ROUTE} {member} must be {expected}"
            ));
        }
    }
}

/// The event union carries exactly the seven transitions, as `resource_kind: process` with an
/// operation cause and the process-transition observation.
fn check_event_variant(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/event.json";
    let Some(event) = json_at(released, path, failures) else {
        return;
    };
    let variants: Vec<&Value> = event
        .get("oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|variant| {
            variant.pointer("/properties/resource_kind/const") == Some(&json!("process"))
        })
        .collect();
    let [variant] = variants.as_slice() else {
        failures.push(format!(
            "{path}: exactly one process variant, found {}",
            variants.len()
        ));
        return;
    };
    let expected = [
        (
            "/properties/transition/enum",
            json!(wire::PROCESS_EVENT_TRANSITIONS),
        ),
        (
            "/properties/cause/properties/kind/const",
            json!("operation"),
        ),
        (
            "/properties/observation/$ref",
            json!("resource.json#/$defs/process-transition"),
        ),
        (
            "/properties/resource/$ref",
            json!("common.json#/$defs/process-id"),
        ),
        ("/additionalProperties", json!(false)),
    ];
    for (pointer, value) in expected {
        if variant.pointer(pointer) != Some(&value) {
            failures.push(format!(
                "{path}: the process variant {pointer} must be {value}"
            ));
        }
    }
    let Some(resource) = json_at(released, "schemas/resource.json", failures) else {
        return;
    };
    let observation = resource.pointer("/$defs/process-transition");
    let members = observation
        .and_then(|observation| observation.get("properties"))
        .and_then(Value::as_object)
        .map(|properties| properties.keys().map(String::as_str).collect::<Vec<_>>());
    if members.as_deref() != Some(&["ended_by", "exit", "process", "state"][..])
        || observation.and_then(|observation| observation.get("additionalProperties"))
            != Some(&json!(false))
    {
        failures.push(
            "schemas/resource.json: process-transition must be the closed {process, state, \
             ended_by, exit}; an output line is never an event"
                .to_owned(),
        );
    }
}

/// The upgrade's descriptor: the frame schema and the two bounds, at the wire crate's values.
fn check_protocol(released: &Tree, failures: &mut Vec<String>) {
    let Some(result) = json_at(released, RESULT, failures) else {
        return;
    };
    for (member, value) in [
        ("frame_schema", json!(FRAME)),
        ("max_readers", json!(wire::MAX_PROCESS_EVENT_READERS)),
        (
            "max_subject_streams",
            json!(wire::MAX_PROCESS_EVENT_STREAMS_PER_SUBJECT),
        ),
        (
            "max_deployment_streams",
            json!(wire::MAX_PROCESS_EVENT_STREAMS_PER_DEPLOYMENT),
        ),
        ("max_line_bytes", json!(wire::MAX_PROCESS_EVENT_LINE_BYTES)),
        ("terminal", json!("ended-then-close")),
    ] {
        if result.pointer(&format!("/properties/{member}/const")) != Some(&value) {
            failures.push(format!("{RESULT}: {member} must be the constant {value}"));
        }
    }
}

/// Four closed item kinds, each with `cursor` and the one member its kind names.
fn check_frame(released: &Tree, failures: &mut Vec<String>) {
    let Some(frame) = json_at(released, FRAME, failures) else {
        return;
    };
    let kinds: Vec<(Option<&Value>, Option<&Value>)> = frame
        .get("oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|variant| {
            (
                variant.pointer("/properties/kind/const"),
                variant.get("required"),
            )
        })
        .collect();
    let wanted = ["event", "line", "gap", "ended"]
        .map(|kind| (json!(kind), json!(["kind", "cursor", kind])));
    if kinds.len() != wanted.len()
        || wanted
            .iter()
            .any(|(kind, required)| !kinds.contains(&(Some(kind), Some(required))))
    {
        failures.push(format!(
            "{FRAME}: the items must be exactly event, line, gap and ended, each requiring kind, \
             cursor and its own member"
        ));
    }
    let Some(resource) = json_at(released, "schemas/resource.json", failures) else {
        return;
    };
    if resource.pointer("/$defs/process-log-line/properties/text/x-b10x-bound/max_bytes")
        != Some(&json!(wire::MAX_PROCESS_EVENT_LINE_BYTES))
    {
        failures.push(format!(
            "schemas/resource.json: a streamed line's text is at most {} bytes",
            wire::MAX_PROCESS_EVENT_LINE_BYTES
        ));
    }
}

/// The input declares the readers refusal at the wire crate's status and class, and a vector
/// answers it with the wire crate's message before any upgrade.
fn check_refusal(released: &Tree, failures: &mut Vec<String>) {
    let Some(refusal) = wire::process_refusal(wire::PROCESS_EVENTS_READERS_EXCEEDED) else {
        failures.push("substrate_wire: no process.events-readers-exceeded refusal".to_owned());
        return;
    };
    let class = serde_json::to_value(refusal.class).unwrap_or(Value::Null);
    let declared = json!({
        "class": class,
        "code": refusal.code,
        "status": refusal.status,
    });
    let Some(input) = json_at(released, INPUT, failures) else {
        return;
    };
    let annotations = input.get("x-b10x-refusals").and_then(Value::as_array);
    if !annotations.is_some_and(|annotations| annotations.contains(&declared)) {
        failures.push(format!("{INPUT}: must declare {declared}"));
    }
    let answered = released.iter().any(|(path, bytes)| {
        path.starts_with("vectors/")
            && serde_json::from_slice::<Value>(bytes).is_ok_and(|vector| {
                vector.pointer("/action/operation") == Some(&json!(ROUTE))
                    && vector.pointer("/expected/response/status") == Some(&json!(refusal.status))
                    && vector.pointer("/expected/response/body/error/code")
                        == Some(&json!(refusal.code))
                    && vector.pointer("/expected/response/body/error/class") == Some(&class)
                    && vector.pointer("/expected/response/body/error/message")
                        == Some(&json!(refusal.message))
                    && vector
                        .get("postconditions")
                        .and_then(Value::as_array)
                        .is_some_and(|postconditions| {
                            postconditions.contains(&json!({
                                "actual": "/probes/websocket_upgraded",
                                "expected": false,
                                "operator": "equals"
                            }))
                        })
            })
    });
    if !answered {
        failures.push(format!(
            "vectors: no {ROUTE} vector answers {} {} with the wire message before the upgrade",
            refusal.status, refusal.code
        ));
    }
}

#[cfg(test)]
mod tests {
    use crate::repo;

    fn released() -> super::Tree {
        super::super::tree_of(
            &repo::root()
                .expect("root")
                .join("contracts/substrate-wire/0.24.0"),
        )
        .expect("the bundle reads")
    }

    fn failures_of(tree: &super::Tree) -> Vec<String> {
        let mut failures = Vec::new();
        super::super::check_additions("0.24.0", tree, &mut failures);
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

    #[test]
    fn the_successor_carries_the_process_event_stream() {
        let failures = failures_of(&released());
        assert!(failures.is_empty(), "{failures:?}");
    }

    /// A check that passes on the tree that predates the change proves nothing: `0.23.0` has no
    /// such route, so the class check refuses it.
    #[test]
    fn the_predecessor_does_not_carry_it() {
        let mut failures = Vec::new();
        let predecessor = super::super::tree_of(
            &repo::root()
                .expect("root")
                .join("contracts/substrate-wire/0.23.0"),
        )
        .expect("the bundle reads");
        super::check(&predecessor, &mut failures);
        assert_reported(&failures, "process.events must be declared exactly once");
    }

    #[test]
    fn an_eighth_transition_is_refused() {
        let tree = edited("schemas/event.json", |event| {
            for variant in event["oneOf"].as_array_mut().expect("union") {
                if variant["properties"]["resource_kind"]["const"] == "process" {
                    variant["properties"]["transition"]["enum"]
                        .as_array_mut()
                        .expect("enum")
                        .push("process.output".into());
                }
            }
        });
        assert_reported(
            &failures_of(&tree),
            "the process variant /properties/transition/enum",
        );
    }

    #[test]
    fn a_readers_bound_other_than_the_wire_crates_is_refused() {
        let tree = edited(super::RESULT, |result| {
            result["properties"]["max_readers"]["const"] = 9.into();
        });
        assert_reported(&failures_of(&tree), "max_readers must be the constant 8");
    }

    #[test]
    fn a_subject_stream_cap_other_than_the_wire_crates_is_refused() {
        let tree = edited(super::RESULT, |result| {
            result["properties"]["max_subject_streams"]["const"] = 2560.into();
        });
        assert_reported(
            &failures_of(&tree),
            "max_subject_streams must be the constant 512",
        );
    }

    /// The pattern 0.24.0 first published, which names no record: it admits a cursor the daemon
    /// refuses, so a client the schema promised a valid cursor is answered `event.cursor-invalid`.
    #[test]
    fn a_cursor_pattern_the_daemon_does_not_read_is_refused() {
        let tree = edited("schemas/common.json", |common| {
            common["$defs"]["process-event-cursor"]["pattern"] = serde_json::json!(
                "^pe1\\.scope_[A-Za-z0-9_-]+\\.[1-9][0-9]*\\.(?:0|[1-9][0-9]*)\\.(?:0|[1-9][0-9]*)$"
            );
        });
        assert_reported(&failures_of(&tree), "process-event-cursor admits");
        let tree = edited("schemas/common.json", |common| {
            common["$defs"]["process-event-cursor"]["pattern"] = serde_json::json!("^pe1\\..+$");
        });
        assert_reported(&failures_of(&tree), "and the daemon's reader refuses it");
    }

    #[test]
    fn a_cursor_length_other_than_the_wire_crates_is_refused() {
        let tree = edited("schemas/common.json", |common| {
            common["$defs"]["process-event-cursor"]["maxLength"] = 1024.into();
        });
        assert_reported(
            &failures_of(&tree),
            "process-event-cursor maxLength must be the constant 512",
        );
    }

    #[test]
    fn a_readers_refusal_with_another_message_is_refused() {
        let path = "vectors/http/process-events-beyond-the-readers-bound-is-exhausted.json";
        let tree = edited(path, |vector| {
            vector["expected"]["response"]["body"]["error"]["message"] = "Too many.".into();
        });
        assert_reported(
            &failures_of(&tree),
            "with the wire message before the upgrade",
        );
    }

    #[test]
    fn a_route_gated_by_a_capability_is_refused() {
        let tree = edited("operations.json", |registry| {
            for operation in registry["operations"].as_array_mut().expect("operations") {
                if operation["id"] == super::ROUTE {
                    operation["capability_predicates"] =
                        serde_json::json!([{"fact": "process.host", "op": "eq", "value": true}]);
                }
            }
        });
        assert_reported(&failures_of(&tree), "process.events capability_predicates");
    }
}
