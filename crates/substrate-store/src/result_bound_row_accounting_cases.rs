//! ADR 0053 decision 8, what the operation ledger names and charges: the resource an accepted
//! operation names is bounded in the bytes the row accounting charges (`operation_row_bytes`), at
//! acceptance and at its terminal update; a refused row names what the caller named and reserves
//! no headroom; a rebuilt accounting charges a row that is not terminal as the store charged it at
//! run time; and the operation id is at most `MAX_OPERATION_ID_CHARS` characters of the wire's
//! operation-id alphabet.

use substrate_wire::{
    ErrorClass, ErrorDetail, MAX_OPERATION_ID_CHARS, MAX_RESOURCE_ID_CHARS, OperationOutcome,
    OperationState,
};

use super::{lease_at, operation_named, scope, workspace};
use crate::{ExecRetireReservation, Reservation, Scope, Store, StoreConfig, StoreError};

const SUBJECT: &str = "local:1000";
const TERMINAL_AT: &str = "2026-11-24T12:34:56.123456789+00:00";

fn least_headroom_config() -> StoreConfig {
    StoreConfig {
        operation_terminal_headroom_bytes: StoreConfig::minimum_terminal_headroom_bytes(),
        ..StoreConfig::default()
    }
}

fn widest_operation(tag: &str) -> String {
    format!("{tag}{}", "0".repeat(MAX_OPERATION_ID_CHARS - tag.len()))
}

/// `MAX_RESOURCE_ID_CHARS` characters of two bytes each: twice the bound in bytes.
fn two_byte_resource() -> String {
    let resource = "é".repeat(MAX_RESOURCE_ID_CHARS);
    assert_eq!(resource.len(), 2 * MAX_RESOURCE_ID_CHARS);
    resource
}

/// A path id one byte over the bound and more: what a caller may send, never what the daemon
/// mints.
fn path_id_over_the_bound(prefix: &str) -> String {
    format!("{prefix}{}", "p".repeat(MAX_RESOURCE_ID_CHARS))
}

fn usage_bytes(store: &Store) -> i64 {
    store
        .connection
        .lock()
        .query_row(
            "SELECT byte_count FROM operation_ledger_usage
             WHERE deployment = 'dep_test' AND subject = 'local:1000'",
            [],
            |row| row.get(0),
        )
        .expect("usage")
}

fn charged_bytes(store: &Store, scope: &Scope, operation: &str) -> i64 {
    store
        .connection
        .lock()
        .query_row(
            "SELECT charged_bytes FROM operations
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
            rusqlite::params![scope.deployment, scope.subject, operation],
            |row| row.get(0),
        )
        .expect("charged bytes")
}

fn too_long(error: &StoreError, bytes: usize) -> bool {
    matches!(
        error,
        StoreError::OperationResourceTooLong { bytes: found, max }
            if *found == bytes && *max == MAX_RESOURCE_ID_CHARS
    )
}

/// The store accepts a resource of at most `MAX_RESOURCE_ID_CHARS` bytes: 32 characters of two
/// bytes each are 64 bytes, refused by name, and no row is written.
#[test]
fn a_resource_over_its_bound_in_bytes_is_refused_at_acceptance_and_writes_no_row() {
    let store = Store::open_with_config(":memory:", least_headroom_config()).expect("opens");
    let resource = two_byte_resource();
    let refused = operation_named(
        SUBJECT,
        &widest_operation("BYTESACCEPT"),
        "workspace.file.write",
        &resource,
        &"0".repeat(64),
    );
    let answer = store.reserve(&refused);
    assert!(
        matches!(&answer, Err(error) if too_long(error, resource.len())),
        "{answer:?}"
    );
    assert!(
        store
            .operation(&refused.scope, &refused.operation)
            .expect("read")
            .is_none()
    );
}

/// A terminal update that names a resource over `MAX_RESOURCE_ID_CHARS` bytes is refused by name
/// and rolls back: the row stays accepted without a resource and no event is written. Here 32
/// characters of two bytes each, with a result over the reserve at the least headroom.
#[test]
fn a_terminal_update_naming_a_resource_over_its_bound_in_bytes_is_refused_and_the_row_stays_accepted()
 {
    let store = Store::open_with_config(":memory:", least_headroom_config()).expect("opens");
    let operation = widest_operation("BYTESCOMPLETE");
    let mut accepted = operation_named(
        SUBJECT,
        &operation,
        "workspace.file.write",
        "ws_unused",
        &"1".repeat(64),
    );
    accepted.resource = None;
    assert_eq!(
        store.reserve(&accepted).expect("accept"),
        Reservation::Accepted
    );
    let resource = two_byte_resource();
    let mark = store.stream_position(&accepted.scope).expect("position").2;
    let usage = usage_bytes(&store);
    let reserve = usize::try_from(store.success_result_json_bytes()).expect("usize");
    let completed = store.complete_success(
        &accepted.scope,
        &operation,
        TERMINAL_AT,
        200,
        Some(&resource),
        &"x".repeat(reserve - 1),
    );
    assert!(
        matches!(&completed, Err(error) if too_long(error, resource.len())),
        "{completed:?}"
    );
    let record = store
        .operation(&accepted.scope, &operation)
        .expect("operation")
        .expect("ledger row");
    assert_eq!(record.state, OperationState::Accepted);
    assert_eq!(record.resource, None);
    assert_eq!(
        store.stream_position(&accepted.scope).expect("position").2,
        mark
    );
    assert_eq!(usage_bytes(&store), usage);
}

/// The refusal of a resource over the bound at a terminal update is a rule of the bound, not of
/// the headroom: at the default headroom, which holds the row, a recorded success, an error and
/// a dispatch absence that name one are each refused, and each row stays accepted.
#[test]
fn a_terminal_update_naming_a_resource_over_its_bound_is_refused_whatever_the_headroom() {
    let store = Store::open(":memory:").expect("open store");
    let resource = format!("ws_{}", "w".repeat(MAX_RESOURCE_ID_CHARS));
    let error = ErrorDetail {
        class: ErrorClass::Refused,
        code: "workspace.not-ready".to_owned(),
        message: "A test outcome.".to_owned(),
        retriable: false,
        address: None,
        operation: None,
    };
    for (index, path) in ["success", "error", "dispatch absence"].iter().enumerate() {
        let operation = widest_operation(&format!("HEADROOM{index}"));
        let mut accepted = operation_named(
            SUBJECT,
            &operation,
            "workspace.create",
            "ws_unused",
            &format!("{index:0>64}"),
        );
        accepted.resource = None;
        assert_eq!(
            store.reserve(&accepted).expect("accept"),
            Reservation::Accepted
        );
        let completed = match *path {
            "success" => store.complete_success(
                &accepted.scope,
                &operation,
                TERMINAL_AT,
                201,
                Some(&resource),
                &"x",
            ),
            "error" => store.complete_error(
                &accepted.scope,
                &operation,
                TERMINAL_AT,
                409,
                Some(&resource),
                &error,
            ),
            _ => store.complete_dispatch_absence(
                &accepted.scope,
                &operation,
                TERMINAL_AT,
                409,
                "workspace",
                &resource,
                &error,
            ),
        };
        assert!(
            matches!(&completed, Err(found) if too_long(found, resource.len())),
            "{path}: {completed:?}"
        );
        let record = store
            .operation(&accepted.scope, &operation)
            .expect("operation")
            .expect("ledger row");
        assert_eq!(record.state, OperationState::Accepted, "{path}");
        assert_eq!(record.resource, None, "{path}");
    }
}

/// A terminal update refused for its resource, with a lease and a workspace to write, rolls
/// back whole: the row stays accepted, and no event, usage, workspace or lease is written.
#[test]
fn a_refused_terminal_update_writes_no_event_usage_workspace_or_lease() {
    let store = Store::open_with_config(":memory:", least_headroom_config()).expect("opens");
    let scope = scope(SUBJECT);
    let id = format!("ws_{}", "w".repeat(MAX_RESOURCE_ID_CHARS));
    let (mut create, mut lease) = lease_at(SUBJECT, &id, 60_000);
    // The widest operation id, so the unrecorded outcome is the widest the headroom counts.
    create.operation = widest_operation("ROLLBACK");
    lease.authorizing_operation.clone_from(&create.operation);
    create.resource = None;
    assert_eq!(
        store.reserve(&create).expect("accept"),
        Reservation::Accepted
    );
    let mut resource = workspace(&id);
    resource.lease = Some(lease.observation());
    for label in 0..4 {
        resource.labels.insert(
            format!("label-{label}"),
            "l".repeat(substrate_wire::MAX_LABEL_VALUE_CHARS),
        );
    }
    assert!(!store.records_success_result(&resource));
    let mark = store.stream_position(&scope).expect("position").2;
    let usage = usage_bytes(&store);
    let completed = store.complete_workspace_leased(
        &scope,
        &create.operation,
        TERMINAL_AT,
        201,
        "rollback-root",
        &resource,
        Some(&lease),
    );
    assert!(
        matches!(&completed, Err(error) if too_long(error, id.len())),
        "{completed:?}"
    );
    let record = store
        .operation(&scope, &create.operation)
        .expect("operation")
        .expect("row");
    assert_eq!(record.state, OperationState::Accepted);
    assert_eq!(record.resource, None);
    assert_eq!(store.stream_position(&scope).expect("position").2, mark);
    assert_eq!(usage_bytes(&store), usage);
    assert!(store.workspace(&scope, &id).expect("read").is_none());
    let leases: i64 = store
        .connection
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM leases WHERE resource_id = ?1",
            [&id],
            |row| row.get(0),
        )
        .expect("leases");
    assert_eq!(leases, 0);
}

/// A row that is `unknown` is charged as it was accepted, with the 8 bytes of `accepted`, which
/// `terminal` has too: at run time `mark_dispatch_unknown` leaves its charge, a reopen of a store
/// that holds the `operation-accounting-v1` record changes nothing, and a rebuild of the
/// accounting (a store without that record) charges the same. At the least headroom an
/// over-reserve completion naming a resource of `MAX_RESOURCE_ID_CHARS` bytes then ends the row
/// with `operation.result-unrecorded`.
#[test]
fn an_unknown_row_is_charged_as_accepted_when_its_accounting_is_rebuilt_and_ends_unrecorded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("state.db");
    let config = least_headroom_config();
    let operation = widest_operation("UNKNOWNREBUILT");
    let mut accepted = operation_named(
        SUBJECT,
        &operation,
        "workspace.file.write",
        "ws_unknown",
        &"2".repeat(64),
    );
    accepted.resource = None;
    let at_run_time = {
        let store = Store::open_with_config(&path, config).expect("opens");
        assert_eq!(
            store.reserve(&accepted).expect("accept"),
            Reservation::Accepted
        );
        let when_accepted = charged_bytes(&store, &accepted.scope, &operation);
        store
            .mark_dispatch_unknown(
                &accepted.scope,
                &operation,
                "2026-11-24T12:00:00Z",
                "workspace",
                "ws_unknown",
            )
            .expect("unknown");
        let when_unknown = charged_bytes(&store, &accepted.scope, &operation);
        assert_eq!(when_unknown, when_accepted);
        when_unknown
    };
    {
        let store = Store::open_with_config(&path, config).expect("reopens with the record");
        assert_eq!(
            charged_bytes(&store, &accepted.scope, &operation),
            at_run_time
        );
    }
    {
        let raw = rusqlite::Connection::open(&path).expect("raw");
        raw.execute(
            "DELETE FROM store_metadata WHERE key = 'operation-accounting-v1'",
            [],
        )
        .expect("a store without the accounting record");
    }
    let store = Store::open_with_config(&path, config).expect("reopens and rebuilds");
    assert_eq!(
        charged_bytes(&store, &accepted.scope, &operation),
        at_run_time
    );
    let resource = format!("ws_{}", "w".repeat(MAX_RESOURCE_ID_CHARS - 3));
    let reserve = usize::try_from(store.success_result_json_bytes()).expect("usize");
    let completed = store.complete_success(
        &accepted.scope,
        &operation,
        TERMINAL_AT,
        200,
        Some(&resource),
        &"x".repeat(reserve - 1),
    );
    let record = store
        .operation(&accepted.scope, &operation)
        .expect("operation")
        .expect("ledger row");
    assert!(
        completed.is_ok(),
        "{completed:?}, the row is {:?}",
        record.state
    );
    assert_eq!(record.state, OperationState::Terminal);
    assert_eq!(record.resource.as_deref(), Some(resource.as_str()));
    assert_eq!(
        record.outcome,
        Some(OperationOutcome::Error {
            error: crate::result_unrecorded(&operation),
        })
    );
}

/// A refused row is not held to the resource bound: a refusal of an exec retire that names a
/// path id over `MAX_RESOURCE_ID_CHARS` bytes is recorded with that id, is charged the row's own
/// bytes and reserves no terminal headroom, and the same request replays the same answer.
#[test]
fn a_refused_retire_names_the_callers_path_id_and_reserves_no_headroom() {
    let store = Store::open(":memory:").expect("open store");
    let id = path_id_over_the_bound("ex_");
    let retire = operation_named(
        SUBJECT,
        "01JREFUSEDEXECRETIRE01",
        "exec.retire",
        &id,
        &"3".repeat(64),
    );
    let at = "2026-11-24T12:00:00Z".parse().expect("time");
    let ExecRetireReservation::Refused(first) =
        store.retire_exec(&retire, &id, at).expect("retire")
    else {
        panic!("a retire of an exec nothing holds is refused");
    };
    assert_eq!(first.status, 404);
    let record = store
        .operation(&retire.scope, &retire.operation)
        .expect("operation")
        .expect("refused row");
    assert_eq!(record.state, OperationState::Refused);
    assert_eq!(record.resource.as_deref(), Some(id.as_str()));
    let (row_bytes, charged): (i64, i64) = store
        .connection
        .lock()
        .query_row(
            "SELECT row_bytes, charged_bytes FROM operations
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
            rusqlite::params![
                retire.scope.deployment,
                retire.scope.subject,
                retire.operation
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("charge");
    let measured = crate::operations::operation_row_bytes(
        &store.connection.lock(),
        &retire.scope,
        &retire.operation,
    )
    .expect("row bytes");
    assert_eq!(u64::try_from(row_bytes).expect("u64"), measured);
    assert_eq!(charged, row_bytes);
    assert_eq!(usage_bytes(&store), row_bytes);
    let ExecRetireReservation::Existing(Reservation::Replay(replayed)) =
        store.retire_exec(&retire, &id, at).expect("replay")
    else {
        panic!("the same request replays the refusal");
    };
    assert_eq!(replayed, first);
    assert_eq!(
        serde_json::to_vec(&replayed.outcome).expect("bytes"),
        serde_json::to_vec(&first.outcome).expect("bytes")
    );
}

/// The wire's operation-id alphabet is ASCII letters, digits, `_` and `-`, and
/// `validate_operation_id` admits 16 to 128 of them. The store's rule is the alphabet and the
/// maximum; the minimum is the wire's.
#[test]
fn the_operation_id_alphabet_and_lengths_are_the_wires() {
    for byte in 0..=u8::MAX {
        let admitted = byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-');
        assert_eq!(
            substrate_wire::is_operation_id_byte(byte),
            admitted,
            "{byte}"
        );
    }
    for (length, admitted) in [
        (0, false),
        (15, false),
        (16, true),
        (128, true),
        (129, false),
    ] {
        assert_eq!(
            substrate_wire::validate_operation_id(&"a".repeat(length)).is_ok(),
            admitted,
            "{length}"
        );
    }
}

/// Every terminal update of this crate settles its row through `finalize_operation_accounting`,
/// the one function that refuses a resource over the bound, in the same source file: each file
/// under `src/` that is not a test has as many calls to it as statements that set a row
/// `terminal`.
#[test]
fn every_terminal_update_settles_its_row_through_the_resource_check() {
    fn sources(directory: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read src") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    let mut files = Vec::new();
    sources(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut updates = 0;
    for file in files {
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name == "tests.rs" || name.ends_with("_tests.rs") || name.ends_with("_cases.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("source");
        let terminal = text.matches("SET state = 'terminal'").count();
        let settled = text.matches("finalize_operation_accounting(").count()
            - text.matches("fn finalize_operation_accounting(").count();
        assert_eq!(terminal, settled, "{}", file.display());
        updates += terminal;
    }
    assert!(updates >= 11, "{updates}");
}
