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

/// An application over an in-memory store and a minimum host driver, with the directory its
/// workspaces live in. For the `app` modules' own tests, which are clients of the driver port.
pub(super) fn host_app(deployment: &str) -> (tempfile::TempDir, Arc<super::App>) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open(":memory:").expect("state store"));
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    (directory, super::App::new(store, driver, deployment))
}

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

/// Adversary cases against `story:upgraded-connections-keep-their-permit`, inverted by
/// `story:transport-admission-and-stream-lifetime-disagree` (ADR 0042): an upgraded socket hands
/// its transport slot to the stream it became, and is bounded by that stream's published caps.
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

    use crate::runtime::{TcpConnectionLimits, admitted_service, enforce_connection_lifetime};
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
        /// The response head, kept so a refused handshake's body can be read to its length.
        head: String,
        /// Held, and read only for a refusal's body: an open stream is exactly a peer that has
        /// not gone away, and a dropped one would end the upgraded socket for a reason that is not
        /// the one under test.
        stream: TcpStream,
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
                head: head.to_owned(),
                stream,
            }
        }

        /// The body of a response that did not upgrade, read to its `content-length`.
        async fn body(&mut self) -> String {
            let length = self
                .head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .expect("a refusal states its content-length");
            assert!(length <= 16 * 1_024, "bounded refusal body");
            let mut body = vec![0_u8; length];
            self.stream
                .read_exact(&mut body)
                .await
                .expect("read the refusal body");
            String::from_utf8(body).expect("UTF-8 refusal body")
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
    pub(super) async fn admitted_listener(
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
                    // The production composition itself rather than a copy of its two lines, so
                    // the hand-off ADR 0042 makes in each upgraded task is the one measured here.
                    let _ =
                        enforce_connection_lifetime(permit, CONNECTION_LIFETIME, connection).await;
                });
            }
        });
        (address, limits)
    }

    /// **An upgraded stream returns its transport slot at the upgrade** (ADR 0042, decision 1).
    ///
    /// This case pinned the defect `story:transport-admission-and-stream-lifetime-disagree` named
    /// as `an_upgraded_stream_holds_its_transport_slot_past_the_connection_lifetime`: since
    /// `story:upgraded-connections-keep-their-permit` an upgraded socket kept the transport permit
    /// that admitted it for the stream's hour, not the transport's five minutes. It asserts the
    /// guarantee now.
    ///
    /// Measured through the production TCP listener's shape and the real `/v1/events/stream`
    /// route: once the stream has upgraded, all sixteen of the source's transport slots come back
    /// — polled, because the hand-off runs in the upgraded task after the `101` is sent — while the
    /// stream still serves and still holds its own permit, so the socket is counted by the
    /// stream's cap instead (`an_ended_upgraded_stream_returns_its_stream_slot`).
    #[tokio::test(flavor = "multi_thread")]
    async fn an_upgraded_stream_returns_its_transport_slot_at_the_upgrade() {
        let (address, limits) = admitted_listener("dep_upgraded_transport_slot", 1).await;

        let upgraded = Handshake::open(address, "/v1/events/stream?limit=1").await;
        assert_eq!(
            upgraded.status, 101,
            "the event stream must upgrade before its admission can be measured"
        );

        // The guarantee: the source's whole transport budget is free again while the stream
        // serves, well inside the transport's connection lifetime, let alone the stream's hour.
        let window = Instant::now() + CONNECTION_LIFETIME;
        let mut held = Vec::new();
        while Instant::now() < window {
            held = (0..PER_SOURCE)
                .map_while(|_| limits.acquire(CLIENT))
                .collect::<Vec<_>>();
            if held.len() == PER_SOURCE {
                break;
            }
            held.clear();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            held.len(),
            PER_SOURCE,
            "the upgraded socket still holds a transport slot {CONNECTION_LIFETIME:?} after its \
             handshake; ADR 0042 hands it over to the stream at the upgrade"
        );
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

    /// **Callers inside their published caps cannot deny a co-located caller transport admission**
    /// (ADR 0042; the acceptance of `story:transport-admission-and-stream-lifetime-disagree`).
    ///
    /// This case pinned the defect as
    /// `upgraded_streams_hold_a_shared_source_address_past_the_connection_lifetime`. Over TCP and
    /// TLS the transport scope is the source address — sixteen per `IpAddr` — so every identity
    /// behind one proxy address shares one counter, and four identities holding four event streams
    /// each, every one inside the published per-subject cap of four, held all sixteen for the
    /// stream's hour. It asserts the guarantee now.
    ///
    /// Measured, not inferred: the same sixteen streams from the same four identities, and a caller
    /// sharing that address is served throughout a window five times the transport's connection
    /// lifetime. Watched throughout rather than sampled at the end, so a refusal at any point inside
    /// the window is caught.
    #[tokio::test(flavor = "multi_thread")]
    async fn upgraded_streams_do_not_hold_a_shared_source_address() {
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

        let window = Instant::now() + OBSERVATION;
        while Instant::now() < window {
            let answer = ask_over_a_new_connection(address, DEADLINE).await;
            assert!(
                answer
                    .as_deref()
                    .is_some_and(|answer| answer.starts_with("HTTP/1.1 ")),
                "sixteen upgraded streams, each inside its subject's published cap, denied a \
                 caller sharing their source address transport admission; it answered: {answer:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(streams);
    }

    /// **Upgraded sockets stay bounded by the deployment's stream cap** (ADR 0042, decision 3).
    ///
    /// The transport no longer counts an upgraded socket, so what bounds them is the stream
    /// family's own deployment-wide cap, and that bound has to hold where the transport's no
    /// longer does. Sixty-four event streams from one source address — four times its sixteen
    /// transport slots — over thirty-two identities, two each, so no per-subject cap binds: every
    /// one upgrades, and the sixty-fifth is refused `429` `event.stream-capacity`, the refusal the
    /// `events.stream-bounds` fact publishes, at the published `global` of sixty-four.
    #[tokio::test(flavor = "multi_thread")]
    async fn upgraded_sockets_stay_bounded_by_the_deployment_stream_cap() {
        const SUBJECTS: u32 = 32;
        let global = usize::try_from(substrate_wire::EVENT_STREAMS_GLOBAL).expect("small cap");
        assert!(
            global > PER_SOURCE,
            "the case measures past the transport bound"
        );

        let (address, _limits) = admitted_listener("dep_deployment_stream_cap", SUBJECTS).await;

        let mut streams = Vec::new();
        for index in 0..global {
            let stream = Handshake::open(address, "/v1/events/stream?limit=1").await;
            assert_eq!(
                stream.status, 101,
                "event stream {index} is inside the per-subject and deployment caps and must \
                 upgrade, whatever the source's transport budget"
            );
            streams.push(stream);
        }

        let mut refused = Handshake::open(address, "/v1/events/stream?limit=1").await;
        assert_eq!(
            refused.status, 429,
            "the stream past the published deployment cap of {global} must be refused"
        );
        let body = refused.body().await;
        assert!(
            body.contains(&format!("\"{}\"", substrate_wire::EVENT_STREAM_CAPACITY)),
            "the refusal must carry the code the events.stream-bounds fact publishes: {body}"
        );
        drop(streams);
    }

    /// **The other direction: the stream slot an upgraded socket is counted by is a slot it gives
    /// back.**
    ///
    /// This case was `an_ended_upgraded_stream_returns_its_transport_slot`. Since ADR 0042 the
    /// budget that counts an upgraded socket is its stream's, so the leak this guards against —
    /// a slot never returned, retiring capacity for the life of the process — is measured on that
    /// budget: one identity, so the published per-subject cap of four binds. While the first
    /// stream serves, three more upgrade and the fifth is refused `429`; once the first is dropped
    /// the way a client drops one, a new stream upgrades again.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_ended_upgraded_stream_returns_its_stream_slot() {
        let per_subject =
            usize::try_from(substrate_wire::EVENT_STREAMS_PER_SUBJECT).expect("small cap");
        let (address, _limits) = admitted_listener("dep_returned_stream_slot", 1).await;

        let upgraded = Handshake::open(address, "/v1/events/stream?limit=1").await;
        assert_eq!(
            upgraded.status, 101,
            "the event stream must upgrade before its admission can be measured"
        );
        let mut others = Vec::new();
        for index in 1..per_subject {
            let stream = Handshake::open(address, "/v1/events/stream?limit=1").await;
            assert_eq!(
                stream.status, 101,
                "stream {index} is inside the subject's cap"
            );
            others.push(stream);
        }
        let over = Handshake::open(address, "/v1/events/stream?limit=1").await;
        assert_eq!(
            over.status, 429,
            "the upgraded sockets must hold the subject's {per_subject} stream slots while they \
             serve"
        );

        drop(upgraded);
        let window = Instant::now() + OBSERVATION;
        let mut status = 429;
        while Instant::now() < window {
            status = Handshake::open(address, "/v1/events/stream?limit=1")
                .await
                .status;
            if status == 101 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            status, 101,
            "the upgraded socket ended and did not return its stream slot within {OBSERVATION:?}; \
             a leaked slot is capacity the published exhausted-and-retriable refusal cannot \
             recover"
        );
        drop(others);
        drop(over);
    }
}

/// Adversary pass 2 against `story:upgraded-connections-keep-their-permit`.
///
/// The unit's correction round rewrote what the crate says about the transport admission, and
/// both cases here compare one of those statements against the thing it describes. The shape is
/// the crate's own: `the_published_cap_and_refusal_are_the_ones_the_public_guide_states`
/// (`app/metrics.rs`) already ties a public guide's prose to the policy it states, and
/// `the_advertised_sample_interval_is_the_one_every_released_bundle_requires` ties a constant to
/// five frozen bundles. A document nothing compares against the code is a document that drifts,
/// and the two swept in this round drifted in the same commit that swept them.
mod transport_documents_and_the_transport {
    /// Every upgradeable listener under the crate's `src/`, as `path:line`.
    ///
    /// The same set
    /// `app::metrics::tests::every_upgradeable_connection_publishes_its_transport_admission`
    /// walks, counted independently of it so that its docstring's claim about the set's size is
    /// compared against the set rather than against itself.
    ///
    /// Line-based, and a line whose first non-space bytes are `//` is prose: every mention of the
    /// call in a doc comment is one of those, and no line in this crate carries the call after
    /// code on the same line. The needle is assembled with `concat!` so this file cannot count
    /// itself, exactly as the checks it is measuring do.
    fn upgradeable_listeners() -> Vec<String> {
        const UPGRADES: &str = concat!(".with_", "upgrades()");
        let mut found = Vec::new();
        let mut pending = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().is_some_and(|value| value == "rs") {
                    let source = std::fs::read_to_string(&path).expect("crate source");
                    for (offset, line) in source.lines().enumerate() {
                        if !line.trim_start().starts_with("//") && line.contains(UPGRADES) {
                            found.push(format!("{}:{}", path.display(), offset + 1));
                        }
                    }
                }
            }
        }
        found.sort();
        found
    }

    /// The English number word `prose` states after `phrase`, as a count.
    fn stated_count(prose: &str, phrase: &str) -> usize {
        let word = prose
            .split(phrase)
            .nth(1)
            .unwrap_or_else(|| panic!("no document states \"{phrase}\""))
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect::<String>();
        match word.as_str() {
            "one" => 1,
            "two" => 2,
            "three" => 3,
            "four" => 4,
            "five" => 5,
            "six" => 6,
            "seven" => 7,
            "eight" => 8,
            "nine" => 9,
            "ten" => 10,
            other => panic!("\"{phrase}{other}\" is not a number this case can read"),
        }
    }

    /// `source` with its `///` doc comments joined into one whitespace-normalised paragraph, so a
    /// re-wrapped sentence is not a failure and a changed one is.
    fn doc_prose(source: &str) -> String {
        source
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("///"))
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// **The check's docstring enumerates the listeners it covers, and the commit that wrote the
    /// enumeration added one it does not name.**
    ///
    /// `every_upgradeable_connection_publishes_its_transport_admission` is the only thing holding
    /// the TLS listener — its own docstring says so — so what a reader needs from that docstring
    /// is an accurate account of the set it walks. The correction round gave it one: "Today that
    /// set is five: the unix, TCP and TLS listeners, the metrics harness below and the unix
    /// fixture in `runtime.rs`" (`app/metrics.rs`), and `admitted_service`'s own doc says the same
    /// in words — "the three production listeners and the two in-crate harnesses"
    /// (`runtime.rs`). Five was right at the wave's base commit. The same commit that wrote both
    /// sentences added a sixth, `app::tests::upgraded_transport_slot::admitted_listener` in this
    /// file, so both sentences were false the moment they were committed.
    ///
    /// It is not a spelling matter: the assertion those docstrings stand behind is
    /// `checked >= 3`, whose message reads "the unix, TCP and TLS listeners must all be read". A
    /// floor of three over a set of six no longer means what that message says — three of the six
    /// are harnesses, so a walk that read only the harnesses satisfies the floor and reports
    /// itself green while reading no production listener at all. The floor was two thirds of the
    /// set when it was written and is half of it now.
    ///
    /// The fix is not this case's to make. Either sentence may be corrected, and the floor may be
    /// raised with it; this compares the number the document states against the number of
    /// listeners there are, so any of those turns it green.
    #[test]
    fn the_upgradeable_listener_set_the_check_documents_is_the_set_that_exists() {
        const PHRASE: &str = "Today that set is ";
        let metrics = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app/metrics.rs");
        let prose = doc_prose(&std::fs::read_to_string(&metrics).expect("metrics source"));
        let stated = stated_count(&prose, PHRASE);
        let listeners = upgradeable_listeners();
        assert_eq!(
            listeners.len(),
            stated,
            "{} states \"{PHRASE}{stated}\", and the crate's src/ holds {} upgradeable \
             listeners; the check's own docstring is what tells a reader which listeners the TLS \
             listener's only guard covers:\n{}",
            metrics.display(),
            listeners.len(),
            listeners.join("\n")
        );
    }

    /// **The design document that publishes this transport's invariants states a keep-alive
    /// setting the transport does not have.**
    ///
    /// The correction round rewrote `UnixTransportPolicy::production`'s justification for
    /// `keep_alive: true` because the old one implied the connection lifetime bounds every
    /// connection, and reported that a grep of `crates/`, `website/docs/`, `docs/design/` and
    /// `adr/` found nothing else stating the transport's bounds.
    /// `docs/design/08-phase-3-closure-invariants.md` states one: "The local HTTP transport
    /// disables keep-alive so an idle connection has no unbounded between-request state", in the
    /// bullet that also states the connection limits and lifetimes, in a document whose own
    /// header says each invariant it records has deterministic capacity evidence.
    ///
    /// `UnixTransportPolicy::production` sets `keep_alive: true` and `http1_builder` passes it to
    /// hyper, on all three listeners. `keep-alive is disabled` is therefore false, and has been
    /// since `a9f234b`; no ADR, no `CHANGELOG.md` entry and no other design document records the
    /// change, so the sentence is the only standing statement of the property and it is wrong.
    /// `docs/design/` is not the immutable category — `AGENTS.md` names only
    /// `docs/reviews/archived/` there — so correcting it is a line, not a record falsified.
    ///
    /// Read from source rather than from the type, because `UnixTransportPolicy` is private to
    /// `runtime`. Either side may move to make this green: the sentence, or the setting.
    #[test]
    fn the_keep_alive_the_closure_invariants_publish_is_the_one_the_transport_sets() {
        const CLAIM: &str = "The local HTTP transport disables keep-alive";
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let runtime =
            std::fs::read_to_string(manifest.join("src/runtime.rs")).expect("runtime source");
        let policy = runtime
            .split_once("impl UnixTransportPolicy {")
            .expect("the transport policy is declared")
            .1;
        let setting = policy
            .split_once("keep_alive:")
            .expect("the production transport policy sets keep-alive")
            .1
            .trim_start();
        let enabled = setting.starts_with("true");
        assert!(
            enabled || setting.starts_with("false"),
            "the production transport policy sets keep-alive to a value this case cannot read"
        );
        let design = manifest.join("../../docs/design/08-phase-3-closure-invariants.md");
        let prose = std::fs::read_to_string(&design)
            .expect("phase 3 closure invariants")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            !(enabled && prose.contains(CLAIM)),
            "{} states \"{CLAIM}\" while UnixTransportPolicy::production sets keep_alive: true \
             and http1_builder passes it to hyper on the unix, TCP and TLS listeners; an \
             invariant a design document publishes and the transport contradicts is the class \
             this unit's correction round set out to close",
            design.display()
        );
    }
}

/// The 1-based lines of `source` that start a task or thread nothing owns, outside test modules.
///
/// A line names a spawn when it holds the identifier `spawn`, any `spawn_…` (`spawn_blocking`,
/// `spawn_local`), or `JoinSet` — whatever path, receiver or `use` group it is reached through. A
/// line whose code starts with `//` is prose and is not read.
///
/// The body of a `#[cfg(test)] mod … {` is skipped up to the line that closes it: the `}` at the
/// `mod` line's own indentation, which `cargo fmt` (a gate step) guarantees. A string literal in a
/// test module holding such a line ends the skip early, which can only report a test line as a
/// finding — the scan fails closed, never open. Code after a test module is read again.
fn detached_spawn_lines(source: &str) -> Vec<usize> {
    let names_a_spawn = |code: &str| {
        code.split(|character: char| !(character.is_alphanumeric() || character == '_'))
            .any(|token| token == "spawn" || token.starts_with("spawn_") || token == "JoinSet")
    };
    let mut found = Vec::new();

    // The closer and the line of the `mod` that opened the skip.
    let mut test_module: Option<(String, usize)> = None;
    let mut after_cfg_test = false;
    for (index, line) in source.lines().enumerate() {
        if let Some((closer, _)) = &test_module {
            // A trailing comment on the closing brace still closes the module.
            let without_comment = line.split("//").next().unwrap_or_default().trim_end();
            if without_comment == closer {
                test_module = None;
            }
            continue;
        }
        let code = line.trim_start();
        if after_cfg_test && code.starts_with("mod ") && code.trim_end().ends_with('{') {
            let indentation = &line[..line.len() - code.len()];
            test_module = Some((format!("{indentation}}}"), index + 1));
            after_cfg_test = false;
            continue;
        }
        after_cfg_test = code.trim_end() == "#[cfg(test)]";
        if !code.starts_with("//") && names_a_spawn(code) {
            found.push(index + 1);
        }
    }
    // A test module whose closer was never found would hide everything after it: the scan
    // reports the module's own line instead of passing unread code.
    if let Some((_, opened)) = test_module {
        found.push(opened);
    }
    found
}

/// Every `.rs` file beneath `directory`, subdirectories included, in a stable order.
fn rust_sources(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut sources = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("read an app source directory") {
            let path = entry.expect("app source entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
    }
    sources.sort();
    sources
}

/// The scan rule's own cases: each spelling of an unowned task or thread is found, wherever it sits
/// outside a test module — including after one — and nothing inside a test module is.
#[test]
fn the_detached_spawn_scan_finds_every_spelling_outside_test_modules() {
    let cases: &[(&str, &str, &[usize])] = &[
        (
            "tokio::spawn",
            "fn a() {\n    tokio::spawn(async {});\n}\n",
            &[2],
        ),
        (
            "tokio::task::spawn",
            "fn a() {\n    tokio::task::spawn(async {});\n}\n",
            &[2],
        ),
        (
            "spawn_blocking",
            "fn a() {\n    task::spawn_blocking(|| ());\n}\n",
            &[2],
        ),
        (
            "an imported spawn",
            "use tokio::spawn;\nfn a() {\n    spawn(async {});\n}\n",
            &[1, 3],
        ),
        (
            "an imported spawn in a multi-line group",
            "use tokio::task::{\n    JoinHandle,\n    spawn,\n};\n",
            &[3],
        ),
        (
            "a JoinSet",
            "fn a() {\n    let mut set = JoinSet::new();\n    set.spawn(async {});\n}\n",
            &[2, 3],
        ),
        (
            "a runtime handle",
            "fn a(handle: Handle) {\n    handle.spawn(async {});\n}\n",
            &[2],
        ),
        (
            "a thread",
            "fn a() {\n    std::thread::spawn(|| ());\n}\n",
            &[2],
        ),
        (
            "production code after a test module",
            "#[cfg(test)]\nmod tests {\n    fn t() {\n        tokio::spawn(async {});\n    }\n}\n\nfn a() {\n    tokio::spawn(async {});\n}\n",
            &[9],
        ),
        (
            "a test module declared out of line",
            "#[cfg(test)]\nmod tests;\n\nfn a() {\n    tokio::spawn(async {});\n}\n",
            &[5],
        ),
        (
            "only a test module",
            "#[cfg(test)]\nmod tests {\n    fn t() {\n        tokio::spawn(async {});\n        std::thread::spawn(|| ());\n    }\n}\n",
            &[],
        ),
        (
            "production code after a test module whose closing brace carries a comment",
            "#[cfg(test)]\nmod tests {\n    fn t() {}\n} // mod tests\n\nfn a() {\n    tokio::spawn(async {});\n}\n",
            &[7],
        ),
        (
            "a test module whose closer is never found",
            "#[cfg(test)]\nmod tests {\n    fn t() {}\n  }\n  fn a() {\n    tokio::spawn(async {});\n  }\n",
            &[2],
        ),
        (
            "prose about spawning",
            "// No task is spawned here.\n/// Nothing calls `tokio::spawn`.\nfn respawned() {}\n",
            &[],
        ),
    ];
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(name, source, expected)| {
            let found = detached_spawn_lines(source);
            (found != *expected).then(|| format!("{name}: found {found:?}, expected {expected:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "the scan misreads: {wrong:#?}");
}

/// The daemon's `app` module spawns no detached task: containment that follows a failed upgrade is
/// queued for the containment step the runtime owns and aborts at shutdown, rather than run in a
/// task nothing owns.
///
/// Reads `src/app.rs` and every file beneath `src/app/`. What a test may spawn is excluded: the
/// test-only `app/tests.rs` whole (declared `#[cfg(test)] mod tests;` in `app.rs`), and the body
/// of every `#[cfg(test)]` module elsewhere, which `detached_spawn_lines` skips.
#[test]
fn app_code_spawns_no_detached_task() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = rust_sources(&root.join("app"));
    sources.push(root.join("app.rs"));
    let mut checked = 0_usize;
    let mut spawning = Vec::new();
    for path in sources {
        if path == root.join("app").join("tests.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read app source");
        checked += 1;
        for line in detached_spawn_lines(&source) {
            spawning.push(format!("{}:{line}", path.display()));
        }
    }
    assert!(checked >= 10, "only {checked} app source files were read");
    assert!(
        spawning.is_empty(),
        "detached tasks in the daemon's app code: {spawning:?}"
    );
}

/// **The caps each stream family enforces are the caps its capability fact publishes** (ADR 0042,
/// decision 4; `story:the-metrics-stream-cap-is-machine-readable`).
///
/// The three production policies and the `substrate_wire` constants the host probe publishes as
/// `events.stream-bounds`, `metrics.stream-bounds` and `sessions.attachment-bounds` are compared
/// here, so a policy that moves without its fact fails, and so does a fact that moves without its
/// policy.
#[test]
fn the_stream_policies_enforce_the_bounds_their_facts_publish() {
    let events = EventStreamPolicy::production();
    let metrics = MetricsStreamPolicy::production();
    let sessions = super::sessions::PipeSessionPolicy::production();
    let published = substrate_wire::StreamBoundsFacts::events();
    let lifetime = Duration::from_millis(substrate_wire::UPGRADED_STREAM_LIFETIME_MS);
    assert_eq!(
        (
            events.streams_per_subject,
            events.global_streams,
            events.lifetime
        ),
        (
            usize::try_from(published.per_subject).expect("small cap"),
            usize::try_from(published.global).expect("small cap"),
            lifetime
        ),
        "events.stream-bounds"
    );
    let published = substrate_wire::StreamBoundsFacts::metrics();
    assert_eq!(
        (
            metrics.streams_per_subject,
            metrics.global_streams,
            metrics.lifetime
        ),
        (
            usize::try_from(published.per_subject).expect("small cap"),
            usize::try_from(published.global).expect("small cap"),
            lifetime
        ),
        "metrics.stream-bounds"
    );
    assert_eq!(
        super::metrics::METRICS_STREAM_CAPACITY,
        published.refusal,
        "the metrics stream refuses with the code its fact publishes"
    );
    let published = substrate_wire::AttachmentBoundsFacts::sessions();
    assert_eq!(
        (
            sessions.attachments_per_subject,
            sessions.global_attachments,
            sessions.lifetime
        ),
        (
            usize::try_from(published.per_subject).expect("small cap"),
            usize::try_from(published.global).expect("small cap"),
            lifetime
        ),
        "sessions.attachment-bounds"
    );
}

/// Security review pass 1 on unit b2 (ADR 0042): integrity conformance checks of the hand-over of
/// an upgraded connection's transport slot to its stream, through the production TCP listener's
/// shape (`upgraded_transport_slot::admitted_listener`, reused rather than copied so the crate's
/// listener inventory checks count no new listener) and the real router.
///
/// Each case measures one clause of ADR 0042 against the running composition: no path leaks a
/// slot of either budget, the two budgets together bound the open sockets under concurrency, and
/// the bound a client reads from `/v1/machine` is the one the route enforces.
mod stream_caps_security_pass1 {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::time::{Duration, Instant};

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpStream;

    use super::upgraded_transport_slot::admitted_listener;
    use crate::runtime::TcpConnectionLimits;

    const HANDSHAKE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
    const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    /// `TcpConnectionLimits::production`'s per-source bound.
    const PER_SOURCE: usize = 16;
    const SETTLE: Duration = Duration::from_secs(3);
    const EVENTS: &str = "/v1/events/stream?limit=1";

    fn global() -> usize {
        usize::try_from(substrate_wire::EVENT_STREAMS_GLOBAL).expect("small cap")
    }

    /// A handshake's status, its response head and the client half, or `None` when the listener
    /// refused the connection at accept.
    async fn handshake(address: SocketAddr, path: &str) -> Option<(u16, String, TcpStream)> {
        let mut stream = TcpStream::connect(address).await.ok()?;
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 16 * 1_024, "bounded handshake response");
            let mut byte = [0_u8; 1];
            match tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut byte)).await {
                Ok(Ok(_)) => head.push(byte[0]),
                Ok(Err(_)) if head.is_empty() => return None,
                Ok(Err(error)) => panic!("read the handshake response: {error}"),
                Err(elapsed) => panic!("no handshake response within five seconds: {elapsed}"),
            }
        }
        let head = String::from_utf8(head).expect("ASCII handshake response");
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .expect("HTTP handshake status");
        Some((status, head, stream))
    }

    /// The first answer on a connection the transport admits, retried past refusals at accept.
    async fn admitted_handshake(address: SocketAddr, path: &str) -> (u16, String, TcpStream) {
        let deadline = Instant::now() + SETTLE;
        loop {
            if let Some(answer) = handshake(address, path).await {
                return answer;
            }
            assert!(
                Instant::now() < deadline,
                "no connection was admitted at the transport within {SETTLE:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// `count` event stream upgrades at once: the upgraded client halves, and how many were
    /// refused `429`. Any other answer fails the case.
    async fn concurrent_upgrades(address: SocketAddr, count: usize) -> (Vec<TcpStream>, usize) {
        let mut attempts = tokio::task::JoinSet::new();
        for _ in 0..count {
            attempts.spawn(async move { admitted_handshake(address, EVENTS).await });
        }
        let mut upgraded = Vec::new();
        let mut refused = 0_usize;
        while let Some(joined) = attempts.join_next().await {
            let (status, head, stream) = joined.expect("handshake task");
            match status {
                101 => upgraded.push(stream),
                429 => refused += 1,
                other => panic!("unexpected handshake status {other}: {head}"),
            }
        }
        (upgraded, refused)
    }

    /// Waits until the deployment's whole event stream cap upgrades at once again — sixty-four
    /// concurrent upgrades over a listener whose subjects are at least sixteen, so no per-subject
    /// cap binds — and says whether it did.
    async fn deployment_cap_returns(address: SocketAddr) -> bool {
        let deadline = Instant::now() + SETTLE;
        while Instant::now() < deadline {
            let (upgraded, _) = concurrent_upgrades(address, global()).await;
            let whole = upgraded.len() == global();
            drop(upgraded);
            if whole {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    async fn body(head: &str, stream: &mut TcpStream) -> String {
        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .expect("a refusal states its content-length");
        let mut body = vec![0_u8; length];
        stream.read_exact(&mut body).await.expect("read the body");
        String::from_utf8(body).expect("UTF-8 body")
    }

    /// Waits until the whole per-source transport budget is free, and says whether it became so.
    async fn transport_budget_returns(limits: &TcpConnectionLimits) -> bool {
        let deadline = Instant::now() + SETTLE;
        while Instant::now() < deadline {
            let held = (0..PER_SOURCE)
                .map_while(|_| limits.acquire(CLIENT))
                .collect::<Vec<_>>();
            if held.len() == PER_SOURCE {
                return true;
            }
            drop(held);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    /// **No aborted handshake leaks a slot of either budget.**
    ///
    /// A client that closes mid-upgrade reaches one of two paths: the `101` write fails and the
    /// upgrade is abandoned (the callback, the stream permit and the transport clone are dropped
    /// unrun), or the upgrade completes into a socket whose peer has already gone (the upgraded
    /// task hands over and then ends on end of file). Sixty-four such aborts over sixteen subjects,
    /// half before reading anything and half after reading the answer, concurrently, and then: the
    /// source's whole transport budget is free, and the deployment's whole event stream cap of
    /// sixty-four upgrades at once — sixteen subjects at their per-subject cap of four each, so
    /// every per-subject and every deployment slot an aborted upgrade took came back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn aborted_handshakes_leak_no_slot_of_either_budget() {
        let (address, limits) = admitted_listener("dep_sec_aborted_handshakes", 16).await;
        let mut aborts = tokio::task::JoinSet::new();
        for index in 0..64_u32 {
            aborts.spawn(async move {
                let Ok(mut stream) = TcpStream::connect(address).await else {
                    return;
                };
                let request = format!(
                    "GET {EVENTS} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
                );
                if stream.write_all(request.as_bytes()).await.is_err() {
                    return;
                }
                if index % 2 == 1 {
                    let mut some = [0_u8; 16];
                    let _ =
                        tokio::time::timeout(Duration::from_millis(500), stream.read(&mut some))
                            .await;
                }
                drop(stream);
            });
        }
        while aborts.join_next().await.is_some() {}

        assert!(
            transport_budget_returns(&limits).await,
            "the source's transport budget did not return within {SETTLE:?} of every aborted \
             handshake ending: an aborted upgrade leaked a transport slot"
        );
        assert!(
            deployment_cap_returns(address).await,
            "the deployment's event stream cap did not upgrade whole within {SETTLE:?} of every \
             aborted handshake ending: an aborted upgrade leaked a stream slot"
        );
    }

    /// **Under concurrency, the open sockets stay inside transport budget + stream cap, and both
    /// budgets are returned whole.**
    ///
    /// One hundred concurrent upgrades from one source over fifty identities (two each, so no
    /// per-subject cap binds): exactly the published `global` of sixty-four upgrade and hold, the
    /// rest are refused — at accept by the source's sixteen transport slots, retried, then `429` by
    /// the deployment cap — and while the sixty-four serve, all sixteen transport slots are free.
    /// After every client closes, the same holds a second time: no slot was retired by the race.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_upgrades_stay_inside_both_budgets_and_return_them() {
        let (address, limits) = admitted_listener("dep_sec_concurrent_upgrades", 50).await;
        for round in 0..2 {
            let (upgraded, refused) = concurrent_upgrades(address, 100).await;
            assert_eq!(
                (upgraded.len(), refused),
                (global(), 100 - global()),
                "round {round}: exactly the deployment cap of {} upgraded sockets is served \
                 concurrently",
                global()
            );
            assert!(
                transport_budget_returns(&limits).await,
                "round {round}: with {} upgraded sockets serving, a transport slot is still \
                 held: an upgraded socket is counted by both budgets past the upgrade",
                global()
            );
            drop(upgraded);
            assert!(
                transport_budget_returns(&limits).await,
                "round {round}: the transport budget did not return after every client closed"
            );
            assert!(
                deployment_cap_returns(address).await,
                "round {round}: the deployment's event stream cap did not upgrade whole within \
                 {SETTLE:?} of every upgraded client closing"
            );
        }
    }

    /// **The bound a client reads is the bound the route enforces.** The `events.stream-bounds`
    /// fact is read from `/v1/machine` over the same listener, and the route is driven from it: one
    /// subject upgrades `per_subject` streams and the next is refused with the fact's `refusal`.
    /// The presence ties of the other two facts are read from the same snapshot.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_event_bounds_a_client_reads_are_the_ones_the_route_enforces() {
        let (address, _limits) = admitted_listener("dep_sec_fact_drives_route", 1).await;
        let mut stream = TcpStream::connect(address).await.expect("connect");
        stream
            .write_all(
                format!("GET /v1/machine HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .expect("send the machine request");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("read the machine response");
        let response = String::from_utf8(response).expect("UTF-8 response");
        let (_, json) = response.split_once("\r\n\r\n").expect("a response body");
        let machine: serde_json::Value = serde_json::from_str(json).expect("JSON machine body");
        let facts = &machine["result"]["facts"];
        let bounds = &facts["events.stream-bounds"];
        let per_subject = usize::try_from(
            bounds["per_subject"]
                .as_u64()
                .expect("the fact publishes per_subject"),
        )
        .expect("small cap");
        let refusal = bounds["refusal"]
            .as_str()
            .expect("the fact publishes refusal")
            .to_owned();
        assert_eq!(
            facts.get("metrics.stream-bounds").is_some(),
            facts.get("metrics.stream").is_some(),
            "{machine}"
        );
        assert_eq!(
            facts.get("sessions.attachment-bounds").is_some(),
            facts.get("exec.argv-only").is_some(),
            "{machine}"
        );

        let mut open = Vec::new();
        for index in 0..per_subject {
            let (status, head, stream) = admitted_handshake(address, EVENTS).await;
            assert_eq!(
                status, 101,
                "stream {index} is inside the published cap: {head}"
            );
            open.push(stream);
        }
        let (status, head, mut refused) = admitted_handshake(address, EVENTS).await;
        assert_eq!(status, 429, "past the published per_subject: {head}");
        let refused_body = body(&head, &mut refused).await;
        assert!(
            refused_body.contains(&format!("\"{refusal}\"")),
            "the refusal is the one the fact publishes, {refusal}: {refused_body}"
        );
        drop(open);
    }
}
