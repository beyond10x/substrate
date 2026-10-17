use chrono::{DateTime, Duration, Utc};
use substrate_wire::{MAX_LEASE_TTL_MS, MIN_LEASE_TTL_MS, OperationState};
use tempfile::{TempDir, tempdir};

use super::{
    HOST_PROCESS_CONFINEMENT, HOST_PROCESS_PROFILE, HOST_PROCESS_RENEW_KIND,
    HOST_PROCESS_START_KIND, HOST_PROCESS_STOP_KIND, HOST_PROCESS_SUPERVISOR_ACTOR,
    HostProcessDeadlineSource, HostProcessEnd, HostProcessEndedBy, HostProcessExit,
    HostProcessFilter, HostProcessLease, HostProcessLeaseRequest, HostProcessLimits,
    HostProcessRecord, HostProcessRefusal, HostProcessRenewal, HostProcessStartReservation,
    HostProcessState, HostProcessStopReservation, HostProcessTransition, NewHostProcess,
};
use crate::{NewOperation, Reservation, Scope, Store, StoreError};

const T0: &str = "2026-10-17T06:00:00Z";

fn t0() -> DateTime<Utc> {
    T0.parse().expect("base instant")
}

fn at(ms: i64) -> DateTime<Utc> {
    t0() + Duration::milliseconds(ms)
}

fn scope(subject: &str) -> Scope {
    Scope {
        deployment: "dep_test".to_owned(),
        subject: subject.to_owned(),
    }
}

fn open() -> (TempDir, Store) {
    let dir = tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("state.sqlite")).expect("open store");
    (dir, store)
}

fn op(
    scope: &Scope,
    operation: &str,
    kind: &str,
    resource: &str,
    when: DateTime<Utc>,
) -> NewOperation {
    NewOperation {
        scope: scope.clone(),
        operation: operation.to_owned(),
        operation_kind: kind.to_owned(),
        request_hash: format!("sha256:{}", "a".repeat(64)),
        accepted_at: when.to_rfc3339(),
        capability_snapshot: None,
        actor: "local:1000".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(resource.to_owned()),
    }
}

fn process(id: &str, session: &str, name: &str) -> NewHostProcess {
    NewHostProcess {
        id: id.to_owned(),
        owner_session: session.to_owned(),
        owner_name: name.to_owned(),
        reason: "serve the preview".to_owned(),
        argv: vec!["npm".to_owned(), "run".to_owned(), "dev".to_owned()],
        cwd: "/srv/app".to_owned(),
        env_names: vec!["PATH".to_owned(), "HOME".to_owned()],
        env_digest: format!("sha256:{}", "b".repeat(64)),
        lease: HostProcessLeaseRequest::Ttl { ttl_ms: 60_000 },
        timeout_ms: None,
        expires_at: None,
    }
}

fn start_at(
    store: &Store,
    scope: &Scope,
    operation: &str,
    new: &NewHostProcess,
    when: DateTime<Utc>,
    limits: &HostProcessLimits,
) -> HostProcessStartReservation {
    store
        .reserve_host_process_start(
            &op(scope, operation, HOST_PROCESS_START_KIND, &new.id, when),
            new,
            limits,
        )
        .expect("start reservation")
}

fn start(store: &Store, scope: &Scope, operation: &str, new: &NewHostProcess) -> HostProcessRecord {
    match start_at(
        store,
        scope,
        operation,
        new,
        t0(),
        &HostProcessLimits::default(),
    ) {
        HostProcessStartReservation::Accepted(record) => *record,
        other => panic!("expected an accepted start, got {other:?}"),
    }
}

fn assert_refused(
    store: &Store,
    scope: &Scope,
    operation: &str,
    new: &NewHostProcess,
    expected: HostProcessRefusal,
) {
    assert_refused_with(
        store,
        scope,
        operation,
        new,
        &HostProcessLimits::default(),
        expected,
    );
}

fn assert_refused_with(
    store: &Store,
    scope: &Scope,
    operation: &str,
    new: &NewHostProcess,
    limits: &HostProcessLimits,
    expected: HostProcessRefusal,
) {
    match start_at(store, scope, operation, new, t0(), limits) {
        HostProcessStartReservation::Refused { refusal, answer } => {
            assert_eq!(refusal, expected);
            assert_eq!(answer.status, expected.status());
            let text = serde_json::to_string(&answer.outcome).expect("outcome json");
            assert!(
                text.contains(expected.code()),
                "{text} names {}",
                expected.code()
            );
        }
        other => panic!("expected {expected:?}, got {other:?}"),
    }
    let record = store
        .operation(scope, operation)
        .expect("ledger read")
        .expect("the refusal is a durable operation");
    assert_eq!(record.state, OperationState::Refused);
    assert_eq!(
        store.host_process(scope, &new.id).expect("record read"),
        None,
        "a refused start leaves no record"
    );
}

fn end_exit(store: &Store, scope: &Scope, id: &str, code: i32) -> HostProcessRecord {
    match store
        .record_host_process_end(
            scope,
            id,
            at(10),
            &HostProcessEnd::Exit(HostProcessExit::Code(code)),
        )
        .expect("end")
    {
        HostProcessTransition::Applied(record) => *record,
        other => panic!("expected the end to apply, got {other:?}"),
    }
}

#[test]
fn start_records_a_starting_unconfined_record_durably_before_dispatch() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let record = start(
        &store,
        &alice,
        "op-start-1",
        &process("proc_1", "s-1", "dev"),
    );
    assert_eq!(record.state, HostProcessState::Starting);
    assert_eq!(record.profile, HOST_PROCESS_PROFILE);
    assert_eq!(record.confinement, HOST_PROCESS_CONFINEMENT);
    assert_eq!(record.ended_by, None);
    assert_eq!(record.exit, None);
    assert_eq!(record.start_operation, "op-start-1");
    assert_eq!(record.started_at, t0());
    assert_eq!(record.env_names, vec!["PATH".to_owned(), "HOME".to_owned()]);
    assert_eq!(
        record.lease,
        HostProcessLease::Ttl {
            ttl_ms: 60_000,
            issued_at: t0(),
            expires_at: at(60_000),
        }
    );
    let ledger = store
        .operation(&alice, "op-start-1")
        .expect("ledger read")
        .expect("operation recorded");
    assert_eq!(ledger.state, OperationState::Accepted);
    assert_eq!(
        store.host_process(&alice, "proc_1").expect("read"),
        Some(record)
    );
}

#[test]
fn start_replays_and_conflicts_through_the_ledger() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let new = process("proc_1", "s-1", "dev");
    start(&store, &alice, "op-start-1", &new);
    match start_at(
        &store,
        &alice,
        "op-start-1",
        &new,
        t0(),
        &HostProcessLimits::default(),
    ) {
        HostProcessStartReservation::Existing(Reservation::Pending(_)) => {}
        other => panic!("expected the pending operation, got {other:?}"),
    }
    let mut conflicting = op(
        &alice,
        "op-start-1",
        HOST_PROCESS_START_KIND,
        "proc_1",
        t0(),
    );
    conflicting.request_hash = format!("sha256:{}", "c".repeat(64));
    match store
        .reserve_host_process_start(&conflicting, &new, &HostProcessLimits::default())
        .expect("reservation")
    {
        HostProcessStartReservation::Existing(Reservation::Conflict) => {}
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[test]
fn keep_without_its_own_reason_is_refused() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let mut new = process("proc_1", "s-1", "dev");
    new.lease = HostProcessLeaseRequest::Keep {
        reason: "  ".to_owned(),
    };
    assert_refused(
        &store,
        &alice,
        "op-1",
        &new,
        HostProcessRefusal::KeepReasonRequired,
    );
    assert_eq!(
        HostProcessRefusal::KeepReasonRequired.code(),
        "process.keep-reason-required"
    );
    assert_eq!(HostProcessRefusal::KeepReasonRequired.status(), 422);
}

#[test]
fn keep_with_a_reason_has_no_ttl_and_no_expiry() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let mut new = process("proc_1", "s-1", "dev");
    new.lease = HostProcessLeaseRequest::Keep {
        reason: "shared cluster for the week".to_owned(),
    };
    let record = start(&store, &alice, "op-1", &new);
    assert_eq!(
        record.lease,
        HostProcessLease::Keep {
            reason: "shared cluster for the week".to_owned()
        }
    );
}

#[test]
fn lease_ttl_outside_the_lease_bounds_is_refused_and_the_bounds_are_admitted() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    for (index, ttl_ms) in [0, MIN_LEASE_TTL_MS - 1, MAX_LEASE_TTL_MS + 1]
        .into_iter()
        .enumerate()
    {
        let mut new = process(&format!("proc_bad_{index}"), "s-1", &format!("bad-{index}"));
        new.lease = HostProcessLeaseRequest::Ttl { ttl_ms };
        assert_refused(
            &store,
            &alice,
            &format!("op-bad-{index}"),
            &new,
            HostProcessRefusal::LeaseInvalid,
        );
    }
    for (index, ttl_ms) in [MIN_LEASE_TTL_MS, MAX_LEASE_TTL_MS].into_iter().enumerate() {
        let mut new = process(&format!("proc_ok_{index}"), "s-1", &format!("ok-{index}"));
        new.lease = HostProcessLeaseRequest::Ttl { ttl_ms };
        start(&store, &alice, &format!("op-ok-{index}"), &new);
    }
    assert_eq!(
        HostProcessRefusal::LeaseInvalid.code(),
        "process.lease-invalid"
    );
}

/// One lexical case: its name, the edit that breaks the start, and the refusal it must earn.
type LexicalCase = (
    &'static str,
    Box<dyn Fn(&mut NewHostProcess)>,
    HostProcessRefusal,
);

#[test]
fn lexical_start_fields_are_named_refusals() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let cases: Vec<LexicalCase> = vec![
        (
            "owner-session",
            Box::new(|p| p.owner_session.clear()),
            HostProcessRefusal::OwnerInvalid,
        ),
        (
            "owner-name",
            Box::new(|p| p.owner_name.clear()),
            HostProcessRefusal::OwnerInvalid,
        ),
        (
            "reason",
            Box::new(|p| p.reason = " ".to_owned()),
            HostProcessRefusal::ReasonRequired,
        ),
        (
            "argv-empty",
            Box::new(|p| p.argv.clear()),
            HostProcessRefusal::ArgvInvalid,
        ),
        (
            "argv-nul",
            Box::new(|p| p.argv.push("a\0b".to_owned())),
            HostProcessRefusal::ArgvInvalid,
        ),
        (
            "argv-program-empty",
            Box::new(|p| p.argv[0].clear()),
            HostProcessRefusal::ArgvInvalid,
        ),
        (
            "cwd",
            Box::new(|p| p.cwd = "srv/app".to_owned()),
            HostProcessRefusal::CwdInvalid,
        ),
        (
            "env-name",
            Box::new(|p| p.env_names.push("A=B".to_owned())),
            HostProcessRefusal::EnvInvalid,
        ),
        (
            "env-duplicate",
            Box::new(|p| p.env_names.push("PATH".to_owned())),
            HostProcessRefusal::EnvInvalid,
        ),
        (
            "env-digest",
            Box::new(|p| p.env_digest = "sha256:XYZ".to_owned()),
            HostProcessRefusal::EnvInvalid,
        ),
    ];
    for (label, mutate, expected) in cases {
        let mut new = process(&format!("proc_{label}"), "s-1", label);
        mutate(&mut new);
        assert_refused(&store, &alice, &format!("op-{label}"), &new, expected);
    }
}

#[test]
fn timeout_outside_its_bounds_is_refused() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let limits = HostProcessLimits::default();
    for (index, timeout_ms) in [0, limits.max_timeout_ms + 1].into_iter().enumerate() {
        let mut new = process(&format!("proc_{index}"), "s-1", &format!("t-{index}"));
        new.timeout_ms = Some(timeout_ms);
        assert_refused(
            &store,
            &alice,
            &format!("op-{index}"),
            &new,
            HostProcessRefusal::TimeoutInvalid,
        );
    }
}

#[test]
fn expires_at_too_soon_or_too_far_is_refused() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let limits = HostProcessLimits::default();
    let too_soon = at(i64::try_from(MIN_LEASE_TTL_MS).expect("ms") - 1);
    let too_far = at(i64::try_from(limits.max_timeout_ms).expect("ms") + 1);
    for (index, instant) in [at(-1), too_soon, too_far].into_iter().enumerate() {
        let mut new = process(&format!("proc_{index}"), "s-1", &format!("e-{index}"));
        new.expires_at = Some(instant);
        assert_refused(
            &store,
            &alice,
            &format!("op-{index}"),
            &new,
            HostProcessRefusal::ExpiresAtInvalid,
        );
    }
}

#[test]
fn the_earlier_deadline_wins_and_names_its_source() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let mut by_timeout = process("proc_t", "s-1", "t");
    by_timeout.timeout_ms = Some(5_000);
    by_timeout.expires_at = Some(at(9_000));
    let record = start(&store, &alice, "op-t", &by_timeout);
    let deadline = record.deadline.expect("deadline");
    assert_eq!(deadline.at, at(5_000));
    assert_eq!(deadline.source, HostProcessDeadlineSource::Timeout);

    let mut by_instant = process("proc_e", "s-1", "e");
    by_instant.timeout_ms = Some(9_000);
    by_instant.expires_at = Some(at(5_000));
    let record = start(&store, &alice, "op-e", &by_instant);
    let deadline = record.deadline.expect("deadline");
    assert_eq!(deadline.at, at(5_000));
    assert_eq!(deadline.source, HostProcessDeadlineSource::ExpiresAt);

    let none = start(&store, &alice, "op-n", &process("proc_n", "s-1", "n"));
    assert_eq!(none.deadline, None);
}

#[test]
fn owner_name_is_unique_among_the_live_records_of_one_owner_session() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let bob = scope("local:1001");
    start(&store, &alice, "op-1", &process("proc_1", "s-1", "dev"));
    assert_refused(
        &store,
        &alice,
        "op-2",
        &process("proc_2", "s-1", "dev"),
        HostProcessRefusal::NameConflict,
    );
    assert_eq!(HostProcessRefusal::NameConflict.status(), 409);
    start(&store, &alice, "op-3", &process("proc_3", "s-2", "dev"));
    start(&store, &bob, "op-4", &process("proc_4", "s-1", "dev"));
    end_exit(&store, &alice, "proc_1", 0);
    start(&store, &alice, "op-5", &process("proc_5", "s-1", "dev"));
}

#[test]
fn the_live_record_bound_per_subject_is_limit_exceeded() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let bob = scope("local:1001");
    let limits = HostProcessLimits {
        subject_max_live: 2,
        ..HostProcessLimits::default()
    };
    for index in 0..2 {
        let new = process(&format!("proc_{index}"), "s-1", &format!("n-{index}"));
        assert!(matches!(
            start_at(&store, &alice, &format!("op-{index}"), &new, t0(), &limits),
            HostProcessStartReservation::Accepted(_)
        ));
    }
    assert_refused_with(
        &store,
        &alice,
        "op-over",
        &process("proc_over", "s-1", "over"),
        &limits,
        HostProcessRefusal::LimitExceeded,
    );
    assert_eq!(
        HostProcessRefusal::LimitExceeded.code(),
        "process.limit-exceeded"
    );
    assert_eq!(HostProcessRefusal::LimitExceeded.status(), 429);
    assert!(matches!(
        start_at(
            &store,
            &bob,
            "op-bob",
            &process("proc_bob", "s-1", "n"),
            t0(),
            &limits
        ),
        HostProcessStartReservation::Accepted(_)
    ));
    end_exit(&store, &alice, "proc_0", 0);
    assert!(matches!(
        start_at(
            &store,
            &alice,
            "op-after",
            &process("proc_after", "s-1", "after"),
            t0(),
            &limits
        ),
        HostProcessStartReservation::Accepted(_)
    ));
}

#[test]
fn start_run_exit_ends_by_exit_with_the_code() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    start(&store, &alice, "op-1", &process("proc_1", "s-1", "dev"));
    let running = match store
        .mark_host_process_running(&alice, "proc_1", at(1))
        .expect("run")
    {
        HostProcessTransition::Applied(record) => *record,
        other => panic!("expected running, got {other:?}"),
    };
    assert_eq!(running.state, HostProcessState::Running);
    assert_eq!(running.running_at, Some(at(1)));
    assert!(matches!(
        store
            .mark_host_process_running(&alice, "proc_1", at(2))
            .expect("run again"),
        HostProcessTransition::Unchanged(_)
    ));
    let ended = end_exit(&store, &alice, "proc_1", 3);
    assert_eq!(ended.state, HostProcessState::Ended);
    assert_eq!(ended.ended_by, Some(HostProcessEndedBy::Exit));
    assert_eq!(ended.exit, Some(HostProcessExit::Code(3)));
    assert_eq!(ended.ended_at, Some(at(10)));
    assert!(matches!(
        store
            .record_host_process_end(&alice, "proc_1", at(11), &HostProcessEnd::Vanished)
            .expect("end again"),
        HostProcessTransition::Unchanged(_)
    ));
    assert_eq!(
        store
            .host_process(&alice, "proc_1")
            .expect("read")
            .expect("record")
            .ended_by,
        Some(HostProcessEndedBy::Exit)
    );
}

#[test]
fn a_record_whose_cgroup_vanished_ends_vanished_without_an_exit_status() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    start(&store, &alice, "op-1", &process("proc_1", "s-1", "dev"));
    match store
        .record_host_process_end(&alice, "proc_1", at(5), &HostProcessEnd::Vanished)
        .expect("end")
    {
        HostProcessTransition::Applied(record) => {
            assert_eq!(record.ended_by, Some(HostProcessEndedBy::Vanished));
            assert_eq!(record.exit, None);
        }
        other => panic!("expected vanished, got {other:?}"),
    }
}

#[test]
fn stop_moves_to_stopping_and_the_first_cause_stands_through_the_end() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    start(&store, &alice, "op-1", &process("proc_1", "s-1", "dev"));
    store
        .mark_host_process_running(&alice, "proc_1", at(1))
        .expect("run");
    let stopping = match store
        .reserve_host_process_stop(
            &op(&alice, "op-stop", HOST_PROCESS_STOP_KIND, "proc_1", at(2)),
            "proc_1",
            HostProcessEndedBy::Stop,
        )
        .expect("stop")
    {
        HostProcessStopReservation::Accepted(record) => *record,
        other => panic!("expected an accepted stop, got {other:?}"),
    };
    assert_eq!(stopping.state, HostProcessState::Stopping);
    assert_eq!(stopping.ended_by, Some(HostProcessEndedBy::Stop));
    assert_eq!(stopping.stopping_at, Some(at(2)));
    assert_eq!(
        store
            .operation(&alice, "op-stop")
            .expect("ledger")
            .expect("op")
            .state,
        OperationState::Accepted
    );
    // A later supervisor cause does not overwrite the recorded one.
    let mut expiry = op(
        &alice,
        "op-expire",
        HOST_PROCESS_STOP_KIND,
        "proc_1",
        at(120_000),
    );
    expiry.actor = HOST_PROCESS_SUPERVISOR_ACTOR.to_owned();
    assert!(matches!(
        store
            .reserve_host_process_stop(&expiry, "proc_1", HostProcessEndedBy::LeaseExpiry)
            .expect("expiry"),
        HostProcessStopReservation::AlreadyEnding(_)
    ));
    assert_eq!(store.operation(&alice, "op-expire").expect("ledger"), None);
    let ended = match store
        .record_host_process_end(
            &alice,
            "proc_1",
            at(3),
            &HostProcessEnd::Exit(HostProcessExit::Signal("TERM".to_owned())),
        )
        .expect("end")
    {
        HostProcessTransition::Applied(record) => *record,
        other => panic!("expected the end, got {other:?}"),
    };
    assert_eq!(ended.state, HostProcessState::Ended);
    assert_eq!(ended.ended_by, Some(HostProcessEndedBy::Stop));
    assert_eq!(ended.exit, Some(HostProcessExit::Signal("TERM".to_owned())));
}

#[test]
fn stop_of_an_absent_or_foreign_record_is_a_durable_not_found() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let bob = scope("local:1001");
    start(&store, &alice, "op-1", &process("proc_1", "s-1", "dev"));
    for (who, operation, id) in [(&alice, "op-a", "proc_absent"), (&bob, "op-b", "proc_1")] {
        match store
            .reserve_host_process_stop(
                &op(who, operation, HOST_PROCESS_STOP_KIND, id, at(1)),
                id,
                HostProcessEndedBy::Stop,
            )
            .expect("stop")
        {
            HostProcessStopReservation::Refused { refusal, answer } => {
                assert_eq!(refusal, HostProcessRefusal::NotFound);
                assert_eq!(answer.status, 404);
            }
            other => panic!("expected not-found, got {other:?}"),
        }
        assert_eq!(
            store
                .operation(who, operation)
                .expect("ledger")
                .expect("op")
                .state,
            OperationState::Refused
        );
    }
    assert_eq!(
        store
            .host_process(&alice, "proc_1")
            .expect("read")
            .expect("record")
            .state,
        HostProcessState::Starting,
        "another subject's stop never reaches the record"
    );
}

#[test]
fn supervisor_causes_apply_only_once_due() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let mut new = process("proc_1", "s-1", "dev");
    new.timeout_ms = Some(30_000);
    start(&store, &alice, "op-1", &new);
    let supervisor = |operation: &str, when| {
        let mut new = op(&alice, operation, HOST_PROCESS_STOP_KIND, "proc_1", when);
        new.actor = HOST_PROCESS_SUPERVISOR_ACTOR.to_owned();
        new
    };
    assert!(matches!(
        store
            .reserve_host_process_stop(
                &supervisor("op-early", at(59_999)),
                "proc_1",
                HostProcessEndedBy::LeaseExpiry
            )
            .expect("early expiry"),
        HostProcessStopReservation::NotDue(_)
    ));
    assert!(matches!(
        store
            .reserve_host_process_stop(
                &supervisor("op-wrong", at(40_000)),
                "proc_1",
                HostProcessEndedBy::ExpiresAt
            )
            .expect("wrong deadline source"),
        HostProcessStopReservation::NotDue(_)
    ));
    assert_eq!(store.operation(&alice, "op-early").expect("ledger"), None);
    match store
        .reserve_host_process_stop(
            &supervisor("op-timeout", at(30_000)),
            "proc_1",
            HostProcessEndedBy::Timeout,
        )
        .expect("timeout")
    {
        HostProcessStopReservation::Accepted(record) => {
            assert_eq!(record.ended_by, Some(HostProcessEndedBy::Timeout));
        }
        other => panic!("expected the timeout stop, got {other:?}"),
    }
}

#[test]
fn exit_and_vanished_are_not_stop_causes() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    start(&store, &alice, "op-1", &process("proc_1", "s-1", "dev"));
    for cause in [HostProcessEndedBy::Exit, HostProcessEndedBy::Vanished] {
        let result = store.reserve_host_process_stop(
            &op(&alice, "op-x", HOST_PROCESS_STOP_KIND, "proc_1", at(1)),
            "proc_1",
            cause,
        );
        assert!(
            matches!(result, Err(StoreError::HostProcessInvariant(_))),
            "{result:?}"
        );
    }
    let wrong_kind = store.reserve_host_process_stop(
        &op(&alice, "op-y", HOST_PROCESS_START_KIND, "proc_1", at(1)),
        "proc_1",
        HostProcessEndedBy::Stop,
    );
    assert!(
        matches!(wrong_kind, Err(StoreError::HostProcessInvariant(_))),
        "{wrong_kind:?}"
    );
}

fn reserve_renew(store: &Store, scope: &Scope, operation: &str, id: &str, when: DateTime<Utc>) {
    assert_eq!(
        store
            .reserve(&op(scope, operation, HOST_PROCESS_RENEW_KIND, id, when))
            .expect("reserve renewal"),
        Reservation::Accepted
    );
}

#[test]
fn renewal_moves_the_expiry_from_the_renewal_instant_and_completes_the_operation() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let mut new = process("proc_1", "s-1", "dev");
    new.timeout_ms = Some(500_000);
    start(&store, &alice, "op-1", &new);
    reserve_renew(&store, &alice, "op-renew", "proc_1", at(30_000));
    match store
        .renew_host_process_lease(&alice, "op-renew", at(30_000), 200, "proc_1", 120_000)
        .expect("renew")
    {
        HostProcessRenewal::Renewed(record) => {
            assert_eq!(
                record.lease,
                HostProcessLease::Ttl {
                    ttl_ms: 120_000,
                    issued_at: at(30_000),
                    expires_at: at(150_000),
                }
            );
            assert_eq!(
                record.deadline.expect("deadline").at,
                at(500_000),
                "a renewal never moves the hard deadline"
            );
        }
        other => panic!("expected a renewal, got {other:?}"),
    }
    assert_eq!(
        store
            .operation(&alice, "op-renew")
            .expect("ledger")
            .expect("op")
            .state,
        OperationState::Terminal
    );
}

#[test]
fn renewal_refusals_leave_the_operation_for_the_caller_to_conclude() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let bob = scope("local:1001");
    start(&store, &alice, "op-1", &process("proc_1", "s-1", "dev"));
    reserve_renew(&store, &alice, "op-r1", "proc_1", at(1));
    assert_eq!(
        store
            .renew_host_process_lease(&alice, "op-r1", at(1), 200, "proc_1", MIN_LEASE_TTL_MS - 1)
            .expect("renew"),
        HostProcessRenewal::Refused(HostProcessRefusal::LeaseInvalid)
    );
    reserve_renew(&store, &bob, "op-r2", "proc_1", at(1));
    assert_eq!(
        store
            .renew_host_process_lease(&bob, "op-r2", at(1), 200, "proc_1", 60_000)
            .expect("renew"),
        HostProcessRenewal::Refused(HostProcessRefusal::NotFound),
        "another subject's record answers as absent"
    );
    reserve_renew(&store, &alice, "op-r3", "proc_1", at(60_000));
    assert!(matches!(
        store
            .renew_host_process_lease(&alice, "op-r3", at(60_000), 200, "proc_1", 60_000)
            .expect("renew"),
        HostProcessRenewal::Expired(_)
    ));
    end_exit(&store, &alice, "proc_1", 0);
    reserve_renew(&store, &alice, "op-r4", "proc_1", at(20));
    assert!(matches!(
        store
            .renew_host_process_lease(&alice, "op-r4", at(20), 200, "proc_1", 60_000)
            .expect("renew"),
        HostProcessRenewal::NotLive(_)
    ));
    for (who, operation) in [
        (&alice, "op-r1"),
        (&bob, "op-r2"),
        (&alice, "op-r3"),
        (&alice, "op-r4"),
    ] {
        assert_eq!(
            store
                .operation(who, operation)
                .expect("ledger")
                .expect("op")
                .state,
            OperationState::Accepted
        );
    }
}

#[test]
fn a_keep_may_become_a_ttl_through_renewal() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let mut new = process("proc_1", "s-1", "dev");
    new.lease = HostProcessLeaseRequest::Keep {
        reason: "kept".to_owned(),
    };
    start(&store, &alice, "op-1", &new);
    reserve_renew(&store, &alice, "op-r", "proc_1", at(5));
    match store
        .renew_host_process_lease(&alice, "op-r", at(5), 200, "proc_1", 60_000)
        .expect("renew")
    {
        HostProcessRenewal::Renewed(record) => assert_eq!(
            record.lease,
            HostProcessLease::Ttl {
                ttl_ms: 60_000,
                issued_at: at(5),
                expires_at: at(60_005),
            }
        ),
        other => panic!("expected the keep to become a TTL, got {other:?}"),
    }
}

#[test]
fn the_due_query_names_each_live_record_whose_lease_or_deadline_was_reached() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let bob = scope("local:1001");
    let elsewhere = Scope {
        deployment: "dep_other".to_owned(),
        subject: "local:1000".to_owned(),
    };
    start(
        &store,
        &alice,
        "op-lease",
        &process("proc_lease", "s-1", "lease"),
    );
    let mut deadline = process("proc_deadline", "s-1", "deadline");
    deadline.lease = HostProcessLeaseRequest::Keep {
        reason: "kept".to_owned(),
    };
    deadline.expires_at = Some(at(30_000));
    start(&store, &bob, "op-deadline", &deadline);
    let mut kept = process("proc_kept", "s-1", "kept");
    kept.lease = HostProcessLeaseRequest::Keep {
        reason: "kept".to_owned(),
    };
    start(&store, &alice, "op-kept", &kept);
    start(
        &store,
        &alice,
        "op-ended",
        &process("proc_ended", "s-1", "ended"),
    );
    end_exit(&store, &alice, "proc_ended", 0);
    start(
        &store,
        &elsewhere,
        "op-else",
        &process("proc_else", "s-1", "else"),
    );

    assert_eq!(
        store
            .due_host_processes("dep_test", at(29_999), 16)
            .expect("due"),
        vec![]
    );
    let due = store
        .due_host_processes("dep_test", at(60_000), 16)
        .expect("due");
    let summary: Vec<_> = due
        .iter()
        .map(|item| {
            (
                item.scope.subject.as_str(),
                item.id.as_str(),
                item.cause,
                item.due_at,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "local:1001",
                "proc_deadline",
                HostProcessEndedBy::ExpiresAt,
                at(30_000)
            ),
            (
                "local:1000",
                "proc_lease",
                HostProcessEndedBy::LeaseExpiry,
                at(60_000)
            ),
        ]
    );
    assert_eq!(
        store
            .due_host_processes("dep_test", at(60_000), 1)
            .expect("due")
            .len(),
        1
    );
}

#[test]
fn stop_by_owner_selects_every_live_record_of_the_subject_and_session() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let bob = scope("local:1001");
    start(&store, &alice, "op-1", &process("proc_1", "s-1", "a"));
    let mut kept = process("proc_2", "s-1", "b");
    kept.lease = HostProcessLeaseRequest::Keep {
        reason: "kept".to_owned(),
    };
    start(&store, &alice, "op-2", &kept);
    start(&store, &alice, "op-3", &process("proc_3", "s-1", "c"));
    store
        .reserve_host_process_stop(
            &op(&alice, "op-stop", HOST_PROCESS_STOP_KIND, "proc_3", at(1)),
            "proc_3",
            HostProcessEndedBy::Stop,
        )
        .expect("stop");
    start(&store, &alice, "op-4", &process("proc_4", "s-1", "d"));
    end_exit(&store, &alice, "proc_4", 0);
    start(&store, &alice, "op-5", &process("proc_5", "s-2", "a"));
    start(&store, &bob, "op-6", &process("proc_6", "s-1", "a"));

    assert_eq!(
        store
            .host_processes_for_owner(&alice, "s-1", false)
            .expect("select"),
        vec![
            "proc_1".to_owned(),
            "proc_2".to_owned(),
            "proc_3".to_owned()
        ]
    );
    assert_eq!(
        store
            .host_processes_for_owner(&alice, "s-1", true)
            .expect("select"),
        vec!["proc_1".to_owned(), "proc_3".to_owned()]
    );
    assert_eq!(
        store
            .host_processes_for_owner(&bob, "s-2", false)
            .expect("select"),
        Vec::<String>::new()
    );
}

#[test]
fn listing_is_scoped_by_subject_filtered_and_paged_within_its_bound() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let bob = scope("local:1001");
    for index in 0..5 {
        let session = if index % 2 == 0 { "s-even" } else { "s-odd" };
        start(
            &store,
            &alice,
            &format!("op-{index}"),
            &process(&format!("proc_{index}"), session, &format!("n-{index}")),
        );
    }
    end_exit(&store, &alice, "proc_4", 0);
    start(
        &store,
        &bob,
        "op-bob",
        &process("proc_bob", "s-even", "n-0"),
    );
    let limits = HostProcessLimits {
        max_page: 2,
        ..HostProcessLimits::default()
    };
    let all = HostProcessFilter::default();
    let first = store
        .list_host_processes(&alice, &all, None, 10, &limits)
        .expect("page 1");
    assert_eq!(
        first
            .items
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["proc_0", "proc_1"]
    );
    assert_eq!(first.next_after.as_deref(), Some("proc_1"));
    let second = store
        .list_host_processes(&alice, &all, first.next_after.as_deref(), 10, &limits)
        .expect("page 2");
    assert_eq!(
        second
            .items
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["proc_2", "proc_3"]
    );
    let third = store
        .list_host_processes(&alice, &all, second.next_after.as_deref(), 10, &limits)
        .expect("page 3");
    assert_eq!(
        third
            .items
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["proc_4"]
    );
    assert_eq!(third.next_after, None);

    let wide = HostProcessLimits::default();
    let even = HostProcessFilter {
        owner_session: Some("s-even".to_owned()),
        ..HostProcessFilter::default()
    };
    let page = store
        .list_host_processes(&alice, &even, None, 50, &wide)
        .expect("even");
    assert_eq!(
        page.items.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["proc_0", "proc_2", "proc_4"]
    );
    let ended = HostProcessFilter {
        state: Some(HostProcessState::Ended),
        ..HostProcessFilter::default()
    };
    let page = store
        .list_host_processes(&alice, &ended, None, 50, &wide)
        .expect("ended");
    assert_eq!(
        page.items.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["proc_4"]
    );
    let named = HostProcessFilter {
        name: Some("n-0".to_owned()),
        ..HostProcessFilter::default()
    };
    let page = store
        .list_host_processes(&bob, &named, None, 50, &wide)
        .expect("bob");
    assert_eq!(
        page.items.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["proc_bob"]
    );
    assert_eq!(store.host_process(&bob, "proc_0").expect("read"), None);
}

#[test]
fn ended_records_are_retained_for_a_bounded_time_and_count() {
    let (_dir, store) = open();
    let alice = scope("local:1000");
    let bob = scope("local:1001");
    for index in 0..4 {
        start(
            &store,
            &alice,
            &format!("op-{index}"),
            &process(&format!("proc_{index}"), "s-1", &format!("n-{index}")),
        );
        store
            .record_host_process_end(
                &alice,
                &format!("proc_{index}"),
                at(i64::from(index) * 1_000),
                &HostProcessEnd::Vanished,
            )
            .expect("end");
    }
    start(
        &store,
        &alice,
        "op-live",
        &process("proc_live", "s-1", "live"),
    );
    start(&store, &bob, "op-bob", &process("proc_bob", "s-1", "n"));
    store
        .record_host_process_end(&bob, "proc_bob", at(0), &HostProcessEnd::Vanished)
        .expect("end");
    let limits = HostProcessLimits {
        retain_ended_ms: 10_000,
        retain_ended_count: 2,
        ..HostProcessLimits::default()
    };
    // Count: alice keeps her two newest ended records; bob's one is within both bounds.
    let removed = store
        .prune_ended_host_processes("dep_test", at(5_000), &limits, 16)
        .expect("prune by count");
    assert_eq!(
        removed,
        vec![
            (alice.clone(), "proc_0".to_owned()),
            (alice.clone(), "proc_1".to_owned())
        ]
    );
    // Time: everything ended at or before now - retain_ms goes, one batch at a time.
    let removed = store
        .prune_ended_host_processes("dep_test", at(13_000), &limits, 1)
        .expect("prune by time, batch 1");
    assert_eq!(removed, vec![(alice.clone(), "proc_2".to_owned())]);
    let removed = store
        .prune_ended_host_processes("dep_test", at(13_000), &limits, 16)
        .expect("prune by time");
    assert_eq!(
        removed,
        vec![
            (alice.clone(), "proc_3".to_owned()),
            (bob.clone(), "proc_bob".to_owned())
        ]
    );
    assert!(
        store
            .host_process(&alice, "proc_live")
            .expect("read")
            .is_some()
    );
}

#[test]
fn records_survive_reopening_the_store() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("state.sqlite");
    let alice = scope("local:1000");
    let record = {
        let store = Store::open(&path).expect("open");
        start(&store, &alice, "op-1", &process("proc_1", "s-1", "dev"))
    };
    let store = Store::open(&path).expect("reopen");
    assert_eq!(
        store.host_process(&alice, "proc_1").expect("read"),
        Some(record)
    );
}

#[test]
fn the_table_refuses_a_lease_and_a_keep_together() {
    let (_dir, store) = open();
    let connection = store.connection.lock();
    let result = connection.execute(
        "INSERT INTO host_processes (
            deployment, subject, id, owner_session, owner_name, reason, argv_json, cwd,
            env_names_json, env_digest, profile, confinement, lease_ttl_ms, lease_issued_ms,
            lease_expires_ms, keep_reason, state, start_operation, started_ms
         ) VALUES ('d','s','p','o','n','r','[\"a\"]','/','[]','sha256:x','host','none',
                   1000, 0, 1000, 'both', 'starting', 'op', 0)",
        [],
    );
    assert!(
        result.is_err(),
        "a TTL and a keep together violate the row's check"
    );
}
