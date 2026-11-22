//! ADR 0017 in the transaction that decides: a stored row answers a request only after the row's
//! recorded grant is compared with the grant the request verified.
//!
//! Every case records one row and then sends a second request of the same deployment, subject
//! and operation identity straight to an entry point that can find that row, with no lookup
//! before it. That is the order two concurrent requests reach the store in, driven once.

use chrono::{TimeZone as _, Utc};
use substrate_wire::{ErrorClass, ErrorDetail, ExecState, OperationOutcome, OperationState};

use crate::tests::{exec, lease_at, operation_named, pipe_session, workspace};
use crate::{
    ExecRetireReservation, HOST_PROCESS_START_KIND, HOST_PROCESS_STOP_KIND, HostProcessEndedBy,
    HostProcessLeaseRequest, HostProcessLimits, HostProcessStartReservation,
    HostProcessStopReservation, NewHostProcess, NewOperation, Reservation,
    SessionRetireReservation, Store, StoredAnswer, WorkspaceAdoptReservation,
    WorkspaceDestroyReservation, WorkspaceRootHolder,
};

const SUBJECT: &str = "local:1000";
const OPERATION: &str = "01JREPLAYGRANT00000001";
const RESOURCE: &str = "res_replay_grant";
const GRANT_A: &str = "grant:a";
const GRANT_B: &str = "grant:b";
const TERMINAL_AT: &str = "2026-08-13T12:00:01Z";

/// One store entry point that can answer a request from an existing row.
struct Deciding {
    name: &'static str,
    operation_kind: &'static str,
    call: fn(&Store, &NewOperation) -> Reservation,
}

const DECIDING: &[Deciding] = &[
    Deciding {
        name: "inspect_reservation",
        operation_kind: "workspace.create",
        call: |store, new| {
            store
                .inspect_reservation(
                    &new.scope,
                    &new.operation,
                    &new.request_hash,
                    new.grant_ref.as_deref(),
                )
                .expect("inspect")
                .expect("the row exists")
        },
    },
    Deciding {
        name: "reserve",
        operation_kind: "workspace.file.write",
        call: |store, new| store.reserve(new).expect("reserve"),
    },
    Deciding {
        name: "record_refusal",
        operation_kind: "workspace.create",
        call: |store, new| {
            store
                .record_refusal(new, TERMINAL_AT, 422, &detail("request.schema-invalid"))
                .expect("record refusal")
        },
    },
    Deciding {
        name: "reserve_workspace_create",
        operation_kind: "workspace.create",
        call: |store, new| {
            store
                .reserve_workspace_create(new, RESOURCE, &workspace(RESOURCE), None)
                .expect("reserve workspace create")
        },
    },
    Deciding {
        name: "reserve_workspace_adopt",
        operation_kind: "workspace.adopt",
        call: |store, new| match store
            .reserve_workspace_adopt(
                new,
                RESOURCE,
                WorkspaceRootHolder::Free,
                &workspace(RESOURCE),
                TERMINAL_AT,
            )
            .expect("reserve workspace adopt")
        {
            WorkspaceAdoptReservation::Answered(reservation) => reservation,
            other @ WorkspaceAdoptReservation::Adopted(_) => {
                panic!("an existing row decides the adoption, got {other:?}")
            }
        },
    },
    Deciding {
        name: "reserve_workspace_destroy",
        operation_kind: "workspace.destroy",
        call: |store, new| match store
            .reserve_workspace_destroy(new, None)
            .expect("reserve workspace destroy")
        {
            WorkspaceDestroyReservation::Existing(reservation) => reservation,
            other => panic!("an existing row decides the destroy, got {other:?}"),
        },
    },
    Deciding {
        name: "reserve_exec_start",
        operation_kind: "exec.start",
        call: |store, new| {
            store
                .reserve_exec_start(
                    new,
                    &exec(RESOURCE, "ws_replay_grant", ExecState::Accepted),
                    None,
                    None,
                )
                .expect("reserve exec start")
        },
    },
    Deciding {
        name: "retire_exec",
        operation_kind: "exec.retire",
        call: |store, new| match store
            .retire_exec(new, RESOURCE, observed_at())
            .expect("retire exec")
        {
            ExecRetireReservation::Existing(reservation) => reservation,
            other => panic!("an existing row decides the retirement, got {other:?}"),
        },
    },
    Deciding {
        name: "reserve_pipe_session_start",
        operation_kind: "session.start",
        call: |store, new| {
            let (_, lease) = lease_at(SUBJECT, "exec_replay_grant", 1_000);
            store
                .reserve_pipe_session_start(
                    new,
                    &pipe_session(RESOURCE, "exec_replay_grant", "ws_replay_grant", &lease),
                    &exec("exec_replay_grant", "ws_replay_grant", ExecState::Accepted),
                    &lease,
                    None,
                )
                .expect("reserve pipe session start")
        },
    },
    Deciding {
        name: "retire_pipe_session",
        operation_kind: "session.retire",
        call: |store, new| match store
            .retire_pipe_session(new, RESOURCE, observed_at())
            .expect("retire pipe session")
        {
            SessionRetireReservation::Existing(reservation) => reservation,
            other => panic!("an existing row decides the retirement, got {other:?}"),
        },
    },
    Deciding {
        name: "reserve_host_process_start",
        operation_kind: HOST_PROCESS_START_KIND,
        call: |store, new| match store
            .reserve_host_process_start(new, &host_process(), &HostProcessLimits::default())
            .expect("reserve host process start")
        {
            HostProcessStartReservation::Existing(reservation) => reservation,
            other => panic!("an existing row decides the start, got {other:?}"),
        },
    },
    Deciding {
        name: "reserve_host_process_stop",
        operation_kind: HOST_PROCESS_STOP_KIND,
        call: |store, new| match store
            .reserve_host_process_stop(new, RESOURCE, HostProcessEndedBy::Stop)
            .expect("reserve host process stop")
        {
            HostProcessStopReservation::Existing(reservation) => reservation,
            other => panic!("an existing row decides the stop, got {other:?}"),
        },
    },
];

fn observed_at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 2).unwrap()
}

fn host_process() -> NewHostProcess {
    NewHostProcess {
        id: RESOURCE.to_owned(),
        owner_session: "session-replay-grant".to_owned(),
        owner_name: "preview".to_owned(),
        reason: "serve the preview".to_owned(),
        argv: vec!["true".to_owned()],
        cwd: "/srv/app".to_owned(),
        env_names: vec!["PATH".to_owned()],
        env_digest: format!("sha256:{}", "b".repeat(64)),
        lease: HostProcessLeaseRequest::Ttl { ttl_ms: 60_000 },
        timeout_ms: None,
        expires_at: None,
    }
}

fn detail(code: &str) -> ErrorDetail {
    ErrorDetail {
        class: ErrorClass::Refused,
        code: code.to_owned(),
        message: "A test outcome.".to_owned(),
        retriable: false,
        address: None,
        operation: Some(OPERATION.to_owned()),
    }
}

fn hash(digit: &str) -> String {
    digit.repeat(64)
}

/// A request of the one operation identity these cases share, under `grant` or under none.
fn request(operation_kind: &str, request_hash: &str, grant: Option<&str>) -> NewOperation {
    NewOperation {
        grant_ref: grant.map(ToOwned::to_owned),
        platform_principal: grant.map(|_| "platform:principal-1".to_owned()),
        ..operation_named(SUBJECT, OPERATION, operation_kind, RESOURCE, request_hash)
    }
}

/// The first request's row, accepted and not yet terminal.
fn record_pending(store: &Store, first: &NewOperation) {
    assert_eq!(
        store.reserve(first).expect("first request"),
        Reservation::Accepted
    );
}

/// The first request's row with its terminal outcome stored.
fn record_outcome(store: &Store, first: &NewOperation) -> StoredAnswer {
    record_pending(store, first);
    let outcome = detail("first.outcome");
    store
        .complete_error(
            &first.scope,
            &first.operation,
            TERMINAL_AT,
            409,
            None,
            &outcome,
        )
        .expect("first request's outcome");
    StoredAnswer {
        status: 409,
        outcome: OperationOutcome::Error { error: outcome },
    }
}

/// What the first request's row holds when the second request reaches the store.
#[derive(Clone, Copy)]
enum FirstRow {
    Pending,
    Outcome,
}

/// What ADR 0017 owes the second request.
#[derive(Clone, Copy)]
enum Owed {
    GrantConflict,
    RequestConflict,
    /// The first request's stored outcome.
    Replay,
}

/// Sends one pair of requests through every deciding entry point, each on a store of its own,
/// and names every entry point whose answer is not the one owed or which changed the row.
fn every_deciding_transaction_answers(
    row: FirstRow,
    first: (&str, Option<&str>),
    second: (&str, Option<&str>),
    owed: Owed,
) {
    let mut failures = Vec::new();
    for entry in DECIDING {
        let store = Store::open(":memory:").expect("open store");
        let first = request(entry.operation_kind, &hash(first.0), first.1);
        let (stored, state) = match row {
            FirstRow::Pending => {
                record_pending(&store, &first);
                (None, OperationState::Accepted)
            }
            FirstRow::Outcome => (
                Some(record_outcome(&store, &first)),
                OperationState::Terminal,
            ),
        };
        let expected = match owed {
            Owed::GrantConflict => Reservation::GrantConflict,
            Owed::RequestConflict => Reservation::Conflict,
            Owed::Replay => Reservation::Replay(stored.expect("a stored outcome to replay")),
        };

        let second = request(entry.operation_kind, &hash(second.0), second.1);
        let answered = (entry.call)(&store, &second);
        if answered != expected {
            failures.push(format!(
                "{}: answered {answered:?}, owed {expected:?}",
                entry.name
            ));
        }

        // The row is what the first request wrote: its grant, its input, its state.
        let row = store
            .operation(&first.scope, &first.operation)
            .expect("row lookup")
            .expect("the first request's row");
        if row.grant_ref != first.grant_ref
            || row.platform_principal != first.platform_principal
            || row.request_hash != first.request_hash
            || row.state != state
        {
            failures.push(format!("{}: the row was rewritten: {row:?}", entry.name));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} deciding entry points broke ADR 0017:\n{}",
        failures.len(),
        DECIDING.len(),
        failures.join("\n")
    );
}

#[test]
fn a_stored_outcome_is_returned_to_no_request_that_verified_a_different_grant() {
    every_deciding_transaction_answers(
        FirstRow::Outcome,
        ("1", Some(GRANT_A)),
        ("1", Some(GRANT_B)),
        Owed::GrantConflict,
    );
}

#[test]
fn a_pending_row_is_handed_to_no_request_that_verified_a_different_grant() {
    every_deciding_transaction_answers(
        FirstRow::Pending,
        ("1", Some(GRANT_A)),
        ("1", Some(GRANT_B)),
        Owed::GrantConflict,
    );
}

/// The grant is compared first, as the daemon's lookup compares it: a request under another
/// grant does not learn whether its input is the row's.
#[test]
fn a_different_grant_is_refused_before_the_request_hash_is_compared() {
    every_deciding_transaction_answers(
        FirstRow::Outcome,
        ("1", Some(GRANT_A)),
        ("2", Some(GRANT_B)),
        Owed::GrantConflict,
    );
}

#[test]
fn the_recorded_grant_replays_the_stored_outcome() {
    every_deciding_transaction_answers(
        FirstRow::Outcome,
        ("1", Some(GRANT_A)),
        ("1", Some(GRANT_A)),
        Owed::Replay,
    );
}

/// The historical row stays unattributed: the replaying grant is written nowhere.
#[test]
fn a_row_recorded_without_a_grant_replays_to_a_verified_grant_and_is_never_rewritten() {
    every_deciding_transaction_answers(
        FirstRow::Outcome,
        ("1", None),
        ("1", Some(GRANT_B)),
        Owed::Replay,
    );
}

/// The store compares a recorded grant with a verified one. A request that verified none is not
/// a different grant: whether its missing context is refused is the deployment's requirement,
/// which the daemon enforces before any replay response and the store does not know.
#[test]
fn a_request_that_verified_no_grant_is_not_a_grant_conflict() {
    every_deciding_transaction_answers(
        FirstRow::Outcome,
        ("1", Some(GRANT_A)),
        ("1", None),
        Owed::Replay,
    );
}

#[test]
fn the_recorded_grant_with_different_input_is_a_request_conflict() {
    every_deciding_transaction_answers(
        FirstRow::Outcome,
        ("1", Some(GRANT_A)),
        ("2", Some(GRANT_A)),
        Owed::RequestConflict,
    );
}
