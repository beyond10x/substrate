use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::Value;
use substrate_host::{HostConfig, HostDriver};
use tokio::sync::Semaphore;
use tower::ServiceExt as _;

use substrate_store::{CommitEffect, CommitEffectSink, Reservation, Scope, Store};

use super::events::{
    ClientFrame, ControlRate, EventStreamLimits, EventStreamPolicy, EventWakeups, WakePosition,
    bounded_event_frame, classify_client_frame, enforce_event_stream_lifetime,
    enforce_stream_send_deadline, event_frame_or_backpressure,
};
use super::operations::read_bounded_body;
use super::service::{
    CompletedDriverAction, WorkspaceLockDomains, bounded_blocking, completed_driver_action,
    run_maintenance_driver,
};
use super::{MAINTENANCE_DRIVER_TIMEOUT, REQUEST_BODY_READ_TIMEOUT};

async fn bound_refusal_response(app: Arc<super::App>, operation: &str) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/workspaces")
        .header("content-type", "application/json")
        .header("x-request-id", "req_refusal_race")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "op": operation,
                "input": {
                    "source": "empty",
                    "labels": {},
                    "unexpected": true
                }
            }))
            .expect("request JSON"),
        ))
        .expect("request");
    let identity = super::Identity {
        subject: "local:1000".to_owned(),
        actor: "refusal-test".to_owned(),
        principal: None,
    };
    let response = super::router(app)
        .layer(Extension(identity))
        .oneshot(request)
        .await
        .expect("router response");
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2_097_152)
            .await
            .expect("response bytes"),
    )
    .expect("response JSON")
}

#[tokio::test]
async fn development_router_serves_neither_session_mint_nor_attachment() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open(":memory:").expect("state store"));
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    let app = super::App::new(store, driver, "dep_transport_routes");
    let identity = super::Identity {
        subject: "development:test".to_owned(),
        actor: "route-test".to_owned(),
        principal: None,
    };
    for (method, uri) in [
        ("POST", "/v1/sessions"),
        ("GET", "/v1/sessions/ses_test/attach"),
        ("POST", "/v1/sessions/ses_test/attachment-authorities"),
    ] {
        let response = super::development_router(Arc::clone(&app))
            .layer(Extension(identity.clone()))
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("router response");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn legacy_session_route_family_is_not_registered() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open(":memory:").expect("state store"));
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    let app = super::App::new(store, driver, "dep_legacy_session_routes");
    let identity = super::Identity {
        subject: "local:1000".to_owned(),
        actor: "route-test".to_owned(),
        principal: None,
    };

    for (method, uri) in [
        ("GET", "/v1/pipe-sessions"),
        ("POST", "/v1/pipe-sessions"),
        ("GET", "/v1/pipe-sessions/ses_test"),
        ("GET", "/v1/pipe-sessions/ses_test/attach"),
        ("POST", "/v1/pipe-sessions/ses_test/attachment-authorities"),
        ("POST", "/v1/pipe-sessions/ses_test/signal"),
        ("DELETE", "/v1/pipe-sessions/ses_test"),
        ("POST", "/v1/pipe-sessions/ses_test/lease/renew"),
    ] {
        let response = super::router(Arc::clone(&app))
            .layer(Extension(identity.clone()))
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("router response");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::NOT_FOUND,
            "{uri}"
        );
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 2_097_152)
                .await
                .expect("response bytes"),
        )
        .expect("response JSON");
        assert_eq!(
            body["error"]["code"], "resource.not-found",
            "{method} {uri}"
        );
    }
}

fn effect(subject: &str, through_seq: u64) -> CommitEffect {
    CommitEffect {
        scope: Scope {
            deployment: "dep_test".to_owned(),
            subject: subject.to_owned(),
        },
        source_scope: format!("source-{subject}"),
        generation: 1,
        through_seq,
    }
}

#[test]
fn completed_exec_ack_requires_every_scope_to_commit_exactly() {
    assert_eq!(
        completed_driver_action(2, 2, false, false),
        CompletedDriverAction::Acknowledge
    );
    assert_eq!(
        completed_driver_action(2, 1, false, true),
        CompletedDriverAction::Retain
    );
    assert_eq!(
        completed_driver_action(2, 1, false, false),
        CompletedDriverAction::Retain
    );
    assert_eq!(
        completed_driver_action(2, 1, true, false),
        CompletedDriverAction::DiscardSuperseded
    );
    assert_eq!(
        completed_driver_action(2, 1, true, true),
        CompletedDriverAction::Retain
    );
    assert_eq!(
        completed_driver_action(0, 0, false, false),
        CompletedDriverAction::Retain
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saturated_blocking_lane_does_not_starve_unrelated_async_work() {
    let slots = Arc::new(Semaphore::new(2));
    let tasks = (0..8)
        .map(|_| {
            let slots = Arc::clone(&slots);
            tokio::spawn(async move {
                bounded_blocking(&slots, || std::thread::sleep(Duration::from_millis(50))).await;
            })
        })
        .collect::<Vec<_>>();

    tokio::time::timeout(Duration::from_millis(25), async {
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(1)).await;
    })
    .await
    .expect("unrelated accept/event-shaped work remains schedulable under saturation");

    for task in tasks {
        task.await.expect("bounded blocking task");
    }
}

#[tokio::test]
async fn commit_wakeups_are_scope_local_coalesced_hints_with_raii_cleanup() {
    let wakeups = Arc::new(EventWakeups::default());
    let scope_a = effect("subject-a", 1).scope;
    let scope_b = effect("subject-b", 1).scope;
    let mut a = wakeups.subscribe(&scope_a);
    let mut b = wakeups.subscribe(&scope_b);

    for sequence in 1..=10_000 {
        wakeups.committed(&[effect("subject-b", sequence)]);
    }
    b.changed().await.expect("B receives its coalesced hint");
    assert!(!a.receiver.has_changed().expect("A watch remains open"));

    // Callback order may differ from commit order. Coalescing retains the greatest durable
    // position observed for this source; the store remains the read authority.
    wakeups.committed(&[effect("subject-a", 9)]);
    wakeups.committed(&[effect("subject-a", 8)]);
    a.changed().await.expect("A receives a change hint");
    assert_eq!(
        a.receiver.borrow_and_update().as_ref(),
        Some(&WakePosition {
            generation: 1,
            through_seq: 9,
            source_scope: "source-subject-a".to_owned(),
        })
    );

    drop(a);
    drop(b);
    assert!(wakeups.scopes.lock().is_empty());
}

#[test]
fn event_stream_limits_are_scope_local_and_recover_by_raii() {
    let limits = EventStreamLimits::new(2, 1);
    let scope_a = effect("subject-a", 1).scope;
    let scope_b = effect("subject-b", 1).scope;
    let scope_c = effect("subject-c", 1).scope;

    let a = limits.acquire(&scope_a).expect("A stream capacity");
    assert!(limits.acquire(&scope_a).is_none());
    let b = limits.acquire(&scope_b).expect("B stream capacity");
    assert!(limits.acquire(&scope_c).is_none());
    assert_eq!(limits.scopes.lock().len(), 2);

    drop(a);
    let a_again = limits.acquire(&scope_a).expect("A capacity recovered");
    drop(a_again);
    drop(b);
    assert!(limits.scopes.lock().is_empty());
    assert_eq!(limits.global.available_permits(), 2);
}

#[test]
fn event_stream_client_data_is_rejected_and_controls_are_classified() {
    assert_eq!(
        classify_client_frame(&axum::extract::ws::Message::Text("data".into())),
        ClientFrame::Data
    );
    assert_eq!(
        classify_client_frame(&axum::extract::ws::Message::Binary(Vec::new().into())),
        ClientFrame::Data
    );
    assert_eq!(
        classify_client_frame(&axum::extract::ws::Message::Ping(Vec::new().into())),
        ClientFrame::Control
    );
    assert_eq!(
        classify_client_frame(&axum::extract::ws::Message::Close(None)),
        ClientFrame::Close
    );
}

#[test]
fn event_stream_output_serialization_stops_at_the_byte_limit() {
    let page = substrate_wire::EventPage {
        source_scope: "source-a".to_owned(),
        generation: 1,
        items: Vec::new(),
        next_cursor: "cursor-a".to_owned(),
        through_seq: 0,
        first_retained_seq: None,
    };
    let encoded = bounded_event_frame(&page, 1_024).expect("bounded frame");
    assert!(encoded.len() <= 1_024);
    assert!(bounded_event_frame(&page, encoded.len() - 1).is_err());
}

#[test]
fn manifest_stream_backpressure_vector_executes_the_production_boundary() {
    let vector: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../contracts/substrate-wire/0.2.0/vectors/driver/event-stream-backpressure.json"
    ))
    .expect("backpressure vector");
    let setup = &vector["setup"][0]["state"];
    let event_count = setup["event_count"].as_u64().expect("event count");
    let payload_bytes = setup["observation_payload_bytes"]
        .as_u64()
        .expect("observation payload bytes");
    let max_output_bytes = usize::try_from(
        setup["max_output_bytes"]
            .as_u64()
            .expect("max output bytes"),
    )
    .expect("max output range");
    let items = (0..event_count)
        .map(|index| {
            serde_json::from_value(serde_json::json!({
                "actor": "vector-client",
                "cause": {
                    "kind": "operation",
                    "operation": "01JPHASE3EVENTSOURCE0001"
                },
                "generation": 41,
                "observation": {
                    "payload": "x".repeat(usize::try_from(payload_bytes).expect("payload range"))
                },
                "observed_at": "2026-08-13T12:00:00Z",
                "principal": null,
                "resource": format!("ws_event{index:02}"),
                "resource_kind": "workspace",
                "seq": index + 8,
                "transition": "workspace.created"
            }))
            .expect("event fixture")
        })
        .collect();
    let expected = &vector["expected"]["outcome"];
    let page = substrate_wire::EventPage {
        source_scope: "scope_vector_subject".to_owned(),
        generation: 41,
        items,
        next_cursor: "ev2.scope_vector_subject.41.72".to_owned(),
        through_seq: 72,
        first_retained_seq: None,
    };
    let boundary =
        event_frame_or_backpressure(&page, max_output_bytes, expected["last_cursor"].as_str())
            .expect_err("oversized event frame must produce a recovery boundary");
    assert_eq!(boundary["kind"], "backpressure");
    assert_eq!(boundary["code"], expected["code"]);
    assert_eq!(boundary["last_cursor"], expected["last_cursor"]);
    assert_eq!(boundary["recovery"], expected["recovery"]);
}

#[tokio::test(start_paused = true)]
async fn event_stream_control_rate_is_bounded_and_resets_per_window() {
    let mut policy = EventStreamPolicy::production();
    policy.max_controls_per_window = 2;
    policy.control_window = Duration::from_secs(5);
    let mut rate = ControlRate::new();

    assert!(!rate.exceeded(policy.max_controls_per_window, policy.control_window));
    assert!(!rate.exceeded(policy.max_controls_per_window, policy.control_window));
    assert!(rate.exceeded(policy.max_controls_per_window, policy.control_window));
    tokio::time::advance(policy.control_window).await;
    assert!(!rate.exceeded(policy.max_controls_per_window, policy.control_window));
}

#[tokio::test(start_paused = true)]
async fn event_stream_lifetime_cancels_session_and_recovers_permits() {
    let limits = EventStreamLimits::new(1, 1);
    let scope = effect("subject-a", 1).scope;
    let permit = limits.acquire(&scope).expect("stream capacity");
    let task = tokio::spawn(enforce_event_stream_lifetime(
        Duration::from_secs(5),
        async move {
            let _permit = permit;
            std::future::pending::<()>().await;
        },
    ));
    tokio::task::yield_now().await;

    tokio::time::advance(Duration::from_secs(5)).await;
    assert!(!task.await.expect("lifetime task"));
    assert!(limits.acquire(&scope).is_some());
}

#[tokio::test(start_paused = true)]
async fn event_stream_send_deadline_is_hard() {
    let task = tokio::spawn(enforce_stream_send_deadline(
        Duration::from_secs(2),
        std::future::pending::<Result<(), std::convert::Infallible>>(),
    ));
    tokio::task::yield_now().await;

    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(task.await.expect("deadline task"), Err(()));
}

#[tokio::test(start_paused = true)]
async fn mutation_body_read_has_a_hard_deadline() {
    let body = axum::body::Body::from_stream(futures_util::stream::pending::<
        Result<axum::body::Bytes, std::convert::Infallible>,
    >());
    let task = tokio::spawn(read_bounded_body(body, "req_body_timeout"));
    tokio::task::yield_now().await;

    tokio::time::advance(REQUEST_BODY_READ_TIMEOUT).await;
    let result = task.await.expect("decode task");
    let Err(response) = result else {
        panic!("pending body must time out");
    };
    assert_eq!(response.status(), axum::http::StatusCode::REQUEST_TIMEOUT);
}

#[tokio::test(flavor = "multi_thread")]
async fn bound_refusal_store_failure_never_returns_the_original_refusal() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let state_path = directory.path().join("state.db");
    let store = Arc::new(Store::open(&state_path).expect("state store"));
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    let app = super::App::new(Arc::clone(&store), driver, "dep_refusal_test");
    let break_path = state_path.clone();
    app.refusal_before_record.lock().replace(Arc::new(move |_| {
        rusqlite::Connection::open(&break_path)
            .expect("fault connection")
            .execute("DROP TABLE operations", [])
            .expect("inject refusal persistence failure");
    }));

    let response = bound_refusal_response(app, "01JREFUSALSTOREFAILURE1").await;
    assert_eq!(response["error"]["code"], "state.store-failed");
    assert_ne!(response["error"]["code"], "request.schema-invalid");
}

#[tokio::test(flavor = "multi_thread")]
async fn bound_refusal_losing_to_accepted_reservation_returns_outcome_unknown() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    let app = super::App::new(Arc::clone(&store), driver, "dep_refusal_test");
    let racing_store = Arc::clone(&store);
    app.refusal_before_record
        .lock()
        .replace(Arc::new(move |new| {
            assert_eq!(
                racing_store.reserve(new).expect("racing acceptance"),
                Reservation::Accepted
            );
        }));

    let operation = "01JREFUSALACCEPTEDRACE1";
    let response = bound_refusal_response(Arc::clone(&app), operation).await;
    assert_eq!(response["error"]["code"], "operation.outcome-unknown");
    assert_eq!(
        store
            .operation(
                &Scope {
                    deployment: "dep_refusal_test".to_owned(),
                    subject: "local:1000".to_owned(),
                },
                operation,
            )
            .expect("operation lookup")
            .expect("accepted operation")
            .state,
        substrate_wire::OperationState::Accepted
    );
}

#[tokio::test(start_paused = true)]
async fn maintenance_driver_deadline_is_hard_and_retriable() {
    let task = tokio::spawn(run_maintenance_driver(std::future::pending::<
        Result<(), substrate_host::DriverError>,
    >()));
    tokio::task::yield_now().await;

    tokio::time::advance(MAINTENANCE_DRIVER_TIMEOUT).await;
    let error = task
        .await
        .expect("deadline task")
        .expect_err("pending driver call must time out");
    assert_eq!(error.code, "maintenance.driver-timeout");
    assert!(error.retriable);
}

#[tokio::test]
async fn workspace_lock_domains_isolate_subjects_but_serialize_one_scope() {
    let locks = WorkspaceLockDomains::default();
    let scope_a = effect("subject-a", 1).scope;
    let scope_b = effect("subject-b", 1).scope;
    let domain_a = locks.domain(&scope_a);
    let same_a = locks.domain(&scope_a);
    let domain_b = locks.domain(&scope_b);

    assert!(Arc::ptr_eq(&domain_a, &same_a));
    assert!(!Arc::ptr_eq(&domain_a, &domain_b));

    let held = Arc::clone(&domain_a.stripes[0]).lock_owned().await;
    assert!(domain_a.stripes[0].try_lock().is_err());
    assert!(domain_b.stripes[0].try_lock().is_ok());
    drop(held);
    assert!(domain_a.stripes[0].try_lock().is_ok());
}
