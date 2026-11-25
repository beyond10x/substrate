//! The daemon's checks of the executable lists before dispatch (design 37 § 6 rows 1 to 4, § 7),
//! on an exec start and on the exec a session start embeds, and the start of a daemon whose
//! driver withholds the lists' facts (design 37 § 8).
//!
//! Every case runs against a driver that states a snapshot and is never dispatched to
//! (`snapshot_app`): an answer a case reads was given before dispatch. Each refusal is compared
//! with the wire crate's row or function, never with a literal of this file.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use substrate_store::Scope;
use substrate_wire::{
    CapabilitySnapshot, CgroupLimitFacts, ConfinementRequest, EXEC_DENIED_PATHS_ADDRESS,
    EXEC_LIST_REFUSALS, EXEC_SUBPATHS_ADDRESS, ExecListRefusal, ExecStartInput,
    MAX_EXEC_DENIED_PATHS, MAX_EXEC_LIST_ENTRY_BYTES, MAX_EXEC_SUBPATHS, NamespaceFacts,
    OperationState, WithheldAddress, WithheldCause, WithheldFact, WithheldFactName,
    canonical_request_hash_v2, exec_lists_fact_refusal, withheld_facts,
};
use tower::ServiceExt as _;

use super::snapshot_driver::snapshot_app;
use crate::app::{App, Identity, router};

const DEPLOYMENT: &str = "dep_exec_lists";
const SUBJECT: &str = "local:1000";

/// The two starts that carry the lists: the exec start, and the session start whose `exec` the
/// daemon checks with the exec start's own function.
#[derive(Debug, Clone, Copy)]
enum Start {
    Exec,
    Session,
}

const STARTS: [Start; 2] = [Start::Exec, Start::Session];

impl Start {
    fn uri(self) -> &'static str {
        match self {
            Self::Exec => "/v1/execs",
            Self::Session => "/v1/sessions",
        }
    }

    /// The start's input, with `lists` added to the exec's `sandbox` as written.
    fn input(self, snapshot: &str, lists: &Value) -> Value {
        let mut sandbox = json!({
            "require": true,
            "profile": "workspace",
            "network": "none",
            "capability_snapshot": snapshot
        });
        for (member, value) in lists.as_object().expect("lists are an object") {
            sandbox[member] = value.clone();
        }
        let exec = json!({
            "workspace": "ws_execlists",
            "argv": ["/usr/bin/true"],
            "env": {"allow": [], "set": {}},
            "limits": {
                "timeout_ms": 10_000,
                "output_bytes": 1_048_576,
                "processes": 8,
                "memory_bytes": 67_108_864,
                "cpu_millis": 1_000
            },
            "sandbox": sandbox,
            "wait": false,
            "lease_ttl_ms": 60_000
        });
        match self {
            Self::Exec => exec,
            Self::Session => json!({
                "exec": exec,
                "input_limit_bytes": 1_048_576,
                "frame_limit_bytes": 65_536,
                "queued_frames": 16
            }),
        }
    }
}

/// A snapshot that admits an exec start and a session start, proving the allow-list's fact when
/// `subpaths` is set and the deny-list's when `denied` is.
fn serving(subpaths: bool, denied: bool) -> impl FnOnce(&mut CapabilitySnapshot) {
    move |snapshot| {
        let facts = &mut snapshot.facts;
        facts.exec_namespaces = Some(NamespaceFacts {
            user: true,
            mount: true,
            pid: true,
            ipc: true,
            uts: true,
            network: true,
        });
        facts.exec_cgroup_limits = Some(CgroupLimitFacts {
            processes: true,
            memory: true,
            cpu: true,
        });
        facts.exec_cgroup_kill = Some(true);
        facts.exec_no_egress = Some(true);
        facts.leases_explicit = Some(true);
        facts.exec_output_limit_bytes = Some(1_048_576);
        facts.confinement_exec_subpaths = subpaths.then_some(true);
        facts.confinement_exec_denied_paths = denied.then_some(true);
    }
}

fn operation_id(start: Start) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!(
        "op-exec-lists-{start:?}-{:04}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

async fn post(app: &Arc<App>, start: Start, operation: &str, input: &Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(start.uri())
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({"op": operation, "input": input})).expect("request JSON"),
        ))
        .expect("request");
    let identity = Identity {
        subject: SUBJECT.to_owned(),
        actor: "exec-lists-test".to_owned(),
        principal: None,
    };
    let response = router(Arc::clone(app))
        .layer(Extension(identity))
        .oneshot(request)
        .await
        .expect("router response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 2_097_152)
        .await
        .expect("response bytes");
    (
        status,
        serde_json::from_slice(&body).expect("response JSON"),
    )
}

fn recorded(app: &App, operation: &str) -> OperationState {
    app.store
        .operation(
            &Scope {
                deployment: DEPLOYMENT.to_owned(),
                subject: SUBJECT.to_owned(),
            },
            operation,
        )
        .expect("row lookup")
        .expect("a recorded row")
        .state
}

fn snapshot_of(app: &App) -> String {
    app.driver.machine().snapshot
}

fn row(code: &str) -> ExecListRefusal {
    EXEC_LIST_REFUSALS
        .into_iter()
        .find(|row| row.code == code)
        .expect("a row of the wire crate's table")
}

/// Starts `lists` on `start` and returns the answer, holding that it was recorded as refused.
async fn refused(app: &Arc<App>, start: Start, lists: &Value) -> (StatusCode, Value) {
    let operation = operation_id(start);
    let input = start.input(&snapshot_of(app), lists);
    let answer = post(app, start, &operation, &input).await;
    assert_eq!(
        recorded(app, &operation),
        OperationState::Refused,
        "{start:?} {lists}: {answer:?}"
    );
    answer
}

/// The answer is `row` at `address` with `message`, whole.
fn assert_row(
    answer: &(StatusCode, Value),
    row: ExecListRefusal,
    address: &str,
    message: &str,
    context: &str,
) {
    let (status, body) = answer;
    let error = &body["error"];
    assert_eq!(error["code"], row.code, "{context}: {body}");
    assert_eq!(status.as_u16(), row.status, "{context}: {body}");
    assert!(row.addresses.contains(&address), "{context}: {address}");
    assert_eq!(error["address"], address, "{context}: {body}");
    assert_eq!(error["message"], message, "{context}: {body}");
    assert_eq!(error["retriable"], false, "{context}: {body}");
}

fn assert_schema_invalid(answer: &(StatusCode, Value), address: &str, context: &str) {
    let (status, body) = answer;
    assert_eq!(
        *status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{context}: {body}"
    );
    assert_eq!(body["error"]["code"], "request.schema-invalid", "{context}");
    assert_eq!(body["error"]["address"], address, "{context}: {body}");
}

fn entries(count: usize, bytes: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("/{index:0>width$}", width = bytes - 1))
        .collect()
}

/// Row 1 of design 37 § 6: the lexical rules are `request.schema-invalid` (422) at the member's
/// address, before row 2 and before any fact is read; `null` is the decoder's, at `input`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_lexical_rule_of_a_list_is_the_schema_refusal_on_both_starts() {
    let (_directory, app) = snapshot_app(DEPLOYMENT, serving(false, false));
    let over_bound = entries(MAX_EXEC_SUBPATHS + 1, 8);
    let over_bound_denied = entries(MAX_EXEC_DENIED_PATHS + 1, 8);
    let too_long = "/".repeat(MAX_EXEC_LIST_ENTRY_BYTES + 1);
    let cases = [
        (json!({"exec_subpaths": over_bound}), EXEC_SUBPATHS_ADDRESS),
        (
            json!({"exec_subpaths": ["/usr/bin", ""]}),
            EXEC_SUBPATHS_ADDRESS,
        ),
        (json!({"exec_subpaths": [too_long]}), EXEC_SUBPATHS_ADDRESS),
        (
            json!({"exec_subpaths": ["/usr/bin", "/usr/bin"]}),
            EXEC_SUBPATHS_ADDRESS,
        ),
        (
            json!({"exec_subpaths": ["/usr/bin"], "exec_denied_paths": over_bound_denied}),
            EXEC_DENIED_PATHS_ADDRESS,
        ),
        (
            json!({"exec_subpaths": ["/usr/bin"], "exec_denied_paths": [""]}),
            EXEC_DENIED_PATHS_ADDRESS,
        ),
        (
            json!({"exec_subpaths": ["/usr/bin"], "exec_denied_paths": ["a", "a"]}),
            EXEC_DENIED_PATHS_ADDRESS,
        ),
        // Before row 2: an empty allow-list beside a deny-list that breaks a rule.
        (
            json!({"exec_subpaths": [], "exec_denied_paths": ["a", "a"]}),
            EXEC_DENIED_PATHS_ADDRESS,
        ),
        // The decoder's answer.
        (json!({"exec_subpaths": null}), "input"),
        (json!({"exec_denied_paths": null}), "input"),
    ];
    for start in STARTS {
        for (lists, address) in &cases {
            let answer = refused(&app, start, lists).await;
            assert_schema_invalid(&answer, address, &format!("{start:?} {lists}"));
        }
    }
}

/// Row 2: a present and empty allow-list, after `exec.capability-stale` and before the fact is
/// read, on a snapshot that proves neither fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_present_and_empty_allow_list_is_row_two_on_both_starts() {
    let (_directory, app) = snapshot_app(DEPLOYMENT, serving(false, false));
    let empty = row(substrate_wire::EXEC_SANDBOX_EXEC_SUBPATHS_EMPTY);
    for start in STARTS {
        for lists in [
            json!({"exec_subpaths": []}),
            json!({"exec_subpaths": [], "exec_denied_paths": ["tools/unsafe"]}),
        ] {
            let answer = refused(&app, start, &lists).await;
            assert_row(
                &answer,
                empty,
                EXEC_SUBPATHS_ADDRESS,
                empty.message.expect("row 2 has a fixed message"),
                &format!("{start:?} {lists}"),
            );
            assert_eq!(answer.1["error"]["class"], "refused");
        }
        // The stale snapshot is answered first.
        let operation = operation_id(start);
        let stale = format!("sha256:{}", "0".repeat(64));
        let (status, body) = post(
            &app,
            start,
            &operation,
            &start.input(&stale, &json!({"exec_subpaths": []})),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "exec.capability-stale", "{body}");
    }
}

/// Row 3: a deny entry without an allow-list, before the fact is read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deny_list_without_an_allow_list_is_row_three_before_the_fact_is_read() {
    let (_directory, app) = snapshot_app(DEPLOYMENT, serving(false, false));
    let without = row(substrate_wire::CONFINEMENT_EXEC_DENIED_PATHS_WITHOUT_SUBPATHS);
    for start in STARTS {
        let answer = refused(&app, start, &json!({"exec_denied_paths": ["tools/unsafe"]})).await;
        assert_row(
            &answer,
            without,
            EXEC_DENIED_PATHS_ADDRESS,
            without.message.expect("row 3 has a fixed message"),
            &format!("{start:?}"),
        );
        assert_eq!(answer.1["error"]["class"], "refused");
    }
}

/// Row 4: a list whose fact the admitted snapshot does not prove is `confinement.fact-unserved`
/// (501) at its member, the allow-list's first, with the message `exec_lists_fact_refusal`
/// computes from the facts of that snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_list_whose_fact_is_not_proved_is_row_four_at_its_member() {
    let unserved = row(substrate_wire::CONFINEMENT_FACT_UNSERVED);
    for (subpaths, denied, lists, address) in [
        (
            false,
            false,
            json!({"exec_subpaths": ["/usr/bin"]}),
            EXEC_SUBPATHS_ADDRESS,
        ),
        (
            false,
            false,
            json!({"exec_subpaths": ["/usr/bin"], "exec_denied_paths": ["/usr/bin/sudo"]}),
            EXEC_SUBPATHS_ADDRESS,
        ),
        (
            false,
            true,
            json!({"exec_subpaths": ["/usr/bin"], "exec_denied_paths": ["/usr/bin/sudo"]}),
            EXEC_SUBPATHS_ADDRESS,
        ),
        (
            true,
            false,
            json!({"exec_subpaths": ["/usr/bin"], "exec_denied_paths": ["/usr/bin/sudo"]}),
            EXEC_DENIED_PATHS_ADDRESS,
        ),
    ] {
        let (_directory, app) = snapshot_app(DEPLOYMENT, serving(subpaths, denied));
        for start in STARTS {
            let context = format!("{start:?} {lists} subpaths={subpaths} denied={denied}");
            let sandbox: ConfinementRequest = serde_json::from_value(
                Start::Exec.input(&snapshot_of(&app), &lists)["sandbox"].clone(),
            )
            .expect("sandbox");
            let (expected_row, expected_address, message) =
                exec_lists_fact_refusal(&sandbox, &app.driver.machine().facts)
                    .expect("the wire crate refuses this start");
            assert_eq!(expected_row, unserved, "{context}");
            assert_eq!(expected_address, address, "{context}");
            let answer = refused(&app, start, &lists).await;
            assert_row(&answer, unserved, address, &message, &context);
            assert_eq!(answer.1["error"]["class"], "unserved");
        }
    }
}

/// A start whose lists' facts are proved passes the four checks: on this driver it next meets
/// the workspace, which does not exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_whose_lists_are_proved_passes_the_four_checks() {
    let (_directory, app) = snapshot_app(DEPLOYMENT, serving(true, true));
    for start in STARTS {
        let lists = json!({"exec_subpaths": ["/usr/bin"], "exec_denied_paths": ["/usr/bin/sudo"]});
        let operation = operation_id(start);
        let (status, body) = post(
            &app,
            start,
            &operation,
            &start.input(&snapshot_of(&app), &lists),
        )
        .await;
        let code = body["error"]["code"].as_str().unwrap_or_default();
        assert!(
            EXEC_LIST_REFUSALS.iter().all(|row| row.code != code)
                && code != "request.schema-invalid",
            "{start:?}: {status} {body}"
        );
    }
}

/// The request hash covers both members: a retried start with the same operation id and other
/// lists is `operation.request-conflict`, and the same start is the stored answer again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retried_start_with_other_lists_is_a_request_conflict() {
    let (_directory, app) = snapshot_app(DEPLOYMENT, serving(false, false));
    for start in STARTS {
        let operation = operation_id(start);
        let snapshot = snapshot_of(&app);
        let first = json!({"exec_subpaths": ["/usr/bin"], "exec_denied_paths": ["/usr/bin/sudo"]});
        let answered = post(&app, start, &operation, &start.input(&snapshot, &first)).await;
        assert_eq!(answered.0, StatusCode::NOT_IMPLEMENTED, "{:?}", answered.1);
        for other in [
            json!({"exec_subpaths": ["/usr/local/bin"], "exec_denied_paths": ["/usr/bin/sudo"]}),
            json!({"exec_subpaths": ["/usr/bin"], "exec_denied_paths": ["/usr/bin/su"]}),
            json!({"exec_subpaths": ["/usr/bin"]}),
            json!({}),
        ] {
            let (status, body) =
                post(&app, start, &operation, &start.input(&snapshot, &other)).await;
            assert_eq!(status, StatusCode::CONFLICT, "{start:?} {other}: {body}");
            assert_eq!(
                body["error"]["code"], "operation.request-conflict",
                "{start:?} {other}: {body}"
            );
        }
        let again = post(&app, start, &operation, &start.input(&snapshot, &first)).await;
        assert_eq!(again.0, answered.0, "{start:?}");
        assert_eq!(again.1["error"], answered.1["error"], "{start:?}");
    }
}

/// Design 37 § 3 (D21): the request hash covers the request as sent. An empty deny-list reads as
/// an absent one, and a retry that adds or drops the member is still another request:
/// `operation.request-conflict`, as for every other member a start may write or leave out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retry_that_adds_or_drops_an_empty_deny_list_is_a_request_conflict() {
    let (_directory, app) = snapshot_app(DEPLOYMENT, serving(false, false));
    for start in STARTS {
        let snapshot = snapshot_of(&app);
        let without = json!({"exec_subpaths": ["/usr/bin"]});
        let with_empty = json!({"exec_subpaths": ["/usr/bin"], "exec_denied_paths": []});
        for (first, retried) in [(&without, &with_empty), (&with_empty, &without)] {
            let operation = operation_id(start);
            let answered = post(&app, start, &operation, &start.input(&snapshot, first)).await;
            assert_eq!(answered.0, StatusCode::NOT_IMPLEMENTED, "{:?}", answered.1);
            let (status, body) =
                post(&app, start, &operation, &start.input(&snapshot, retried)).await;
            assert_eq!(status, StatusCode::CONFLICT, "{start:?} {retried}: {body}");
            assert_eq!(
                body["error"]["code"], "operation.request-conflict",
                "{start:?} {retried}: {body}"
            );
            let again = post(&app, start, &operation, &start.input(&snapshot, first)).await;
            assert_eq!(again.1["error"], answered.1["error"], "{start:?}");
        }
    }
}

/// A start without the lists has the request hash it had before they existed: the wire type
/// skips both members when they are absent, so a client that writes the type sends the bytes it
/// sent before, and a start that names one hashes otherwise.
#[test]
fn a_start_without_the_lists_hashes_as_it_did_before() {
    let snapshot = format!("sha256:{}", "a".repeat(64));
    let before = Start::Exec.input(&snapshot, &json!({}));
    let written: ExecStartInput = serde_json::from_value(before.clone()).expect("exec start");
    assert_eq!(written.sandbox.exec_subpaths, None);
    assert!(written.sandbox.exec_denied_paths.is_empty());
    let rewritten = serde_json::to_value(&written).expect("exec start JSON");
    assert_eq!(rewritten["sandbox"].get("exec_subpaths"), None);
    assert_eq!(rewritten["sandbox"].get("exec_denied_paths"), None);
    let hash = |input: &Value| {
        canonical_request_hash_v2("POST", "/v1/execs", input, None).expect("request hash")
    };
    assert_eq!(hash(&rewritten), hash(&before));
    let mut named = written;
    named.sandbox.exec_subpaths = Some(vec!["/usr/bin".to_owned()]);
    let with_allow = serde_json::to_value(&named).expect("exec start JSON");
    named.sandbox.exec_denied_paths = vec!["/usr/bin/sudo".to_owned()];
    let with_both = serde_json::to_value(&named).expect("exec start JSON");
    assert_ne!(hash(&with_allow), hash(&before));
    assert_ne!(hash(&with_both), hash(&with_allow));
}

/// ADR 0053 and design 37 § 7: both lists count toward the widest result as sent, so a start
/// with 64 entries of 4095 bytes in each is `operation.result-limit` (429) before dispatch, on
/// an exec start and on a session start, whose check counts its exec's record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_at_their_bounds_are_over_the_result_reserve_on_both_starts() {
    let (_directory, app) = snapshot_app(DEPLOYMENT, serving(true, true));
    let lists = json!({
        "exec_subpaths": entries(MAX_EXEC_SUBPATHS, MAX_EXEC_LIST_ENTRY_BYTES),
        "exec_denied_paths": entries(MAX_EXEC_DENIED_PATHS, MAX_EXEC_LIST_ENTRY_BYTES),
    });
    for start in STARTS {
        let (status, body) = refused(&app, start, &lists).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{start:?}: {body}");
        assert_eq!(
            body["error"]["code"],
            substrate_wire::OPERATION_RESULT_LIMIT,
            "{start:?}"
        );
        assert_eq!(body["error"]["address"], "result", "{start:?}");
    }
}

/// Design 37 § 7 and § 8: a daemon whose driver withholds both facts starts, and serves the
/// driver's snapshot without them and with the entries that say why.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_whose_driver_withholds_both_facts_starts_and_serves_them_absent() {
    let withheld = withheld_facts(WithheldFactName::ALL.into_iter().map(|fact| WithheldFact {
        fact,
        cause: WithheldCause::FileSystemUnlisted,
        address: WithheldAddress::WorkspaceRoot,
    }));
    let stated = withheld.clone();
    let (_directory, app) = snapshot_app(DEPLOYMENT, move |snapshot| {
        snapshot.facts.confinement_exec_subpaths = None;
        snapshot.facts.confinement_exec_denied_paths = None;
        snapshot.withheld = stated;
    });
    let request = Request::builder()
        .method("GET")
        .uri("/v1/machine")
        .body(Body::empty())
        .expect("request");
    let identity = Identity {
        subject: SUBJECT.to_owned(),
        actor: "exec-lists-test".to_owned(),
        principal: None,
    };
    let response = router(Arc::clone(&app))
        .layer(Extension(identity))
        .oneshot(request)
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 2_097_152)
            .await
            .expect("response bytes"),
    )
    .expect("response JSON");
    let facts = &body["result"]["facts"];
    for fact in WithheldFactName::ALL {
        assert_eq!(facts.get(fact.fact()), None, "{body}");
    }
    assert_eq!(
        body["result"]["withheld"],
        serde_json::to_value(&withheld).expect("withheld JSON"),
        "{body}"
    );
}
