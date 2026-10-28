//! ADR 0036 S12: a host process's lifecycle in the existing event log, and where the per-process
//! stream places each transition among the record's log lines.

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use substrate_wire::{Event, EventCause};
use tempfile::{TempDir, tempdir};

use super::{
    HOST_PROCESS_RENEW_KIND, HOST_PROCESS_START_KIND, HOST_PROCESS_STOP_KIND,
    HOST_PROCESS_SUPERVISOR_ACTOR, HostProcessEnd, HostProcessEndedBy, HostProcessExit,
    HostProcessLeaseRequest, HostProcessLimits, HostProcessRenewal, HostProcessStartReservation,
    HostProcessStopReservation, HostProcessTransition, NewHostProcess,
};
use crate::{NewOperation, Reservation, Scope, Store};

fn at(ms: i64) -> DateTime<Utc> {
    "2026-10-17T06:00:00Z"
        .parse::<DateTime<Utc>>()
        .expect("base instant")
        + Duration::milliseconds(ms)
}

fn scope() -> Scope {
    Scope {
        deployment: "dep_test".to_owned(),
        subject: "local:1000".to_owned(),
    }
}

fn open() -> (TempDir, Store) {
    let dir = tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("state.sqlite")).expect("open store");
    (dir, store)
}

fn op(operation: &str, kind: &str, when: DateTime<Utc>) -> NewOperation {
    NewOperation {
        scope: scope(),
        operation: operation.to_owned(),
        operation_kind: kind.to_owned(),
        request_hash: format!("sha256:{}", "a".repeat(64)),
        accepted_at: when.to_rfc3339(),
        capability_snapshot: None,
        actor: "local:1000".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some("proc_1".to_owned()),
    }
}

fn running(store: &Store) {
    let new = NewHostProcess {
        id: "proc_1".to_owned(),
        owner_session: "s-1".to_owned(),
        owner_name: "dev".to_owned(),
        reason: "serve the preview".to_owned(),
        argv: vec!["npm".to_owned(), "run".to_owned(), "dev".to_owned()],
        cwd: "/srv/app".to_owned(),
        env_names: vec!["PATH".to_owned()],
        env_digest: format!("sha256:{}", "b".repeat(64)),
        lease: HostProcessLeaseRequest::Ttl { ttl_ms: 60_000 },
        timeout_ms: None,
        expires_at: None,
    };
    let reservation = store
        .reserve_host_process_start(
            &op("op-start", HOST_PROCESS_START_KIND, at(0)),
            &new,
            &HostProcessLimits::default(),
        )
        .expect("start");
    assert!(matches!(
        reservation,
        HostProcessStartReservation::Accepted(_)
    ));
    assert!(matches!(
        store
            .mark_host_process_running(&scope(), "proc_1", at(1))
            .expect("run"),
        HostProcessTransition::Applied(_)
    ));
}

fn process_events(store: &Store) -> Vec<Event> {
    store
        .events(&scope(), None, 100)
        .expect("store")
        .expect("page")
        .items
        .into_iter()
        .filter(|event| event.resource_kind == "process")
        .collect()
}

fn cause(event: &Event) -> &str {
    match &event.cause {
        EventCause::Operation { operation } => operation,
        EventCause::Control { .. } => panic!("a process event names an operation"),
    }
}

fn end(store: &Store, end: &HostProcessEnd) {
    assert!(matches!(
        store
            .record_host_process_end(&scope(), "proc_1", at(10), end)
            .expect("end"),
        HostProcessTransition::Applied(_)
    ));
}

#[test]
fn a_record_that_runs_and_exits_appends_started_then_exited_caused_by_its_start() {
    let (_dir, store) = open();
    running(&store);
    end(&store, &HostProcessEnd::Exit(HostProcessExit::Code(3)));
    let events = process_events(&store);
    let transitions: Vec<&str> = events.iter().map(|e| e.transition.as_str()).collect();
    assert_eq!(transitions, ["process.started", "process.exited"]);
    for event in &events {
        assert_eq!(event.resource, "proc_1");
        assert_eq!(cause(event), "op-start");
        assert_eq!(event.actor, "local:1000");
        event.validate_closed_shape().expect("a closed event shape");
    }
    assert_eq!(
        events[0].observation,
        json!({ "process": "proc_1", "state": "running" })
    );
    assert_eq!(
        events[1].observation,
        json!({ "process": "proc_1", "state": "ended", "ended_by": "exit", "exit": { "code": 3 } })
    );
}

#[test]
fn a_caller_stop_appends_nothing_until_the_end_and_the_end_names_the_stop() {
    let (_dir, store) = open();
    running(&store);
    assert!(matches!(
        store
            .reserve_host_process_stop(
                &op("op-stop", HOST_PROCESS_STOP_KIND, at(2)),
                "proc_1",
                HostProcessEndedBy::Stop
            )
            .expect("stop"),
        HostProcessStopReservation::Accepted(_)
    ));
    assert_eq!(process_events(&store).len(), 1, "only process.started");
    end(
        &store,
        &HostProcessEnd::Exit(HostProcessExit::Signal("TERM".to_owned())),
    );
    let events = process_events(&store);
    assert_eq!(events[1].transition, "process.stopped");
    assert_eq!(cause(&events[1]), "op-stop");
    assert_eq!(events[1].observation["ended_by"], "stop");
    assert_eq!(events[1].observation["exit"], json!({ "signal": "TERM" }));
}

#[test]
fn a_supervisor_expiry_appends_lease_expired_at_its_decision_then_stopped() {
    let (_dir, store) = open();
    running(&store);
    let mut expiry = op("op-expire", HOST_PROCESS_STOP_KIND, at(60_000));
    expiry.actor = HOST_PROCESS_SUPERVISOR_ACTOR.to_owned();
    assert!(matches!(
        store
            .reserve_host_process_stop_at(
                &expiry,
                "proc_1",
                HostProcessEndedBy::LeaseExpiry,
                Some(42)
            )
            .expect("expiry"),
        HostProcessStopReservation::Accepted(_)
    ));
    end(
        &store,
        &HostProcessEnd::Exit(HostProcessExit::Signal("TERM".to_owned())),
    );
    let events = process_events(&store);
    let transitions: Vec<&str> = events.iter().map(|e| e.transition.as_str()).collect();
    assert_eq!(
        transitions,
        [
            "process.started",
            "process.lease-expired",
            "process.stopped"
        ]
    );
    assert_eq!(cause(&events[1]), "op-expire");
    assert_eq!(events[1].actor, HOST_PROCESS_SUPERVISOR_ACTOR);
    assert_eq!(cause(&events[2]), "op-expire");
    assert_eq!(events[2].observation["ended_by"], "lease-expiry");

    let stream = store
        .process_stream_events(&scope(), "proc_1", 0)
        .expect("stream");
    let anchors: Vec<Option<u64>> = stream.events.iter().map(|e| e.log_offset).collect();
    assert_eq!(anchors, [Some(0), Some(42), None]);
    assert_eq!(stream.through_seq, stream.events[2].event.seq);
}

#[test]
fn a_renewal_appends_lease_renewed_caused_by_the_renewal() {
    let (_dir, store) = open();
    running(&store);
    assert_eq!(
        store
            .reserve(&op("op-renew", HOST_PROCESS_RENEW_KIND, at(30_000)))
            .expect("reserve"),
        Reservation::Accepted
    );
    let renewed = store
        .renew_host_process_lease_at(
            &scope(),
            "op-renew",
            at(30_000),
            200,
            "proc_1",
            120_000,
            Some(7),
            |record| Ok(json!({ "id": record.id })),
        )
        .expect("renew");
    assert!(matches!(renewed, HostProcessRenewal::Renewed(_)));
    let events = process_events(&store);
    assert_eq!(events[1].transition, "process.lease-renewed");
    assert_eq!(cause(&events[1]), "op-renew");
    let stream = store
        .process_stream_events(&scope(), "proc_1", events[0].seq)
        .expect("stream");
    assert_eq!(stream.events.len(), 1, "after the started event's seq");
    assert_eq!(stream.events[0].log_offset, Some(7));
}

#[test]
fn the_stream_reads_only_its_own_record() {
    let (_dir, store) = open();
    running(&store);
    let stream = store
        .process_stream_events(&scope(), "proc_other", 0)
        .expect("stream");
    assert!(stream.events.is_empty());
    assert!(stream.first_retained_seq.is_some());
}
