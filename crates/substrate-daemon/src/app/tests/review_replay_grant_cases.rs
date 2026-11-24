//! Review cases for ADR 0017 and its addendum of 2026-11-23 at the daemon's routes, written from
//! the ADR's text and independent of `replay_compares_the_grant_where_the_row_is_found`.
//!
//! Three places find a row another request wrote: the lookup at the head of a request, the
//! transaction that records a refusal decided after the request was decoded, and the transaction
//! that would have accepted the request. Each is driven once here through `workspace.create`,
//! with the other request's row written at one fixed step and no second task.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signer as _, SigningKey};
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use substrate_store::{LeaseClock, NewOperation, Reservation, Scope, Store};
use substrate_wire::{
    DELEGATED_CONTEXT_AUDIENCE, DELEGATED_CONTEXT_TYPE, ErrorClass, ErrorDetail, OperationRecord,
    OperationState, canonical_request_hash_v2,
};
use tower::ServiceExt as _;

use crate::app::{App, Authority, Identity, PrivateHostPaths, SystemAuthority};
use crate::delegation::{DelegatedContextPolicy, TrustedKey};

const DEPLOYMENT: &str = "dep_review_replay_grant";
const SUBJECT: &str = "local:1000";
const ISSUER: &str = "https://issuer.invalid";
const KID: &str = "review-test-key-1";
const GRANT_A: &str = "grant:a";
const GRANT_B: &str = "grant:b";

type Step = Box<dyn FnOnce() + Send>;

/// The system authority with one step taken while a `workspace.create` is between the lookup at
/// its head and its accepting transaction: the route mints the workspace id there.
struct BetweenLookupAndReservation {
    step: Mutex<Option<Step>>,
}

impl Authority for BetweenLookupAndReservation {
    fn now(&self) -> DateTime<Utc> {
        SystemAuthority.now()
    }

    fn request_id(&self) -> String {
        SystemAuthority.request_id()
    }

    fn workspace_id(&self) -> String {
        if let Some(step) = self.step.lock().take() {
            step();
        }
        SystemAuthority.workspace_id()
    }

    fn exec_id(&self) -> String {
        SystemAuthority.exec_id()
    }

    fn session_id(&self) -> String {
        SystemAuthority.session_id()
    }

    fn snapshot_id(&self) -> String {
        SystemAuthority.snapshot_id()
    }

    fn lease_clock(&self) -> Result<LeaseClock, String> {
        SystemAuthority.lease_clock()
    }
}

/// A signing key for these cases only: its seed is the SHA-256 of this sentence, so no key
/// material is committed.
fn signing_key() -> SigningKey {
    let seed: [u8; 32] =
        Sha256::digest(b"substrate review-case delegated-context signing seed").into();
    SigningKey::from_bytes(&seed)
}

/// A context that verifies for this deployment and subject and names `grant`.
fn context(grant: &str) -> String {
    let instant = Utc::now().timestamp();
    let header = json!({ "alg": "EdDSA", "kid": KID, "typ": DELEGATED_CONTEXT_TYPE });
    let claims = json!({
        "act": { "sub": "svc:actor" },
        "aud": DELEGATED_CONTEXT_AUDIENCE,
        "bound_deployment": DEPLOYMENT,
        "bound_subject": SUBJECT,
        "exp": instant + 60,
        "grant_ref": grant,
        "grant_revision": "rev_1",
        "iat": instant - 10,
        "iss": ISSUER,
        "jti": "jti_1",
        "nbf": instant - 10,
        "sub": "platform:principal-1",
        "tenant": "tenant_review",
    });
    let signing_input = format!(
        "{}.{}",
        BASE64URL.encode(serde_json::to_vec(&header).expect("header")),
        BASE64URL.encode(serde_json::to_vec(&claims).expect("claims"))
    );
    let signature = signing_key().sign(signing_input.as_bytes());
    format!("{signing_input}.{}", BASE64URL.encode(signature.to_bytes()))
}

struct Daemon {
    directory: tempfile::TempDir,
    store: Arc<Store>,
    authority: Arc<BetweenLookupAndReservation>,
    app: Arc<App>,
}

fn daemon(required: bool) -> Daemon {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open(":memory:").expect("state store"));
    let driver = super::minimum_host_driver(directory.path());
    let policy = DelegatedContextPolicy::new(
        vec![TrustedKey {
            kid: KID.to_owned(),
            issuer: ISSUER.to_owned(),
            verifying_key: signing_key().verifying_key(),
        }],
        required,
    )
    .expect("one key is a policy");
    let authority = Arc::new(BetweenLookupAndReservation {
        step: Mutex::new(None),
    });
    let app = App::with_delegated_context(
        Arc::clone(&store),
        driver,
        DEPLOYMENT,
        Arc::clone(&authority) as Arc<dyn Authority>,
        policy,
        PrivateHostPaths::of_no_daemon_configuration(),
    )
    .expect("application");
    Daemon {
        directory,
        store,
        authority,
        app,
    }
}

fn scope() -> Scope {
    Scope {
        deployment: DEPLOYMENT.to_owned(),
        subject: SUBJECT.to_owned(),
    }
}

fn valid_input() -> Value {
    json!({ "source": "empty", "labels": {} })
}

fn first_outcome() -> ErrorDetail {
    ErrorDetail {
        class: ErrorClass::Conflict,
        code: "first.outcome".to_owned(),
        message: "The first request's stored outcome.".to_owned(),
        retriable: false,
        address: None,
        operation: None,
    }
}

/// The other request: the same subject, operation id, route and input, under `grant`.
fn first_request(operation: &str, input: &Value, grant: Option<&str>) -> NewOperation {
    NewOperation {
        scope: scope(),
        operation: operation.to_owned(),
        operation_kind: "workspace.create".to_owned(),
        request_hash: canonical_request_hash_v2("POST", "/v1/workspaces", input, None)
            .expect("request hash"),
        accepted_at: Utc::now().to_rfc3339(),
        capability_snapshot: None,
        actor: "review-first-request".to_owned(),
        principal: None,
        grant_ref: grant.map(ToOwned::to_owned),
        platform_principal: grant.map(|_| "platform:principal-0".to_owned()),
        resource: None,
    }
}

/// Writes the other request's row: accepted, and terminal with its outcome when `outcome`.
fn write_first(store: &Store, first: &NewOperation, outcome: bool) {
    assert_eq!(
        store.reserve(first).expect("first request"),
        Reservation::Accepted
    );
    if outcome {
        store
            .complete_error(
                &first.scope,
                &first.operation,
                &Utc::now().to_rfc3339(),
                409,
                None,
                &first_outcome(),
            )
            .expect("first request's outcome");
    }
}

async fn create_workspace(app: Arc<App>, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-request-id", "req_review_replay_grant")
        .body(Body::from(serde_json::to_vec(&body).expect("request JSON")))
        .expect("request");
    let identity = Identity {
        subject: SUBJECT.to_owned(),
        actor: "review-second-request".to_owned(),
        principal: None,
    };
    let response = crate::app::router(app)
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

fn envelope(operation: &str, input: &Value, context: Option<&str>) -> Value {
    let mut body = json!({ "op": operation, "input": input });
    if let Some(context) = context {
        body["delegated_context"] = json!(context);
    }
    body
}

fn row(store: &Store, operation: &str) -> OperationRecord {
    store
        .operation(&scope(), operation)
        .expect("row lookup")
        .expect("the first request's row")
}

/// How far the subject's event stream has been written.
fn events_written(store: &Store) -> u64 {
    store
        .events(&scope(), None, 1)
        .expect("event page")
        .expect("event cursor")
        .through_seq
}

fn host_entries(daemon: &Daemon) -> usize {
    std::fs::read_dir(daemon.directory.path().join("workspaces")).map_or(0, Iterator::count)
}

/// Sends one `workspace.create` under `second_context` whose lookup finds no row and whose
/// accepting transaction finds the row another request wrote under `first_grant` in between.
/// Returns the answer after checking that the row, the event stream and the host are as the
/// other request left them.
async fn answered_by_the_accepting_transaction(
    operation: &str,
    first_grant: Option<&'static str>,
    first_has_outcome: bool,
    second_context: Option<&str>,
) -> (StatusCode, Value) {
    let daemon = daemon(false);
    let input = valid_input();
    let first = first_request(operation, &input, first_grant);
    let stepped = Arc::new(AtomicBool::new(false));
    {
        let store = Arc::clone(&daemon.store);
        let first = first.clone();
        let stepped = Arc::clone(&stepped);
        *daemon.authority.step.lock() = Some(Box::new(move || {
            assert!(
                store
                    .operation(&first.scope, &first.operation)
                    .expect("row lookup")
                    .is_none(),
                "the lookup at the head of the request found no row"
            );
            write_first(&store, &first, first_has_outcome);
            stepped.store(true, Ordering::SeqCst);
        }));
    }
    let host_before = host_entries(&daemon);

    let answer = create_workspace(
        Arc::clone(&daemon.app),
        "/v1/workspaces",
        envelope(operation, &input, second_context),
    )
    .await;

    assert!(
        stepped.load(Ordering::SeqCst),
        "the request reached the step between its lookup and its reservation: {answer:?}"
    );
    let after = row(&daemon.store, operation);
    assert_eq!(after.grant_ref.as_deref(), first_grant);
    assert_eq!(after.platform_principal, first.platform_principal);
    assert_eq!(after.actor, "review-first-request");
    assert_eq!(after.request_hash, first.request_hash);
    assert_eq!(
        after.state,
        if first_has_outcome {
            OperationState::Terminal
        } else {
            OperationState::Accepted
        }
    );
    assert_eq!(
        host_entries(&daemon),
        host_before,
        "the driver was not dispatched for a request answered from a row"
    );
    answer
}

#[tokio::test(flavor = "multi_thread")]
async fn a_route_s_accepting_transaction_refuses_another_grant_and_dispatches_nothing() {
    let (status, response) = answered_by_the_accepting_transaction(
        "01JREVIEWGRANTACCEPT001",
        Some(GRANT_A),
        true,
        Some(&context(GRANT_B)),
    )
    .await;
    assert_eq!(
        response["error"]["code"],
        "delegated-context.grant-conflict"
    );
    assert_eq!(status, StatusCode::CONFLICT);
}

/// The row of a request still in flight holds no outcome. A request under another grant is told
/// the grant conflict, not that the outcome of that row is unknown.
#[tokio::test(flavor = "multi_thread")]
async fn a_route_s_accepting_transaction_refuses_another_grant_for_a_row_still_in_flight() {
    let (status, response) = answered_by_the_accepting_transaction(
        "01JREVIEWGRANTACCEPT002",
        Some(GRANT_A),
        false,
        Some(&context(GRANT_B)),
    )
    .await;
    assert_eq!(
        response["error"]["code"],
        "delegated-context.grant-conflict"
    );
    assert_eq!(status, StatusCode::CONFLICT);
}

/// The control for the two cases above: the same step with the row's own grant replays the
/// stored outcome, so the conflict above is decided by the grant and by nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn a_route_s_accepting_transaction_replays_the_stored_outcome_to_the_recorded_grant() {
    let (status, response) = answered_by_the_accepting_transaction(
        "01JREVIEWGRANTACCEPT003",
        Some(GRANT_A),
        true,
        Some(&context(GRANT_A)),
    )
    .await;
    assert_eq!(response["error"]["code"], "first.outcome");
    assert_eq!(status, StatusCode::CONFLICT);
}

/// ADR 0017: "A valid context may replay an operation originally recorded without attribution;
/// that historical row remains unattributed and is never rewritten."
#[tokio::test(flavor = "multi_thread")]
async fn a_route_s_accepting_transaction_replays_an_unattributed_row_and_leaves_it_so() {
    let (status, response) = answered_by_the_accepting_transaction(
        "01JREVIEWGRANTACCEPT004",
        None,
        true,
        Some(&context(GRANT_B)),
    )
    .await;
    assert_eq!(response["error"]["code"], "first.outcome");
    assert_eq!(status, StatusCode::CONFLICT);
}

/// The addendum: "where the deployment does not require delegated context, a valid request
/// without one replays a row that records a grant."
#[tokio::test(flavor = "multi_thread")]
async fn where_context_is_optional_a_request_without_one_replays_a_row_that_records_a_grant() {
    let (status, response) =
        answered_by_the_accepting_transaction("01JREVIEWGRANTACCEPT005", Some(GRANT_A), true, None)
            .await;
    assert_eq!(response["error"]["code"], "first.outcome");
    assert_eq!(status, StatusCode::CONFLICT);
}

/// A refusal a route decides after the request was decoded (here a lease below the minimum) is
/// recorded through `refuse_before_dispatch_response`. A row under another grant that appears
/// before it is recorded answers with the grant conflict, never with its stored outcome.
#[tokio::test(flavor = "multi_thread")]
async fn a_route_s_own_refusal_recorded_after_a_row_under_another_grant_is_the_grant_conflict() {
    let daemon = daemon(false);
    let operation = "01JREVIEWGRANTREFUSE001";
    let input = json!({ "source": "empty", "labels": {}, "lease_ttl_ms": 1 });

    // Without another request the route refuses this input itself and records the refusal under
    // the grant the request verified.
    let alone = "01JREVIEWGRANTREFUSE000";
    let (status, response) = create_workspace(
        Arc::clone(&daemon.app),
        "/v1/workspaces",
        envelope(alone, &input, Some(&context(GRANT_B))),
    )
    .await;
    assert_eq!(response["error"]["code"], "request.schema-invalid");
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let recorded = row(&daemon.store, alone);
    assert_eq!(recorded.state, OperationState::Refused);
    assert_eq!(recorded.grant_ref.as_deref(), Some(GRANT_B));

    let stepped = Arc::new(AtomicBool::new(false));
    {
        let store = Arc::clone(&daemon.store);
        let stepped = Arc::clone(&stepped);
        daemon
            .app
            .refusal_before_record
            .lock()
            .replace(Arc::new(move |new: &NewOperation| {
                let first = NewOperation {
                    actor: "review-first-request".to_owned(),
                    grant_ref: Some(GRANT_A.to_owned()),
                    platform_principal: Some("platform:principal-0".to_owned()),
                    ..new.clone()
                };
                write_first(&store, &first, true);
                stepped.store(true, Ordering::SeqCst);
            }));
    }
    let (status, response) = create_workspace(
        Arc::clone(&daemon.app),
        "/v1/workspaces",
        envelope(operation, &input, Some(&context(GRANT_B))),
    )
    .await;

    assert!(stepped.load(Ordering::SeqCst), "the refusal was recorded");
    assert_eq!(
        response["error"]["code"],
        "delegated-context.grant-conflict"
    );
    assert_eq!(status, StatusCode::CONFLICT);
    let after = row(&daemon.store, operation);
    assert_eq!(after.grant_ref.as_deref(), Some(GRANT_A));
    assert_eq!(after.actor, "review-first-request");
    assert_eq!(after.state, OperationState::Terminal);
}

/// One request that finds the row of `GRANT_A`: its name, its address, its body, and the status
/// and code ADR 0017 owes it where context is required.
type HeadCase = (&'static str, &'static str, Value, StatusCode, &'static str);

fn requests_that_find_the_row_at_their_head(operation: &str, input: &Value) -> Vec<HeadCase> {
    let mut unknown_member = envelope(operation, input, Some(&context(GRANT_A)));
    unknown_member["unexpected"] = json!(true);
    vec![
        (
            "no context",
            "/v1/workspaces",
            envelope(operation, input, None),
            StatusCode::UNPROCESSABLE_ENTITY,
            "delegated-context.absent",
        ),
        (
            "a context that is not one",
            "/v1/workspaces",
            envelope(operation, input, Some("not-a-context")),
            StatusCode::UNPROCESSABLE_ENTITY,
            "delegated-context.malformed",
        ),
        (
            "the recorded grant and a query",
            "/v1/workspaces?unexpected=1",
            envelope(operation, input, Some(&context(GRANT_A))),
            StatusCode::UNPROCESSABLE_ENTITY,
            "request.schema-invalid",
        ),
        (
            "the recorded grant and an unknown member",
            "/v1/workspaces",
            unknown_member,
            StatusCode::UNPROCESSABLE_ENTITY,
            "request.schema-invalid",
        ),
        (
            "another grant",
            "/v1/workspaces",
            envelope(operation, input, Some(&context(GRANT_B))),
            StatusCode::CONFLICT,
            "delegated-context.grant-conflict",
        ),
        (
            "another grant and another input",
            "/v1/workspaces",
            envelope(
                operation,
                &json!({ "source": "empty", "labels": { "other": "input" } }),
                Some(&context(GRANT_B)),
            ),
            StatusCode::CONFLICT,
            "delegated-context.grant-conflict",
        ),
        (
            "the recorded grant and another input",
            "/v1/workspaces",
            envelope(
                operation,
                &json!({ "source": "empty", "labels": { "other": "input" } }),
                Some(&context(GRANT_A)),
            ),
            StatusCode::CONFLICT,
            "operation.request-conflict",
        ),
        (
            "the recorded grant",
            "/v1/workspaces",
            envelope(operation, input, Some(&context(GRANT_A))),
            StatusCode::CONFLICT,
            "first.outcome",
        ),
    ]
}

/// ADR 0017's decision at the head of a request, in a deployment that requires context: every
/// request that finds the row is answered by its own refusal, by the grant conflict or by the
/// replay, and none of them changes the row or appends an event.
#[tokio::test(flavor = "multi_thread")]
async fn where_context_is_required_a_row_found_at_the_head_answers_only_its_own_grant() {
    let daemon = daemon(true);
    let operation = "01JREVIEWGRANTHEAD00001";
    let input = valid_input();
    write_first(
        &daemon.store,
        &first_request(operation, &input, Some(GRANT_A)),
        true,
    );
    let before = row(&daemon.store, operation);
    let events_before = events_written(&daemon.store);

    let cases = requests_that_find_the_row_at_their_head(operation, &input);
    assert_eq!(cases.len(), 8, "requests sent");
    let mut failures = Vec::new();
    for (name, uri, body, owed_status, owed_code) in cases {
        let (status, response) = create_workspace(Arc::clone(&daemon.app), uri, body).await;
        if status != owed_status || response["error"]["code"] != owed_code {
            failures.push(format!(
                "{name}: answered {status} {}, owed {owed_status} {owed_code}",
                response["error"]["code"]
            ));
        }
        if row(&daemon.store, operation) != before {
            failures.push(format!("{name}: the row was changed"));
        }
        if events_written(&daemon.store) != events_before {
            failures.push(format!("{name}: an event was appended"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The reach of the comparison, stated by design 09 § 4: "reads never require a context". The
/// grant is compared where a mutation is answered from a row. The operation read and the event
/// page hand the same row, its grant and its outcome to any request of the subject, with no
/// context, also where the deployment requires one for a mutation.
#[tokio::test(flavor = "multi_thread")]
async fn the_read_routes_hand_the_row_to_the_subject_without_a_context() {
    let daemon = daemon(true);
    let operation = "01JREVIEWGRANTREAD00001";
    write_first(
        &daemon.store,
        &first_request(operation, &valid_input(), Some(GRANT_A)),
        true,
    );
    for uri in [
        format!("/v1/ops/{operation}"),
        "/v1/events?limit=10".to_owned(),
    ] {
        let request = Request::builder()
            .method("GET")
            .uri(&uri)
            .header("x-request-id", "req_review_replay_grant")
            .body(Body::empty())
            .expect("request");
        let identity = Identity {
            subject: SUBJECT.to_owned(),
            actor: "review-reader".to_owned(),
            principal: None,
        };
        let response = crate::app::router(Arc::clone(&daemon.app))
            .layer(Extension(identity))
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let body = to_bytes(response.into_body(), 2_097_152)
            .await
            .expect("response bytes");
        let text = String::from_utf8_lossy(&body);
        assert_eq!(status, StatusCode::OK, "{uri}: {text}");
        assert!(
            text.contains("first.outcome"),
            "{uri} returns the stored outcome: {text}"
        );
        assert!(
            text.contains(GRANT_A),
            "{uri} returns the recorded grant: {text}"
        );
    }
}
