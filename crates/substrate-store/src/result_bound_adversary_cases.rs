//! Adversary cases on ADR 0053 decisions 1 and 8 at the store: a completion never fails for
//! size, and a row that was accepted always ends terminal.

use substrate_wire::{OperationOutcome, OperationState};

use super::operation_named;
use crate::{Reservation, Store, StoreConfig, StoreError};

/// The bytes a terminal row adds around its outcome: the longest `terminal_at` and the status
/// charge.
const AROUND_AN_OUTCOME: u64 = 35 + 8;
/// The widest `operation.result-unrecorded` outcome, for the longest operation id (ADR 0053
/// decision 8).
const WIDEST_UNRECORDED_OUTCOME: u64 = 379;

/// ADR 0053 decisions 1 and 8: "A completion never fails for size, and a row that was accepted
/// always ends terminal." The rule holds for every configuration the store opens with: a store
/// either refuses to open with a headroom under which the error `operation.result-unrecorded`
/// cannot itself be recorded, or ends a completion over its reserve terminal with that error.
/// The headroom here is one byte short of the widest such error.
#[test]
fn every_headroom_the_store_opens_with_ends_an_over_reserve_completion_terminal() {
    let headroom = AROUND_AN_OUTCOME + WIDEST_UNRECORDED_OUTCOME - 1;
    let config = StoreConfig {
        operation_max_row_bytes: 8 * 1024,
        operation_terminal_headroom_bytes: headroom,
        ..StoreConfig::default()
    };
    let store = match Store::open_with_config(":memory:", config) {
        // Refusing the configuration, by name, is one form of the rule.
        Err(StoreError::OperationTerminalHeadroomBelowUnrecorded { .. }) => return,
        Err(other) => panic!("the store neither opened nor refused the configuration: {other}"),
        Ok(store) => store,
    };
    let operation = "0".repeat(substrate_wire::MAX_OPERATION_ID_CHARS);
    let accepted = operation_named(
        "local:1000",
        &operation,
        "workspace.lease.renew",
        "ws_headroom",
        &"e".repeat(64),
    );
    assert_eq!(
        store.reserve(&accepted).expect("accept"),
        Reservation::Accepted
    );
    let reserve = usize::try_from(store.success_result_json_bytes()).expect("usize");
    // A JSON string one byte over the reserve: two quotes around plain characters.
    let result = "x".repeat(reserve - 1);
    let terminal_at = "2026-11-24T12:34:56.123456789+00:00";
    let completed = store.complete_success(
        &accepted.scope,
        &accepted.operation,
        terminal_at,
        200,
        Some("ws_headroom"),
        &result,
    );
    let record = store
        .operation(&accepted.scope, &accepted.operation)
        .expect("operation")
        .expect("ledger row");
    assert!(
        completed.is_ok(),
        "a completion over the reserve failed for size ({completed:?}) and left the row {:?}",
        record.state
    );
    assert_eq!(record.state, OperationState::Terminal);
    assert_eq!(
        record.outcome,
        Some(OperationOutcome::Error {
            error: crate::result_unrecorded(&operation),
        })
    );
}
