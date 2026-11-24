//! What the operation ledger records as a success result (ADR 0053 decisions 7 and 8).
//!
//! A success result is recorded when its compact JSON fits the bytes the ledger reserves for it,
//! [`StoreConfig::success_result_json_bytes`], and every member a driver, the configuration or
//! the identity sets is within the bound the wire crate states for it. Any other result is not
//! recorded and not shortened: its row ends terminal with the error
//! [`OPERATION_RESULT_UNRECORDED`], which names the resource, and the resource's own record is
//! stored whole beside it. Every completion of this crate that writes a success outcome takes
//! its outcome from [`terminal_success`], and the daemon answers with
//! [`Store::records_success_result`], so the row and the answer follow one rule.

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use substrate_wire::{
    ErrorClass, ErrorDetail, Event, MAX_APERTURE_DESTINATION_CHARS, MAX_APERTURE_LISTEN_CHARS,
    MAX_APERTURE_NAME_CHARS, MAX_APPLIED_CGROUP_CHARS, MAX_EXEC_REFUSAL_CODE_CHARS,
    MAX_EXEC_REFUSAL_MESSAGE_CHARS, MAX_EXEC_USAGE_CODE_CHARS, MAX_EXEC_USAGE_MESSAGE_CHARS,
    MAX_LEASE_ACTOR_CHARS, MAX_LEASE_PRINCIPAL_CHARS, MAX_OPERATION_ID_CHARS,
    MAX_PROCESS_EXIT_SIGNAL_CHARS, MAX_READ_ONLY_FILE_PATH_BYTES, OPERATION_RESULT_UNRECORDED,
    OperationOutcome,
};

use crate::events::{append_event, published_transition};
use crate::{
    OPERATION_RESPONSE_STATUS_BYTES, OPERATION_TERMINAL_AT_MAX_BYTES, Scope, Store, StoreConfig,
    StoreError,
};

/// The status a row ends with when its result is not recorded (ADR 0053 decision 8).
pub const RESULT_UNRECORDED_STATUS: u16 = 500;

/// The one sentence the error [`OPERATION_RESULT_UNRECORDED`] carries (ADR 0053 decision 8).
pub const RESULT_UNRECORDED_MESSAGE: &str = "The operation took effect, but its result exceeds what \
                                             the operation ledger can record. Read the resource.";

/// The error a row ends with when its effect happened and its result is not recorded: class
/// `failed`, not retriable, address `result` (ADR 0053 decision 8).
#[must_use]
pub fn result_unrecorded(operation: &str) -> ErrorDetail {
    ErrorDetail {
        class: ErrorClass::Failed,
        code: OPERATION_RESULT_UNRECORDED.to_owned(),
        message: RESULT_UNRECORDED_MESSAGE.to_owned(),
        retriable: false,
        address: Some("result".to_owned()),
        operation: Some(operation.to_owned()),
    }
}

/// The bytes of the widest outcome a row ends with when its result is not recorded: the error
/// [`result_unrecorded`] of an operation id of [`MAX_OPERATION_ID_CHARS`] characters, each one
/// byte of JSON (`validate_operation_id` admits ASCII letters, digits, `_` and `-`).
#[must_use]
pub fn widest_unrecorded_outcome_json_bytes() -> u64 {
    let outcome = OperationOutcome::Error {
        error: result_unrecorded(&"0".repeat(MAX_OPERATION_ID_CHARS)),
    };
    serde_json::to_value(&outcome).map_or(u64::MAX, |value| {
        u64::try_from(json_bytes(&value)).unwrap_or(u64::MAX)
    })
}

impl StoreConfig {
    /// The least [`StoreConfig::operation_terminal_headroom_bytes`] a store opens with: what the
    /// row accounting adds to a row that ends with the widest unrecorded outcome, its longest
    /// `terminal_at` ([`OPERATION_TERMINAL_AT_MAX_BYTES`]), the status charge and
    /// [`widest_unrecorded_outcome_json_bytes`]. Under it a completion over the reserve could
    /// not end its row, and the row would stay accepted (ADR 0053 decisions 1 and 8).
    #[must_use]
    pub fn minimum_terminal_headroom_bytes() -> u64 {
        OPERATION_TERMINAL_AT_MAX_BYTES
            .saturating_add(OPERATION_RESPONSE_STATUS_BYTES)
            .saturating_add(widest_unrecorded_outcome_json_bytes())
    }

    /// Whether a success result is recorded as it is: its compact JSON is at most
    /// [`StoreConfig::success_result_json_bytes`] bytes and every member another party sets is
    /// within its wire bound ([`members_within_bounds`]).
    #[must_use]
    pub fn records_success_result(&self, result: &Value) -> bool {
        u64::try_from(json_bytes(result))
            .is_ok_and(|bytes| bytes <= self.success_result_json_bytes())
            && members_within_bounds(result)
    }
}

impl Store {
    /// Whether this store records `result` as the success result of a completion, by
    /// [`StoreConfig::records_success_result`]. A completion of a result for which this is false
    /// ends its row with [`result_unrecorded`] at [`RESULT_UNRECORDED_STATUS`].
    #[must_use]
    pub fn records_success_result<T: Serialize + ?Sized>(&self, result: &T) -> bool {
        serde_json::to_value(result).is_ok_and(|value| self.config.records_success_result(&value))
    }
}

/// The outcome and status a completion writes for `result`: the success itself, or the error
/// [`result_unrecorded`] at [`RESULT_UNRECORDED_STATUS`] when the result is not recorded. Every
/// success completion of this crate writes what this returns.
pub(crate) fn terminal_success(
    config: StoreConfig,
    operation: &str,
    result: Value,
    status: u16,
) -> (OperationOutcome, u16) {
    if config.records_success_result(&result) {
        (OperationOutcome::Success { result }, status)
    } else {
        (
            OperationOutcome::Error {
                error: result_unrecorded(operation),
            },
            RESULT_UNRECORDED_STATUS,
        )
    }
}

/// One event the completion of a path writes when its result is recorded: the resource it
/// names, the resource's kind, its transition and what it carries.
pub(crate) struct CompletionEvent<'a> {
    pub(crate) resource: &'a str,
    pub(crate) resource_kind: &'a str,
    pub(crate) transition: &'a str,
    pub(crate) observation: Option<Value>,
}

/// Appends the events of a completion whose row ends with `outcome`, as [`terminal_success`]
/// returned it, or as the caller's error. `recorded` are the events the path writes when its
/// result is recorded, in order. A success writes them as they are. An error writes the
/// operation's `operation.failed`, which carries the operation's record and so the error, and
/// then each of `recorded` that is the resource's own event, exactly as given: the resource is
/// stored whole and its run goes on, so a reader that follows the resource's events reads the
/// same events whether the result was recorded or not (ADR 0053 decision 8). An event that
/// [`published_transition`] publishes as an operation event is the operation's, and
/// `operation.failed` stands in its place.
#[allow(clippy::too_many_arguments)] // One completion's events, inside its caller's transaction.
pub(crate) fn append_completion_events(
    connection: &Connection,
    retention: u64,
    scope: &Scope,
    operation: &str,
    terminal_at: &str,
    (actor, principal): (&str, Option<&str>),
    outcome: &OperationOutcome,
    recorded: Vec<CompletionEvent<'_>>,
) -> Result<Vec<Event>, StoreError> {
    let failed = matches!(outcome, OperationOutcome::Error { .. });
    let mut events = Vec::with_capacity(recorded.len() + 1);
    if failed {
        events.push(append_event(
            connection,
            retention,
            scope,
            operation,
            "operation",
            "operation.failed",
            terminal_at,
            actor,
            principal,
            operation,
            None,
        )?);
    }
    for event in recorded {
        if failed
            && published_transition(event.resource_kind, event.transition).starts_with("operation.")
        {
            continue;
        }
        events.push(append_event(
            connection,
            retention,
            scope,
            event.resource,
            event.resource_kind,
            event.transition,
            terminal_at,
            actor,
            principal,
            operation,
            event.observation,
        )?);
    }
    Ok(events)
}

/// The bytes of `value` as compact JSON, every escape counted.
fn json_bytes(value: &Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).map_or(usize::MAX, |()| count.0)
}

/// A string member at `key` of `object`, if it is one, is within `max` characters.
fn chars_within(object: &Value, key: &str, max: usize) -> bool {
    object
        .get(key)
        .and_then(Value::as_str)
        .is_none_or(|text| text.chars().count() <= max)
}

/// Whether every member of a result that a driver, the configuration or the identity sets is
/// within the bound the wire crate states for it (ADR 0053 decision 7, one line per row):
///
/// - `applied.cgroup`, [`MAX_APPLIED_CGROUP_CHARS`] characters;
/// - each applied aperture's `name`, `destination` and `listen`, [`MAX_APERTURE_NAME_CHARS`],
///   [`MAX_APERTURE_DESTINATION_CHARS`] and [`MAX_APERTURE_LISTEN_CHARS`];
/// - each applied writable root's `host_path`, [`MAX_READ_ONLY_FILE_PATH_BYTES`] bytes;
/// - a usage's `code` and `message`, [`MAX_EXEC_USAGE_CODE_CHARS`] and
///   [`MAX_EXEC_USAGE_MESSAGE_CHARS`];
/// - a refusal's `code` and `message`, [`MAX_EXEC_REFUSAL_CODE_CHARS`] and
///   [`MAX_EXEC_REFUSAL_MESSAGE_CHARS`];
/// - a process exit's `signal`, [`MAX_PROCESS_EXIT_SIGNAL_CHARS`];
/// - a lease's `actor` and `principal`, [`MAX_LEASE_ACTOR_CHARS`] and
///   [`MAX_LEASE_PRINCIPAL_CHARS`].
///
/// A member is read where every result type that has it holds it; a result without it passes.
#[must_use]
pub fn members_within_bounds(result: &Value) -> bool {
    let applied = result.get("applied").unwrap_or(&Value::Null);
    let apertures = match applied.get("network") {
        Some(Value::Array(apertures)) => apertures.iter().collect::<Vec<_>>(),
        Some(aperture @ Value::Object(_)) => vec![aperture],
        _ => Vec::new(),
    };
    let roots = applied
        .get("writable_roots")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let usage = result.get("usage").unwrap_or(&Value::Null);
    let refusal = result.get("refusal").unwrap_or(&Value::Null);
    let exit = result.get("exit").unwrap_or(&Value::Null);
    let lease = result.get("lease").unwrap_or(&Value::Null);
    chars_within(applied, "cgroup", MAX_APPLIED_CGROUP_CHARS)
        && apertures.iter().all(|aperture| {
            chars_within(aperture, "name", MAX_APERTURE_NAME_CHARS)
                && chars_within(aperture, "destination", MAX_APERTURE_DESTINATION_CHARS)
                && chars_within(aperture, "listen", MAX_APERTURE_LISTEN_CHARS)
        })
        && roots.iter().all(|root| {
            root.get("host_path")
                .and_then(Value::as_str)
                .is_none_or(|path| path.len() <= MAX_READ_ONLY_FILE_PATH_BYTES)
        })
        && chars_within(usage, "code", MAX_EXEC_USAGE_CODE_CHARS)
        && chars_within(usage, "message", MAX_EXEC_USAGE_MESSAGE_CHARS)
        && chars_within(refusal, "code", MAX_EXEC_REFUSAL_CODE_CHARS)
        && chars_within(refusal, "message", MAX_EXEC_REFUSAL_MESSAGE_CHARS)
        && chars_within(exit, "signal", MAX_PROCESS_EXIT_SIGNAL_CHARS)
        && chars_within(lease, "actor", MAX_LEASE_ACTOR_CHARS)
        && chars_within(lease, "principal", MAX_LEASE_PRINCIPAL_CHARS)
}
