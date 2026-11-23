//! Review cases for ADR 0017 and its addendum of 2026-11-23, written from the ADR's text and
//! independent of `replay_grant_tests.rs`.
//!
//! One table: every recorded grant against every verified grant, the same and a different input,
//! every state a row can hold, through every store entry point that can answer a request from an
//! existing row. Each answer is compared with what the ADR owes. The database's own count of
//! changed rows is read before and after each call, and the whole store is compared with itself
//! before and after the requests of one row: a request answered from another request's row
//! writes nothing.

use chrono::{TimeZone as _, Utc};
use serde_json::json;
use substrate_wire::{ErrorClass, ErrorDetail, ExecState, OperationOutcome, OperationRecord};

use super::{exec, lease_at, operation_named, pipe_session, workspace};
use crate::{
    ExecRetireReservation, HOST_PROCESS_START_KIND, HOST_PROCESS_STOP_KIND, HostProcessEndedBy,
    HostProcessLeaseRequest, HostProcessLimits, HostProcessStartReservation,
    HostProcessStopReservation, NewHostProcess, NewOperation, Reservation,
    SessionRetireReservation, Store, StoredAnswer, WorkspaceAdoptReservation,
    WorkspaceDestroyReservation, WorkspaceRootHolder,
};

const SUBJECT: &str = "local:1000";
const TERMINAL_AT: &str = "2026-08-13T12:00:01Z";

/// The grants a row can record and a request can verify: none, the empty string, one grant, the
/// same grant in another case, the same grant with a trailing space, and another grant.
const GRANTS: &[Option<&str>] = &[
    None,
    Some(""),
    Some("grant:a"),
    Some("GRANT:A"),
    Some("grant:a "),
    Some("grant:b"),
];

#[derive(Clone, Copy, Debug)]
enum Row {
    Accepted,
    Unknown,
    Refused,
    TerminalError,
    TerminalSuccess,
}

const ROWS: &[Row] = &[
    Row::Accepted,
    Row::Unknown,
    Row::Refused,
    Row::TerminalError,
    Row::TerminalSuccess,
];

type Call = fn(&Store, &NewOperation, &str) -> Result<Reservation, String>;

/// One store entry point that reads the operation row of the request it is given.
struct Entry {
    name: &'static str,
    operation_kind: &'static str,
    call: Call,
}

fn observed_at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 2).unwrap()
}

fn host_process(id: &str) -> NewHostProcess {
    NewHostProcess {
        id: id.to_owned(),
        owner_session: "session-review".to_owned(),
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

fn detail(code: &str, operation: &str) -> ErrorDetail {
    ErrorDetail {
        class: ErrorClass::Refused,
        code: code.to_owned(),
        message: "A review outcome.".to_owned(),
        retriable: false,
        address: None,
        operation: Some(operation.to_owned()),
    }
}

fn not_from_the_row(entry: &str, got: &dyn std::fmt::Debug) -> String {
    format!("{entry} did not answer from the existing row: {got:?}")
}

const ENTRIES: &[Entry] = &[
    Entry {
        name: "inspect_reservation",
        operation_kind: "workspace.create",
        call: |store, new, _| {
            store
                .inspect_reservation(
                    &new.scope,
                    &new.operation,
                    &new.request_hash,
                    new.grant_ref.as_deref(),
                )
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "inspect_reservation found no row".to_owned())
        },
    },
    Entry {
        name: "reserve",
        operation_kind: "workspace.file.write",
        call: |store, new, _| store.reserve(new).map_err(|error| error.to_string()),
    },
    Entry {
        name: "record_refusal",
        operation_kind: "workspace.create",
        call: |store, new, _| {
            store
                .record_refusal(
                    new,
                    TERMINAL_AT,
                    422,
                    &detail("second.refusal", &new.operation),
                )
                .map_err(|error| error.to_string())
        },
    },
    Entry {
        name: "reserve_workspace_create",
        operation_kind: "workspace.create",
        call: |store, new, resource| {
            store
                .reserve_workspace_create(new, resource, &workspace(resource), None)
                .map_err(|error| error.to_string())
        },
    },
    Entry {
        name: "reserve_workspace_adopt",
        operation_kind: "workspace.adopt",
        call: |store, new, resource| match store
            .reserve_workspace_adopt(
                new,
                resource,
                WorkspaceRootHolder::Free,
                &workspace(resource),
                TERMINAL_AT,
            )
            .map_err(|error| error.to_string())?
        {
            WorkspaceAdoptReservation::Answered(reservation) => Ok(reservation),
            other @ WorkspaceAdoptReservation::Adopted(_) => {
                Err(not_from_the_row("reserve_workspace_adopt", &other))
            }
        },
    },
    Entry {
        name: "reserve_workspace_destroy",
        operation_kind: "workspace.destroy",
        call: |store, new, _| match store
            .reserve_workspace_destroy(new, None)
            .map_err(|error| error.to_string())?
        {
            WorkspaceDestroyReservation::Existing(reservation) => Ok(reservation),
            other => Err(not_from_the_row("reserve_workspace_destroy", &other)),
        },
    },
    Entry {
        name: "reserve_exec_start",
        operation_kind: "exec.start",
        call: |store, new, resource| {
            store
                .reserve_exec_start(
                    new,
                    &exec(resource, "ws_review", ExecState::Accepted),
                    None,
                    None,
                )
                .map_err(|error| error.to_string())
        },
    },
    Entry {
        name: "retire_exec",
        operation_kind: "exec.retire",
        call: |store, new, resource| match store
            .retire_exec(new, resource, observed_at())
            .map_err(|error| error.to_string())?
        {
            ExecRetireReservation::Existing(reservation) => Ok(reservation),
            other => Err(not_from_the_row("retire_exec", &other)),
        },
    },
    Entry {
        name: "reserve_pipe_session_start",
        operation_kind: "session.start",
        call: |store, new, resource| {
            let exec_id = format!("exec_{resource}");
            let (_, lease) = lease_at(SUBJECT, &exec_id, 1_000);
            store
                .reserve_pipe_session_start(
                    new,
                    &pipe_session(resource, &exec_id, "ws_review", &lease),
                    &exec(&exec_id, "ws_review", ExecState::Accepted),
                    &lease,
                    None,
                )
                .map_err(|error| error.to_string())
        },
    },
    Entry {
        name: "retire_pipe_session",
        operation_kind: "session.retire",
        call: |store, new, resource| match store
            .retire_pipe_session(new, resource, observed_at())
            .map_err(|error| error.to_string())?
        {
            SessionRetireReservation::Existing(reservation) => Ok(reservation),
            other => Err(not_from_the_row("retire_pipe_session", &other)),
        },
    },
    Entry {
        name: "reserve_host_process_start",
        operation_kind: HOST_PROCESS_START_KIND,
        call: |store, new, resource| match store
            .reserve_host_process_start(new, &host_process(resource), &HostProcessLimits::default())
            .map_err(|error| error.to_string())?
        {
            HostProcessStartReservation::Existing(reservation) => Ok(reservation),
            other => Err(not_from_the_row("reserve_host_process_start", &other)),
        },
    },
    Entry {
        name: "reserve_host_process_stop",
        operation_kind: HOST_PROCESS_STOP_KIND,
        call: |store, new, resource| match store
            .reserve_host_process_stop(new, resource, HostProcessEndedBy::Stop)
            .map_err(|error| error.to_string())?
        {
            HostProcessStopReservation::Existing(reservation) => Ok(reservation),
            other => Err(not_from_the_row("reserve_host_process_stop", &other)),
        },
    },
];

fn operation_id(entry: usize) -> String {
    format!("01JREVIEWREPLAYGRANT{entry:04}")
}

fn resource_id(entry: usize) -> String {
    format!("res_review_{entry}")
}

fn request(entry: usize, same_input: bool, grant: Option<&str>) -> NewOperation {
    let hash = if same_input { "1" } else { "2" }.repeat(64);
    NewOperation {
        grant_ref: grant.map(ToOwned::to_owned),
        platform_principal: grant.map(|_| "platform:principal-review".to_owned()),
        ..operation_named(
            SUBJECT,
            &operation_id(entry),
            ENTRIES[entry].operation_kind,
            &resource_id(entry),
            &hash,
        )
    }
}

/// Writes the first request's row in `row`'s state and returns the outcome it stored, if any.
fn write_row(store: &Store, first: &NewOperation, row: Row) -> Option<StoredAnswer> {
    if matches!(row, Row::Refused) {
        let refusal = detail("first.refusal", &first.operation);
        let answer = StoredAnswer {
            status: 422,
            outcome: OperationOutcome::Error {
                error: refusal.clone(),
            },
        };
        assert_eq!(
            store
                .record_refusal(first, TERMINAL_AT, 422, &refusal)
                .expect("first request's refusal"),
            Reservation::Replay(answer.clone())
        );
        return Some(answer);
    }
    assert_eq!(
        store.reserve(first).expect("first request"),
        Reservation::Accepted
    );
    match row {
        Row::Accepted | Row::Refused => None,
        Row::Unknown => {
            store
                .mark_dispatch_unknown(
                    &first.scope,
                    &first.operation,
                    TERMINAL_AT,
                    "workspace",
                    first.resource.as_deref().expect("resource"),
                )
                .expect("first request's unknown outcome");
            None
        }
        Row::TerminalError => {
            let error = detail("first.outcome", &first.operation);
            store
                .complete_error(
                    &first.scope,
                    &first.operation,
                    TERMINAL_AT,
                    409,
                    None,
                    &error,
                )
                .expect("first request's error");
            Some(StoredAnswer {
                status: 409,
                outcome: OperationOutcome::Error { error },
            })
        }
        Row::TerminalSuccess => {
            let result = json!({ "first": "result" });
            store
                .complete_success(
                    &first.scope,
                    &first.operation,
                    TERMINAL_AT,
                    201,
                    None,
                    &result,
                )
                .expect("first request's success");
            Some(StoredAnswer {
                status: 201,
                outcome: OperationOutcome::Success { result },
            })
        }
    }
}

/// What ADR 0017 owes a request that finds `record`, in the ADR's own order.
fn owed(
    record: &OperationRecord,
    stored: Option<&StoredAnswer>,
    request: &NewOperation,
) -> Reservation {
    match (record.grant_ref.as_deref(), request.grant_ref.as_deref()) {
        // "If the row records a grant, a different verified grant remains
        // `delegated-context.grant-conflict`", and the addendum: "The grant is compared before
        // the request hash".
        (Some(recorded), Some(verified)) if recorded != verified => {
            return Reservation::GrantConflict;
        }
        // "the same grant replays"; "A valid context may replay an operation originally recorded
        // without attribution"; the addendum: "a valid request without one replays a row that
        // records a grant".
        (Some(_), Some(_)) | (None, _) | (_, None) => {}
    }
    if record.request_hash != request.request_hash {
        return Reservation::Conflict;
    }
    match stored {
        Some(answer) => Reservation::Replay(answer.clone()),
        None => Reservation::Pending(Box::new(record.clone())),
    }
}

fn table_names(store: &Store) -> Vec<String> {
    let connection = store.connection.lock();
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .expect("table names");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("table names")
        .collect::<Result<_, _>>()
        .expect("table names")
}

/// The rows this store's connection has inserted, updated or deleted since it opened, counted by
/// the database itself and never reset by a rollback. A call that writes nothing leaves it
/// unchanged.
fn row_changes(store: &Store) -> u64 {
    store.connection.lock().total_changes()
}

/// Every row of every table, cell by cell.
fn contents(store: &Store, tables: &[String]) -> Vec<String> {
    let connection = store.connection.lock();
    let mut cells = Vec::new();
    for table in tables {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{table}\""))
            .expect("table contents");
        let columns = statement.column_count();
        let mut rows = statement.query([]).expect("table contents");
        while let Some(row) = rows.next().expect("table row") {
            let line = (0..columns)
                .map(|column| format!("{:?}", row.get_ref(column).expect("cell")))
                .collect::<Vec<_>>()
                .join("|");
            cells.push(format!("{table}: {line}"));
        }
    }
    cells
}

/// Runs every request of the table against one store holding one row per entry point, recorded
/// under `row_grant` in state `row`, and returns each broken answer and each write.
fn broken_answers(row_grant: Option<&str>, row: Row) -> (usize, Vec<String>) {
    let store = Store::open(":memory:").expect("open store");
    let stored: Vec<Option<StoredAnswer>> = (0..ENTRIES.len())
        .map(|entry| write_row(&store, &request(entry, true, row_grant), row))
        .collect();
    let tables = table_names(&store);
    assert!(
        tables.iter().any(|table| table == "operations"),
        "the store's tables were not read: {tables:?}"
    );
    let written = contents(&store, &tables);
    let mut asked = 0;
    let mut failures = Vec::new();
    for (entry, stored) in stored.iter().enumerate() {
        let first = request(entry, true, row_grant);
        let record = store
            .operation(&first.scope, &first.operation)
            .expect("row lookup")
            .expect("the first request's row");
        assert_eq!(
            record.grant_ref.as_deref(),
            row_grant,
            "the row records the first request's grant as it was given"
        );
        for request_grant in GRANTS {
            for same_input in [true, false] {
                let second = request(entry, same_input, *request_grant);
                let expected = owed(&record, stored.as_ref(), &second);
                let before = row_changes(&store);
                let answered = (ENTRIES[entry].call)(&store, &second, &resource_id(entry));
                let after = row_changes(&store);
                asked += 1;
                let case = format!(
                    "{} row={row:?} recorded={row_grant:?} verified={request_grant:?} \
                     same_input={same_input}",
                    ENTRIES[entry].name
                );
                if answered.as_ref() != Ok(&expected) {
                    failures.push(format!("{case}: answered {answered:?}, owed {expected:?}"));
                }
                if before != after {
                    failures.push(format!(
                        "{case}: the store was written, {} row change(s)",
                        after - before
                    ));
                }
            }
        }
    }
    if contents(&store, &tables) != written {
        failures.push(format!(
            "row={row:?} recorded={row_grant:?}: the store's contents differ after the requests"
        ));
    }
    (asked, failures)
}

#[test]
fn every_entry_point_answers_every_grant_pair_as_adr_0017_states_and_writes_nothing() {
    let mut asked = 0;
    let mut failures = Vec::new();
    for row_grant in GRANTS {
        for row in ROWS {
            let (count, broken) = broken_answers(*row_grant, *row);
            asked += count;
            failures.extend(broken);
        }
    }
    // A table that ran no request would pass: 6 recorded grants, 5 row states, 12 entry points,
    // 6 verified grants, 2 inputs.
    assert_eq!(asked, 6 * 5 * 12 * 6 * 2, "requests asked");
    assert_eq!(ENTRIES.len(), 12, "entry points");
    assert!(
        failures.is_empty(),
        "{} of {asked} answers broke ADR 0017; the first 20:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The empty string is a recorded grant, not the absence of one: a row that records it is not the
/// "row recorded without a grant" that replays to any verified grant.
#[test]
fn a_row_recording_the_empty_grant_is_not_a_row_without_a_grant() {
    let store = Store::open(":memory:").expect("open store");
    let first = request(1, true, Some(""));
    let stored = write_row(&store, &first, Row::TerminalError).expect("a stored outcome");
    let row = store
        .operation(&first.scope, &first.operation)
        .expect("row lookup")
        .expect("the first request's row");
    assert_eq!(row.grant_ref.as_deref(), Some(""));

    assert_eq!(
        store
            .reserve(&request(1, true, Some("grant:b")))
            .expect("reserve"),
        Reservation::GrantConflict
    );
    assert_eq!(
        store.reserve(&request(1, true, Some(""))).expect("reserve"),
        Reservation::Replay(stored)
    );
}

/// Every entry point that calls `existing_reservation` is in the table, and so is the one read
/// outside a transaction. Counted in the sources, so an entry point added later without a row in
/// the table fails here.
#[test]
fn the_table_names_every_store_read_that_answers_a_request_from_a_row() {
    let sources = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = 0;
    let mut calls = 0;
    for entry in std::fs::read_dir(&sources).expect("store sources") {
        let path = entry.expect("store source entry").path();
        let is_source = path.extension().is_some_and(|extension| extension == "rs")
            && path.file_name().is_some_and(|name| name != "tests.rs");
        if !is_source {
            continue;
        }
        files += 1;
        calls += std::fs::read_to_string(&path)
            .expect("store source")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| line.contains("existing_reservation(&transaction, new)"))
            .count();
    }
    assert!(files >= 10, "only {files} store source files were read");
    // `inspect_reservation` reads through `answer_to` without `existing_reservation`.
    assert_eq!(calls + 1, ENTRIES.len());
}
