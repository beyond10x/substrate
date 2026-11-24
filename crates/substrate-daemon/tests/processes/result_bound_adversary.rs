//! Adversary cases on ADR 0053 decision 2 at the shipped reserve: an operation id keeps its
//! idempotency answer whichever of two requests under it is the one no result can fit.
//!
//! The store here is the default one, so the reserve is the published bound. A start reaches past
//! it through what the request sets: an `argv` item of control characters is within
//! `MAX_PROCESS_ARGV_BYTES` and costs six bytes of JSON for each of its characters.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{Harness, input, with};

/// An `argv` within `MAX_PROCESS_ARGV_BYTES` whose record no ledger at the published bound can
/// record: 131,000 characters of six JSON bytes each.
fn too_wide(narrow: &Value) -> Value {
    with(
        narrow.clone(),
        "argv",
        json!(["/bin/true", "\u{1}".repeat(131_000)]),
    )
}

/// ADR 0053 decision 2 and rule 1 of the review: the same operation id replays the refusal, and
/// a different request under it is the idempotency conflict, never a second decision. Nothing
/// starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_different_start_under_an_operation_id_refused_for_the_result_bound_is_the_request_conflict()
 {
    let (harness, fake) = Harness::faked();
    assert_eq!(
        harness.store.success_result_json_bytes(),
        substrate_wire::OPERATION_RESULT_MAX_JSON_BYTES,
        "the shipped reserve"
    );
    let cwd = harness.workdir("adv-limit-then-other");
    let narrow = input(&["/bin/true"], &cwd, "s-adv", "first");
    let (status, refused) = harness
        .start("op_adv_limit_then_other", too_wide(&narrow))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    assert_eq!(refused["error"]["code"], "operation.result-limit");
    let (status, conflict) = harness.start("op_adv_limit_then_other", narrow).await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(conflict["error"]["code"], "operation.request-conflict");
    assert!(fake.started.lock().is_empty(), "nothing was started");
}

/// ADR 0053 decision 2 and rule 1 of the review, the other order: an operation id that already
/// completed a start answers a different, too-wide request under it with the idempotency
/// conflict, not `operation.result-limit`, and starts nothing more.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_no_result_can_fit_under_a_completed_operation_id_is_the_request_conflict() {
    let (harness, fake) = Harness::faked();
    let cwd = harness.workdir("adv-done-then-wide");
    let narrow = input(&["/bin/true"], &cwd, "s-adv", "second");
    let (status, started) = harness.start("op_adv_done_then_wide", narrow.clone()).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let (status, conflict) = harness
        .start("op_adv_done_then_wide", too_wide(&narrow))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(conflict["error"]["code"], "operation.request-conflict");
    assert_eq!(fake.started.lock().len(), 1, "one start only");
}
