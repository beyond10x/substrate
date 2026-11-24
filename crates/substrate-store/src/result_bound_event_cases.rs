//! ADR 0053 decision 8, the events: a completion whose success result is not recorded writes the
//! operation's `operation.failed`, carrying the operation record that ends with
//! `operation.result-unrecorded`, and the resource's own events exactly as the recorded completion
//! of the same path writes them. One case per completion path; each completes one operation
//! whose result is recorded and one whose result has a member past its wire bound, and holds the
//! second's events to the first's.

use serde_json::Value;
use substrate_wire::{
    ErrorClass, Event, ExecRefusal, ExecState, MAX_EXEC_REFUSAL_CODE_CHARS, MAX_LEASE_ACTOR_CHARS,
    MAX_PROCESS_EXIT_SIGNAL_CHARS, OperationOutcome, OperationState,
};

use super::{exec, lease_at, operation_named, scope, seed_leased_workspace, workspace};
use crate::{
    HOST_PROCESS_RENEW_KIND, HOST_PROCESS_START_KIND, HOST_PROCESS_STOP_KIND, HostProcessEndedBy,
    HostProcessLeaseRequest, HostProcessLimits, HostProcessRenewal, HostProcessStartReservation,
    HostProcessStopReservation, NewHostProcess, NewOperation, Reservation, Scope, Store,
};

/// The sequence number the stream of `scope` has reached.
fn mark(store: &Store, scope: &Scope) -> u64 {
    store.stream_position(scope).expect("stream position").2
}

/// Every event of `scope` after `mark`, in order.
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

fn is_operation_event(event: &Event) -> bool {
    event.transition.starts_with("operation.")
}

/// The rule for one path. `recorded` are the events of the completion whose result was recorded,
/// `unrecorded` those of `operation`, whose result was not. `unrecorded` is the
/// `operation.failed` of `operation`, carrying its record terminal with the unrecorded error,
/// and then the events `recorded` wrote for its resource: the same transitions in the same order,
/// on `resource`, each carrying what `observations` names.
fn assert_the_rule(
    recorded: &[Event],
    unrecorded: &[Event],
    operation: &str,
    resource: &str,
    observations: &[Value],
) {
    let transitions = |events: &[Event]| {
        events
            .iter()
            .map(|event| (event.transition.clone(), event.resource_kind.clone()))
            .collect::<Vec<_>>()
    };
    let Some((failed, resource_events)) = unrecorded.split_first() else {
        panic!("the unrecorded completion wrote no event");
    };
    assert_eq!(
        (failed.transition.as_str(), failed.resource.as_str()),
        ("operation.failed", operation),
        "the operation's event comes first: {:?}",
        transitions(unrecorded)
    );
    assert_eq!(failed.observation["state"], "terminal");
    assert_eq!(
        failed.observation["outcome"],
        serde_json::to_value(OperationOutcome::Error {
            error: crate::result_unrecorded(operation),
        })
        .expect("outcome JSON"),
        "operation.failed carries the unrecorded error"
    );
    let recorded_resource_events = recorded
        .iter()
        .filter(|event| !is_operation_event(event))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        transitions(resource_events),
        transitions(&recorded_resource_events),
        "the resource's events are the recorded completion's: {:?} beside {:?}",
        transitions(unrecorded),
        transitions(recorded)
    );
    assert!(
        resource_events
            .iter()
            .all(|event| event.resource == resource),
        "{resource_events:?}"
    );
    for (event, observation) in resource_events.iter().zip(observations) {
        assert_eq!(&event.observation, observation, "{}", event.transition);
    }
}

fn reserve(store: &Store, new: &NewOperation) {
    assert_eq!(
        store.reserve(new).expect("reserve"),
        Reservation::Accepted,
        "{}",
        new.operation
    );
}

fn terminal_unrecorded(store: &Store, scope: &Scope, operation: &str) {
    let record = store
        .operation(scope, operation)
        .expect("operation")
        .expect("ledger row");
    assert_eq!(record.state, OperationState::Terminal);
    assert_eq!(
        record.outcome,
        Some(OperationOutcome::Error {
            error: crate::result_unrecorded(operation),
        })
    );
}

/// The store's general completion, here of a workspace create: a recorded one writes
/// `workspace.created` carrying the workspace; an unrecorded one writes `operation.failed` and
/// the same `workspace.created` carrying the workspace it stored.
#[test]
fn an_unrecorded_general_completion_writes_operation_failed_and_the_resource_events() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let mut events = Vec::new();
    for (index, actor_chars) in [MAX_LEASE_ACTOR_CHARS, MAX_LEASE_ACTOR_CHARS + 1]
        .into_iter()
        .enumerate()
    {
        let id = format!("ws_general_{index}");
        let new = operation_named(
            "local:1000",
            &format!("01JEVENTGENERAL00000{index}"),
            "workspace.create",
            &id,
            &format!("{index:0>64}"),
        );
        reserve(&store, &new);
        let (_, mut lease) = lease_at("local:1000", &id, 60_000);
        lease.actor = "a".repeat(actor_chars);
        let mut resource = workspace(&id);
        resource.lease = Some(lease.observation());
        let before = mark(&store, &scope);
        store
            .complete_workspace(
                &scope,
                &new.operation,
                "2026-11-24T12:00:01Z",
                201,
                &id,
                &resource,
            )
            .expect("the completion ends the row");
        events.push((
            new.operation,
            id,
            serde_json::to_value(&resource).expect("workspace JSON"),
            since(&store, &scope, before),
        ));
    }
    let (recorded, unrecorded) = (&events[0], &events[1]);
    terminal_unrecorded(&store, &scope, &unrecorded.0);
    assert_the_rule(
        &recorded.3,
        &unrecorded.3,
        &unrecorded.0,
        &unrecorded.1,
        std::slice::from_ref(&unrecorded.2),
    );
}

/// An exec completion: a recorded one writes `exec.observed` carrying the exec; an unrecorded
/// one writes `operation.failed` and the same `exec.observed` carrying the exec it stored.
#[test]
fn an_unrecorded_exec_completion_writes_operation_failed_and_the_exec_events() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_exec_events", &workspace("ws_exec_events"))
        .expect("seed workspace");
    let mut events = Vec::new();
    for (index, code_chars) in [MAX_EXEC_REFUSAL_CODE_CHARS, MAX_EXEC_REFUSAL_CODE_CHARS + 1]
        .into_iter()
        .enumerate()
    {
        let id = format!("ex_events_{index}");
        let start = operation_named(
            "local:1000",
            &format!("01JEVENTEXEC0000000{index}"),
            "exec.start",
            &id,
            &format!("{index:0>64}"),
        );
        assert_eq!(
            store
                .reserve_exec_start(
                    &start,
                    &exec(&id, "ws_exec_events", ExecState::Accepted),
                    None,
                    None
                )
                .expect("reserve exec"),
            Reservation::Accepted
        );
        let mut running = exec(&id, "ws_exec_events", ExecState::Running);
        running.resource.refusal = Some(ExecRefusal {
            class: ErrorClass::Exhausted,
            code: "c".repeat(code_chars),
            message: "A driver's refusal.".to_owned(),
        });
        let before = mark(&store, &scope);
        store
            .complete_exec(
                &scope,
                &start.operation,
                "2026-11-24T12:00:01Z",
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
            .expect("the completion ends the row");
        events.push((
            start.operation,
            id,
            serde_json::to_value(&running.resource).expect("exec JSON"),
            since(&store, &scope, before),
        ));
    }
    let (recorded, unrecorded) = (&events[0], &events[1]);
    terminal_unrecorded(&store, &scope, &unrecorded.0);
    assert_the_rule(
        &recorded.3,
        &unrecorded.3,
        &unrecorded.0,
        &unrecorded.1,
        std::slice::from_ref(&unrecorded.2),
    );
}

/// A lease renewal, here of a workspace: a recorded one writes `workspace.lease-renewed`
/// carrying the workspace; an unrecorded one writes `operation.failed` and the same
/// `workspace.lease-renewed` carrying the workspace it stored.
#[test]
fn an_unrecorded_lease_renewal_writes_operation_failed_and_the_resource_events() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let mut events = Vec::new();
    for (index, actor_chars) in [MAX_LEASE_ACTOR_CHARS, MAX_LEASE_ACTOR_CHARS + 1]
        .into_iter()
        .enumerate()
    {
        let id = format!("ws_renewed_{index}");
        seed_leased_workspace(&store, "local:1000", &id, 60_000);
        let renew = operation_named(
            "local:1000",
            &format!("01JEVENTRENEW000000{index}"),
            "workspace.lease.renew",
            &id,
            &format!("{index:0>64}"),
        );
        reserve(&store, &renew);
        let (_, mut lease) = lease_at("local:1000", &id, 60_000);
        lease.actor = "a".repeat(actor_chars);
        lease.authorizing_operation.clone_from(&renew.operation);
        let before = mark(&store, &scope);
        let renewed = store
            .renew_workspace_lease(
                &scope,
                &renew.operation,
                "2026-08-13T12:00:00Z",
                200,
                &id,
                &lease,
            )
            .expect("the renewal ends the row");
        events.push((
            renew.operation,
            id,
            serde_json::to_value(&renewed).expect("workspace JSON"),
            since(&store, &scope, before),
        ));
    }
    let (recorded, unrecorded) = (&events[0], &events[1]);
    terminal_unrecorded(&store, &scope, &unrecorded.0);
    assert_the_rule(
        &recorded.3,
        &unrecorded.3,
        &unrecorded.0,
        &unrecorded.1,
        std::slice::from_ref(&unrecorded.2),
    );
}

/// An instant `seconds` after the processes' base instant.
fn process_at(seconds: i64) -> chrono::DateTime<chrono::Utc> {
    "2026-10-17T06:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .expect("instant")
        + chrono::Duration::seconds(seconds)
}

/// An operation of `kind` on process `id`, accepted at the base instant.
fn process_operation(scope: &Scope, operation: &str, kind: &str, id: &str) -> NewOperation {
    NewOperation {
        scope: scope.clone(),
        operation: operation.to_owned(),
        operation_kind: kind.to_owned(),
        request_hash: format!("sha256:{}", "a".repeat(64)),
        accepted_at: process_at(0).to_rfc3339(),
        capability_snapshot: None,
        actor: "local:1000".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(id.to_owned()),
    }
}

/// Starts process `proc_events_<index>` under a TTL lease and returns its id.
fn start_process(store: &Store, scope: &Scope, index: usize) -> String {
    let id = format!("proc_events_{index}");
    let new = NewHostProcess {
        id: id.clone(),
        owner_session: "s-events".to_owned(),
        owner_name: format!("dev-{index}"),
        reason: "serve the preview".to_owned(),
        argv: vec!["npm".to_owned(), "run".to_owned(), "dev".to_owned()],
        cwd: "/srv/app".to_owned(),
        env_names: vec!["PATH".to_owned()],
        env_digest: format!("sha256:{}", "b".repeat(64)),
        lease: HostProcessLeaseRequest::Ttl { ttl_ms: 60_000 },
        timeout_ms: None,
        expires_at: None,
    };
    let operation = format!("op-events-start-{index}");
    let started = store
        .reserve_host_process_start(
            &process_operation(scope, &operation, HOST_PROCESS_START_KIND, &id),
            &new,
            &HostProcessLimits::default(),
        )
        .expect("start");
    assert!(matches!(started, HostProcessStartReservation::Accepted(_)));
    id
}

/// Renews process `id` under operation `renew`, recording a result whose exit signal is
/// `signal_chars` long, and returns the events the renewal wrote.
fn renew_process(
    store: &Store,
    scope: &Scope,
    renew: &str,
    id: &str,
    signal_chars: usize,
) -> Vec<Event> {
    reserve(
        store,
        &process_operation(scope, renew, HOST_PROCESS_RENEW_KIND, id),
    );
    let before = mark(store, scope);
    let renewal = store
        .renew_host_process_lease_with(scope, renew, process_at(10), 200, id, 60_000, |record| {
            Ok(serde_json::json!({
                "id": record.id,
                "exit": { "signal": "S".repeat(signal_chars) },
            }))
        })
        .expect("the renewal ends the row");
    assert!(matches!(renewal, HostProcessRenewal::Renewed(_)));
    since(store, scope, before)
}

/// A host-process renewal: a recorded one writes `operation.terminal` and
/// `process.lease-renewed`; an unrecorded one writes `operation.failed` and the same
/// `process.lease-renewed`.
#[test]
fn an_unrecorded_process_renewal_writes_operation_failed_and_the_process_events() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let mut events = Vec::new();
    for (index, signal_chars) in [
        MAX_PROCESS_EXIT_SIGNAL_CHARS,
        MAX_PROCESS_EXIT_SIGNAL_CHARS + 1,
    ]
    .into_iter()
    .enumerate()
    {
        let id = start_process(&store, &scope, index);
        let renew = format!("op-events-renew-{index}");
        let written = renew_process(&store, &scope, &renew, &id, signal_chars);
        events.push((renew, id, written));
    }
    let (recorded, unrecorded) = (&events[0], &events[1]);
    assert!(
        recorded.2.iter().any(is_operation_event),
        "a recorded renewal writes the operation's event"
    );
    terminal_unrecorded(&store, &scope, &unrecorded.0);
    let observations = unrecorded
        .2
        .iter()
        .skip(1)
        .map(|event| event.observation.clone())
        .collect::<Vec<_>>();
    assert_the_rule(
        &recorded.2,
        &unrecorded.2,
        &unrecorded.0,
        &unrecorded.1,
        &observations,
    );
}

/// Each event as (transition, resource kind, resource).
fn shape(events: &[Event]) -> Vec<(&str, &str, &str)> {
    events
        .iter()
        .map(|event| {
            (
                event.transition.as_str(),
                event.resource_kind.as_str(),
                event.resource.as_str(),
            )
        })
        .collect()
}

/// A recorded host-process renewal writes `operation.terminal` for its operation and then
/// `process.lease-renewed` for the process.
#[test]
fn a_recorded_process_renewal_writes_operation_terminal_then_process_lease_renewed() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let id = start_process(&store, &scope, 0);
    let events = renew_process(&store, &scope, "op-events-renew-literal", &id, 1);
    assert_eq!(
        shape(&events),
        [
            ("operation.terminal", "operation", "op-events-renew-literal"),
            ("process.lease-renewed", "process", id.as_str()),
        ]
    );
}

/// A recorded host-process stop's completion writes `operation.terminal` for its operation
/// alone; the stop's acceptance writes `operation.accepted` before it.
#[test]
fn a_recorded_process_stop_writes_operation_accepted_then_operation_terminal() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let id = start_process(&store, &scope, 0);
    let stop = process_operation(
        &scope,
        "op-events-stop-literal",
        HOST_PROCESS_STOP_KIND,
        &id,
    );
    let before = mark(&store, &scope);
    let reserved = store
        .reserve_host_process_stop(&stop, &id, HostProcessEndedBy::Stop)
        .expect("stop");
    assert!(matches!(reserved, HostProcessStopReservation::Accepted(_)));
    store
        .complete_success(
            &scope,
            &stop.operation,
            "2026-10-17T06:00:20Z",
            200,
            Some(&id),
            &id,
        )
        .expect("the stop ends the row");
    assert_eq!(
        shape(&since(&store, &scope, before)),
        [
            ("operation.accepted", "operation", "op-events-stop-literal"),
            ("operation.terminal", "operation", "op-events-stop-literal"),
        ]
    );
}
