use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use chrono::TimeZone as _;
use rusqlite::params;
use substrate_wire::{
    ConfinementRequest, ErrorClass, ErrorDetail, EventCause, Exec, ExecKind, ExecState,
    LeaseObservation, LeaseState, NetworkMode, OperationOutcome, OperationState, PipeSession,
    PipeSessionLimits, SandboxProfile, SessionAttachmentState, SessionKind, SessionMode,
    SessionState, SnapshotItemKind, Workspace, WorkspaceKind, WorkspaceState,
};
use tempfile::tempdir;

use crate::events::event_cursor;
use crate::execs::upsert_exec;
use crate::leases::{LEASE_SWEEPER_ACTOR, lease_due, upsert_lease};
use crate::{
    CommitEffect, CommitEffectSink, EventCursorError, ExecRetireReservation, ExecWrite,
    ExpiredLease, LeaseClock, LeaseResource, NewLease, NewOperation, OperationCapacity,
    Reservation, Scope, SessionAttachmentClaim, SnapshotReadError, Store, StoreConfig, StoreError,
    StoredExec, WorkspaceAdmission, WorkspaceDestroyReservation, WorkspaceObservationWrite,
};

#[derive(Default)]
struct RecordingEffects(StdMutex<Vec<CommitEffect>>);

impl CommitEffectSink for RecordingEffects {
    fn committed(&self, effects: &[CommitEffect]) {
        self.0
            .lock()
            .expect("effect recorder")
            .extend_from_slice(effects);
    }
}

fn attach_effect_recorder(store: &Store) -> Arc<RecordingEffects> {
    let effects = Arc::new(RecordingEffects::default());
    let sink: Arc<dyn CommitEffectSink> = effects.clone();
    store.set_commit_effect_sink(sink);
    effects
}

fn clear_effects(effects: &RecordingEffects) {
    effects.0.lock().expect("effect recorder").clear();
}

fn assert_one_exact_effect(store: &Store, effects: &RecordingEffects, expected_scope: &Scope) {
    let recorded = effects.0.lock().expect("effect recorder").clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(&recorded[0].scope, expected_scope);
    let (source_scope, generation, through_seq) = store
        .stream_position(expected_scope)
        .expect("stream position after callback");
    assert_eq!(recorded[0].source_scope, source_scope);
    assert_eq!(recorded[0].generation, generation);
    assert_eq!(recorded[0].through_seq, through_seq);
}

struct ReentrantEffects {
    store: Weak<Store>,
    calls: StdMutex<usize>,
}

impl CommitEffectSink for ReentrantEffects {
    fn committed(&self, effects: &[CommitEffect]) {
        let store = self.store.upgrade().expect("store remains alive");
        for effect in effects {
            let (source_scope, generation, through_seq) = store
                .stream_position(&effect.scope)
                .expect("sink can re-enter the store after commit");
            assert_eq!(effect.source_scope, source_scope);
            assert_eq!(effect.generation, generation);
            assert_eq!(effect.through_seq, through_seq);
        }
        *self.calls.lock().expect("reentrant call counter") += 1;
    }
}

fn scope(subject: &str) -> Scope {
    Scope {
        deployment: "dep_test".to_owned(),
        subject: subject.to_owned(),
    }
}

fn ledger_config(subject_rows: u64, global_rows: u64) -> StoreConfig {
    StoreConfig {
        operation_subject_max_rows: subject_rows,
        operation_global_max_rows: global_rows,
        operation_subject_max_bytes: 8 * 1024 * 1024,
        operation_global_max_bytes: 32 * 1024 * 1024,
        operation_max_row_bytes: 1024 * 1024,
        operation_terminal_headroom_bytes: 512 * 1024,
        ..StoreConfig::default()
    }
}

fn operation(subject: &str, hash: &str) -> NewOperation {
    operation_named(
        subject,
        "01JSTORETEST0000000001",
        "workspace.create",
        "ws_reserved",
        hash,
    )
}

fn operation_named(
    subject: &str,
    operation: &str,
    kind: &str,
    resource: &str,
    hash: &str,
) -> NewOperation {
    NewOperation {
        scope: scope(subject),
        operation: operation.to_owned(),
        operation_kind: kind.to_owned(),
        request_hash: hash.to_owned(),
        accepted_at: "2026-08-13T12:00:00Z".to_owned(),
        capability_snapshot: Some(format!("sha256:{}", "7".repeat(64))),
        actor: "test".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(resource.to_owned()),
    }
}

fn workspace(id: &str) -> Workspace {
    Workspace {
        id: id.to_owned(),
        kind: WorkspaceKind::Workspace,
        labels: BTreeMap::new(),
        observed_at: "2026-08-13T12:00:01Z".parse().expect("time"),
        state: WorkspaceState::Ready,
        lease: None,
    }
}

fn exec(id: &str, workspace: &str, state: ExecState) -> StoredExec {
    StoredExec {
        resource: Exec {
            id: id.to_owned(),
            kind: ExecKind::Exec,
            workspace: workspace.to_owned(),
            state,
            observed_at: "2026-08-13T12:00:01Z".parse().expect("time"),
            requested: ConfinementRequest {
                capability_snapshot: format!("sha256:{}", "7".repeat(64)),
                network: NetworkMode::None,
                aperture: None,
                profile: SandboxProfile::Workspace,
                required: true,
            },
            applied: None,
            exit: None,
            lease: None,
            refusal: None,
        },
        stdout: Vec::new(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        output_complete: false,
        cgroup: None,
        leader_pid: None,
    }
}

fn pipe_session(id: &str, exec_id: &str, workspace: &str, lease: &NewLease) -> PipeSession {
    PipeSession {
        id: id.to_owned(),
        kind: SessionKind::Session,
        mode: SessionMode::Pipes,
        exec: exec_id.to_owned(),
        workspace: workspace.to_owned(),
        state: SessionState::Accepted,
        attachment: SessionAttachmentState::Pending,
        observed_at: "2026-08-13T12:00:01Z".parse().expect("time"),
        capability_snapshot: format!("sha256:{}", "7".repeat(64)),
        limits: PipeSessionLimits {
            input_bytes: 1_024,
            frame_bytes: 256,
            queued_frames: 4,
        },
        exit: None,
        lease: lease.observation(),
    }
}

fn seed_exec(store: &Store, scope: &Scope, resource: &StoredExec) {
    upsert_exec(&store.connection.lock(), scope, resource).expect("seed exec membership");
}

fn lease_at(subject: &str, id: &str, ttl_ms: u64) -> (NewOperation, NewLease) {
    let operation = operation_named(
        subject,
        &format!("lease-authority-{id}"),
        "workspace.create",
        id,
        &"a".repeat(64),
    );
    let lease = NewLease {
        ttl_ms,
        clock: LeaseClock {
            wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap(),
            boot_id: "boot-test".to_owned(),
            boottime_ms: 1_000,
        },
        authorizing_operation: operation.operation.clone(),
        actor: operation.actor.clone(),
        principal: operation.principal.clone(),
    };
    (operation, lease)
}

fn seed_leased_workspace(store: &Store, subject: &str, id: &str, ttl_ms: u64) -> NewOperation {
    let (operation, lease) = lease_at(subject, id, ttl_ms);
    store.reserve(&operation).expect("reserve lease authority");
    let mut resource = workspace(id);
    resource.lease = Some(lease.observation());
    store
        .complete_workspace_leased(
            &operation.scope,
            &operation.operation,
            "2026-08-13T12:00:00Z",
            201,
            id,
            &resource,
            Some(&lease),
        )
        .expect("complete leased workspace");
    operation
}

fn authorize_exec_lease(store: &Store, subject: &str, id: &str) -> NewOperation {
    let operation = operation_named(
        subject,
        &format!("exec-lease-authority-{id}"),
        "exec.start",
        id,
        &"b".repeat(64),
    );
    let lease = NewLease {
        ttl_ms: 1_000,
        clock: LeaseClock {
            wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap(),
            boot_id: "boot-test".to_owned(),
            boottime_ms: 1_000,
        },
        authorizing_operation: operation.operation.clone(),
        actor: operation.actor.clone(),
        principal: operation.principal.clone(),
    };
    store
        .reserve(&operation)
        .expect("reserve exec lease authority");
    let connection = store.connection.lock();
    upsert_lease(
        &connection,
        &operation.scope,
        "exec",
        id,
        &lease,
        &operation.operation,
    )
    .expect("persist exec lease authority");
    drop(connection);
    operation
}

#[test]
fn commit_effects_report_only_new_events_after_successful_commit() {
    let store = Store::open(":memory:").expect("open store");
    let effects = Arc::new(RecordingEffects::default());
    let sink: Arc<dyn CommitEffectSink> = effects.clone();
    store.set_commit_effect_sink(sink);
    let accepted = operation("local:1000", &"1".repeat(64));

    assert_eq!(
        store.reserve(&accepted).expect("accept"),
        Reservation::Accepted
    );
    let recorded = effects.0.lock().expect("effect recorder").clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].scope, accepted.scope);
    let position = store
        .stream_position(&accepted.scope)
        .expect("stream position");
    assert_eq!(recorded[0].source_scope, position.0);
    assert_eq!(recorded[0].generation, position.1);
    assert_eq!(recorded[0].through_seq, 1);

    assert!(matches!(
        store.reserve(&accepted).expect("replay inspection"),
        Reservation::Pending(_)
    ));
    let mut conflicting = accepted.clone();
    conflicting.request_hash = "2".repeat(64);
    assert_eq!(
        store.reserve(&conflicting).expect("conflict inspection"),
        Reservation::Conflict
    );
    assert_eq!(effects.0.lock().expect("effect recorder").len(), 1);

    let failed = operation_named(
        "local:1000",
        "01JEFFECTROLLBACK000001",
        "exec.start",
        "ex_effect_rollback",
        &"3".repeat(64),
    );
    assert!(matches!(
        store.reserve_exec_start(
            &failed,
            &exec("ex_effect_rollback", "ws_missing", ExecState::Accepted),
            None,
            None,
        ),
        Err(StoreError::NotAccepted(_))
    ));
    assert_eq!(effects.0.lock().expect("effect recorder").len(), 1);

    // This helper appends a terminal event before validating the resource kind. The invalid
    // kind forces the transaction to roll back after append; neither state nor callback may
    // escape the rollback.
    let error = substrate_wire::ErrorDetail {
        class: substrate_wire::ErrorClass::Failed,
        code: "driver.failed".to_owned(),
        message: "failure".to_owned(),
        retriable: false,
        address: Some("resource".to_owned()),
        operation: Some(accepted.operation.clone()),
    };
    assert!(matches!(
        store.complete_dispatch_absence(
            &accepted.scope,
            &accepted.operation,
            "2026-08-13T12:00:02Z",
            500,
            "invalid-kind",
            "ws_reserved",
            &error,
        ),
        Err(StoreError::NotAccepted(_))
    ));
    assert_eq!(effects.0.lock().expect("effect recorder").len(), 1);
    assert_eq!(
        store
            .operation(&accepted.scope, &accepted.operation)
            .expect("operation lookup")
            .expect("accepted operation")
            .state,
        OperationState::Accepted
    );
}

#[test]
fn commit_effect_callback_runs_after_database_lock_release() {
    let store = Arc::new(Store::open(":memory:").expect("open store"));
    let sink = Arc::new(ReentrantEffects {
        store: Arc::downgrade(&store),
        calls: StdMutex::new(0),
    });
    let erased: Arc<dyn CommitEffectSink> = sink.clone();
    store.set_commit_effect_sink(erased);

    assert_eq!(
        store
            .reserve(&operation("local:1000", &"4".repeat(64)))
            .expect("accept operation"),
        Reservation::Accepted
    );
    assert_eq!(*sink.calls.lock().expect("reentrant call counter"), 1);
}

#[test]
fn destroy_terminal_and_conflict_report_exact_post_commit_effects() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_effect_destroy", &workspace("ws_effect_destroy"))
        .expect("seed workspace");
    let effects = attach_effect_recorder(&store);
    let destroy = operation_named(
        "local:1000",
        "01JEFFECTDESTROYTERM001",
        "workspace.destroy",
        "ws_effect_destroy",
        &"1".repeat(64),
    );
    assert!(matches!(
        store
            .reserve_workspace_destroy(&destroy, None)
            .expect("reserve destroy"),
        WorkspaceDestroyReservation::Admitted { .. }
    ));
    clear_effects(&effects);
    store
        .complete_workspace_absence(
            &scope,
            &destroy.operation,
            "2026-08-13T12:00:02Z",
            200,
            "ws_effect_destroy",
            &substrate_wire::WorkspaceAbsence {
                kind: WorkspaceKind::Workspace,
                id: "ws_effect_destroy".to_owned(),
                absent: true,
                observed_at: "2026-08-13T12:00:02Z".parse().expect("time"),
            },
        )
        .expect("complete destroy");
    assert_one_exact_effect(&store, &effects, &scope);

    store
        .put_workspace(
            &scope,
            "ws_effect_conflict",
            &workspace("ws_effect_conflict"),
        )
        .expect("seed conflict workspace");
    seed_exec(
        &store,
        &scope,
        &exec("ex_effect_active", "ws_effect_conflict", ExecState::Running),
    );
    clear_effects(&effects);
    let conflict = operation_named(
        "local:1000",
        "01JEFFECTDESTROYCONFLICT1",
        "workspace.destroy",
        "ws_effect_conflict",
        &"2".repeat(64),
    );
    assert!(matches!(
        store
            .reserve_workspace_destroy(&conflict, None)
            .expect("reserve conflict"),
        WorkspaceDestroyReservation::Refused { .. }
    ));
    assert_one_exact_effect(&store, &effects, &scope);
}

#[test]
fn observation_terminal_and_lease_claim_failure_report_exact_effects() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_effect_exec", &workspace("ws_effect_exec"))
        .expect("seed workspace");
    let start = operation_named(
        "local:1000",
        "01JEFFECTEXECSTART00001",
        "exec.start",
        "ex_effect_terminal",
        &"3".repeat(64),
    );
    let running = exec("ex_effect_terminal", "ws_effect_exec", ExecState::Running);
    assert_eq!(
        store
            .reserve_exec_start(&start, &running, None, None)
            .expect("reserve exec"),
        Reservation::Accepted
    );
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
        .expect("complete running observation");
    let effects = attach_effect_recorder(&store);
    let mut terminal = running;
    terminal.resource.state = ExecState::Exited;
    terminal.resource.exit = Some(substrate_wire::ExecExit {
        code: Some(0),
        signal: None,
    });
    terminal.output_complete = true;
    assert!(matches!(
        store
            .put_exec(&scope, &terminal)
            .expect("terminal observation"),
        ExecWrite::PersistedExact(_)
    ));
    assert_one_exact_effect(&store, &effects, &scope);

    let lease_authority = seed_leased_workspace(&store, "local:1001", "ws_effect_lease", 1_000);
    let due = LeaseClock {
        wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 1).unwrap(),
        boot_id: "boot-test".to_owned(),
        boottime_ms: 2_000,
    };
    let candidate = store
        .lease_cleanup_candidates("dep_test", &due, 10)
        .expect("lease candidates")
        .into_iter()
        .find(|candidate| candidate.id == "ws_effect_lease")
        .expect("lease candidate");
    clear_effects(&effects);
    let claimed = store
        .claim_expired_lease(&candidate, &due)
        .expect("claim lease")
        .expect("claimed lease");
    assert_one_exact_effect(&store, &effects, &lease_authority.scope);
    clear_effects(&effects);
    store
        .record_lease_cleanup_failure(&claimed, due.wall, "driver.busy")
        .expect("record cleanup failure");
    assert_one_exact_effect(&store, &effects, &lease_authority.scope);
}

#[test]
fn snapshot_limit_reports_one_exact_effect_after_refusal_commit() {
    let store = Store::open_with_config(
        ":memory:",
        StoreConfig {
            snapshot_max_workspaces: 1,
            snapshot_max_execs: 1,
            snapshot_max_provenance_events: 1,
            ..StoreConfig::default()
        },
    )
    .expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_effect_over_a", &workspace("ws_effect_over_a"))
        .expect("seed first workspace");
    store
        .put_workspace(&scope, "ws_effect_over_b", &workspace("ws_effect_over_b"))
        .expect("seed second workspace");
    let effects = attach_effect_recorder(&store);
    assert!(matches!(
        store.complete_snapshot(
            &scope,
            "test",
            None,
            "2026-08-13T12:00:00Z".parse().expect("observed at"),
            "snap_effect_limit",
            "2026-08-13T13:00:00Z".parse().expect("expiry"),
        ),
        Err(StoreError::SnapshotLimit)
    ));
    assert_one_exact_effect(&store, &effects, &scope);
}

#[test]
fn operation_row_quota_checks_existing_identity_before_capacity() {
    let store = Store::open_with_config(":memory:", ledger_config(1, 2)).expect("open store");
    let first = operation_named(
        "local:1000",
        "01JLEDGERROWFIRST000001",
        "workspace.create",
        "ws_ledger_first",
        &"1".repeat(64),
    );
    assert_eq!(store.reserve(&first).expect("first"), Reservation::Accepted);
    assert!(matches!(
        store.reserve(&first).expect("pending replay at capacity"),
        Reservation::Pending(_)
    ));
    let mut conflicting = first.clone();
    conflicting.request_hash = "2".repeat(64);
    assert_eq!(
        store.reserve(&conflicting).expect("conflict at capacity"),
        Reservation::Conflict
    );

    let same_subject = operation_named(
        "local:1000",
        "01JLEDGERROWSECOND00001",
        "workspace.create",
        "ws_ledger_second",
        &"3".repeat(64),
    );
    assert_eq!(
        store.reserve(&same_subject).expect("subject capacity"),
        Reservation::Capacity(OperationCapacity::SubjectRows)
    );
    assert!(
        store
            .operation(&same_subject.scope, &same_subject.operation)
            .expect("capacity lookup")
            .is_none()
    );

    let other_subject = operation_named(
        "local:1001",
        "01JLEDGERROWOTHER000001",
        "workspace.create",
        "ws_ledger_other",
        &"4".repeat(64),
    );
    assert_eq!(
        store.reserve(&other_subject).expect("other subject"),
        Reservation::Accepted
    );
    let global_full = operation_named(
        "local:1002",
        "01JLEDGERROWGLOBAL00001",
        "workspace.create",
        "ws_ledger_global",
        &"5".repeat(64),
    );
    assert_eq!(
        store.reserve(&global_full).expect("global capacity"),
        Reservation::Capacity(OperationCapacity::GlobalRows)
    );
}

#[test]
fn refused_operations_charge_quota_and_replay_at_capacity() {
    let store = Store::open_with_config(":memory:", ledger_config(1, 10)).expect("open store");
    let refused = operation_named(
        "local:1000",
        "01JLEDGERREFUSED000001",
        "workspace.file.write",
        "ws_refused",
        &"6".repeat(64),
    );
    let detail = ErrorDetail {
        class: ErrorClass::Refused,
        code: "request.schema-invalid".to_owned(),
        message: "invalid".to_owned(),
        retriable: false,
        address: Some("input".to_owned()),
        operation: Some(refused.operation.clone()),
    };
    assert!(matches!(
        store
            .record_refusal(&refused, "2026-08-13T12:00:00Z", 422, &detail)
            .expect("record refusal"),
        Reservation::Replay(_)
    ));
    assert!(matches!(
        store
            .record_refusal(&refused, "2026-08-13T12:00:01Z", 422, &detail)
            .expect("replay refusal"),
        Reservation::Replay(_)
    ));
    let second = operation_named(
        "local:1000",
        "01JLEDGERREFUSED000002",
        "workspace.file.write",
        "ws_refused",
        &"7".repeat(64),
    );
    assert_eq!(
        store
            .record_refusal(&second, "2026-08-13T12:00:01Z", 422, &detail)
            .expect("refusal capacity"),
        Reservation::Capacity(OperationCapacity::SubjectRows)
    );
}

#[test]
fn ledger_accounting_is_atomic_across_connections_and_survives_restart() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("state.db");
    let config = ledger_config(1, 10);
    let first = Store::open_with_config(&path, config).expect("first connection");
    let second = Store::open_with_config(&path, config).expect("second connection");
    let left = operation_named(
        "local:1000",
        "01JLEDGERCONCURRENT0001",
        "workspace.create",
        "ws_concurrent_left",
        &"8".repeat(64),
    );
    let right = operation_named(
        "local:1000",
        "01JLEDGERCONCURRENT0002",
        "workspace.create",
        "ws_concurrent_right",
        &"9".repeat(64),
    );
    let (left_result, right_result) = std::thread::scope(|threads| {
        let left_task = threads.spawn(|| first.reserve(&left).expect("left reservation"));
        let right_task = threads.spawn(|| second.reserve(&right).expect("right reservation"));
        (
            left_task.join().expect("left thread"),
            right_task.join().expect("right thread"),
        )
    });
    assert!(matches!(
        (left_result, right_result),
        (
            Reservation::Accepted,
            Reservation::Capacity(OperationCapacity::SubjectRows)
        ) | (
            Reservation::Capacity(OperationCapacity::SubjectRows),
            Reservation::Accepted
        )
    ));
    drop(first);
    drop(second);

    let reopened = Store::open_with_config(&path, config).expect("reopen at exact cap");
    let usage: (i64, i64) = reopened
        .connection
        .lock()
        .query_row(
            "SELECT row_count, byte_count FROM operation_ledger_usage
             WHERE deployment = 'dep_test' AND subject = 'local:1000'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("durable usage");
    assert_eq!(usage.0, 1);
    assert!(usage.1 > 0);
}

#[test]
fn startup_fails_closed_when_configured_caps_are_below_occupancy() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("state.db");
    let store = Store::open_with_config(&path, ledger_config(2, 10)).expect("open store");
    for index in 0..2 {
        let operation = operation_named(
            "local:1000",
            &format!("01JLEDGEROCCUPANCY{index:06}"),
            "workspace.create",
            &format!("ws_occupancy_{index}"),
            &format!("{index}").repeat(64),
        );
        assert_eq!(
            store.reserve(&operation).expect("seed occupancy"),
            Reservation::Accepted
        );
    }
    drop(store);

    assert!(matches!(
        Store::open_with_config(&path, ledger_config(1, 10)),
        Err(StoreError::OperationOccupancy(
            OperationCapacity::SubjectRows
        ))
    ));
    assert!(Store::open_with_config(&path, ledger_config(2, 10)).is_ok());
}

#[test]
fn byte_quota_accepts_exact_boundary_and_refuses_next_row() {
    let directory = tempdir().expect("tempdir");
    let measure_path = directory.path().join("measure.db");
    let operation = operation_named(
        "local:1000",
        "01JLEDGERBYTEBOUNDARY01",
        "workspace.create",
        "ws_byte_boundary_a",
        &"a".repeat(64),
    );
    let measure =
        Store::open_with_config(&measure_path, ledger_config(10, 10)).expect("measurement store");
    assert_eq!(
        measure.reserve(&operation).expect("measure reservation"),
        Reservation::Accepted
    );
    let charged: i64 = measure
        .connection
        .lock()
        .query_row(
            "SELECT charged_bytes FROM operations
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
            params![
                operation.scope.deployment,
                operation.scope.subject,
                operation.operation
            ],
            |row| row.get(0),
        )
        .expect("charged bytes");
    drop(measure);

    let boundary_path = directory.path().join("boundary.db");
    let charged = u64::try_from(charged).expect("positive charge");
    let config = StoreConfig {
        operation_subject_max_rows: 10,
        operation_global_max_rows: 10,
        operation_subject_max_bytes: charged,
        operation_global_max_bytes: charged * 2,
        operation_max_row_bytes: charged,
        ..StoreConfig::default()
    };
    let store = Store::open_with_config(&boundary_path, config).expect("boundary store");
    assert_eq!(
        store.reserve(&operation).expect("exact byte boundary"),
        Reservation::Accepted
    );
    let second = operation_named(
        "local:1000",
        "01JLEDGERBYTEBOUNDARY02",
        "workspace.create",
        "ws_byte_boundary_b",
        &"b".repeat(64),
    );
    assert_eq!(
        store.reserve(&second).expect("byte capacity"),
        Reservation::Capacity(OperationCapacity::SubjectBytes)
    );
}

#[test]
fn global_byte_and_max_row_boundaries_are_enforced() {
    let directory = tempdir().expect("tempdir");
    let measure_path = directory.path().join("measure-global.db");
    let first = operation_named(
        "local:1000",
        "01JLEDGERGLOBALBYTES001",
        "workspace.create",
        "ws_global_bytes_a",
        &"d".repeat(64),
    );
    let measure =
        Store::open_with_config(&measure_path, ledger_config(10, 10)).expect("measurement store");
    assert_eq!(
        measure.reserve(&first).expect("measurement reservation"),
        Reservation::Accepted
    );
    let charged = measure
        .connection
        .lock()
        .query_row(
            "SELECT charged_bytes FROM operations
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
            params![first.scope.deployment, first.scope.subject, first.operation],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| u64::try_from(value).expect("positive charge"))
        .expect("charged bytes");
    drop(measure);

    let global_path = directory.path().join("global.db");
    let global_config = StoreConfig {
        operation_subject_max_rows: 10,
        operation_global_max_rows: 10,
        operation_subject_max_bytes: charged,
        operation_global_max_bytes: charged,
        operation_max_row_bytes: charged,
        ..StoreConfig::default()
    };
    let global = Store::open_with_config(&global_path, global_config).expect("global store");
    assert_eq!(
        global.reserve(&first).expect("global exact boundary"),
        Reservation::Accepted
    );
    let other = operation_named(
        "local:1001",
        "01JLEDGERGLOBALBYTES002",
        "workspace.create",
        "ws_global_bytes_b",
        &"e".repeat(64),
    );
    assert_eq!(
        global.reserve(&other).expect("global byte capacity"),
        Reservation::Capacity(OperationCapacity::GlobalBytes)
    );

    let row_path = directory.path().join("row.db");
    let row_config = StoreConfig {
        operation_subject_max_rows: 10,
        operation_global_max_rows: 10,
        operation_subject_max_bytes: charged * 2,
        operation_global_max_bytes: charged * 2,
        operation_max_row_bytes: charged - 1,
        ..StoreConfig::default()
    };
    let row = Store::open_with_config(&row_path, row_config).expect("row store");
    assert_eq!(
        row.reserve(&first).expect("row byte capacity"),
        Reservation::Capacity(OperationCapacity::RowBytes)
    );
}

#[test]
fn terminal_update_cannot_exceed_reserved_headroom_and_rolls_back() {
    let config = StoreConfig {
        operation_subject_max_rows: 10,
        operation_global_max_rows: 10,
        operation_subject_max_bytes: 16 * 1024,
        operation_global_max_bytes: 32 * 1024,
        operation_max_row_bytes: 4 * 1024,
        operation_terminal_headroom_bytes: 32,
        ..StoreConfig::default()
    };
    let store = Store::open_with_config(":memory:", config).expect("open store");
    let accepted = operation_named(
        "local:1000",
        "01JLEDGERHEADROOM000001",
        "workspace.create",
        "ws_headroom",
        &"c".repeat(64),
    );
    assert_eq!(
        store.reserve(&accepted).expect("accept"),
        Reservation::Accepted
    );
    let before: i64 = store
        .connection
        .lock()
        .query_row(
            "SELECT byte_count FROM operation_ledger_usage
             WHERE deployment = 'dep_test' AND subject = 'local:1000'",
            [],
            |row| row.get(0),
        )
        .expect("usage before terminal");
    let oversized = "x".repeat(1_024);
    assert!(matches!(
        store.complete_success(
            &accepted.scope,
            &accepted.operation,
            "2026-08-13T12:00:01Z",
            200,
            Some("ws_headroom"),
            &oversized,
        ),
        Err(StoreError::OperationTerminalHeadroom(_))
    ));
    assert_eq!(
        store
            .operation(&accepted.scope, &accepted.operation)
            .expect("operation")
            .expect("accepted operation")
            .state,
        OperationState::Accepted
    );
    let after: i64 = store
        .connection
        .lock()
        .query_row(
            "SELECT byte_count FROM operation_ledger_usage
             WHERE deployment = 'dep_test' AND subject = 'local:1000'",
            [],
            |row| row.get(0),
        )
        .expect("usage after rollback");
    assert_eq!(after, before);
}

#[test]
#[allow(clippy::too_many_lines)] // Proves workspace and exec admission in one transaction story.
fn provisional_workspace_and_exec_membership_commit_with_acceptance() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let workspace_operation = operation_named(
        "local:1000",
        "01JPROVISIONALWORKSPACE01",
        "workspace.create",
        "ws_provisional",
        &"1".repeat(64),
    );
    let mut provisional_workspace = workspace("ws_provisional");
    provisional_workspace.state = WorkspaceState::Unknown;
    assert_eq!(
        store
            .reserve_workspace_create(
                &workspace_operation,
                "arbitrary-root-name",
                &provisional_workspace,
                None,
            )
            .expect("reserve workspace"),
        Reservation::Accepted
    );
    let (root_name, durable_workspace) = store
        .workspace(&scope, "ws_provisional")
        .expect("workspace lookup")
        .expect("provisional workspace");
    assert_eq!(root_name, "arbitrary-root-name");
    assert_eq!(durable_workspace.state, WorkspaceState::Unknown);
    assert_eq!(
        store
            .operation(&scope, &workspace_operation.operation)
            .expect("operation lookup")
            .expect("workspace operation")
            .state,
        OperationState::Accepted
    );

    let mut ready_workspace = durable_workspace;
    ready_workspace.state = WorkspaceState::Ready;
    store
        .complete_workspace(
            &scope,
            &workspace_operation.operation,
            "2026-08-13T12:00:01Z",
            201,
            &root_name,
            &ready_workspace,
        )
        .expect("complete workspace create");
    let exec_operation = operation_named(
        "local:1000",
        "01JPROVISIONALEXEC000001",
        "exec.start",
        "ex_provisional",
        &"2".repeat(64),
    );
    let provisional_exec = exec("ex_provisional", "ws_provisional", ExecState::Accepted);
    assert_eq!(
        store
            .reserve_exec_start(&exec_operation, &provisional_exec, None, None)
            .expect("reserve exec"),
        Reservation::Accepted
    );
    assert_eq!(
        store
            .exec(&scope, "ex_provisional")
            .expect("exec lookup")
            .expect("provisional exec"),
        provisional_exec
    );
    assert!(
        store
            .workspace_has_nonterminal_execs(&scope, "ws_provisional")
            .expect("membership")
    );

    store
        .reconcile_after_restart(
            "dep_test",
            "2026-08-13T12:00:02Z".parse().expect("cutoff"),
            "2026-08-13T12:00:02Z".parse().expect("observed"),
            64,
        )
        .expect("restart reconcile");
    assert_eq!(
        store
            .exec(&scope, "ex_provisional")
            .expect("exec lookup")
            .expect("unknown exec")
            .resource
            .state,
        ExecState::Unknown
    );
    assert!(
        store
            .workspace_has_nonterminal_execs(&scope, "ws_provisional")
            .expect("unknown membership blocks cleanup")
    );
    assert!(
        store
            .mark_workspace_destroying(
                &scope,
                "ws_provisional",
                "2026-08-13T12:00:02Z".parse().expect("time"),
            )
            .expect("destroy admission")
            .is_none()
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One transaction/restart scenario keeps its setup adjacent.
fn restart_makes_pipe_session_nonattachable_without_redispatching_its_exec() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_pipe_restart", &workspace("ws_pipe_restart"))
        .expect("seed workspace");
    let start = operation_named(
        "local:1000",
        "01JPIPESTORERESTART0001",
        "session.start",
        "ses_restart",
        &"b".repeat(64),
    );
    let lease = NewLease {
        ttl_ms: 60_000,
        clock: LeaseClock {
            wall: "2026-08-13T12:00:00Z".parse().expect("time"),
            boot_id: "boot-test".to_owned(),
            boottime_ms: 1_000,
        },
        authorizing_operation: start.operation.clone(),
        actor: "test".to_owned(),
        principal: None,
    };
    let mut running = exec("ex_pipe_restart", "ws_pipe_restart", ExecState::Accepted);
    running.resource.lease = Some(lease.observation());
    let provisional = pipe_session("ses_restart", "ex_pipe_restart", "ws_pipe_restart", &lease);
    assert_eq!(
        store
            .reserve_pipe_session_start(&start, &provisional, &running, &lease, None)
            .expect("reserve session"),
        Reservation::Accepted
    );
    running.resource.state = ExecState::Running;
    let mut ready = provisional;
    ready.state = SessionState::Ready;
    ready.attachment = SessionAttachmentState::Available;
    store
        .complete_pipe_session_start(
            &scope,
            &start.operation,
            "2026-08-13T12:00:01Z",
            202,
            &ready,
            &running,
            &lease,
        )
        .expect("complete session start");
    assert_eq!(
        store
            .claim_pipe_session_attachment(
                &scope,
                "ses_restart",
                "2026-08-13T12:00:02Z".parse().expect("time"),
            )
            .expect("claim attachment"),
        SessionAttachmentClaim::Claimed
    );
    store
        .reconcile_after_restart(
            "dep_test",
            "2026-08-13T12:00:03Z".parse().expect("cutoff"),
            "2026-08-13T12:00:03Z".parse().expect("observed"),
            64,
        )
        .expect("restart reconcile");
    let session = store
        .session(&scope, "ses_restart")
        .expect("session lookup")
        .expect("durable session");
    assert_eq!(session.state, SessionState::Unknown);
    assert_eq!(session.attachment, SessionAttachmentState::Uncertain);
    assert_eq!(
        store
            .exec(&scope, "ex_pipe_restart")
            .expect("exec lookup")
            .expect("durable exec")
            .resource
            .state,
        ExecState::Unknown
    );
    assert!(
        store
            .recovery_execs(
                "dep_test",
                "2026-08-13T12:00:03Z".parse().expect("cutoff"),
                8,
            )
            .expect("recovery candidates")
            .iter()
            .any(|candidate| candidate.stored.resource.id == "ex_pipe_restart")
    );
    let events = store
        .events(&scope, None, 100)
        .expect("session events")
        .expect("event page");
    assert!(
        events
            .items
            .iter()
            .all(|event| event.validate_closed_shape().is_ok())
    );
    assert!(events.items.iter().any(|event| {
        event.transition == "operation.terminal" && event.resource == start.operation
    }));
    assert_eq!(
        store
            .claim_pipe_session_attachment(
                &scope,
                "ses_restart",
                "2026-08-13T12:00:04Z".parse().expect("time"),
            )
            .expect("repeat claim"),
        SessionAttachmentClaim::AlreadyClaimed
    );
}

#[test]
fn restart_reconciliation_is_batched_and_provisional_membership_is_resolvable() {
    let store = Store::open(":memory:").expect("open store");
    let cutoff = "2026-08-13T12:01:00Z".parse().expect("cutoff");
    for index in 0..3 {
        let operation = operation_named(
            "local:1000",
            &format!("01JBOUNDEDRESTART{index:07}"),
            "workspace.file.write",
            "ws_bounded",
            &format!("{index}").repeat(64),
        );
        assert_eq!(
            store.reserve(&operation).expect("reserve"),
            Reservation::Accepted
        );
    }
    assert_eq!(
        store
            .reconcile_after_restart("dep_test", cutoff, cutoff, 4)
            .expect("bounded recovery"),
        2
    );
    let accepted: i64 = store
        .connection
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE state = 'accepted'",
            [],
            |row| row.get(0),
        )
        .expect("accepted count");
    assert_eq!(accepted, 1);

    let workspace_create = operation_named(
        "local:1001",
        "01JRECOVERYWORKSPACE001",
        "workspace.create",
        "ws_recovery_pending",
        &"a".repeat(64),
    );
    let mut provisional = workspace("ws_recovery_pending");
    provisional.state = WorkspaceState::Unknown;
    assert_eq!(
        store
            .reserve_workspace_create(
                &workspace_create,
                "root_recovery_pending",
                &provisional,
                None,
            )
            .expect("reserve provisional workspace"),
        Reservation::Accepted
    );
    store
        .reconcile_after_restart("dep_test", cutoff, cutoff, 8)
        .expect("mark provisional operation unknown");
    let candidates = store
        .recovery_workspaces("dep_test", cutoff, 4)
        .expect("workspace recovery candidates");
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.resource.id == "ws_recovery_pending")
        .expect("provisional workspace candidate");
    let error = ErrorDetail {
        class: ErrorClass::Refused,
        code: "resource.not-found".to_owned(),
        message: "Workspace root is positively absent.".to_owned(),
        retriable: false,
        address: Some("workspace".to_owned()),
        operation: Some(candidate.operation.clone()),
    };
    store
        .complete_dispatch_absence(
            &candidate.scope,
            &candidate.operation,
            "2026-08-13T12:01:01Z",
            404,
            "workspace",
            &candidate.resource.id,
            &error,
        )
        .expect("resolve absent provisional workspace");
    assert!(
        store
            .workspace(&candidate.scope, &candidate.resource.id)
            .expect("workspace lookup")
            .is_none()
    );
}

#[test]
fn proven_absent_restart_exec_remains_observable_but_stops_blocking_cleanup() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_absent_exec", &workspace("ws_absent_exec"))
        .expect("seed workspace");
    let operation = operation_named(
        "local:1000",
        "01JRECOVERYEXECABSENT001",
        "exec.start",
        "ex_absent_restart",
        &"b".repeat(64),
    );
    let accepted = exec("ex_absent_restart", "ws_absent_exec", ExecState::Accepted);
    assert_eq!(
        store
            .reserve_exec_start(&operation, &accepted, None, None)
            .expect("reserve exec"),
        Reservation::Accepted
    );
    let mut running = accepted.clone();
    running.resource.state = ExecState::Running;
    store
        .complete_exec(
            &scope,
            &operation.operation,
            "2026-08-13T12:00:01Z",
            202,
            &running.resource,
            &[],
            &[],
            false,
            false,
            false,
            running.cgroup.as_deref(),
            running.leader_pid,
        )
        .expect("complete exec start");
    let cutoff = "2026-08-13T12:01:00Z".parse().expect("cutoff");
    store
        .reconcile_after_restart("dep_test", cutoff, cutoff, 4)
        .expect("restart recovery");
    let candidate = store
        .recovery_execs("dep_test", cutoff, 4)
        .expect("exec candidates")
        .into_iter()
        .find(|candidate| candidate.stored.resource.id == "ex_absent_restart")
        .expect("unknown exec candidate");
    assert_eq!(candidate.operation_state, OperationState::Terminal);
    store
        .mark_exec_physically_absent(&candidate, cutoff)
        .expect("persist absence proof");
    assert!(
        !store
            .workspace_has_nonterminal_execs(&scope, "ws_absent_exec")
            .expect("cleanup admission")
    );
    assert_eq!(
        store
            .exec(&scope, "ex_absent_restart")
            .expect("exec lookup")
            .expect("exec remains observable")
            .resource
            .state,
        ExecState::Unknown
    );
    assert!(
        store
            .recovery_execs("dep_test", cutoff, 4)
            .expect("recovery candidates after proof")
            .is_empty()
    );
}

#[test]
fn resource_admission_capacity_is_durable_and_replay_precedes_cap() {
    let config = StoreConfig {
        snapshot_max_workspaces: 1,
        snapshot_max_execs: 1,
        snapshot_max_provenance_events: 1,
        ..StoreConfig::default()
    };
    let store = Store::open_with_config(":memory:", config).expect("open store");
    let scope = scope("local:1000");
    let first = operation_named(
        "local:1000",
        "01JRESOURCECAPWORKSPACE1",
        "workspace.create",
        "ws_capacity_a",
        &"1".repeat(64),
    );
    assert_eq!(
        store
            .reserve_workspace_create(&first, "ws_capacity_a", &workspace("ws_capacity_a"), None)
            .expect("first workspace"),
        Reservation::Accepted
    );
    assert!(matches!(
        store
            .reserve_workspace_create(&first, "ws_capacity_a", &workspace("ws_capacity_a"), None)
            .expect("existing replay before cap"),
        Reservation::Pending(_)
    ));
    let second = operation_named(
        "local:1000",
        "01JRESOURCECAPWORKSPACE2",
        "workspace.create",
        "ws_capacity_b",
        &"2".repeat(64),
    );
    let Reservation::Replay(answer) = store
        .reserve_workspace_create(&second, "ws_capacity_b", &workspace("ws_capacity_b"), None)
        .expect("capacity refusal")
    else {
        panic!("resource capacity must be durably replayable");
    };
    assert_eq!(answer.status, 507);
    let OperationOutcome::Error { error } = answer.outcome else {
        panic!("capacity is an error outcome");
    };
    assert_eq!(error.code, "workspace.capacity");
    assert!(!error.retriable);
    assert!(
        store
            .workspace(&scope, "ws_capacity_b")
            .expect("workspace lookup")
            .is_none()
    );
    assert_eq!(
        store
            .operation(&scope, &second.operation)
            .expect("operation lookup")
            .expect("durable refusal")
            .state,
        OperationState::Refused
    );
    assert!(matches!(
        store
            .reserve_workspace_create(&second, "ws_capacity_b", &workspace("ws_capacity_b"), None)
            .expect("exact refusal replay"),
        Reservation::Replay(_)
    ));
}

#[test]
fn terminal_exec_retirement_is_atomic_idempotent_and_releases_capacity() {
    let config = StoreConfig {
        snapshot_max_workspaces: 1,
        snapshot_max_execs: 1,
        snapshot_max_provenance_events: 1,
        ..StoreConfig::default()
    };
    let store = Store::open_with_config(":memory:", config).expect("open store");
    let scope = scope("local:1000");
    seed_exec(
        &store,
        &scope,
        &exec("ex_retire", "ws_retire", ExecState::Exited),
    );
    let retire = operation_named(
        "local:1000",
        "01JEXECRETIRE000000001",
        "exec.retire",
        "ex_retire",
        &"3".repeat(64),
    );
    let ExecRetireReservation::Retired(absence) = store
        .retire_exec(
            &retire,
            "ex_retire",
            "2026-08-13T12:00:00Z".parse().expect("observed at"),
        )
        .expect("retire terminal exec")
    else {
        panic!("terminal exec must retire");
    };
    assert!(absence.absent);
    assert!(
        store
            .exec(&scope, "ex_retire")
            .expect("exec lookup")
            .is_none()
    );
    assert_eq!(
        store
            .operation(&scope, &retire.operation)
            .expect("operation lookup")
            .expect("retirement operation")
            .state,
        OperationState::Terminal
    );
    assert!(matches!(
        store
            .retire_exec(
                &retire,
                "ex_retire",
                "2026-08-13T12:00:01Z".parse().expect("observed at"),
            )
            .expect("retirement replay"),
        ExecRetireReservation::Existing(Reservation::Replay(_))
    ));
    let events = store
        .events(&scope, None, 10)
        .expect("events")
        .expect("event page");
    assert_eq!(
        events.items.last().expect("retired event").transition,
        "exec.retired"
    );
}

#[test]
fn late_exec_observation_cannot_resurrect_retired_membership() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let terminal = exec("ex_retire_race", "ws_retire", ExecState::Exited);
    seed_exec(&store, &scope, &terminal);
    let retire = operation_named(
        "local:1000",
        "01JEXECRETIRERACE00001",
        "exec.retire",
        "ex_retire_race",
        &"4".repeat(64),
    );
    let barrier = std::sync::Barrier::new(2);
    let late = terminal.clone();
    let write = std::thread::scope(|threads| {
        let thread = threads.spawn(|| {
            barrier.wait();
            store.put_exec(&scope, &late).expect("late observation")
        });
        assert!(matches!(
            store
                .retire_exec(
                    &retire,
                    "ex_retire_race",
                    "2026-08-13T12:00:02Z".parse().expect("observed at"),
                )
                .expect("retirement"),
            ExecRetireReservation::Retired(_)
        ));
        barrier.wait();
        thread.join().expect("late writer")
    });
    assert_eq!(write, ExecWrite::Retired);
    assert!(
        store
            .exec(&scope, "ex_retire_race")
            .expect("lookup")
            .is_none()
    );
}

#[test]
fn durable_replay_and_conflict_are_subject_scoped() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("state.db");
    let store = Store::open(&path).expect("open store");
    let first = operation("local:1000", &"1".repeat(64));
    assert_eq!(
        store.reserve(&first).expect("reserve"),
        Reservation::Accepted
    );
    let workspace = workspace("ws_test");
    store
        .complete_workspace(
            &first.scope,
            &first.operation,
            "2026-08-13T12:00:01Z",
            201,
            "ws_test",
            &workspace,
        )
        .expect("complete");
    let position = store
        .stream_position(&first.scope)
        .expect("stream position");
    drop(store);

    let reopened = Store::open(&path).expect("reopen");
    assert_eq!(
        reopened
            .stream_position(&first.scope)
            .expect("persisted stream"),
        position
    );
    let Reservation::Replay(answer) = reopened.reserve(&first).expect("replay") else {
        panic!("expected replay");
    };
    assert_eq!(answer.status, 201);
    assert!(matches!(answer.outcome, OperationOutcome::Success { .. }));
    assert_eq!(
        reopened
            .reserve(&operation("local:1000", &"2".repeat(64)))
            .expect("conflict"),
        Reservation::Conflict
    );
    assert_eq!(
        reopened
            .reserve(&operation("local:1001", &"2".repeat(64)))
            .expect("other subject"),
        Reservation::Accepted
    );
    assert!(
        reopened
            .operation(&scope("local:1001"), "missing")
            .expect("lookup")
            .is_none()
    );
}

#[test]
fn restart_moves_only_accepted_operations_to_unknown() {
    let store = Store::open(":memory:").expect("open store");
    let accepted = operation("local:1000", &"1".repeat(64));
    store.reserve(&accepted).expect("reserve");
    assert_eq!(
        store
            .reconcile_after_restart(
                "dep_test",
                "2026-08-13T12:00:02Z".parse().expect("cutoff"),
                "2026-08-13T12:00:02Z".parse().expect("observed"),
                64,
            )
            .expect("reconcile"),
        1
    );
    let reconciled = store
        .operation(&accepted.scope, &accepted.operation)
        .expect("lookup")
        .expect("record");
    assert_eq!(reconciled.state, substrate_wire::OperationState::Unknown);
    assert_eq!(reconciled.resource.as_deref(), Some("ws_reserved"));
    let page = store
        .events(&accepted.scope, None, 100)
        .expect("events")
        .expect("cursor");
    assert_eq!(
        page.items.last().expect("restart event").transition,
        "operation.unknown"
    );
}

#[test]
fn journal_retention_generation_and_duplicate_are_explicit() {
    let store = Store::open_with_event_retention(":memory:", 3).expect("open store");
    let scope = scope("local:1000");
    let first = operation_named(
        "local:1000",
        "01JSTOREJOURNALFIRST001",
        "workspace.create",
        "ws_first",
        &"1".repeat(64),
    );
    store.reserve(&first).expect("reserve first");
    store
        .complete_workspace(
            &scope,
            &first.operation,
            "2026-08-13T12:00:01Z",
            201,
            "ws_first",
            &workspace("ws_first"),
        )
        .expect("complete first");
    let (source_scope, generation, sequence) = store.stream_position(&scope).expect("position");
    assert_eq!(sequence, 2);
    assert!(matches!(
        store.reserve(&first).expect("duplicate"),
        Reservation::Replay(_)
    ));
    assert_eq!(
        store.stream_position(&scope).expect("unchanged").2,
        sequence
    );

    let second = operation_named(
        "local:1000",
        "01JSTOREJOURNALSECOND01",
        "workspace.create",
        "ws_second",
        &"2".repeat(64),
    );
    store.reserve(&second).expect("reserve second");
    store
        .complete_workspace(
            &scope,
            &second.operation,
            "2026-08-13T12:00:02Z",
            201,
            "ws_second",
            &workspace("ws_second"),
        )
        .expect("complete second");
    assert_eq!(store.stream_position(&scope).expect("position").2, 4);
    assert_eq!(
        store
            .events(
                &scope,
                Some(&event_cursor(&source_scope, generation, 0)),
                10
            )
            .expect("gap query"),
        Err(EventCursorError::Retention { first: 2, last: 4 })
    );
    let replacement = store
        .reset_stream_generation(&scope)
        .expect("generation reset");
    assert_ne!(replacement, generation);
    assert_eq!(
        store
            .events(
                &scope,
                Some(&event_cursor(&source_scope, generation, 4)),
                10,
            )
            .expect("old generation"),
        Err(EventCursorError::Source)
    );
}

#[test]
fn event_stream_positions_retention_and_cursors_are_subject_local() {
    let store = Store::open_with_event_retention(":memory:", 3).expect("open store");
    let scope_a = scope("local:1000");
    let scope_b = scope("local:1001");
    let operation_a = operation_named(
        "local:1000",
        "01JSTREAMSUBJECTA000001",
        "workspace.create",
        "ws_a",
        &"a".repeat(64),
    );
    store.reserve(&operation_a).expect("reserve A");
    store
        .complete_workspace(
            &scope_a,
            &operation_a.operation,
            "2026-08-13T12:00:01Z",
            201,
            "ws_a",
            &workspace("ws_a"),
        )
        .expect("complete A");
    let position_a = store.stream_position(&scope_a).expect("A position");
    assert_eq!(position_a.2, 2);

    for index in 0..4 {
        let operation_b = operation_named(
            "local:1001",
            &format!("01JSTREAMSUBJECTB{index:06}"),
            "workspace.create",
            &format!("ws_b_{index}"),
            &format!("{index:x}").repeat(64),
        );
        store.reserve(&operation_b).expect("reserve B");
        store
            .complete_workspace(
                &scope_b,
                &operation_b.operation,
                "2026-08-13T12:00:02Z",
                201,
                &format!("ws_b_{index}"),
                &workspace(&format!("ws_b_{index}")),
            )
            .expect("complete B");
    }

    assert_eq!(
        store.stream_position(&scope_a).expect("unchanged A"),
        position_a
    );
    let page_a = store
        .events(&scope_a, None, 10)
        .expect("A events")
        .expect("A cursor");
    assert_eq!(page_a.items.len(), 2);
    assert_eq!(page_a.through_seq, 2);
    assert_eq!(page_a.first_retained_seq, Some(1));
    assert!(
        page_a.items.iter().all(|event| {
            event.source_scope == position_a.0 && event.generation == position_a.1
        })
    );

    let position_b = store.stream_position(&scope_b).expect("B position");
    assert_ne!(position_a.0, position_b.0);
    assert_eq!(
        store
            .events(
                &scope_a,
                Some(&event_cursor(&position_b.0, position_b.1, position_b.2)),
                10,
            )
            .expect("cross-subject cursor"),
        Err(EventCursorError::Source)
    );
}

#[test]
fn deployment_global_stream_schema_migrates_to_fresh_subject_scopes() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("legacy.db");
    let legacy = rusqlite::Connection::open(&path).expect("legacy database");
    legacy
        .execute_batch(
            "
            CREATE TABLE stream_meta (
                deployment TEXT NOT NULL PRIMARY KEY,
                generation INTEGER NOT NULL,
                next_seq INTEGER NOT NULL
            ) WITHOUT ROWID;
            CREATE TABLE events (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                generation INTEGER NOT NULL,
                seq INTEGER NOT NULL,
                event_json TEXT NOT NULL,
                PRIMARY KEY (deployment, seq)
            ) WITHOUT ROWID;
            INSERT INTO stream_meta VALUES ('dep_test', 7, 2);
            INSERT INTO events VALUES ('dep_test', 'local:1000', 7, 1, '{}');
            ",
        )
        .expect("legacy schema");
    drop(legacy);

    let store = Store::open(&path).expect("migrated store");
    let a = store
        .stream_position(&scope("local:1000"))
        .expect("A stream");
    let b = store
        .stream_position(&scope("local:1001"))
        .expect("B stream");
    assert_eq!(a.2, 0);
    assert_eq!(b.2, 0);
    assert_ne!(a.0, b.0);
    assert!(
        store
            .events(&scope("local:1000"), None, 10)
            .expect("A events")
            .expect("A cursor")
            .items
            .is_empty()
    );
}

#[test]
fn stream_scope_migration_rotates_every_invalid_suffix_but_preserves_valid_tokens() {
    let connection = rusqlite::Connection::open_in_memory().expect("database");
    connection
        .execute_batch(
            "
            CREATE TABLE stream_meta (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                source_scope TEXT NOT NULL,
                generation INTEGER NOT NULL,
                next_seq INTEGER NOT NULL,
                PRIMARY KEY (deployment, subject),
                UNIQUE (deployment, source_scope)
            ) WITHOUT ROWID;
            CREATE TABLE events (
                deployment TEXT NOT NULL, subject TEXT NOT NULL,
                generation INTEGER NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL
            );
            CREATE TABLE snapshots (
                deployment TEXT NOT NULL, subject TEXT NOT NULL, id TEXT NOT NULL
            );
            INSERT INTO stream_meta VALUES ('valid', 's', 'scope_Az-09_ok', 1, 9);
            INSERT INTO stream_meta VALUES ('dot', 's', 'scope_a.bad', 1, 9);
            INSERT INTO stream_meta VALUES ('slash', 's', 'scope_a/bad', 1, 9);
            INSERT INTO stream_meta VALUES ('space', 's', 'scope_a bad', 1, 9);
            INSERT INTO stream_meta VALUES ('unicode', 's', 'scope_aä', 1, 9);
            ",
        )
        .expect("legacy fixtures");
    crate::schema::migrate_stream_scope_grammar(&connection).expect("migrate scope grammar");
    let valid: (String, i64, i64) = connection
        .query_row(
            "SELECT source_scope, generation, next_seq FROM stream_meta WHERE deployment = 'valid'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("valid row");
    assert_eq!(valid, ("scope_Az-09_ok".to_owned(), 1, 9));
    let mut statement = connection
        .prepare(
            "SELECT source_scope, generation, next_seq FROM stream_meta
             WHERE deployment != 'valid' ORDER BY deployment",
        )
        .expect("invalid rows");
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .expect("query invalid rows")
    {
        let (source_scope, generation, next_seq) = row.expect("invalid row");
        assert!(source_scope.starts_with("scope_"));
        assert!(
            source_scope[6..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        );
        assert_eq!(generation, 2);
        assert_eq!(next_seq, 1);
    }
}

#[test]
fn legacy_snapshot_source_scope_is_backfilled_from_its_subject_stream() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("legacy-snapshot.db");
    let legacy = rusqlite::Connection::open(&path).expect("legacy database");
    legacy
        .execute_batch(
            "
            CREATE TABLE stream_meta (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                source_scope TEXT NOT NULL,
                generation INTEGER NOT NULL,
                next_seq INTEGER NOT NULL,
                PRIMARY KEY (deployment, subject),
                UNIQUE (deployment, source_scope)
            ) WITHOUT ROWID;
            CREATE TABLE snapshots (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                id TEXT NOT NULL,
                generation INTEGER NOT NULL,
                through_seq INTEGER NOT NULL,
                item_count INTEGER NOT NULL,
                expires_at TEXT NOT NULL,
                PRIMARY KEY (deployment, subject, id)
            ) WITHOUT ROWID;
            INSERT INTO stream_meta VALUES ('dep_test', 'local:1000', 'source-legacy', 3, 8);
            INSERT INTO snapshots VALUES (
                'dep_test', 'local:1000', 'snap_legacy', 3, 7, 0,
                '2026-08-13T13:00:00Z'
            );
            ",
        )
        .expect("legacy schema");
    drop(legacy);

    let store = Store::open(&path).expect("migrated store");
    assert_eq!(
        store
            .snapshot_page(
                &scope("local:1000"),
                "snap_legacy",
                None,
                1,
                "2026-08-13T12:00:00Z".parse().expect("read time"),
            )
            .expect("snapshot read"),
        Err(SnapshotReadError::NotFound)
    );
    assert!(
        store
            .stream_position(&scope("local:1000"))
            .expect("rotated stream")
            .0
            .starts_with("scope_")
    );
}

#[test]
fn snapshot_is_materialized_stable_and_detects_incomplete_rows() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_before", &workspace("ws_before"))
        .expect("seed workspace");
    let operation = operation_named(
        "local:1000",
        "01JSTORESNAPSHOTCREATE01",
        "reconciliation.snapshot.create",
        "snap_store",
        &"3".repeat(64),
    );
    store.reserve(&operation).expect("reserve snapshot");
    let metadata = store
        .complete_snapshot(
            &scope,
            "test",
            None,
            "2026-08-13T12:00:03Z".parse().expect("observed at"),
            "snap_store",
            "2026-08-13T12:05:00Z".parse().expect("expiry"),
        )
        .expect("materialize snapshot");
    store
        .connection
        .lock()
        .execute(
            "UPDATE stream_meta SET source_scope = 'source-rotated-after-snapshot'
             WHERE deployment = ?1 AND subject = ?2",
            params![scope.deployment, scope.subject],
        )
        .expect("rotate current stream source after snapshot");
    let first = store
        .snapshot_page(
            &scope,
            "snap_store",
            None,
            1,
            "2026-08-13T12:01:00Z".parse().expect("now"),
        )
        .expect("snapshot read")
        .expect("snapshot available");
    assert_eq!(first.generation, metadata.generation);
    assert_eq!(first.through_seq, metadata.through_seq);
    store
        .put_workspace(&scope, "ws_after", &workspace("ws_after"))
        .expect("concurrent mutation");
    let mut ids = first
        .items
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let mut cursor = first.next_cursor;
    while let Some(value) = cursor {
        let page = store
            .snapshot_page(
                &scope,
                "snap_store",
                Some(&value),
                1,
                "2026-08-13T12:01:00Z".parse().expect("now"),
            )
            .expect("page read")
            .expect("page available");
        ids.extend(page.items.iter().map(|item| item.id.clone()));
        cursor = page.next_cursor;
    }
    assert_eq!(ids.len() as u64, metadata.item_count);
    assert!(ids.iter().any(|id| id == "workspace:ws_before"));
    assert!(!ids.iter().any(|id| id == "workspace:ws_after"));

    store
        .connection
        .lock()
        .execute(
            "DELETE FROM snapshot_items WHERE snapshot_id = 'snap_store' AND ordinal = 1",
            [],
        )
        .expect("corrupt materialization");
    assert_eq!(
        store
            .snapshot_page(
                &scope,
                "snap_store",
                None,
                1,
                "2026-08-13T12:01:00Z".parse().expect("now"),
            )
            .expect("incomplete read"),
        Err(SnapshotReadError::Incomplete)
    );
}

#[test]
fn empty_snapshot_is_non_keyed_and_uses_a_control_barrier() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let observed_at = "2026-08-13T12:00:00Z".parse().expect("observed at");
    let metadata = store
        .complete_snapshot(
            &scope,
            "test-actor",
            Some("test-principal"),
            observed_at,
            "snap_empty",
            "2026-08-13T13:00:00Z".parse().expect("expiry"),
        )
        .expect("empty snapshot");
    assert_eq!(metadata.item_count, 0);
    assert_eq!(metadata.partitions.workspaces, 0);
    assert_eq!(metadata.partitions.execs, 0);
    assert_eq!(metadata.partitions.provenance_events, 0);
    assert_eq!(metadata.history.first_seq, None);
    assert_eq!(metadata.history.through_seq, 0);
    assert_eq!(metadata.history.item_count, 0);
    assert!(!metadata.history.truncated);
    assert_eq!(metadata.through_seq, 1);
    assert!(metadata.source_scope.starts_with("scope_"));
    let connection = store.connection.lock();
    let operation_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .expect("operation count");
    assert_eq!(operation_count, 0);
    drop(connection);
    let events = store
        .events(&scope, None, 10)
        .expect("events")
        .expect("event page");
    assert_eq!(events.items.len(), 1);
    let created = &events.items[0];
    assert_eq!(created.seq, metadata.through_seq);
    assert_eq!(created.transition, "snapshot.created");
    assert_eq!(
        created.cause,
        EventCause::Control {
            control: substrate_wire::EventControl::ReconciliationSnapshotCreate
        }
    );
    assert_eq!(
        created.observation,
        serde_json::to_value(&metadata).unwrap()
    );
    let page = store
        .snapshot_page(&scope, "snap_empty", None, 1, observed_at)
        .expect("page")
        .expect("snapshot exists");
    assert!(page.items.is_empty());
    assert!(page.complete);
    assert!(page.next_cursor.is_none());
}

#[test]
fn snapshot_partitions_history_and_cursors_are_exact() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_partition", &workspace("ws_partition"))
        .expect("seed workspace");
    seed_exec(
        &store,
        &scope,
        &exec("ex_partition", "ws_partition", ExecState::Exited),
    );
    let cause = operation_named(
        "local:1000",
        "01JSNAPSHOTPROVENANCE001",
        "workspace.file.write",
        "ws_partition",
        &"e".repeat(64),
    );
    store.reserve(&cause).expect("accepted event");
    store
        .complete_success(
            &scope,
            &cause.operation,
            "2026-08-13T12:00:01Z",
            200,
            Some("ws_partition"),
            &serde_json::json!({"written": true}),
        )
        .expect("terminal event");
    let metadata = store
        .complete_snapshot(
            &scope,
            "test",
            None,
            "2026-08-13T12:00:02Z".parse().expect("observed at"),
            "snap_partitioned",
            "2026-08-13T13:00:00Z".parse().expect("expiry"),
        )
        .expect("snapshot");
    assert_eq!(metadata.partitions.workspaces, 1);
    assert_eq!(metadata.partitions.execs, 1);
    assert_eq!(metadata.partitions.provenance_events, 2);
    assert_eq!(metadata.item_count, 4);
    assert_eq!(metadata.history.first_seq, Some(1));
    assert_eq!(metadata.history.through_seq, 2);
    assert_eq!(metadata.through_seq, 3);
    assert!(!metadata.history.truncated);

    let first = store
        .snapshot_page(
            &scope,
            "snap_partitioned",
            None,
            2,
            "2026-08-13T12:00:03Z".parse().expect("now"),
        )
        .expect("first page")
        .expect("snapshot");
    assert_eq!(first.items.len(), 2);
    assert!(!first.complete);
    assert_eq!(first.next_cursor.as_deref(), Some("sp2.snap_partitioned.2"));
    let second = store
        .snapshot_page(
            &scope,
            "snap_partitioned",
            first.next_cursor.as_deref(),
            2,
            "2026-08-13T12:00:03Z".parse().expect("now"),
        )
        .expect("second page")
        .expect("snapshot");
    assert_eq!(second.items.len(), 2);
    assert!(second.complete);
    assert!(second.next_cursor.is_none());
    assert_eq!(
        second.items.last().expect("last item").kind,
        SnapshotItemKind::ProvenanceEvent
    );
    for invalid in [
        "sp2.snap_partitioned.0",
        "sp2.snap_partitioned.4",
        "sp2.snap_partitioned.5",
        "sp2.other.2",
        "sp_snap_partitioned_2",
    ] {
        assert_eq!(
            store
                .snapshot_page(
                    &scope,
                    "snap_partitioned",
                    Some(invalid),
                    2,
                    "2026-08-13T12:00:03Z".parse().expect("now"),
                )
                .expect("invalid cursor read"),
            Err(SnapshotReadError::InvalidCursor)
        );
    }
}

#[test]
fn snapshot_barrier_at_full_retention_keeps_bootstrap_available() {
    let store = Store::open_with_config(
        ":memory:",
        StoreConfig {
            event_retention: 2,
            snapshot_max_provenance_events: 2,
            ..StoreConfig::default()
        },
    )
    .expect("open store");
    let scope = scope("local:1000");
    let cause = operation_named(
        "local:1000",
        "01JSNAPSHOTFULLRETENTION1",
        "workspace.file.write",
        "ws_history",
        &"f".repeat(64),
    );
    store.reserve(&cause).expect("accepted event");
    store
        .complete_success(
            &scope,
            &cause.operation,
            "2026-08-13T12:00:01Z",
            200,
            Some("ws_history"),
            &serde_json::json!({"written": true}),
        )
        .expect("terminal event fills retention");

    let metadata = store
        .complete_snapshot(
            &scope,
            "test",
            None,
            "2026-08-13T12:00:02Z".parse().expect("observed at"),
            "snap_full_retention",
            "2026-08-13T13:00:00Z".parse().expect("expiry"),
        )
        .expect("barrier must not make bootstrap unavailable");

    assert_eq!(metadata.through_seq, 3);
    assert_eq!(metadata.partitions.provenance_events, 1);
    assert_eq!(metadata.history.first_seq, Some(2));
    assert_eq!(metadata.history.through_seq, 2);
    assert_eq!(metadata.history.item_count, 1);
    assert!(metadata.history.truncated);
    let page = store
        .snapshot_page(
            &scope,
            "snap_full_retention",
            None,
            2,
            "2026-08-13T12:00:03Z".parse().expect("now"),
        )
        .expect("page")
        .expect("snapshot exists");
    assert!(page.complete);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, format!("event:{}:2", metadata.generation));
}

#[test]
fn snapshot_materialization_limit_commits_only_control_refusal() {
    let config = StoreConfig {
        snapshot_max_workspaces: 1,
        snapshot_max_execs: 1,
        snapshot_max_provenance_events: 1,
        ..StoreConfig::default()
    };
    let store = Store::open_with_config(":memory:", config).expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_over_a", &workspace("ws_over_a"))
        .expect("seed A");
    store
        .put_workspace(&scope, "ws_over_b", &workspace("ws_over_b"))
        .expect("seed B bypassing admission for corruption posture");
    assert!(matches!(
        store.complete_snapshot(
            &scope,
            "test",
            None,
            "2026-08-13T12:00:00Z".parse().expect("observed at"),
            "snap_over",
            "2026-08-13T13:00:00Z".parse().expect("expiry"),
        ),
        Err(StoreError::SnapshotLimit)
    ));
    let connection = store.connection.lock();
    let snapshots: i64 = connection
        .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
        .expect("snapshot count");
    let items: i64 = connection
        .query_row("SELECT COUNT(*) FROM snapshot_items", [], |row| row.get(0))
        .expect("item count");
    let operations: i64 = connection
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .expect("operation count");
    assert_eq!((snapshots, items, operations), (0, 0, 0));
    drop(connection);
    let events = store
        .events(&scope, None, 10)
        .expect("events")
        .expect("event page");
    assert_eq!(events.items.len(), 1);
    assert_eq!(events.items[0].transition, "snapshot.refused");
    assert_eq!(
        events.items[0].observation["code"],
        "snapshot.materialization-limit"
    );
    assert!(matches!(events.items[0].cause, EventCause::Control { .. }));
}

#[test]
fn snapshot_gc_bounds_materialized_rows_and_preserves_expired_posture() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_gc", &workspace("ws_gc"))
        .expect("seed workspace");
    let operation = operation_named(
        "local:1000",
        "01JSTORESNAPSHOTGC00001",
        "reconciliation.snapshot.create",
        "snap_gc",
        &"6".repeat(64),
    );
    store.reserve(&operation).expect("reserve snapshot");
    store
        .complete_snapshot(
            &scope,
            "test",
            None,
            "2026-08-13T12:00:00Z".parse().expect("observed at"),
            "snap_gc",
            "2026-08-13T12:01:00Z".parse().expect("expiry"),
        )
        .expect("materialize snapshot");
    assert_eq!(
        store
            .prune_expired_snapshots(
                "dep_test",
                "2026-08-13T12:02:00Z".parse().expect("prune time"),
            )
            .expect("prune"),
        1
    );
    let connection = store.connection.lock();
    let snapshots: i64 = connection
        .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
        .expect("snapshot count");
    let items: i64 = connection
        .query_row("SELECT COUNT(*) FROM snapshot_items", [], |row| row.get(0))
        .expect("item count");
    assert_eq!((snapshots, items), (0, 0));
    drop(connection);
    assert_eq!(
        store
            .snapshot_page(
                &scope,
                "snap_gc",
                None,
                1,
                "2026-08-13T12:02:00Z".parse().expect("read time"),
            )
            .expect("expired lookup"),
        Err(SnapshotReadError::Expired)
    );
    assert_eq!(
        store
            .snapshot_page(
                &scope,
                "snap_never",
                None,
                1,
                "2026-08-13T12:02:00Z".parse().expect("read time"),
            )
            .expect("missing lookup"),
        Err(SnapshotReadError::NotFound)
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Multiple reopen boundaries prove cursor continuity.
fn snapshot_prune_cursor_continues_across_multiple_batches_and_reopen() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("state.db");
    let config = StoreConfig {
        snapshot_prune_batch_size: 2,
        ..StoreConfig::default()
    };
    let store = Store::open_with_config(&path, config).expect("open store");
    {
        let connection = store.connection.lock();
        for index in 0..6_u32 {
            connection
                .execute(
                    "INSERT INTO snapshots (
                        deployment, subject, id, source_scope, generation, through_seq,
                        item_count, expires_at
                     ) VALUES (?1, ?2, ?3, ?4, 1, 0, 0, ?5)",
                    params![
                        "dep_test",
                        format!("local:{}", 1_000 + index),
                        format!("snap_prune_{index}"),
                        format!("scope_prune_{index}"),
                        if index < 2 {
                            "2026-08-13T14:00:00+00:00"
                        } else {
                            "2026-08-13T12:00:00+00:00"
                        },
                    ],
                )
                .expect("seed snapshot");
        }
    }
    let now = chrono::Utc.with_ymd_and_hms(2026, 8, 13, 13, 0, 0).unwrap();
    assert_eq!(
        store
            .prune_expired_snapshots("dep_test", now)
            .expect("first prune batch"),
        0
    );
    {
        let connection = store.connection.lock();
        let cursor: (String, String) = connection
            .query_row(
                "SELECT subject, resource_id FROM maintenance_cursors
                 WHERE deployment = 'dep_test' AND queue = 'snapshot-prune'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("first durable cursor");
        assert_eq!(cursor, ("local:1001".to_owned(), "snap_prune_1".to_owned()));
    }
    drop(store);

    let reopened = Store::open_with_config(&path, config).expect("reopen for second batch");
    {
        let connection = reopened.connection.lock();
        let persisted_cursor: (String, String) = connection
            .query_row(
                "SELECT subject, resource_id FROM maintenance_cursors
                 WHERE deployment = 'dep_test' AND queue = 'snapshot-prune'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("cursor survives first reopen");
        assert_eq!(
            persisted_cursor,
            ("local:1001".to_owned(), "snap_prune_1".to_owned())
        );
    }
    assert_eq!(
        reopened
            .prune_expired_snapshots("dep_test", now)
            .expect("second prune batch"),
        2
    );
    {
        let connection = reopened.connection.lock();
        let cursor: (String, String) = connection
            .query_row(
                "SELECT subject, resource_id FROM maintenance_cursors
                 WHERE deployment = 'dep_test' AND queue = 'snapshot-prune'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("second durable cursor");
        assert_eq!(cursor, ("local:1003".to_owned(), "snap_prune_3".to_owned()));
    }
    drop(reopened);

    let reopened = Store::open_with_config(&path, config).expect("reopen for final batch");
    {
        let connection = reopened.connection.lock();
        let persisted_cursor: (String, String) = connection
            .query_row(
                "SELECT subject, resource_id FROM maintenance_cursors
                 WHERE deployment = 'dep_test' AND queue = 'snapshot-prune'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("cursor survives second reopen");
        assert_eq!(
            persisted_cursor,
            ("local:1003".to_owned(), "snap_prune_3".to_owned())
        );
    }
    assert_eq!(
        reopened
            .prune_expired_snapshots("dep_test", now)
            .expect("final prune batch"),
        2
    );
    let connection = reopened.connection.lock();
    let snapshots: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM snapshots WHERE deployment = 'dep_test'",
            [],
            |row| row.get(0),
        )
        .expect("remaining snapshots");
    let cursor: (String, String) = connection
        .query_row(
            "SELECT subject, resource_id FROM maintenance_cursors
             WHERE deployment = 'dep_test' AND queue = 'snapshot-prune'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("final durable cursor");
    assert_eq!(snapshots, 2);
    assert_eq!(cursor, ("local:1005".to_owned(), "snap_prune_5".to_owned()));
}

#[test]
fn snapshot_active_cap_terminalizes_and_replays_the_exhaustion() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    for index in 0..64_u32 {
        let operation_id = format!("01JSNAPSHOTCAP{index:010}");
        let snapshot_id = format!("snap_cap_{index:03}");
        let operation = operation_named(
            "local:1000",
            &operation_id,
            "reconciliation.snapshot.create",
            &snapshot_id,
            &format!("{index:064x}"),
        );
        store.reserve(&operation).expect("reserve snapshot");
        store
            .complete_snapshot(
                &scope,
                "test",
                None,
                "2026-08-13T12:00:00Z".parse().expect("observed at"),
                &snapshot_id,
                "2026-08-13T13:00:00Z".parse().expect("expiry"),
            )
            .expect("snapshot below active cap");
    }

    let limited = operation_named(
        "local:1000",
        "01JSNAPSHOTCAP0000000064",
        "reconciliation.snapshot.create",
        "snap_cap_064",
        &"a".repeat(64),
    );
    assert_eq!(
        store.reserve(&limited).expect("reserve cap"),
        Reservation::Accepted
    );
    assert!(matches!(
        store.complete_snapshot(
            &scope,
            "test",
            None,
            "2026-08-13T12:00:00Z".parse().expect("observed at"),
            "snap_cap_064",
            "2026-08-13T13:00:00Z".parse().expect("expiry"),
        ),
        Err(StoreError::SnapshotLimit)
    ));
    let record = store
        .operation(&scope, &limited.operation)
        .expect("lookup")
        .expect("limited operation");
    assert_eq!(record.state, OperationState::Accepted);
    assert!(matches!(
        store.reserve(&limited).expect("stable replay"),
        Reservation::Pending(_)
    ));
    let mut changed = limited;
    changed.request_hash = "b".repeat(64);
    assert_eq!(
        store.reserve(&changed).expect("changed input"),
        Reservation::Conflict
    );
}

#[test]
fn snapshot_item_cap_terminalizes_without_partial_materialization() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    for index in 0..4_096_u32 {
        let id = format!("ws_item_{index:04}");
        store
            .put_workspace(&scope, &id, &workspace(&id))
            .expect("seed item");
    }
    let operation = operation_named(
        "local:1000",
        "01JSNAPSHOTITEMCAP00001",
        "reconciliation.snapshot.create",
        "snap_item_cap",
        &"c".repeat(64),
    );
    store.reserve(&operation).expect("reserve snapshot");
    assert!(matches!(
        store.complete_snapshot(
            &scope,
            "test",
            None,
            "2026-08-13T12:00:00Z".parse().expect("observed at"),
            "snap_item_cap",
            "2026-08-13T13:00:00Z".parse().expect("expiry"),
        ),
        Err(StoreError::SnapshotLimit)
    ));
    let connection = store.connection.lock();
    let materialized: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM snapshot_items WHERE snapshot_id = 'snap_item_cap'",
            [],
            |row| row.get(0),
        )
        .expect("materialized count");
    assert_eq!(materialized, 0);
    drop(connection);
    assert_eq!(
        store
            .operation(&scope, &operation.operation)
            .expect("lookup")
            .expect("operation")
            .state,
        OperationState::Accepted
    );
}

#[test]
fn expired_snapshot_markers_are_bounded_per_scope() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    {
        let connection = store.connection.lock();
        for index in 0..1_025_u32 {
            connection
                .execute(
                    "INSERT INTO expired_snapshots (deployment, subject, id, expired_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        scope.deployment,
                        scope.subject,
                        format!("snap_marker_{index:04}"),
                        format!("2026-08-13T12:{:02}:{:02}Z", (index / 60) % 60, index % 60),
                    ],
                )
                .expect("seed marker");
        }
    }
    store
        .prune_expired_snapshots("dep_test", "2026-08-13T13:00:00Z".parse().expect("now"))
        .expect("prune markers");
    let connection = store.connection.lock();
    let retained: i64 = connection
        .query_row("SELECT COUNT(*) FROM expired_snapshots", [], |row| {
            row.get(0)
        })
        .expect("marker count");
    assert_eq!(retained, 1_024);
}

#[test]
fn exec_observation_preserves_lease_and_snapshot_projects_it() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let mut leased = exec("ex_leased", "ws_lease", ExecState::Running);
    leased.resource.lease = Some(LeaseObservation {
        ttl_ms: 60_000,
        renew_by: "2026-08-13T12:01:00Z".parse().expect("renew by"),
        state: LeaseState::Active,
        clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
        authorizing_operation: "lease-authority-ex_leased".to_owned(),
        actor: "test".to_owned(),
        principal: None,
    });
    seed_exec(&store, &scope, &leased);
    let without_lease = exec("ex_leased", "ws_lease", ExecState::Exited);
    store
        .put_exec(&scope, &without_lease)
        .expect("persist terminal observation");
    assert!(
        store
            .exec(&scope, "ex_leased")
            .expect("lookup")
            .expect("exec")
            .resource
            .lease
            .is_some()
    );

    let operation = operation_named(
        "local:1000",
        "01JSNAPSHOTLEASE0000001",
        "reconciliation.snapshot.create",
        "snap_lease",
        &"d".repeat(64),
    );
    store.reserve(&operation).expect("reserve snapshot");
    store
        .complete_snapshot(
            &scope,
            "test",
            None,
            "2026-08-13T12:00:00Z".parse().expect("observed at"),
            "snap_lease",
            "2026-08-13T13:00:00Z".parse().expect("expiry"),
        )
        .expect("snapshot");
    let page = store
        .snapshot_page(
            &scope,
            "snap_lease",
            None,
            100,
            "2026-08-13T12:01:00Z".parse().expect("now"),
        )
        .expect("snapshot read")
        .expect("snapshot available");
    let projected = page
        .items
        .iter()
        .find(|item| item.kind == SnapshotItemKind::Exec && item.id == "exec:ex_leased")
        .expect("projected exec");
    assert_eq!(projected.value["lease"]["state"], "active");
}

#[test]
fn workspace_observation_merge_never_regresses_store_owned_lifecycle() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    for state in [
        WorkspaceState::Unknown,
        WorkspaceState::Destroying,
        WorkspaceState::Expired,
    ] {
        let id = format!("ws_{state:?}").to_ascii_lowercase();
        let mut durable = workspace(&id);
        durable.state = state;
        durable
            .labels
            .insert("authority".to_owned(), "store".to_owned());
        store
            .put_workspace(&scope, &id, &durable)
            .expect("seed durable workspace");
        let mut observed = workspace(&id);
        observed.observed_at = "2026-08-13T12:01:00Z".parse().expect("time");
        observed
            .labels
            .insert("authority".to_owned(), "driver".to_owned());
        let WorkspaceObservationWrite::Authoritative(authoritative) = store
            .merge_workspace_observation(&scope, &id, &observed)
            .expect("merge observation")
        else {
            panic!("durable workspace must remain authoritative");
        };
        assert_eq!(authoritative.state, state);
        assert_eq!(authoritative.labels["authority"], "store");
        assert_eq!(authoritative.observed_at, durable.observed_at);
    }

    let mut frozen = workspace("ws_expiring");
    frozen.lease = Some(LeaseObservation {
        ttl_ms: 1_000,
        renew_by: "2026-08-13T12:00:01Z".parse().expect("renew by"),
        state: LeaseState::Expiring,
        clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
        authorizing_operation: "lease-authority-ws_expiring".to_owned(),
        actor: "test".to_owned(),
        principal: None,
    });
    store
        .put_workspace(&scope, "ws_expiring", &frozen)
        .expect("seed frozen workspace");
    let observed = workspace("ws_expiring");
    let WorkspaceObservationWrite::Authoritative(authoritative) = store
        .merge_workspace_observation(&scope, "ws_expiring", &observed)
        .expect("merge frozen observation")
    else {
        panic!("frozen workspace must remain authoritative");
    };
    assert_eq!(authoritative.lease, frozen.lease);
}

#[test]
fn due_workspace_admission_freezes_once_and_keeps_real_authorizing_operation() {
    let store = Store::open(":memory:").expect("open store");
    let operation = seed_leased_workspace(&store, "local:1000", "ws_due", 1_000);
    let due = LeaseClock {
        wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 1).unwrap(),
        boot_id: "boot-test".to_owned(),
        boottime_ms: 2_000,
    };

    let WorkspaceAdmission::Frozen {
        resource,
        newly_frozen,
    } = store
        .admit_workspace(&operation.scope, "ws_due", Some(&due))
        .expect("first admission")
    else {
        panic!("due workspace must be frozen");
    };
    assert!(newly_frozen);
    assert_eq!(
        resource.lease.as_ref().expect("lease").state,
        LeaseState::Expiring
    );
    let WorkspaceAdmission::Frozen { newly_frozen, .. } = store
        .admit_workspace(&operation.scope, "ws_due", Some(&due))
        .expect("second admission")
    else {
        panic!("frozen workspace must remain frozen");
    };
    assert!(!newly_frozen);
    let page = store
        .events(&operation.scope, None, 100)
        .expect("events")
        .expect("event page");
    let expiring = page
        .items
        .iter()
        .filter(|event| event.transition == "workspace.lease-expiring")
        .collect::<Vec<_>>();
    assert_eq!(expiring.len(), 1);
    assert_eq!(
        expiring[0].cause,
        EventCause::Operation {
            operation: operation.operation.clone()
        }
    );
    assert_eq!(expiring[0].actor, LEASE_SWEEPER_ACTOR);
}

#[test]
fn due_workspace_rejects_exec_start_without_partial_acceptance() {
    let store = Store::open(":memory:").expect("open store");
    let authority = seed_leased_workspace(&store, "local:1000", "ws_due_exec", 1_000);
    let start = operation_named(
        "local:1000",
        "01JEXECSTARTAFTERDUE001",
        "exec.start",
        "ex_after_due",
        &"c".repeat(64),
    );
    let due = LeaseClock {
        wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 1).unwrap(),
        boot_id: "boot-test".to_owned(),
        boottime_ms: 2_000,
    };
    assert!(matches!(
        store.reserve_exec_start(
            &start,
            &exec("ex_after_due", "ws_due_exec", ExecState::Accepted),
            None,
            Some(&due),
        ),
        Err(StoreError::WorkspaceFrozen)
    ));
    assert!(
        store
            .operation(&start.scope, &start.operation)
            .expect("operation lookup")
            .is_none()
    );
    assert!(
        store
            .exec(&start.scope, "ex_after_due")
            .expect("exec lookup")
            .is_none()
    );
    let page = store
        .events(&authority.scope, None, 100)
        .expect("events")
        .expect("event page");
    assert_eq!(
        page.items
            .iter()
            .filter(|event| event.transition == "workspace.lease-expiring")
            .count(),
        1
    );
}

#[test]
fn workspace_destroy_reservation_and_retry_schedule_are_atomic_and_durable() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("state.db");
    let store = Store::open(&path).expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_destroy_retry", &workspace("ws_destroy_retry"))
        .expect("seed workspace");
    let destroy = operation_named(
        "local:1000",
        "01JDESTROYRETRYSCHEDULE1",
        "workspace.destroy",
        "ws_destroy_retry",
        &"d".repeat(64),
    );
    let WorkspaceDestroyReservation::Admitted { resource, .. } = store
        .reserve_workspace_destroy(&destroy, None)
        .expect("reserve destroy")
    else {
        panic!("destroy must be admitted");
    };
    assert_eq!(resource.state, WorkspaceState::Destroying);
    assert_eq!(
        store
            .operation(&scope, &destroy.operation)
            .expect("operation")
            .expect("reserved operation")
            .state,
        OperationState::Accepted
    );
    let now = chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap();
    let pending = store
        .due_destroying_workspaces("dep_test", now, 10)
        .expect("due destroy");
    assert_eq!(pending.len(), 1);
    let next = store
        .record_workspace_cleanup_failure(&pending[0], now, "driver.busy")
        .expect("schedule retry");
    assert_eq!(next, now + chrono::Duration::milliseconds(250));
    assert!(
        store
            .due_destroying_workspaces("dep_test", now, 10)
            .expect("backoff window")
            .is_empty()
    );
    drop(store);

    let reopened = Store::open(&path).expect("reopen store");
    let due = reopened
        .due_destroying_workspaces("dep_test", next, 10)
        .expect("persisted retry");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].attempt_count, 1);
    assert_eq!(due[0].operation, destroy.operation);
}

#[test]
fn workspace_cleanup_backoff_reaches_and_remains_at_exact_cap_across_reopen() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("state.db");
    let store = Store::open(&path).expect("open store");
    let scope = scope("local:1000");
    store
        .put_workspace(&scope, "ws_destroy_cap", &workspace("ws_destroy_cap"))
        .expect("seed workspace");
    let destroy = operation_named(
        "local:1000",
        "01JDESTROYBACKOFFCAP001",
        "workspace.destroy",
        "ws_destroy_cap",
        &"e".repeat(64),
    );
    assert!(matches!(
        store
            .reserve_workspace_destroy(&destroy, None)
            .expect("reserve destroy"),
        WorkspaceDestroyReservation::Admitted { .. }
    ));
    drop(store);

    let expected_delays_ms = [
        250_i64, 500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000,
    ];
    let mut due_at = chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap();
    for (attempt, expected_delay_ms) in expected_delays_ms.into_iter().enumerate() {
        let store = Store::open(&path).expect("reopen before failure");
        let due = store
            .due_destroying_workspaces("dep_test", due_at, 1)
            .expect("due destroy");
        assert_eq!(due.len(), 1);
        assert_eq!(
            due[0].attempt_count,
            u32::try_from(attempt).expect("attempt fits")
        );
        let next = store
            .record_workspace_cleanup_failure(&due[0], due_at, "driver.busy")
            .expect("schedule failure retry");
        assert_eq!(
            next - due_at,
            chrono::Duration::milliseconds(expected_delay_ms)
        );
        assert!(
            store
                .due_destroying_workspaces("dep_test", next - chrono::Duration::milliseconds(1), 1)
                .expect("before exact retry boundary")
                .is_empty()
        );
        drop(store);

        let reopened = Store::open(&path).expect("reopen after failure");
        let persisted = reopened
            .due_destroying_workspaces("dep_test", next, 1)
            .expect("retry survives reopen");
        assert_eq!(persisted.len(), 1);
        assert_eq!(
            persisted[0].attempt_count,
            u32::try_from(attempt + 1).expect("attempt fits")
        );
        drop(reopened);
        due_at = next;
    }
}

#[test]
#[allow(clippy::too_many_lines)] // The three durable batches and retained states are one proof.
fn workspace_cleanup_continues_fairly_across_pending_batches_and_reopen() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("state.db");
    let store = Store::open(&path).expect("open store");
    let mut destroys = Vec::new();
    for index in 0..5_u32 {
        let subject = format!("local:{}", 1_000 + index);
        let id = format!("ws_destroy_batch_{index}");
        let scope = scope(&subject);
        store
            .put_workspace(&scope, &id, &workspace(&id))
            .expect("seed workspace");
        let destroy = operation_named(
            &subject,
            &format!("01JDESTROYBATCH{index:010}"),
            "workspace.destroy",
            &id,
            &format!("{index:064x}"),
        );
        assert!(matches!(
            store
                .reserve_workspace_destroy(&destroy, None)
                .expect("reserve destroy"),
            WorkspaceDestroyReservation::Admitted { .. }
        ));
        destroys.push(destroy);
    }

    let first_clock = chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 1).unwrap();
    let first = store
        .due_destroying_workspaces("dep_test", first_clock, 2)
        .expect("first batch");
    assert_eq!(
        first
            .iter()
            .map(|pending| pending.id.as_str())
            .collect::<Vec<_>>(),
        ["ws_destroy_batch_0", "ws_destroy_batch_1"]
    );
    store
        .record_workspace_cleanup_progress(&first[0], first_clock, 1)
        .expect("first remains pending after progress");
    store
        .record_workspace_cleanup_failure(&first[1], first_clock, "driver.busy")
        .expect("second remains pending after failure");
    drop(store);

    let reopened = Store::open(&path).expect("reopen before second batch");
    let second = reopened
        .due_destroying_workspaces("dep_test", first_clock, 2)
        .expect("second batch");
    assert_eq!(
        second
            .iter()
            .map(|pending| pending.id.as_str())
            .collect::<Vec<_>>(),
        ["ws_destroy_batch_2", "ws_destroy_batch_3"]
    );
    let second_clock = first_clock + chrono::Duration::seconds(1);
    for pending in &second {
        reopened
            .record_workspace_cleanup_progress(pending, second_clock, 1)
            .expect("second batch remains pending");
    }
    drop(reopened);

    let reopened = Store::open(&path).expect("reopen before third batch");
    let third = reopened
        .due_destroying_workspaces("dep_test", second_clock, 2)
        .expect("third batch");
    assert_eq!(third[0].id, "ws_destroy_batch_4");
    assert_eq!(third[1].id, "ws_destroy_batch_0");
    for destroy in &destroys {
        assert_eq!(
            reopened
                .operation(&destroy.scope, &destroy.operation)
                .expect("operation lookup")
                .expect("durable destroy operation")
                .state,
            OperationState::Accepted
        );
        assert_eq!(
            reopened
                .workspace(
                    &destroy.scope,
                    destroy.resource.as_deref().expect("workspace id"),
                )
                .expect("workspace lookup")
                .expect("destroying workspace")
                .1
                .state,
            WorkspaceState::Destroying
        );
    }
}

#[test]
fn lease_cleanup_backoff_and_fair_cursor_survive_restart() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("state.db");
    let store = Store::open(&path).expect("open store");
    let first_authority = seed_leased_workspace(&store, "local:1000", "ws_fair_a", 1_000);
    seed_leased_workspace(&store, "local:1001", "ws_fair_b", 1_000);
    seed_leased_workspace(&store, "local:1002", "ws_fair_c", 1_000);
    let due = LeaseClock {
        wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 1).unwrap(),
        boot_id: "boot-test".to_owned(),
        boottime_ms: 2_000,
    };
    let first = store
        .lease_cleanup_candidates("dep_test", &due, 1)
        .expect("first fair batch");
    assert_eq!(first.len(), 1);
    drop(store);

    let reopened = Store::open(&path).expect("reopen store");
    let second = reopened
        .lease_cleanup_candidates("dep_test", &due, 1)
        .expect("second fair batch");
    assert_eq!(second.len(), 1);
    assert_ne!(second[0].scope.subject, first[0].scope.subject);

    let candidate = if first[0].id == "ws_fair_a" {
        first[0].clone()
    } else {
        reopened
            .lease_cleanup_candidates("dep_test", &due, 3)
            .expect("find first candidate")
            .into_iter()
            .find(|candidate| candidate.id == "ws_fair_a")
            .expect("candidate A")
    };
    let claimed = reopened
        .claim_expired_lease(&candidate, &due)
        .expect("claim candidate")
        .expect("claimed candidate");
    reopened
        .record_lease_cleanup_failure(&claimed, due.wall, "driver.busy")
        .expect("persist cleanup failure");
    assert!(
        reopened
            .lease_cleanup_candidates("dep_test", &due, 10)
            .expect("backoff batch")
            .iter()
            .all(|candidate| candidate.id != "ws_fair_a")
    );
    let retry_clock = LeaseClock {
        wall: due.wall + chrono::Duration::milliseconds(250),
        ..due
    };
    assert!(
        reopened
            .lease_cleanup_candidates("dep_test", &retry_clock, 10)
            .expect("retry batch")
            .iter()
            .any(|candidate| candidate.id == "ws_fair_a")
    );
    let page = reopened
        .events(&first_authority.scope, None, 100)
        .expect("events")
        .expect("event page");
    let failure = page
        .items
        .iter()
        .find(|event| event.transition == "workspace.cleanup-failed")
        .expect("cleanup failure event");
    assert_eq!(
        failure.cause,
        EventCause::Operation {
            operation: first_authority.operation.clone()
        }
    );
    assert_eq!(failure.actor, LEASE_SWEEPER_ACTOR);
}

#[test]
#[allow(clippy::too_many_lines)] // One test proves renewal plus both boot-clock expiry branches.
fn lease_renewal_uses_boot_clock_and_changed_boot_expires_conservatively() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let issued = LeaseClock {
        wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap(),
        boot_id: "boot-a".to_owned(),
        boottime_ms: 1_000,
    };
    let create = operation_named(
        "local:1000",
        "01JSTORELEASECREATE0001",
        "workspace.create",
        "ws_lease",
        &"4".repeat(64),
    );
    let lease = NewLease {
        ttl_ms: 1_000,
        clock: issued.clone(),
        authorizing_operation: create.operation.clone(),
        actor: create.actor.clone(),
        principal: create.principal.clone(),
    };
    store.reserve(&create).expect("reserve create");
    let mut resource = workspace("ws_lease");
    resource.lease = Some(lease.observation());
    store
        .complete_workspace_leased(
            &scope,
            &create.operation,
            "2026-08-13T12:00:00Z",
            201,
            "ws_lease",
            &resource,
            Some(&lease),
        )
        .expect("complete leased create");
    let renewal = operation_named(
        "local:1000",
        "01JSTORELEASERENEW0001",
        "workspace.lease.renew",
        "ws_lease",
        &"5".repeat(64),
    );
    store.reserve(&renewal).expect("reserve renewal");
    let renewed = NewLease {
        ttl_ms: 2_000,
        clock: LeaseClock {
            wall: issued.wall + chrono::Duration::milliseconds(500),
            boot_id: "boot-a".to_owned(),
            boottime_ms: 1_500,
        },
        authorizing_operation: renewal.operation.clone(),
        actor: renewal.actor.clone(),
        principal: renewal.principal.clone(),
    };
    store
        .renew_workspace_lease(
            &scope,
            &renewal.operation,
            "2026-08-13T12:00:00.500Z",
            200,
            "ws_lease",
            &renewed,
        )
        .expect("renew lease");
    assert!(
        store
            .claim_expired_leases(
                "dep_test",
                &LeaseClock {
                    wall: issued.wall + chrono::Duration::milliseconds(900),
                    boot_id: "boot-a".to_owned(),
                    boottime_ms: 1_900,
                }
            )
            .expect("active sweep")
            .is_empty()
    );
    let expired = store
        .claim_expired_leases(
            "dep_test",
            &LeaseClock {
                wall: issued.wall + chrono::Duration::milliseconds(901),
                boot_id: "boot-b".to_owned(),
                boottime_ms: 100,
            },
        )
        .expect("changed boot sweep");
    assert_eq!(expired.len(), 1);
    assert!(matches!(
        expired[0].resource,
        LeaseResource::Workspace { .. }
    ));
    store
        .complete_workspace_lease_expiry(
            &expired[0],
            issued.wall + chrono::Duration::milliseconds(901),
        )
        .expect("complete expiry");
    assert!(
        store
            .workspace(&scope, "ws_lease")
            .expect("workspace lookup")
            .is_none()
    );
}

#[test]
fn lease_wall_skew_has_an_exact_thirty_second_ceiling() {
    let issued_wall = chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap();
    let renew_by = issued_wall + chrono::Duration::seconds(60);
    let clock = |skew_ms| LeaseClock {
        wall: issued_wall
            + chrono::Duration::milliseconds(500)
            + chrono::Duration::milliseconds(skew_ms),
        boot_id: "boot-a".to_owned(),
        boottime_ms: 1_500,
    };
    assert!(!lease_due(
        &clock(30_000),
        "boot-a",
        &issued_wall,
        1_000,
        &renew_by,
        61_000,
    ));
    assert!(lease_due(
        &clock(30_001),
        "boot-a",
        &issued_wall,
        1_000,
        &renew_by,
        61_000,
    ));
    assert!(lease_due(
        &LeaseClock {
            wall: issued_wall + chrono::Duration::seconds(60),
            boot_id: "boot-a".to_owned(),
            boottime_ms: 61_000,
        },
        "boot-a",
        &issued_wall,
        1_000,
        &renew_by,
        61_000,
    ));
}

#[test]
fn unknown_exec_blocks_cleanup_until_physical_absence_is_proven() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let resource = workspace("ws_recovery");
    store
        .put_workspace(&scope, "ws_recovery", &resource)
        .expect("seed workspace");
    seed_exec(
        &store,
        &scope,
        &exec("ex_unknown", "ws_recovery", ExecState::Unknown),
    );
    assert!(
        store
            .workspace_has_nonterminal_execs(&scope, "ws_recovery")
            .expect("unknown check")
    );
    store
        .remove_workspace(&scope, "ws_recovery")
        .expect("remove workspace");
    assert!(matches!(
        store
            .merge_workspace_observation(&scope, "ws_recovery", &resource)
            .expect("conditional observation"),
        WorkspaceObservationWrite::Missing
    ));
    assert!(
        store
            .workspace(&scope, "ws_recovery")
            .expect("workspace lookup")
            .is_none()
    );
}

#[test]
fn concurrent_maintenance_cannot_regress_durable_terminal_exec_states() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    for terminal in [ExecState::Exited, ExecState::Cancelled, ExecState::Expired] {
        let id = format!("ex_terminal_{terminal:?}").to_ascii_lowercase();
        seed_exec(&store, &scope, &exec(&id, "ws_terminal", terminal));
        std::thread::scope(|threads| {
            for proposed in [
                ExecState::Accepted,
                ExecState::Running,
                ExecState::Unknown,
                ExecState::Exited,
                ExecState::Cancelled,
                ExecState::Expired,
            ] {
                let store = &store;
                let scope = scope.clone();
                let id = id.clone();
                threads.spawn(move || {
                    store
                        .put_exec(&scope, &exec(&id, "ws_terminal", proposed))
                        .expect("maintenance update");
                });
            }
        });
        assert_eq!(
            store
                .exec(&scope, &id)
                .expect("terminal lookup")
                .expect("terminal retained")
                .resource
                .state,
            terminal
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Full-row durability is clearest in one scenario.
fn terminal_exec_authority_preserves_full_winner_across_signal_and_expiry() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let mut natural = exec("ex_terminal_full", "ws_terminal", ExecState::Exited);
    natural.stdout = b"complete stdout".to_vec();
    natural.stderr = b"complete stderr".to_vec();
    natural.output_complete = true;
    natural.resource.exit = Some(substrate_wire::ExecExit {
        code: Some(7),
        signal: None,
    });
    seed_exec(&store, &scope, &natural);
    authorize_exec_lease(&store, "local:1000", &natural.resource.id);
    let expiry = ExpiredLease {
        scope: scope.clone(),
        id: natural.resource.id.clone(),
        resource: LeaseResource::Exec {
            workspace_id: "ws_terminal".to_owned(),
        },
    };
    assert!(matches!(
        store
            .complete_exec_lease_expiry(
                &expiry,
                "2026-08-13T12:01:00Z".parse().expect("time"),
                None,
            )
            .expect("expiry after natural terminal"),
        ExecWrite::Superseded(_)
    ));
    assert_eq!(
        store
            .exec(&scope, &natural.resource.id)
            .expect("lookup")
            .expect("terminal"),
        natural
    );

    let mut partial = exec("ex_expiry_partial", "ws_terminal", ExecState::Running);
    partial.stdout = b"durable partial stdout".to_vec();
    partial.stderr = b"durable partial stderr".to_vec();
    partial.stdout_truncated = true;
    partial.cgroup = Some("cg-partial".to_owned());
    partial.leader_pid = Some(4242);
    partial.resource.lease = Some(LeaseObservation {
        ttl_ms: 1_000,
        renew_by: "2026-08-13T12:00:02Z".parse().expect("renew by"),
        state: LeaseState::Active,
        clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
        authorizing_operation: format!("exec-lease-authority-{}", partial.resource.id),
        actor: "test".to_owned(),
        principal: None,
    });
    seed_exec(&store, &scope, &partial);
    authorize_exec_lease(&store, "local:1000", &partial.resource.id);
    let partial_expiry = ExpiredLease {
        scope: scope.clone(),
        id: partial.resource.id.clone(),
        resource: LeaseResource::Exec {
            workspace_id: "ws_terminal".to_owned(),
        },
    };
    let ExecWrite::PersistedTransformed(expired_partial) = store
        .complete_exec_lease_expiry(
            &partial_expiry,
            "2026-08-13T12:00:03Z".parse().expect("time"),
            None,
        )
        .expect("expiry without driver observation")
    else {
        panic!("expiry must transform the durable partial observation");
    };
    assert_eq!(expired_partial.stdout, partial.stdout);
    assert_eq!(expired_partial.stderr, partial.stderr);
    assert_eq!(expired_partial.stdout_truncated, partial.stdout_truncated);
    assert_eq!(expired_partial.cgroup, partial.cgroup);
    assert_eq!(expired_partial.leader_pid, partial.leader_pid);
    assert_eq!(expired_partial.resource.state, ExecState::Expired);
    assert!(expired_partial.output_complete);

    let mut running = exec("ex_expiry_first", "ws_terminal", ExecState::Running);
    running.resource.lease = Some(LeaseObservation {
        ttl_ms: 1_000,
        renew_by: "2026-08-13T12:00:02Z".parse().expect("renew by"),
        state: LeaseState::Active,
        clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
        authorizing_operation: format!("exec-lease-authority-{}", running.resource.id),
        actor: "test".to_owned(),
        principal: None,
    });
    seed_exec(&store, &scope, &running);
    authorize_exec_lease(&store, "local:1000", &running.resource.id);
    let mut cancelled = running.clone();
    cancelled.resource.state = ExecState::Cancelled;
    cancelled.resource.exit = Some(substrate_wire::ExecExit {
        code: None,
        signal: Some(substrate_wire::Signal::Kill),
    });
    cancelled.stdout = b"captured before expiry".to_vec();
    cancelled.output_complete = true;
    let expiry = ExpiredLease {
        scope: scope.clone(),
        id: running.resource.id.clone(),
        resource: LeaseResource::Exec {
            workspace_id: "ws_terminal".to_owned(),
        },
    };
    let ExecWrite::PersistedTransformed(expired) = store
        .complete_exec_lease_expiry(
            &expiry,
            "2026-08-13T12:01:00Z".parse().expect("time"),
            Some(&cancelled),
        )
        .expect("expiry wins")
    else {
        panic!("expiry must transform the first terminal observation");
    };
    assert_eq!(expired.resource.state, ExecState::Expired);
    assert_eq!(expired.stdout, cancelled.stdout);
    assert!(expired.output_complete);
    assert_eq!(
        expired.resource.lease.as_ref().expect("lease").state,
        LeaseState::Expired
    );

    let signal = operation_named(
        "local:1000",
        "01JTERMINALRACESIGNAL001",
        "exec.signal",
        &running.resource.id,
        &"d".repeat(64),
    );
    store.reserve(&signal).expect("reserve signal");
    let write = store
        .complete_exec(
            &scope,
            &signal.operation,
            "2026-08-13T12:01:01Z",
            200,
            &cancelled.resource,
            &cancelled.stdout,
            &cancelled.stderr,
            cancelled.stdout_truncated,
            cancelled.stderr_truncated,
            cancelled.output_complete,
            cancelled.cgroup.as_deref(),
            cancelled.leader_pid,
        )
        .expect("stale signal completion");
    assert_eq!(write, ExecWrite::Superseded(expired.clone()));
    let signal_record = store
        .operation(&scope, &signal.operation)
        .expect("signal lookup")
        .expect("signal operation");
    let Some(OperationOutcome::Success { result }) = signal_record.outcome else {
        panic!("signal outcome");
    };
    assert_eq!(
        serde_json::from_value::<Exec>(result).expect("signal result"),
        expired.resource
    );
}

#[test]
fn independent_store_connections_commit_only_one_full_terminal_winner() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("shared.db");
    let store_a = Store::open(&path).expect("store A");
    let store_b = Store::open(&path).expect("store B");
    let scope = scope("local:1000");
    let mut exited = exec("ex_cross_connection", "ws_test", ExecState::Exited);
    exited.stdout = b"winner A".to_vec();
    exited.output_complete = true;
    exited.resource.exit = Some(substrate_wire::ExecExit {
        code: Some(0),
        signal: None,
    });
    let mut cancelled = exec("ex_cross_connection", "ws_test", ExecState::Cancelled);
    cancelled.stderr = b"winner B".to_vec();
    cancelled.output_complete = true;
    cancelled.resource.exit = Some(substrate_wire::ExecExit {
        code: None,
        signal: Some(substrate_wire::Signal::Kill),
    });
    seed_exec(
        &store_a,
        &scope,
        &exec("ex_cross_connection", "ws_test", ExecState::Running),
    );
    let barrier = std::sync::Barrier::new(2);
    let (a, b) = std::thread::scope(|threads| {
        let first = threads.spawn(|| {
            barrier.wait();
            store_a.put_exec(&scope, &exited).expect("write A")
        });
        let second = threads.spawn(|| {
            barrier.wait();
            store_b.put_exec(&scope, &cancelled).expect("write B")
        });
        (
            first.join().expect("A thread"),
            second.join().expect("B thread"),
        )
    });
    assert!(matches!(
        (&a, &b),
        (ExecWrite::PersistedExact(_), ExecWrite::Superseded(_))
            | (ExecWrite::Superseded(_), ExecWrite::PersistedExact(_))
    ));
    let durable = store_a
        .exec(&scope, "ex_cross_connection")
        .expect("lookup")
        .expect("terminal");
    for result in [a, b] {
        match result {
            ExecWrite::PersistedExact(winner) | ExecWrite::Superseded(winner) => {
                assert_eq!(winner, durable);
            }
            ExecWrite::PersistedTransformed(_) => panic!("put cannot transform"),
            ExecWrite::Retired => panic!("fixture must establish exec membership"),
        }
    }
}

#[test]
fn put_exec_reports_lease_inheritance_as_a_transformation() {
    let store = Store::open(":memory:").expect("open store");
    let scope = scope("local:1000");
    let mut accepted = exec("ex_normalized", "ws_test", ExecState::Accepted);
    accepted.resource.lease = Some(LeaseObservation {
        ttl_ms: 1_000,
        renew_by: "2026-08-13T12:00:02Z".parse().expect("renew by"),
        state: LeaseState::Active,
        clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
        authorizing_operation: "exec-lease-authority-ex_normalized".to_owned(),
        actor: "test".to_owned(),
        principal: None,
    });
    seed_exec(&store, &scope, &accepted);
    let mut running_without_lease = accepted.clone();
    running_without_lease.resource.state = ExecState::Running;
    running_without_lease.resource.lease = None;
    let ExecWrite::PersistedTransformed(authoritative) = store
        .put_exec(&scope, &running_without_lease)
        .expect("normalized write")
    else {
        panic!("lease inheritance must be visible to the caller");
    };
    assert_eq!(authoritative.resource.state, ExecState::Running);
    assert_eq!(authoritative.resource.lease, accepted.resource.lease);
}
