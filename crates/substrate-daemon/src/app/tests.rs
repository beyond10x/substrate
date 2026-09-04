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
use super::metrics::MetricsStreamPolicy;
use super::operations::read_bounded_body;
use super::service::{
    CompletedDriverAction, WorkspaceLockDomains, bounded_blocking, completed_driver_action,
    run_maintenance_driver,
};
use super::{BODY_LIMIT, MAINTENANCE_DRIVER_TIMEOUT, REQUEST_BODY_READ_TIMEOUT};

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

/// The metrics stream restates nine of `EventStreamPolicy`'s bounds rather than holding them.
/// Every one is compared against its original here, so the restatement cannot drift unremarked,
/// and every published value is also pinned against its own literal, so moving both together is a
/// change somebody makes on purpose.
///
/// The literal half has to be a *literal*. `max_output_bytes` is `BODY_LIMIT` on both policies, so
/// pinning it against `BODY_LIMIT` compared the constant with itself and would have followed the
/// constant anywhere it moved; it is pinned against `2_097_152`, the value `BODY_LIMIT` has at
/// `app.rs:6`, which is what the pin is for.
#[test]
fn metrics_stream_policy_publishes_the_event_stream_bounds() {
    let metrics = MetricsStreamPolicy::production();
    let events = EventStreamPolicy::production();

    assert_eq!(metrics.global_streams, events.global_streams);
    assert_eq!(metrics.streams_per_subject, events.streams_per_subject);
    assert_eq!(metrics.max_input_bytes, events.max_input_bytes);
    assert_eq!(metrics.max_output_bytes, events.max_output_bytes);
    assert_eq!(metrics.write_buffer_bytes, events.write_buffer_bytes);
    assert_eq!(
        metrics.max_controls_per_window,
        events.max_controls_per_window
    );
    assert_eq!(metrics.control_window, events.control_window);
    assert_eq!(metrics.send_timeout, events.send_timeout);
    assert_eq!(metrics.lifetime, events.lifetime);

    assert_eq!(metrics.global_streams, 64);
    assert_eq!(metrics.streams_per_subject, 4);
    assert_eq!(metrics.max_input_bytes, 1_024);
    assert_eq!(metrics.max_output_bytes, 2_097_152);
    assert_eq!(BODY_LIMIT, 2_097_152);
    assert_eq!(metrics.write_buffer_bytes, 16 * 1_024);
    assert_eq!(metrics.max_controls_per_window, 120);
    assert_eq!(metrics.control_window, Duration::from_mins(1));
    assert_eq!(metrics.send_timeout, Duration::from_secs(5));
    assert_eq!(metrics.lifetime, Duration::from_hours(1));
}

/// The metrics twin of `event_stream_lifetime_cancels_session_and_recovers_permits`: the
/// composition the upgrade runs — `enforce_event_stream_lifetime(policy.lifetime, session)`,
/// `app/metrics.rs` — cuts a session that would otherwise never end, and the subject's permit
/// comes back when it does.
///
/// **The limit, as a rule:** this drives the composition, not the served route. Nothing here
/// proves `metrics_stream` passes *its* policy's lifetime to it; what asserts that is
/// `every_websocket_upgrade_declares_its_frame_message_and_lifetime_bounds`
/// (`app/metrics.rs`), which fails an upgrade whose body does not name `policy.lifetime`.
/// A one-hour bound cannot be observed end to end on a live socket: with the 5 s send deadline
/// running, an auto-advancing clock fires the deadline long before the lifetime.
#[tokio::test(start_paused = true)]
async fn metrics_stream_lifetime_cancels_session_and_recovers_permits() {
    let policy = MetricsStreamPolicy::production();
    let limits = EventStreamLimits::new(policy.global_streams, policy.streams_per_subject);
    let scope = effect("subject-metrics", 1).scope;

    let mut held: Vec<_> = (1..policy.streams_per_subject)
        .map(|index| {
            limits
                .acquire(&scope)
                .unwrap_or_else(|| panic!("metrics stream capacity {index}"))
        })
        .collect();
    let last = limits.acquire(&scope).expect("the cap's last permit");
    assert!(
        limits.acquire(&scope).is_none(),
        "the published per-subject cap must bind at {}",
        policy.streams_per_subject
    );

    let task = tokio::spawn(enforce_event_stream_lifetime(policy.lifetime, async move {
        let _permit = last;
        std::future::pending::<()>().await;
    }));
    tokio::task::yield_now().await;
    assert!(
        limits.acquire(&scope).is_none(),
        "a live metrics stream keeps its permit"
    );

    tokio::time::advance(policy.lifetime).await;
    assert!(
        !task.await.expect("lifetime task"),
        "the session must be cut at the published lifetime, not run past it"
    );
    assert!(
        limits.acquire(&scope).is_some(),
        "the cut metrics stream must return its subject permit"
    );
    held.clear();
    assert!(limits.scopes.lock().is_empty());
}

/// Adversary cases against `story:upgraded-connections-keep-their-permit`.
///
/// Kept in their own module so they carry their own imports and change no line another case
/// reads. This file is `#[cfg(test)]`-only (`app.rs:28`) and is the crate's designated unit-test
/// file; the surface under attack — `TransportPermit`, `admitted_service`, `TcpConnectionLimits` —
/// is `pub(crate)`, so no file under `tests/` can reach it.
mod upgraded_transport_slot {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use axum::Extension;
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;
    use hyper_util::service::TowerToHyperService;
    use substrate_host::{HostConfig, HostDriver};
    use substrate_store::Store;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};

    use crate::runtime::{TcpConnectionLimits, admitted_service};
    use crate::{App, Identity, router};

    const HANDSHAKE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
    /// The source every client in this module connects from, and therefore the scope of the
    /// per-source transport bound it is measured against.
    const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    /// `TcpConnectionLimits::production`, which offers no other constructor.
    const PER_SOURCE: usize = 16;
    /// Short enough to observe, and the same order as the 300 ms the story's own measurement used.
    const CONNECTION_LIFETIME: Duration = Duration::from_millis(300);
    /// How long a case watches before concluding a slot is held rather than slow to come back:
    /// five times the transport's connection lifetime, which would have ended the connection five
    /// times over. Watched throughout rather than sampled at the end, so a slot that came back at
    /// any point inside the window is caught.
    const OBSERVATION: Duration = Duration::from_millis(1_500);

    /// One connection's client half, held open for as long as the binding lives.
    struct Handshake {
        status: u16,
        /// Held, never read: an open stream is exactly a peer that has not gone away, and a
        /// dropped one would end the upgraded socket for a reason that is not the one under test.
        _stream: TcpStream,
    }

    impl Handshake {
        async fn open(address: SocketAddr, path: &str) -> Self {
            let mut stream = TcpStream::connect(address).await.expect("connect a client");
            let request = format!(
                "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
            );
            stream
                .write_all(request.as_bytes())
                .await
                .expect("write the handshake");
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 16 * 1_024, "bounded handshake response");
                let mut byte = [0_u8; 1];
                stream
                    .read_exact(&mut byte)
                    .await
                    .expect("read the handshake response");
                head.push(byte[0]);
            }
            let head = std::str::from_utf8(&head).expect("ASCII handshake response");
            let status = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse::<u16>().ok())
                .expect("HTTP handshake status");
            Self {
                status,
                _stream: stream,
            }
        }
    }

    /// The production TCP listener's shape, over the real router: one admission per accepted
    /// connection, published into every request the connection makes, and the transport's
    /// connection lifetime around the connection future.
    ///
    /// `subjects` is how many distinct authenticated identities the listener spreads accepted
    /// connections across, round robin. Over TCP and TLS the transport scope is the *source
    /// address*, not the identity, so this is what a shared-address proxy looks like from the
    /// listener's side: distinct callers, one counter.
    ///
    /// Returns the address to connect to and the budget the listener admits from, so a case can
    /// read the budget from outside the listener.
    async fn admitted_listener(
        deployment: &str,
        subjects: u32,
    ) -> (SocketAddr, Arc<TcpConnectionLimits>) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open(":memory:").expect("state store"));
        let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver");
        let app = App::new(store, driver, deployment);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the admitted listener");
        let address = listener.local_addr().expect("listener address");
        let limits = Arc::new(TcpConnectionLimits::production());
        let admitting = Arc::clone(&limits);
        tokio::spawn(async move {
            // Held for the listener's life so the workspace root outlives every connection.
            let _directory = directory;
            let mut accepted = 0_u32;
            loop {
                let Ok((stream, peer)) = listener.accept().await else {
                    return;
                };
                let Some(permit) = admitting.acquire(peer.ip()) else {
                    continue;
                };
                let subject = format!("local:{}", 1000 + accepted % subjects);
                accepted = accepted.wrapping_add(1);
                let identity = Identity {
                    subject,
                    actor: "transport-slot-test".to_owned(),
                    principal: None,
                };
                let service = router(Arc::clone(&app)).layer(Extension(identity));
                tokio::spawn(async move {
                    let connection = http1::Builder::new()
                        .serve_connection(
                            TokioIo::new(stream),
                            TowerToHyperService::new(admitted_service(&permit, service)),
                        )
                        .with_upgrades();
                    // The two lines `runtime::enforce_connection_lifetime` is, spelled here
                    // because that function is private to `runtime`: the connection's admission
                    // held for the connection's life, and the transport's connection lifetime
                    // around the connection future.
                    let _permit = permit;
                    let _ = tokio::time::timeout(CONNECTION_LIFETIME, connection).await;
                });
            }
        });
        (address, limits)
    }

    /// **This case pins a defect, not a guarantee.**
    ///
    /// What it would be right to assert is that the transport's connection lifetime bounds how
    /// long one connection occupies a transport slot, upgraded or not. That is not what the daemon
    /// does, and this records what it does instead.
    ///
    /// `UnixTransportPolicy::production` states it where it turns keep-alive on — "The outer
    /// connection lifetime remains the finite bound for idle and upgraded peers"
    /// (`crates/substrate-daemon/src/runtime.rs:60-62`) — and that sentence is the stated reason
    /// keep-alive is safe to enable at all.
    ///
    /// Since `story:upgraded-connections-keep-their-permit` the admission survives the upgrade and
    /// nothing carries the lifetime across with it. `enforce_connection_lifetime`'s timeout wraps
    /// the hyper connection future, which hyper resolves when it hands the socket to the upgrade,
    /// and the clone the upgraded task holds carries no deadline. Occupancy is therefore bounded
    /// by the *stream's* lifetime — `EventStreamPolicy::production().lifetime` is one hour
    /// (`app/events.rs:57`), as are the metrics stream's and the pipe attachment's — rather than
    /// by the transport's five minutes.
    ///
    /// Measured through the production TCP listener's shape and the real `/v1/events/stream`
    /// route: one upgraded socket holds one of the source's sixteen slots, and goes on holding it
    /// for five times the transport's connection lifetime and, in production, for the stream's
    /// hour.
    ///
    /// That occupancy is deliberate — an upgraded connection the budget does not count is review
    /// finding 4, closed by `story:upgraded-connections-keep-their-permit` — while the disagreement
    /// between a five-minute transport bound and a one-hour stream is the open design question,
    /// `story:transport-admission-and-stream-lifetime-disagree`. Enforcing the transport's bound on
    /// an upgraded stream would cut every published one-hour stream at five minutes, which is why
    /// the answer is a decision and not a patch. **When this case goes red, that story landed —
    /// invert it back to asserting the guarantee, never relax it.**
    #[tokio::test(flavor = "multi_thread")]
    async fn an_upgraded_stream_holds_its_transport_slot_past_the_connection_lifetime() {
        let (address, limits) = admitted_listener("dep_upgraded_transport_slot", 1).await;

        let upgraded = Handshake::open(address, "/v1/events/stream?limit=1").await;
        assert_eq!(
            upgraded.status, 101,
            "the event stream must upgrade before its admission can be measured"
        );

        // The admission is held at the handshake: fifteen of this source's sixteen slots are
        // free and the sixteenth is refused, which is one slot occupied by that connection.
        let held = (0..PER_SOURCE - 1)
            .map(|index| {
                limits
                    .acquire(CLIENT)
                    .unwrap_or_else(|| panic!("slot {index} of the source's free capacity"))
            })
            .collect::<Vec<_>>();
        assert!(
            limits.acquire(CLIENT).is_none(),
            "the upgraded socket must hold the connection's admission while it serves"
        );

        // Today's behaviour, pinned: nothing carries the transport's deadline across the upgrade,
        // so the slot is not given back when the connection's time is up. Watched throughout the
        // window rather than sampled at the end, so a slot returned at any point during it is
        // caught rather than missed.
        let window = Instant::now() + OBSERVATION;
        while Instant::now() < window {
            assert!(
                limits.acquire(CLIENT).is_none(),
                "the upgraded socket returned the connection's transport slot inside \
                 {OBSERVATION:?}, five times the transport's connection lifetime of \
                 {CONNECTION_LIFETIME:?}, which means \
                 story:transport-admission-and-stream-lifetime-disagree has landed — invert this \
                 case back to asserting the guarantee rather than relaxing it"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(held);
        drop(upgraded);
    }

    /// What one plain HTTP request on a brand new connection is answered with, or `None` when the
    /// listener served nothing at all.
    ///
    /// A connection refused at capacity is dropped by the accept loop before anything reads it,
    /// which the kernel reports to the client as an orderly end of file or as a reset, depending
    /// on whether the request bytes were still queued. Both are the same observation — nothing was
    /// served — and neither is confusable with the bytes of a response. The same reading
    /// `runtime::tests::ask_over_a_new_connection` makes on the unix listener.
    async fn ask_over_a_new_connection(address: SocketAddr, deadline: Duration) -> Option<String> {
        let mut stream = TcpStream::connect(address).await.expect("connect a client");
        let request = format!("GET /v1/events/stream?limit=1 HTTP/1.1\r\nHost: {address}\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("send a plain request");
        let mut served = [0_u8; 128];
        match tokio::time::timeout(deadline, stream.read(&mut served))
            .await
            .expect("the connection is refused or served within the deadline")
        {
            Ok(0) => None,
            Ok(read) => Some(String::from_utf8_lossy(&served[..read]).into_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => None,
            Err(error) => panic!("read the connection: {error}"),
        }
    }

    /// **This case pins a defect, not a guarantee.**
    ///
    /// What it would be right to assert is that one caller cannot deny a co-located caller
    /// transport admission for the length of a stream. That is not what the daemon does, and this
    /// records what it does instead.
    ///
    /// Over TCP and TLS the transport scope is the source address — `TcpConnectionLimits` counts
    /// sixteen per `IpAddr` (`runtime.rs:275`) — so every identity behind one proxy address shares
    /// one counter. Since `story:upgraded-connections-keep-their-permit` an upgraded socket keeps
    /// its slot, and nothing carries the transport's connection lifetime across the upgrade, so
    /// the sixteen slots are held for the *stream's* lifetime: one hour
    /// (`app/events.rs:57`) rather than the transport's five minutes.
    ///
    /// Measured, not inferred: four identities, four event streams each — each inside the
    /// published per-subject cap of four (`app/events.rs:49`), so no caller does anything it is
    /// not entitled to do — fill the source's sixteen slots, and the next caller from that address
    /// is refused at accept with nothing served, and stays refused past the transport's connection
    /// lifetime. `TcpConnectionLimits` will not evict the source entry either: eviction requires
    /// `available_permits() == per_source` (`runtime.rs`, `acquire`), which a held slot denies.
    ///
    /// The occupancy itself is deliberate — an upgraded connection the budget does not count is
    /// review finding 4, closed by `story:upgraded-connections-keep-their-permit`. What is open is
    /// the bound that ends it, and raising or re-scoping a sixteen-per-source budget that callers
    /// inside their own published caps can exhaust is a design decision needing an ADR under
    /// invariant 8, not a correction: it is
    /// `story:transport-admission-and-stream-lifetime-disagree`. **When this case goes red, that
    /// story landed — invert it back to asserting the guarantee, never relax it.**
    #[tokio::test(flavor = "multi_thread")]
    async fn upgraded_streams_hold_a_shared_source_address_past_the_connection_lifetime() {
        const DEADLINE: Duration = Duration::from_secs(2);
        const SUBJECTS: u32 = 4;

        let (address, _limits) = admitted_listener("dep_shared_source_denial", SUBJECTS).await;

        // Sixteen event streams over four identities: four each, the published per-subject cap.
        let mut streams = Vec::new();
        for index in 0..PER_SOURCE {
            let stream = Handshake::open(address, "/v1/events/stream?limit=1").await;
            assert_eq!(
                stream.status, 101,
                "event stream {index} is inside every published cap and must upgrade"
            );
            streams.push(stream);
        }

        // The source's whole transport budget is now held by upgraded sockets, so the next caller
        // sharing that address is refused at accept and served nothing at all.
        let denied = ask_over_a_new_connection(address, DEADLINE).await;
        assert_eq!(
            denied, None,
            "sixteen upgraded sockets must hold the source's whole transport budget for this \
             case to measure what ends the denial; the next caller was served: {denied:?}"
        );

        // Today's behaviour, pinned: the transport's connection lifetime is not what ends it, so
        // the co-located caller stays refused for the whole window — and in production for the
        // stream's hour. Watched throughout rather than sampled at the end, so an admission that
        // came back at any point during it is caught rather than missed.
        let window = Instant::now() + OBSERVATION;
        while Instant::now() < window {
            let answer = ask_over_a_new_connection(address, DEADLINE).await;
            assert_eq!(
                answer, None,
                "a caller sharing the source address was served inside {OBSERVATION:?}, five times \
                 the transport's connection lifetime of {CONNECTION_LIFETIME:?}, which means \
                 story:transport-admission-and-stream-lifetime-disagree has landed — invert this \
                 case back to asserting the guarantee rather than relaxing it; it answered: \
                 {answer:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(streams);
    }
}
