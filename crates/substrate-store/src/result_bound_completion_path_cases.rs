//! ADR 0053 decision 8 at the store, path by path: a completion whose result is recorded writes
//! the events its path writes, in order, each caused by its operation and observed at the
//! completion's instant; one whose result is not recorded ends its row with
//! `operation.result-unrecorded`, writes the operation's `operation.failed` and then the same
//! resource events, and a replay of the same operation id answers that error and writes nothing.
//! The expected events are literals, never another completion's output.

use chrono::{DateTime, Utc};
use serde_json::Value;
use substrate_wire::{
    Event, EventCause, ExecState, OperationOutcome, OperationState, SessionAttachmentState,
    SessionState,
};

use super::{exec, operation_named, pipe_session, scope, seed_exec, workspace};
use crate::{
    ExecRetireReservation, LeaseClock, NewLease, NewOperation, Reservation, Scope,
    SessionRetireReservation, Store, StoreConfig, WorkspaceAdoptReservation,
    WorkspaceDestroyReservation, WorkspaceRootHolder,
};

const SUBJECT: &str = "local:1000";

fn least_headroom_store() -> Store {
    Store::open_with_config(
        ":memory:",
        StoreConfig {
            operation_terminal_headroom_bytes: StoreConfig::minimum_terminal_headroom_bytes(),
            ..StoreConfig::default()
        },
    )
    .expect("a store at the least headroom opens")
}

fn mark(store: &Store, scope: &Scope) -> u64 {
    store.stream_position(scope).expect("stream position").2
}

fn since(store: &Store, scope: &Scope, mark: u64) -> Vec<Event> {
    store
        .events(scope, None, 1_000)
        .expect("events")
        .expect("cursor")
        .items
        .into_iter()
        .filter(|event| event.seq > mark)
        .collect()
}

/// Each event as (transition, resource kind, resource).
fn shape(events: &[Event]) -> Vec<(String, String, String)> {
    events
        .iter()
        .map(|event| {
            (
                event.transition.clone(),
                event.resource_kind.clone(),
                event.resource.clone(),
            )
        })
        .collect()
}

fn expect_shape(events: &[Event], expected: &[(&str, &str, &str)]) {
    assert_eq!(
        shape(events),
        expected
            .iter()
            .map(|(transition, kind, resource)| (
                (*transition).to_owned(),
                (*kind).to_owned(),
                (*resource).to_owned()
            ))
            .collect::<Vec<_>>()
    );
}

/// Every event follows the one before it in the stream, is caused by `operation` and observed
/// at `at`.
fn caused_in_order(events: &[Event], mark: u64, operation: &str, at: &str) {
    let at: DateTime<Utc> = at.parse().expect("instant");
    for (offset, event) in events.iter().enumerate() {
        assert_eq!(event.seq, mark + 1 + offset as u64, "{}", event.transition);
        assert_eq!(
            event.cause,
            EventCause::Operation {
                operation: operation.to_owned()
            },
            "{}",
            event.transition
        );
        assert_eq!(event.observed_at, at, "{}", event.transition);
        assert!(
            event.validate_closed_shape().is_ok(),
            "{}",
            event.transition
        );
    }
}

fn terminal_success(store: &Store, scope: &Scope, operation: &str) {
    let record = store
        .operation(scope, operation)
        .expect("operation")
        .expect("ledger row");
    assert_eq!(record.state, OperationState::Terminal);
    assert!(
        matches!(record.outcome, Some(OperationOutcome::Success { .. })),
        "{:?}",
        record.outcome
    );
}

fn unrecorded_outcome(operation: &str) -> OperationOutcome {
    OperationOutcome::Error {
        error: crate::result_unrecorded(operation),
    }
}

fn terminal_unrecorded(store: &Store, scope: &Scope, operation: &str) {
    let record = store
        .operation(scope, operation)
        .expect("operation")
        .expect("ledger row");
    assert_eq!(record.state, OperationState::Terminal);
    assert_eq!(record.outcome, Some(unrecorded_outcome(operation)));
}

/// `operation.failed` carries the operation's record, terminal with the unrecorded error.
fn carries_the_unrecorded_error(event: &Event, operation: &str) {
    assert_eq!(event.transition, "operation.failed");
    assert_eq!(event.observation["state"], "terminal");
    assert_eq!(
        event.observation["outcome"],
        serde_json::to_value(unrecorded_outcome(operation)).expect("outcome JSON")
    );
}

/// An operation id of the widest form the wire admits, distinct per `tag`.
fn widest_operation(tag: &str) -> String {
    format!(
        "{tag}{}",
        "0".repeat(substrate_wire::MAX_OPERATION_ID_CHARS - tag.len())
    )
}

/// A resource id of the widest form the store accepts, `MAX_RESOURCE_ID_CHARS` characters.
fn widest_resource(prefix: &str) -> String {
    format!(
        "{prefix}{}",
        "w".repeat(substrate_wire::MAX_RESOURCE_ID_CHARS - prefix.len())
    )
}

fn lease(operation: &str, boottime_ms: u64) -> NewLease {
    NewLease {
        ttl_ms: 60_000,
        clock: LeaseClock {
            wall: "2026-08-13T12:00:00Z".parse().expect("time"),
            boot_id: "boot-test".to_owned(),
            boottime_ms,
        },
        authorizing_operation: operation.to_owned(),
        actor: "test".to_owned(),
        principal: None,
    }
}

fn renewal_clock() -> LeaseClock {
    LeaseClock {
        wall: "2026-08-13T12:00:05Z".parse().expect("time"),
        boot_id: "boot-test".to_owned(),
        boottime_ms: 6_000,
    }
}

// --- exec retire -------------------------------------------------------------------------

fn retire_exec(store: &Store, operation: &str, id: &str) -> (NewOperation, Vec<Event>, u64, Value) {
    let scope = scope(SUBJECT);
    seed_exec(store, &scope, &exec(id, "ws_retire", ExecState::Exited));
    let retire = operation_named(SUBJECT, operation, "exec.retire", id, &"3".repeat(64));
    let before = mark(store, &scope);
    let ExecRetireReservation::Retired(absence) = store
        .retire_exec(&retire, id, "2026-08-13T12:00:00Z".parse().expect("time"))
        .expect("retire")
    else {
        panic!("a terminal exec retires");
    };
    let events = since(store, &scope, before);
    (
        retire,
        events,
        before,
        serde_json::to_value(absence).expect("absence JSON"),
    )
}

#[test]
fn a_recorded_exec_retire_writes_operation_accepted_then_exec_retired() {
    let store = Store::open(":memory:").expect("open store");
    let (retire, events, before, absence) = retire_exec(&store, "01JADV3EXECRETIRE0001", "ex_r");
    expect_shape(
        &events,
        &[
            ("operation.accepted", "operation", &retire.operation),
            ("exec.retired", "exec", "ex_r"),
        ],
    );
    caused_in_order(&events, before, &retire.operation, "2026-08-13T12:00:00Z");
    assert_eq!(events[1].observation, absence);
    terminal_success(&store, &retire.scope, &retire.operation);
}

/// The store accepts no resource longer than `MAX_RESOURCE_ID_CHARS`, so an exec retire's
/// absence, built from the id and an instant, fits the reserve of every store that opens: at the
/// least headroom with the widest ids the retire records its absence and replays it.
#[test]
fn at_the_least_headroom_an_exec_retire_of_the_widest_ids_records_its_absence_and_replays() {
    let store = least_headroom_store();
    let operation = widest_operation("EXECRETIRE");
    let id = widest_resource("ex_");
    let (retire, events, before, absence) = retire_exec(&store, &operation, &id);
    terminal_success(&store, &retire.scope, &operation);
    expect_shape(
        &events,
        &[
            ("operation.accepted", "operation", &operation),
            ("exec.retired", "exec", &id),
        ],
    );
    caused_in_order(&events, before, &operation, "2026-08-13T12:00:00Z");
    assert_eq!(events[1].observation, absence);
    let replay_mark = mark(&store, &retire.scope);
    let replay = store
        .retire_exec(&retire, &id, "2026-08-13T12:00:09Z".parse().expect("time"))
        .expect("replay");
    let ExecRetireReservation::Existing(Reservation::Replay(answer)) = replay else {
        panic!("the same operation id replays: {replay:?}");
    };
    assert_eq!(
        answer.outcome,
        OperationOutcome::Success { result: absence }
    );
    assert!(since(&store, &retire.scope, replay_mark).is_empty());
}

// --- workspace adopt ---------------------------------------------------------------------

/// Adopts `id`, observed with `labels` labels whose values are `MAX_LABEL_VALUE_CHARS` long.
fn adopt(
    store: &Store,
    operation: &str,
    id: &str,
    labels: usize,
) -> (NewOperation, Vec<Event>, u64, Value) {
    let scope = scope(SUBJECT);
    let new = operation_named(SUBJECT, operation, "workspace.adopt", id, &"4".repeat(64));
    let mut observed = workspace(id);
    for label in 0..labels {
        observed.labels.insert(
            format!("label-{label}"),
            "l".repeat(substrate_wire::MAX_LABEL_VALUE_CHARS),
        );
    }
    let before = mark(store, &scope);
    let WorkspaceAdoptReservation::Adopted(resource) = store
        .reserve_workspace_adopt(
            &new,
            "adopted-root",
            WorkspaceRootHolder::Free,
            &observed,
            "2026-08-13T12:00:01Z",
        )
        .expect("adopt")
    else {
        panic!("a free name is adopted");
    };
    let events = since(store, &scope, before);
    (
        new,
        events,
        before,
        serde_json::to_value(resource).expect("workspace JSON"),
    )
}

#[test]
fn a_recorded_workspace_adopt_writes_operation_accepted_then_workspace_created() {
    let store = Store::open(":memory:").expect("open store");
    let (new, events, before, resource) = adopt(&store, "01JADV3WORKSPACEADOPT1", "ws_adopted", 0);
    expect_shape(
        &events,
        &[
            ("operation.accepted", "operation", &new.operation),
            ("workspace.created", "workspace", "ws_adopted"),
        ],
    );
    caused_in_order(
        &events[1..],
        before + 1,
        &new.operation,
        "2026-08-13T12:00:01Z",
    );
    assert_eq!(events[1].observation, resource);
    terminal_success(&store, &new.scope, &new.operation);
}

#[test]
fn an_unrecorded_workspace_adopt_writes_operation_failed_and_workspace_created_and_replays() {
    let store = least_headroom_store();
    let operation = widest_operation("WSADOPT");
    let id = widest_resource("ws_");
    // Two labels at their value bound put the adopted workspace over the least reserve.
    let (new, events, before, resource) = adopt(&store, &operation, &id, 2);
    terminal_unrecorded(&store, &new.scope, &operation);
    expect_shape(
        &events,
        &[
            ("operation.accepted", "operation", &operation),
            ("operation.failed", "operation", &operation),
            ("workspace.created", "workspace", &id),
        ],
    );
    caused_in_order(&events[1..], before + 1, &operation, "2026-08-13T12:00:01Z");
    carries_the_unrecorded_error(&events[1], &operation);
    assert_eq!(events[2].observation, resource);
    let replay_mark = mark(&store, &new.scope);
    let replay = store
        .reserve_workspace_adopt(
            &new,
            "adopted-root",
            WorkspaceRootHolder::Free,
            &workspace(&id),
            "2026-08-13T12:00:09Z",
        )
        .expect("replay");
    let WorkspaceAdoptReservation::Answered(Reservation::Replay(answer)) = replay else {
        panic!("the same operation id replays: {replay:?}");
    };
    assert_eq!(answer.outcome, unrecorded_outcome(&operation));
    assert!(since(&store, &new.scope, replay_mark).is_empty());
}

// --- workspace destroy -------------------------------------------------------------------

fn destroy(store: &Store, operation: &str, id: &str) -> (NewOperation, Vec<Event>, u64, Value) {
    let scope = scope(SUBJECT);
    store
        .put_workspace(
            &scope,
            "destroyed-root",
            &substrate_wire::Workspace {
                root_name: substrate_wire::RootName::from("destroyed-root"),
                ..workspace(id)
            },
        )
        .expect("seed workspace");
    let new = operation_named(SUBJECT, operation, "workspace.destroy", id, &"5".repeat(64));
    assert!(matches!(
        store
            .reserve_workspace_destroy(&new, None)
            .expect("reserve destroy"),
        WorkspaceDestroyReservation::Admitted { .. }
    ));
    let absence = substrate_wire::WorkspaceAbsence {
        kind: substrate_wire::WorkspaceKind::Workspace,
        id: id.to_owned(),
        absent: true,
        observed_at: "2026-08-13T12:00:02Z".parse().expect("time"),
    };
    let before = mark(store, &scope);
    store
        .complete_workspace_absence(&scope, operation, "2026-08-13T12:00:02Z", 200, id, &absence)
        .expect("complete destroy");
    let events = since(store, &scope, before);
    (
        new,
        events,
        before,
        serde_json::to_value(absence).expect("absence JSON"),
    )
}

#[test]
fn a_recorded_workspace_destroy_writes_workspace_destroyed() {
    let store = Store::open(":memory:").expect("open store");
    let (new, events, before, absence) = destroy(&store, "01JADV3WORKSPACEDESTROY", "ws_gone");
    expect_shape(&events, &[("workspace.destroyed", "workspace", "ws_gone")]);
    caused_in_order(&events, before, &new.operation, "2026-08-13T12:00:02Z");
    assert_eq!(events[0].observation, absence);
    terminal_success(&store, &new.scope, &new.operation);
}

/// The store accepts no resource longer than `MAX_RESOURCE_ID_CHARS`, so a workspace's absence
/// fits the reserve of every store that opens: at the least headroom with the widest ids the
/// destroy records its absence and replays it.
#[test]
fn at_the_least_headroom_a_workspace_destroy_of_the_widest_ids_records_its_absence() {
    let store = least_headroom_store();
    let operation = widest_operation("WSDESTROY");
    let id = widest_resource("ws_");
    let (new, events, before, absence) = destroy(&store, &operation, &id);
    terminal_success(&store, &new.scope, &operation);
    expect_shape(&events, &[("workspace.destroyed", "workspace", &id)]);
    caused_in_order(&events, before, &operation, "2026-08-13T12:00:02Z");
    assert_eq!(events[0].observation, absence);
    let replay_mark = mark(&store, &new.scope);
    assert!(matches!(
        store.reserve(&new).expect("replay"),
        Reservation::Replay(crate::StoredAnswer {
            outcome: OperationOutcome::Success { .. },
            ..
        })
    ));
    assert!(since(&store, &new.scope, replay_mark).is_empty());
}

// --- exec lease renewal ------------------------------------------------------------------

fn renew_exec(
    store: &Store,
    operation: &str,
    actor: &str,
) -> (NewOperation, Vec<Event>, u64, Value) {
    let scope = scope(SUBJECT);
    let id = "ex_leased";
    seed_exec(store, &scope, &exec(id, "ws_leased", ExecState::Running));
    super::authorize_exec_lease(store, SUBJECT, id);
    let renew = operation_named(SUBJECT, operation, "exec.lease.renew", id, &"6".repeat(64));
    assert_eq!(
        store.reserve(&renew).expect("reserve"),
        Reservation::Accepted
    );
    let mut renewed_lease = lease(operation, 1_000);
    renewed_lease.actor = actor.to_owned();
    let before = mark(store, &scope);
    let resource = store
        .renew_exec_lease_reading(
            &scope,
            operation,
            "2026-08-13T12:00:00.5Z",
            200,
            id,
            &renewed_lease,
            || {
                Some(LeaseClock {
                    wall: "2026-08-13T12:00:00.5Z".parse().expect("time"),
                    boot_id: "boot-test".to_owned(),
                    boottime_ms: 1_500,
                })
            },
        )
        .expect("renew");
    let events = since(store, &scope, before);
    (
        renew,
        events,
        before,
        serde_json::to_value(resource).expect("exec JSON"),
    )
}

#[test]
fn a_recorded_exec_lease_renewal_writes_exec_lease_renewed() {
    let store = Store::open(":memory:").expect("open store");
    let (renew, events, before, resource) = renew_exec(&store, "01JADV3EXECLEASERENEW1", "test");
    expect_shape(&events, &[("exec.lease-renewed", "exec", "ex_leased")]);
    caused_in_order(&events, before, &renew.operation, "2026-08-13T12:00:00.5Z");
    assert_eq!(events[0].observation, resource);
    terminal_success(&store, &renew.scope, &renew.operation);
}

#[test]
fn an_unrecorded_exec_lease_renewal_writes_operation_failed_then_exec_lease_renewed() {
    let store = Store::open(":memory:").expect("open store");
    let operation = widest_operation("EXECRENEW");
    let actor = "a".repeat(substrate_wire::MAX_LEASE_ACTOR_CHARS + 1);
    let (renew, events, before, resource) = renew_exec(&store, &operation, &actor);
    terminal_unrecorded(&store, &renew.scope, &operation);
    expect_shape(
        &events,
        &[
            ("operation.failed", "operation", &operation),
            ("exec.lease-renewed", "exec", "ex_leased"),
        ],
    );
    caused_in_order(&events, before, &operation, "2026-08-13T12:00:00.5Z");
    carries_the_unrecorded_error(&events[0], &operation);
    assert_eq!(events[1].observation, resource);
}

// --- session start, lease renewal, observation and retire --------------------------------

/// The four session completions in their order; each returns its operation and its events.
#[allow(clippy::too_many_lines)] // One session's four completions, in the order they run.
fn session_chain(
    store: &Store,
    tag: &str,
    session_id: &str,
) -> Vec<(String, Vec<Event>, u64, &'static str)> {
    let scope = scope(SUBJECT);
    store
        .put_workspace(&scope, "ws_session", &workspace("ws_session"))
        .expect("seed workspace");
    let exec_id = "ex_session";
    let op = |name: &str| widest_operation(&format!("{tag}{name}"));
    let mut out = Vec::new();

    let start = operation_named(
        SUBJECT,
        &op("START"),
        "session.start",
        session_id,
        &"7".repeat(64),
    );
    let start_lease = lease(&start.operation, 1_000);
    let mut running = exec(exec_id, "ws_session", ExecState::Accepted);
    running.resource.lease = Some(start_lease.observation());
    let provisional = pipe_session(session_id, exec_id, "ws_session", &start_lease);
    assert_eq!(
        store
            .reserve_pipe_session_start(&start, &provisional, &running, &start_lease, None)
            .expect("reserve session"),
        Reservation::Accepted
    );
    running.resource.state = ExecState::Running;
    let mut ready = provisional;
    ready.state = SessionState::Ready;
    ready.attachment = SessionAttachmentState::Available;
    let before = mark(store, &scope);
    store
        .complete_pipe_session_start(
            &scope,
            &start.operation,
            "2026-08-13T12:00:01Z",
            202,
            &ready,
            &running,
            &start_lease,
        )
        .expect("complete session start");
    out.push((
        start.operation.clone(),
        since(store, &scope, before),
        before,
        "2026-08-13T12:00:01Z",
    ));

    let renew = operation_named(
        SUBJECT,
        &op("RENEW"),
        "session.lease.renew",
        session_id,
        &"8".repeat(64),
    );
    assert_eq!(
        store.reserve(&renew).expect("reserve"),
        Reservation::Accepted
    );
    let before = mark(store, &scope);
    store
        .renew_pipe_session_lease_reading(
            &scope,
            &renew.operation,
            "2026-08-13T12:00:05Z",
            200,
            session_id,
            &lease(&renew.operation, 1_000),
            || Some(renewal_clock()),
        )
        .expect("renew session lease");
    out.push((
        renew.operation.clone(),
        since(store, &scope, before),
        before,
        "2026-08-13T12:00:05Z",
    ));

    let signal = operation_named(
        SUBJECT,
        &op("SIGNAL"),
        "session.signal",
        session_id,
        &"9".repeat(64),
    );
    assert_eq!(
        store.reserve(&signal).expect("reserve"),
        Reservation::Accepted
    );
    let mut exited = running.clone();
    exited.resource.state = ExecState::Exited;
    exited.resource.exit = Some(substrate_wire::ExecExit {
        code: Some(0),
        signal: None,
    });
    exited.output_complete = true;
    let before = mark(store, &scope);
    store
        .complete_pipe_session_observation(
            &scope,
            &signal.operation,
            "2026-08-13T12:00:06Z",
            202,
            session_id,
            &exited,
        )
        .expect("complete session observation");
    out.push((
        signal.operation.clone(),
        since(store, &scope, before),
        before,
        "2026-08-13T12:00:06Z",
    ));

    let retire = operation_named(
        SUBJECT,
        &op("RETIRE"),
        "session.retire",
        session_id,
        &"a".repeat(64),
    );
    let before = mark(store, &scope);
    let retired = store
        .retire_pipe_session(
            &retire,
            session_id,
            "2026-08-13T12:00:07Z".parse().expect("time"),
        )
        .expect("retire session");
    assert!(
        matches!(retired, SessionRetireReservation::Retired(_)),
        "{retired:?}"
    );
    out.push((
        retire.operation.clone(),
        since(store, &scope, before),
        before,
        "2026-08-13T12:00:07Z",
    ));
    out
}

#[test]
fn recorded_session_completions_write_operation_terminal_alone() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope(SUBJECT);
    let chain = session_chain(&store, "R", "ses_recorded");
    for (index, (operation, events, before, at)) in chain.iter().enumerate() {
        terminal_success(&store, &scope, operation);
        // A retire accepts and completes in one transaction; its acceptance event is the first.
        let skip = usize::from(index == 3);
        assert_eq!(events.len(), 1 + skip, "{:?}", shape(events));
        expect_shape(
            &events[skip..],
            &[("operation.terminal", "operation", operation)],
        );
        caused_in_order(&events[skip..], *before + skip as u64, operation, at);
        let last = events.last().expect("an event");
        assert_eq!(last.observation["state"], "terminal");
        assert_eq!(last.observation["outcome"]["kind"], "success");
    }
}

/// At the least headroom with the widest ids, a session's start, lease renewal and observation
/// are not recorded and each writes `operation.failed` alone. Its retire's absence fits every
/// reserve a store opens with (the store accepts no resource past `MAX_RESOURCE_ID_CHARS`), so
/// the retire records it.
#[test]
fn unrecorded_session_completions_write_operation_failed_alone() {
    let store = least_headroom_store();
    let scope = scope(SUBJECT);
    let chain = session_chain(&store, "U", &widest_resource("ses_"));
    for (index, (operation, events, before, at)) in chain.iter().enumerate() {
        let skip = usize::from(index == 3);
        assert_eq!(events.len(), 1 + skip, "{:?}", shape(events));
        if index == 3 {
            terminal_success(&store, &scope, operation);
            expect_shape(
                &events[skip..],
                &[("operation.terminal", "operation", operation)],
            );
            continue;
        }
        terminal_unrecorded(&store, &scope, operation);
        expect_shape(
            &events[skip..],
            &[("operation.failed", "operation", operation)],
        );
        caused_in_order(&events[skip..], *before + skip as u64, operation, at);
        carries_the_unrecorded_error(events.last().expect("an event"), operation);
    }
}

// --- general and exec completion, workspace lease renewal ----------------------------------

#[test]
fn a_recorded_general_completion_writes_its_success_event_and_an_error_writes_operation_failed() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope(SUBJECT);
    let create = operation_named(
        SUBJECT,
        "01JADV3GENERALCREATE01",
        "workspace.create",
        "ws_general",
        &"c".repeat(64),
    );
    assert_eq!(
        store.reserve(&create).expect("reserve"),
        Reservation::Accepted
    );
    let resource = workspace("ws_general");
    let before = mark(&store, &scope);
    store
        .complete_workspace(
            &scope,
            &create.operation,
            "2026-08-13T12:00:01Z",
            201,
            "ws_general",
            &resource,
        )
        .expect("complete");
    let events = since(&store, &scope, before);
    expect_shape(&events, &[("workspace.created", "workspace", "ws_general")]);
    caused_in_order(&events, before, &create.operation, "2026-08-13T12:00:01Z");
    assert_eq!(
        events[0].observation,
        serde_json::to_value(&resource).expect("workspace JSON")
    );
    terminal_success(&store, &scope, &create.operation);

    let failing = operation_named(
        SUBJECT,
        "01JADV3GENERALFAILED01",
        "workspace.create",
        "ws_failing",
        &"d".repeat(64),
    );
    assert_eq!(
        store.reserve(&failing).expect("reserve"),
        Reservation::Accepted
    );
    let error = substrate_wire::ErrorDetail {
        class: substrate_wire::ErrorClass::Failed,
        code: "workspace.create-failed".to_owned(),
        message: "The driver failed.".to_owned(),
        retriable: false,
        address: None,
        operation: Some(failing.operation.clone()),
    };
    let before = mark(&store, &scope);
    store
        .complete_error(
            &scope,
            &failing.operation,
            "2026-08-13T12:00:02Z",
            500,
            Some("ws_failing"),
            &error,
        )
        .expect("complete error");
    let events = since(&store, &scope, before);
    expect_shape(
        &events,
        &[("operation.failed", "operation", &failing.operation)],
    );
    caused_in_order(&events, before, &failing.operation, "2026-08-13T12:00:02Z");
    assert_eq!(
        events[0].observation["outcome"],
        serde_json::to_value(OperationOutcome::Error { error }).expect("outcome JSON")
    );
}

#[test]
fn a_recorded_exec_completion_writes_exec_observed() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope(SUBJECT);
    store
        .put_workspace(&scope, "ws_exec_done", &workspace("ws_exec_done"))
        .expect("seed workspace");
    let start = operation_named(
        SUBJECT,
        "01JADV3EXECCOMPLETE001",
        "exec.start",
        "ex_done",
        &"e".repeat(64),
    );
    assert_eq!(
        store
            .reserve_exec_start(
                &start,
                &exec("ex_done", "ws_exec_done", ExecState::Accepted),
                None,
                None
            )
            .expect("reserve exec"),
        Reservation::Accepted
    );
    let running = exec("ex_done", "ws_exec_done", ExecState::Running);
    let before = mark(&store, &scope);
    store
        .complete_exec(
            &scope,
            &start.operation,
            "2026-08-13T12:00:01Z",
            202,
            &running.resource,
            &[],
            &[],
            false,
            false,
            false,
            None,
            None,
        )
        .expect("complete exec");
    let events = since(&store, &scope, before);
    expect_shape(&events, &[("exec.observed", "exec", "ex_done")]);
    caused_in_order(&events, before, &start.operation, "2026-08-13T12:00:01Z");
    assert_eq!(
        events[0].observation,
        serde_json::to_value(&running.resource).expect("exec JSON")
    );
    terminal_success(&store, &scope, &start.operation);
}

#[test]
fn a_recorded_workspace_lease_renewal_writes_workspace_lease_renewed() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope(SUBJECT);
    super::seed_leased_workspace(&store, SUBJECT, "ws_renew", 60_000);
    let renew = operation_named(
        SUBJECT,
        "01JADV3WORKSPACERENEW1",
        "workspace.lease.renew",
        "ws_renew",
        &"f".repeat(64),
    );
    assert_eq!(
        store.reserve(&renew).expect("reserve"),
        Reservation::Accepted
    );
    let (_, mut renewed_lease) = super::lease_at(SUBJECT, "ws_renew", 60_000);
    renewed_lease
        .authorizing_operation
        .clone_from(&renew.operation);
    let before = mark(&store, &scope);
    let renewed = store
        .renew_workspace_lease(
            &scope,
            &renew.operation,
            "2026-08-13T12:00:00Z",
            200,
            "ws_renew",
            &renewed_lease,
        )
        .expect("renew");
    let events = since(&store, &scope, before);
    expect_shape(
        &events,
        &[("workspace.lease-renewed", "workspace", "ws_renew")],
    );
    caused_in_order(&events, before, &renew.operation, "2026-08-13T12:00:00Z");
    assert_eq!(
        events[0].observation,
        serde_json::to_value(&renewed).expect("workspace JSON")
    );
    terminal_success(&store, &scope, &renew.operation);
}

// --- the least headroom ------------------------------------------------------------------

/// ADR 0053 decision 8: a store does not open with a terminal headroom under what a row needs
/// to end with `operation.result-unrecorded`, and at the least headroom it opens with, a
/// completion over the reserve ends its row with that error. A completion names its resource
/// (`complete_success` takes the resource id), and a row accepted without one is still a row the
/// store admits.
#[test]
fn at_the_least_headroom_an_over_reserve_completion_that_names_its_resource_ends_its_row() {
    let store = least_headroom_store();
    let operation = widest_operation("NAMED");
    let mut accepted = operation_named(
        SUBJECT,
        &operation,
        "workspace.file.write",
        "ws_named",
        &"b".repeat(64),
    );
    accepted.resource = None;
    assert_eq!(
        store.reserve(&accepted).expect("accept"),
        Reservation::Accepted
    );
    let reserve = usize::try_from(store.success_result_json_bytes()).expect("usize");
    let completed = store.complete_success(
        &accepted.scope,
        &operation,
        "2026-11-24T12:34:56.123456789+00:00",
        200,
        Some("ws_named"),
        &"x".repeat(reserve - 1),
    );
    let record = store
        .operation(&accepted.scope, &operation)
        .expect("operation")
        .expect("ledger row");
    assert!(
        completed.is_ok(),
        "a completion over the reserve failed ({completed:?}) and left the row {:?}",
        record.state
    );
    assert_eq!(record.state, OperationState::Terminal);
    assert_eq!(record.outcome, Some(unrecorded_outcome(&operation)));
}
