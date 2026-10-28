//! ADR 0036 S12: the seven `process.*` variants of the one event union, the per-process stream
//! item generated from `spec/host-processes/wire.yaml`, its query, bounds and refusal.

use serde_json::{Value, json};

use super::{
    Event, EventCause, EventControl, MAX_PROCESS_EVENT_LINE_BYTES, MAX_PROCESS_EVENT_READERS,
    MAX_PROCESS_LOG_LINE_BYTES, PROCESS_EVENT_TRANSITIONS, PROCESS_EVENTS_READERS_EXCEEDED,
    ProcessEndedBy, ProcessEventsQuery, ProcessExit, ProcessPresence, ProcessState,
    ProcessStreamEnd, ProcessStreamGap, ProcessStreamItem, ProcessStreamItemKind,
    ProcessTransition, process_refusal,
};

fn transition(state: ProcessState, ended_by: Option<ProcessEndedBy>) -> ProcessTransition {
    ProcessTransition {
        process: "proc_01".to_owned(),
        state: Box::new(state),
        ended_by: ended_by.map(Box::new).into(),
        exit: ProcessPresence::Absent,
    }
}

fn event(
    transition_name: &str,
    resource_kind: &str,
    cause: EventCause,
    observation: Value,
) -> Event {
    Event {
        source_scope: String::new(),
        generation: 1,
        seq: 7,
        resource: "proc_01".to_owned(),
        resource_kind: resource_kind.to_owned(),
        transition: transition_name.to_owned(),
        observed_at: "2026-10-28T09:00:00Z".parse().expect("instant"),
        actor: "local:1000".to_owned(),
        principal: None,
        cause,
        observation,
    }
}

fn operation() -> EventCause {
    EventCause::Operation {
        operation: "op_01".to_owned(),
    }
}

/// ADR 0036 S12: exactly the seven lifecycle transitions, in the ADR's order.
#[test]
fn process_event_transitions_are_the_adr_seven() {
    assert_eq!(
        PROCESS_EVENT_TRANSITIONS,
        [
            "process.started",
            "process.lease-renewed",
            "process.lease-expired",
            "process.timed-out",
            "process.stopped",
            "process.exited",
            "process.vanished",
        ]
    );
}

/// Each `process.*` transition is a closed variant: `resource_kind: process`, an operation cause
/// and a `ProcessTransition` observation, and nothing else.
#[test]
fn process_event_variants_are_closed() {
    let observation =
        serde_json::to_value(transition(ProcessState::Ended, Some(ProcessEndedBy::Exit)))
            .expect("observation");
    for name in PROCESS_EVENT_TRANSITIONS {
        assert!(
            event(name, "process", operation(), observation.clone())
                .validate_closed_shape()
                .is_ok(),
            "{name}"
        );
        assert!(
            event(name, "exec", operation(), observation.clone())
                .validate_closed_shape()
                .is_err(),
            "{name} with another resource kind"
        );
        let control = EventCause::Control {
            control: EventControl::ReconciliationSnapshotCreate,
        };
        assert!(
            event(name, "process", control, observation.clone())
                .validate_closed_shape()
                .is_err(),
            "{name} with a control cause"
        );
        assert!(
            event(name, "process", operation(), json!({"process": "proc_01"}))
                .validate_closed_shape()
                .is_err(),
            "{name} without a state"
        );
        assert!(
            event(
                name,
                "process",
                operation(),
                json!({"process": "proc_01", "state": "running", "line": "x"})
            )
            .validate_closed_shape()
            .is_err(),
            "{name} with an output line"
        );
    }
    assert!(
        event("process.output", "process", operation(), observation)
            .validate_closed_shape()
            .is_err(),
        "no output-line variant exists in the event log"
    );
}

/// D1: the stream's line bound is its own constant, at the S7 line bound's value.
#[test]
fn process_event_bounds_are_the_decided_values() {
    assert_eq!(MAX_PROCESS_EVENT_LINE_BYTES, 16 * 1024);
    assert_eq!(MAX_PROCESS_EVENT_LINE_BYTES, MAX_PROCESS_LOG_LINE_BYTES);
    assert_eq!(MAX_PROCESS_EVENT_READERS, 8);
}

/// ADR 0036 § Named refusals: `process.events-readers-exceeded` is 429, exhausted.
#[test]
fn process_events_readers_exceeded_is_a_named_429() {
    assert_eq!(
        PROCESS_EVENTS_READERS_EXCEEDED,
        "process.events-readers-exceeded"
    );
    let refusal = process_refusal(PROCESS_EVENTS_READERS_EXCEEDED).expect("named refusal");
    assert_eq!(refusal.status, 429);
    assert_eq!(
        serde_json::to_value(refusal.class).expect("class"),
        json!("exhausted")
    );
}

/// Each item kind serializes with exactly its own member beside `kind` and `cursor`.
#[test]
fn process_stream_items_carry_one_member() {
    let gap = ProcessStreamItem {
        kind: Box::new(ProcessStreamItemKind::Gap),
        cursor: "c".to_owned(),
        event: ProcessPresence::Absent,
        line: ProcessPresence::Absent,
        gap: ProcessPresence::Present(Box::new(ProcessStreamGap {
            discarded_bytes: 4096.into(),
        })),
        ended: ProcessPresence::Absent,
    };
    assert_eq!(
        serde_json::to_value(&gap).expect("gap"),
        json!({"kind": "gap", "cursor": "c", "gap": {"discarded_bytes": 4096}})
    );
    let ended = ProcessStreamItem {
        kind: Box::new(ProcessStreamItemKind::Ended),
        cursor: "c".to_owned(),
        event: ProcessPresence::Absent,
        line: ProcessPresence::Absent,
        gap: ProcessPresence::Absent,
        ended: ProcessPresence::Present(Box::new(ProcessStreamEnd {
            ended_by: Box::new(ProcessEndedBy::Exit),
            exit: ProcessPresence::Present(Box::new(ProcessExit {
                code: ProcessPresence::Present(0.into()),
                signal: ProcessPresence::Absent,
            })),
        })),
    };
    assert_eq!(
        serde_json::to_value(&ended).expect("ended"),
        json!({"kind": "ended", "cursor": "c", "ended": {"ended_by": "exit", "exit": {"code": 0}}})
    );
    assert!(
        serde_json::from_value::<ProcessStreamItem>(
            json!({"kind": "ended", "cursor": "c", "extra": 1})
        )
        .is_err()
    );
}

/// The stream's query has one optional member, `cursor`; anything else is refused.
#[test]
fn process_events_query_is_closed() {
    let query: ProcessEventsQuery = serde_urlencoded::from_str("").expect("empty");
    assert_eq!(query.cursor, None);
    let query: ProcessEventsQuery = serde_urlencoded::from_str("cursor=abc").expect("cursor");
    assert_eq!(query.cursor.as_deref(), Some("abc"));
    assert!(serde_urlencoded::from_str::<ProcessEventsQuery>("limit=3").is_err());
}

/// D5: a subject and a deployment hold at most the readers bound of every live record a subject
/// may hold, so a stream of an ended record counts against the same total.
#[test]
fn process_event_stream_caps_are_the_live_records_readers() {
    let live = usize::try_from(super::MAX_PROCESSES_PER_SUBJECT).expect("small bound");
    assert_eq!(
        super::MAX_PROCESS_EVENT_STREAMS_PER_SUBJECT,
        live * MAX_PROCESS_EVENT_READERS
    );
    assert_eq!(super::MAX_PROCESS_EVENT_STREAMS_PER_DEPLOYMENT, 512);
}

/// D1 and D2: the cursor names its record, round-trips, and anything outside the published form
/// is not a cursor.
#[test]
fn process_event_cursor_names_its_record_and_reads_only_its_form() {
    let cursor = super::ProcessEventCursor {
        process: "proc_01ABC".to_owned(),
        source: "scope_a-b_c".to_owned(),
        generation: 3,
        seq: 0,
        offset: 340,
    };
    assert_eq!(cursor.to_string(), "pe1.proc_01ABC.scope_a-b_c.3.0.340");
    assert_eq!(
        super::ProcessEventCursor::parse(&cursor.to_string()),
        Some(cursor)
    );
    let long = format!("pe1.proc_{}.scope_a.1.0.0", "a".repeat(500));
    for foreign in [
        "",
        "pe1.scope_a.1.0.0",
        "pe1.proc_01.scope_a.1.0",
        "pe1.proc_01.scope_a.1.0.0.0",
        "pe2.proc_01.scope_a.1.0.0",
        "pe1.proc_.scope_a.1.0.0",
        "pe1.proc_0-1.scope_a.1.0.0",
        "pe1.proc_01.scope_.1.0.0",
        "pe1.proc_01.scope_a.b.1.0.0",
        "pe1.proc_01.other.1.0.0",
        "pe1.proc_01.scope_a.0.0.0",
        "pe1.proc_01.scope_a.01.0.0",
        "pe1.proc_01.scope_a.1.00.0",
        "pe1.proc_01.scope_a.1.0.+1",
        "pe1.proc_01.scope_\u{e9}.1.0.0",
        &long,
    ] {
        assert_eq!(super::ProcessEventCursor::parse(foreign), None, "{foreign}");
    }
    // The pattern bounds no number's width: a number beyond `u64` is read as `u64::MAX`, so a
    // foreign generation still answers `event.source-mismatch` and a seq or offset no stream
    // reaches answers `event.cursor-invalid`.
    let wide = "18446744073709551616";
    assert_eq!(
        super::ProcessEventCursor::parse(&format!("pe1.proc_01.scope_a.{wide}.{wide}.{wide}")),
        Some(super::ProcessEventCursor {
            process: "proc_01".to_owned(),
            source: "scope_a".to_owned(),
            generation: u64::MAX,
            seq: u64::MAX,
            offset: u64::MAX,
        })
    );
}
