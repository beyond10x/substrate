//! Security review of ADR 0053 on the host-process routes, at the reserve the daemon ships with
//! (`Store::open`, no test headroom): a start no record of which can be recorded is refused before
//! anything starts and keeps the operation id's idempotency, every other published refusal keeps
//! its code, and a start at the shipped reserve made of the costliest characters completes every
//! row it opens with its success result.

use axum::http::StatusCode;
use serde_json::{Value, json};
use substrate_store::{HOST_PROCESS_RENEW_KIND, HOST_PROCESS_START_KIND, HOST_PROCESS_STOP_KIND};
use substrate_wire::{ProcessOwner, ProcessRecord};

use super::{Harness, input, text, with, without};

/// The widest record a start of `input` can lead to, in bytes of compact JSON.
fn record_width(input: &Value) -> usize {
    let argv: Vec<String> = input["argv"]
        .as_array()
        .expect("argv")
        .iter()
        .map(|item| text(item).to_owned())
        .collect();
    let env_names: Vec<String> = input["env"]
        .as_object()
        .expect("env")
        .keys()
        .cloned()
        .collect();
    let owner = ProcessOwner {
        name: text(&input["owner"]["name"]).to_owned(),
        session: text(&input["owner"]["session"]).to_owned(),
    };
    ProcessRecord::widest_json_bytes(
        &argv,
        text(&input["cwd"]),
        &env_names,
        &owner,
        text(&input["reason"]),
    )
}

fn count(harness: &Harness, sql: &str) -> i64 {
    let connection =
        rusqlite::Connection::open(harness.directory.path().join("state.db")).expect("ledger");
    connection
        .query_row(sql, [], |row| row.get(0))
        .expect("count")
}

/// A start of `cwd` whose second `argv` item is `chars` characters that cost six bytes each in
/// JSON. Each item is within `MAX_PROCESS_ARGV_BYTES` as the start counts it, in bytes.
fn costly_start(cwd: &std::path::Path, chars: usize) -> Value {
    let argv = json!(["/bin/true", "\u{1}".repeat(chars)]);
    with(input(&["/bin/true"], cwd, "s-review", "wide"), "argv", argv)
}

/// Rule 1 at the shipped reserve: a start whose widest record is over it is refused
/// `operation.result-limit` before anything starts, with no record and no lease; the same
/// operation id with another request is the idempotency conflict, not a start; and the refusal
/// does not stay with the subject: a fitting start under a new operation id runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_over_the_shipped_reserve_is_refused_and_its_id_stays_bound_to_that_request() {
    let (harness, fake) = Harness::faked();
    let cwd = harness.workdir("wide");
    let wide = costly_start(&cwd, 100_000);
    let reserve = usize::try_from(harness.store.success_result_json_bytes()).expect("usize");
    assert!(
        record_width(&wide) > reserve,
        "the start is over the reserve"
    );

    let refused = harness.start("op_review_wide_start01", wide).await;
    assert_eq!(refused.0, StatusCode::TOO_MANY_REQUESTS, "{}", refused.1);
    assert_eq!(refused.1["error"]["code"], "operation.result-limit");
    assert!(fake.started.lock().is_empty(), "nothing was started");
    assert_eq!(count(&harness, "SELECT count(*) FROM host_processes"), 0);

    let fitting = input(&["/bin/true"], &cwd, "s-review", "fits");
    let conflict = harness
        .start("op_review_wide_start01", fitting.clone())
        .await;
    assert_eq!(conflict.0, StatusCode::CONFLICT, "{}", conflict.1);
    assert_eq!(conflict.1["error"]["code"], "operation.request-conflict");
    assert!(fake.started.lock().is_empty(), "a conflict starts nothing");

    let started = harness.started("op_review_fit_start001", fitting).await;
    assert_eq!(fake.started.lock().as_slice(), [started]);
    let rows = harness.operations(HOST_PROCESS_START_KIND);
    assert_eq!(rows.len(), 2, "{rows:?}");
}

/// Rule 3 at the shipped reserve: a start no record of which could be recorded, which another
/// published code refuses, keeps that code, and nothing starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_another_code_refuses_keeps_that_code_though_no_record_could_fit() {
    let (harness, fake) = Harness::faked();
    let cwd = harness.workdir("order");
    let wide = costly_start(&cwd, 100_000);
    let reserve = usize::try_from(harness.store.success_result_json_bytes()).expect("usize");
    assert!(record_width(&wide) > reserve);
    let cases = [
        (
            with(wide.clone(), "profile", json!("vm")),
            "process.profile-unknown",
        ),
        (
            with(wide.clone(), "cwd", json!("relative/dir")),
            "process.cwd-invalid",
        ),
        (
            with(wide.clone(), "env", json!({ "1BAD": "v" })),
            "process.env-invalid",
        ),
        (
            with(
                wide.clone(),
                "owner",
                json!({ "session": " ", "name": "x" }),
            ),
            "process.owner-invalid",
        ),
        (without(wide.clone(), "reason"), "process.reason-required"),
        (
            with(wide.clone(), "lease", json!({ "ttl_ms": 1 })),
            "process.lease-invalid",
        ),
        (
            with(wide.clone(), "expires_at", json!("not an instant")),
            "process.expires-at-invalid",
        ),
    ];
    for (index, (start, code)) in cases.into_iter().enumerate() {
        let operation = format!("op_review_order_{index:06}");
        let refused = harness.start(&operation, start).await;
        assert_eq!(refused.1["error"]["code"], code, "{code}: {}", refused.1);
        assert_ne!(refused.0, StatusCode::TOO_MANY_REQUESTS, "{code}");
    }
    assert!(fake.started.lock().is_empty());

    let portable = Harness::portable();
    let wide = costly_start(&portable.workdir("unserved"), 100_000);
    let refused = portable.start("op_review_unserved_01", wide).await;
    assert_eq!(
        refused.1["error"]["code"], "process.host-unserved",
        "{}",
        refused.1
    );
}

/// Rules 2 and 5 at the shipped reserve: a start whose widest record is exactly the reserve, its
/// second `argv` item made of characters that cost six bytes each in JSON, is started; its
/// renewal, its stop and the end the supervisor clock collects each end their row terminal with
/// the success result, never `operation.result-unrecorded`.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_of_costly_text_at_the_shipped_reserve_completes_every_row_it_opens() {
    let (harness, fake) = Harness::faked();
    let cwd = harness.workdir("costly");
    let reserve = usize::try_from(harness.store.success_result_json_bytes()).expect("usize");
    let base = record_width(&costly_start(&cwd, 0));
    let gap = reserve - base;
    let mut item = "\u{1}".repeat(gap / 6);
    item.push_str(&"\"".repeat(gap % 6 / 2));
    item.push_str(&"a".repeat(gap % 2));
    let start = with(costly_start(&cwd, 0), "argv", json!(["/bin/true", item]));
    assert_eq!(record_width(&start), reserve, "the start is at the reserve");

    let id = harness.started("op_review_costly_start", start).await;
    let renewed = harness
        .renew("op_review_costly_renew", &id, json!({ "ttl_ms": 120_000 }))
        .await;
    assert_eq!(renewed.0, StatusCode::OK, "{}", renewed.1["error"]);
    let stopped = harness.stop("op_review_costly_stop01", &id).await;
    assert_eq!(stopped.0, StatusCode::OK, "{}", stopped.1["error"]);
    let ended = harness.ended(&id).await;
    assert_eq!(ended["state"], "ended");
    assert_eq!(fake.stops().len(), 1);
    for kind in [
        HOST_PROCESS_START_KIND,
        HOST_PROCESS_RENEW_KIND,
        HOST_PROCESS_STOP_KIND,
    ] {
        let unrecorded = count(
            &harness,
            &format!(
                "SELECT count(*) FROM operations WHERE operation_kind = '{kind}'
                 AND (state <> 'terminal' OR outcome_json NOT LIKE '{{\"kind\":\"success\"%')"
            ),
        );
        assert_eq!(
            unrecorded, 0,
            "{kind}: every row ends with its success result"
        );
    }
}
