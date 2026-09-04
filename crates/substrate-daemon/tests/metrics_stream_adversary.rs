#![forbid(unsafe_code)]
//! Adversarial cases against `story:metrics-streams-are-bounded` (unit u1). Test file only; no
//! implementation is changed here.
//!
//! The unit bounded `GET /v1/metrics/stream` with a permit, a frame ceiling and a lifetime, and
//! asserted the frame ceiling *structurally* — `every_websocket_upgrade_declares_its_frame_message
//! _and_lifetime_bounds` (`crates/substrate-daemon/src/app/metrics.rs:806`) reads the source and
//! fails an `on_upgrade` whose builder chain omits `.max_frame_size(`. A scan for the word is not
//! an observation that the bound fires, and nothing in the unit observes the stream's own pacing
//! at all. These cases drive the served route over a real WebSocket handshake instead.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Extension;
use chrono::Utc;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use serde_json::Value;
use substrate_daemon::{App, Identity, router};
use substrate_host::{HostConfig, HostDriver};
use substrate_store::{ExecWrite, NewOperation, Reservation, Scope, Store, StoredExec};
use substrate_wire::{
    ConfinementRequest, Exec, ExecExit, ExecKind, ExecState, ExecUsage, NetworkMode, ResourceUsage,
    SandboxProfile, Workspace, WorkspaceKind, WorkspaceState,
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const SUBJECT: &str = "local:1000";
const DEPLOYMENT: &str = "dep_metrics_adversary";
const WORKSPACE_ID: &str = "ws_metrics_adversary";
const EXEC_ID: &str = "ex_metrics_adversary";
const HANDSHAKE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

fn identity() -> Identity {
    Identity {
        subject: SUBJECT.to_owned(),
        actor: "metrics-adversary".to_owned(),
        principal: None,
    }
}

fn scope() -> Scope {
    Scope {
        deployment: DEPLOYMENT.to_owned(),
        subject: SUBJECT.to_owned(),
    }
}

fn metrics_path() -> String {
    format!("/v1/metrics/stream?exec_id={EXEC_ID}")
}

struct Harness {
    _directory: TempDir,
    address: SocketAddr,
    server: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Harness {
    async fn open() -> Self {
        let directory = tempfile::tempdir().expect("temporary metrics adversary harness");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver");
        let app = App::new(Arc::clone(&store), driver, DEPLOYMENT);
        seed_streaming_exec(&store);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind metrics adversary server");
        let address = listener.local_addr().expect("test server address");
        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    return;
                };
                let service = router(Arc::clone(&app)).layer(Extension(identity()));
                tokio::spawn(async move {
                    let connection = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), TowerToHyperService::new(service))
                        .with_upgrades();
                    let _result = connection.await;
                });
            }
        });
        Self {
            _directory: directory,
            address,
            server,
        }
    }

    async fn connect(&self, path: &str) -> Handshake {
        Handshake::open(self.address, path).await
    }
}

/// The same durable shape the unit's own fixture uses: a terminal exec whose usage observation is
/// not complete, so `load_exec_usage` answers from the store, the driver is never called, and the
/// upgraded socket keeps sampling exactly as it does for a live exec.
#[allow(clippy::too_many_lines)] // One durable fixture; splitting it hides the shape.
fn seed_streaming_exec(store: &Arc<Store>) {
    let workspace = Workspace {
        id: WORKSPACE_ID.to_owned(),
        kind: WorkspaceKind::Workspace,
        labels: BTreeMap::new(),
        observed_at: Utc::now(),
        state: WorkspaceState::Ready,
        storage: None,
        lease: None,
    };
    let workspace_operation = NewOperation {
        scope: scope(),
        operation: "01JADVMETRICSSTREAMSWS".to_owned(),
        operation_kind: "workspace.create".to_owned(),
        request_hash: "a".repeat(64),
        accepted_at: Utc::now().to_rfc3339(),
        capability_snapshot: None,
        actor: "metrics-adversary".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(WORKSPACE_ID.to_owned()),
    };
    let root_name = "root_metrics_adversary";
    assert_eq!(
        store
            .reserve_workspace_create(&workspace_operation, root_name, &workspace, None)
            .expect("reserve workspace fixture"),
        Reservation::Accepted
    );
    store
        .complete_workspace(
            &scope(),
            &workspace_operation.operation,
            &Utc::now().to_rfc3339(),
            201,
            root_name,
            &workspace,
        )
        .expect("complete workspace fixture");

    let exec = Exec {
        id: EXEC_ID.to_owned(),
        kind: ExecKind::Exec,
        workspace: WORKSPACE_ID.to_owned(),
        state: ExecState::Exited,
        observed_at: Utc::now(),
        requested: ConfinementRequest {
            capability_snapshot: "cap_metrics_adversary".to_owned(),
            network: NetworkMode::None,
            aperture: None,
            profile: SandboxProfile::Workspace,
            required: true,
        },
        applied: None,
        exit: Some(ExecExit {
            code: Some(0),
            signal: None,
        }),
        usage: Some(ExecUsage::Observed(ResourceUsage {
            complete: false,
            observed_at: Utc::now(),
            wall_time_us: 1_000,
            cpu_time_us: 500,
            memory_current_bytes: Some(1_024),
            memory_peak_bytes: 2_048,
            processes_current: Some(1),
            processes_peak: 1,
            process_limit_hits: 0,
            memory_oom_kills: 0,
            io_read_bytes: 0,
            io_write_bytes: 0,
            scratch: None,
        })),
        lease: None,
        refusal: None,
    };
    let exec_operation = NewOperation {
        scope: scope(),
        operation: "01JADVMETRICSSTREAMSEX".to_owned(),
        operation_kind: "exec.start".to_owned(),
        request_hash: "b".repeat(64),
        accepted_at: Utc::now().to_rfc3339(),
        capability_snapshot: Some("cap_metrics_adversary".to_owned()),
        actor: "metrics-adversary".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(EXEC_ID.to_owned()),
    };
    let mut provisional = exec.clone();
    provisional.state = ExecState::Accepted;
    provisional.exit = None;
    assert_eq!(
        store
            .reserve_exec_start(
                &exec_operation,
                &StoredExec {
                    resource: provisional,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    stdout_truncated: false,
                    stderr_truncated: false,
                    output_complete: false,
                    cgroup: None,
                    leader_pid: None,
                },
                None,
                None,
            )
            .expect("reserve streaming exec fixture"),
        Reservation::Accepted
    );
    assert!(matches!(
        store
            .complete_exec(
                &scope(),
                &exec_operation.operation,
                &Utc::now().to_rfc3339(),
                201,
                &exec,
                b"",
                &[],
                false,
                false,
                true,
                None,
                None,
            )
            .expect("complete streaming exec fixture"),
        ExecWrite::PersistedExact(_)
    ));
}

struct Handshake {
    status: u16,
    stream: TcpStream,
}

impl Handshake {
    async fn open(address: SocketAddr, path: &str) -> Self {
        let mut stream = TcpStream::connect(address)
            .await
            .expect("connect metrics adversary client");
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write metrics adversary handshake");
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 16 * 1_024, "bounded handshake response");
            let mut byte = [0_u8; 1];
            stream
                .read_exact(&mut byte)
                .await
                .expect("read metrics adversary handshake");
            head.push(byte[0]);
        }
        let head = std::str::from_utf8(&head).expect("ASCII handshake response");
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .expect("HTTP handshake status");
        Self { status, stream }
    }

    fn upgraded(self) -> WebSocketClient {
        assert_eq!(self.status, 101, "metrics stream upgrade must succeed");
        WebSocketClient {
            stream: self.stream,
        }
    }
}

#[derive(Debug)]
struct ServerFrame {
    opcode: u8,
    payload: Vec<u8>,
}

struct WebSocketClient {
    stream: TcpStream,
}

impl WebSocketClient {
    async fn next_frame(&mut self) -> Option<ServerFrame> {
        let mut header = [0_u8; 2];
        if let Err(error) = self.stream.read_exact(&mut header).await {
            assert!(
                matches!(
                    error.kind(),
                    ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset
                ),
                "unexpected websocket read error: {error}"
            );
            return None;
        }
        let opcode = header[0] & 0x0f;
        let masked = header[1] & 0x80 != 0;
        let mut length = u64::from(header[1] & 0x7f);
        if length == 126 {
            let mut bytes = [0_u8; 2];
            self.stream
                .read_exact(&mut bytes)
                .await
                .expect("read medium frame length");
            length = u64::from(u16::from_be_bytes(bytes));
        } else if length == 127 {
            let mut bytes = [0_u8; 8];
            self.stream
                .read_exact(&mut bytes)
                .await
                .expect("read large frame length");
            length = u64::from_be_bytes(bytes);
        }
        let mask = if masked {
            let mut value = [0_u8; 4];
            self.stream
                .read_exact(&mut value)
                .await
                .expect("read server frame mask");
            Some(value)
        } else {
            None
        };
        let mut payload = vec![0_u8; usize::try_from(length).expect("frame length in memory")];
        self.stream
            .read_exact(&mut payload)
            .await
            .expect("read server frame payload");
        if let Some(mask) = mask {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % mask.len()];
            }
        }
        Some(ServerFrame { opcode, payload })
    }

    async fn send_frame(&mut self, fin: bool, opcode: u8, payload: &[u8]) {
        let mut encoded = Vec::with_capacity(payload.len().saturating_add(14));
        encoded.push((if fin { 0x80 } else { 0 }) | opcode);
        match payload.len() {
            length @ 0..=125 => {
                encoded.push(0x80 | u8::try_from(length).expect("short frame length"));
            }
            length @ 126..=65_535 => {
                encoded.push(0x80 | 0x7e);
                encoded.extend_from_slice(
                    &u16::try_from(length)
                        .expect("medium frame length")
                        .to_be_bytes(),
                );
            }
            length => {
                encoded.push(0x80 | 127);
                encoded.extend_from_slice(
                    &u64::try_from(length)
                        .expect("large frame length")
                        .to_be_bytes(),
                );
            }
        }
        let mask = [0x11, 0x22, 0x33, 0x44];
        encoded.extend_from_slice(&mask);
        encoded.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % mask.len()]),
        );
        self.stream
            .write_all(&encoded)
            .await
            .expect("write websocket frame");
    }

    /// The close code the server ended the stream with, or `None` when it ended it by EOF.
    async fn wait_for_end(&mut self) -> Option<u16> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let frame = self.next_frame().await?;
                if frame.opcode == 0x8 {
                    return frame
                        .payload
                        .get(..2)
                        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]));
                }
            }
        })
        .await
        .expect("the bounded metrics stream must end")
    }

    /// The arrival instant of the next `usage` frame the server sends.
    async fn next_usage_sample(&mut self) -> Instant {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(20), self.next_frame())
                .await
                .expect("the metrics stream must keep sampling")
                .expect("the metrics stream must not close before the samples under test");
            if frame.opcode == 0x1 {
                let value: Value =
                    serde_json::from_slice(&frame.payload).expect("metrics stream frame JSON");
                assert_eq!(
                    value["type"], "usage",
                    "the metrics stream publishes only usage frames"
                );
                return Instant::now();
            }
        }
    }
}

/// The route publishes its own sampling cadence, and a client is told to pace on it.
///
/// - `crates/substrate-wire/src/lib.rs:37` — `RESOURCE_USAGE_SAMPLE_INTERVAL_MS = 1_000`, the value
///   `run_stream` itself hands to `tokio::time::interval`
///   (`crates/substrate-daemon/src/app/metrics.rs:161-163`).
/// - `contracts/substrate-wire/0.15.0/operations.json` — the released bundle makes
///   `metrics.stream` callable only where the machine publishes the capability fact
///   `{"latest_wins": true, "replay": false, "sample_interval_ms": 1000}`.
/// - `website/docs/guides/storage-and-metrics.md:135` — "emits one immediate observation, then the
///   latest sample approximately once per second".
/// - `website/docs/guides/run-a-command.md:156` — "sends an immediate latest sample and then
///   samples at the advertised interval".
///
/// `run_stream`'s loop awaits `interval.tick()` twice per sample — once at the head of the body and
/// once again in the `tokio::select!` at its foot — so it consumes two periods for every frame it
/// sends. The ceiling below is one and a half advertised periods: an implementation that spends one
/// period per sample passes it, and one that spends two does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_metrics_stream_samples_at_the_interval_its_contract_advertises() {
    let harness = Harness::open().await;
    let mut client = harness.connect(&metrics_path()).await.upgraded();

    let mut arrivals = Vec::new();
    for _ in 0..3_u8 {
        arrivals.push(client.next_usage_sample().await);
    }

    let advertised = Duration::from_millis(substrate_wire::RESOURCE_USAGE_SAMPLE_INTERVAL_MS);
    let ceiling = advertised.mul_f64(1.5);
    let gaps: Vec<Duration> = arrivals
        .windows(2)
        .map(|pair| pair[1].saturating_duration_since(pair[0]))
        .collect();
    assert!(
        gaps.iter().all(|gap| *gap <= ceiling),
        "the metrics stream advertises a {advertised:?} sample interval and delivered its samples \
         {gaps:?} apart (ceiling {ceiling:?})"
    );
}

/// The per-subject cap this unit published (`MetricsStreamPolicy::production()`,
/// `crates/substrate-daemon/src/app/metrics.rs:45`), and the same number
/// `EventStreamPolicy::production()` publishes.
const SUBJECT_STREAM_LIMIT: usize = 4;

/// How many metrics streams this subject can hold right now: upgrade until one is refused.
async fn subject_capacity(harness: &Harness) -> usize {
    let mut open = Vec::new();
    loop {
        let handshake = harness.connect(&metrics_path()).await;
        if handshake.status != 101 {
            assert_eq!(
                handshake.status, 429,
                "a metrics stream at the cap is refused 429, not {}",
                handshake.status
            );
            return open.len();
        }
        open.push(handshake);
        assert!(
            open.len() <= SUBJECT_STREAM_LIMIT * 4,
            "the per-subject metrics cap did not bind at all"
        );
    }
}

/// Every one of the subject's metrics permits is free again. A permit stranded by whatever the
/// caller did just before shows up here as capacity that never returns. Permits are released when
/// the server-side stream ends, so this retries: what it rejects is a permit that never comes back,
/// not one that comes back a scheduler tick late.
async fn assert_full_subject_capacity(harness: &Harness) {
    let mut observed = 0;
    for _ in 0..40_u8 {
        observed = subject_capacity(harness).await;
        if observed == SUBJECT_STREAM_LIMIT {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "the subject holds {observed} of its {SUBJECT_STREAM_LIMIT} published metrics stream \
         permits; the rest are stranded"
    );
}

/// The unit's frame ceiling, driven rather than scanned.
///
/// `every_websocket_upgrade_declares_its_frame_message_and_lifetime_bounds`
/// (`crates/substrate-daemon/src/app/metrics.rs:806`) asserts that the *characters*
/// `.max_frame_size(` appear within 800 bytes before the `on_upgrade` call. This asserts that the
/// bound fires: a client frame one byte over `max_input_bytes` ends the stream, and the permit that
/// stream held comes back.
#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_client_frame_ends_the_metrics_stream_and_returns_its_permit() {
    let harness = Harness::open().await;
    let mut client = harness.connect(&metrics_path()).await.upgraded();
    client.send_frame(true, 0x2, &[0; 1_025]).await;
    assert!(
        matches!(client.wait_for_end().await, None | Some(1009)),
        "a client frame over the declared ceiling must end the stream"
    );
    drop(client);
    assert_full_subject_capacity(&harness).await;
}

/// The two caps are separate resources, and the unit's own three cases cannot tell.
///
/// `App` holds a second `EventStreamLimits` for metrics (`app/service.rs:168`), and every case the
/// unit added pins the metrics cap to `EventStreamPolicy::production().streams_per_subject` — so a
/// `metrics_stream` wired to `app.event_stream_limits` instead keeps all three of them green while
/// halving what a subject may hold. This is the case that sees the difference.
#[tokio::test(flavor = "multi_thread")]
async fn the_metrics_stream_cap_is_not_shared_with_the_event_stream_cap() {
    let harness = Harness::open().await;

    let mut events = Vec::new();
    for index in 0..SUBJECT_STREAM_LIMIT {
        let handshake = harness.connect("/v1/events/stream?limit=1").await;
        assert_eq!(handshake.status, 101, "event stream {index} must upgrade");
        events.push(handshake);
    }
    assert_eq!(
        harness.connect("/v1/events/stream?limit=1").await.status,
        429,
        "the subject's event stream cap must be full before the metrics cap is read"
    );

    assert_full_subject_capacity(&harness).await;
}

/// The `u3` class, on this route: a permit consumed before an upgrade that never completes.
///
/// The permit is taken in the handler (`app/metrics.rs:125`) and moved into the `on_upgrade`
/// closure, so a client that writes the handshake and vanishes before reading a byte of the
/// response is the case where nothing ever runs `run_stream`. Repeated more times than the cap, a
/// permit that does not come back locks the subject out of the route entirely.
#[tokio::test(flavor = "multi_thread")]
async fn a_handshake_abandoned_before_its_first_sample_returns_its_permit() {
    let harness = Harness::open().await;
    for _ in 0..(SUBJECT_STREAM_LIMIT * 3) {
        let mut stream = TcpStream::connect(harness.address)
            .await
            .expect("connect abandoning client");
        let address = harness.address;
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n",
            metrics_path()
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write abandoned handshake");
        drop(stream);
    }

    assert_full_subject_capacity(&harness).await;
}

/// The cadence is a property of the route, not of what the client happens to send.
///
/// `run_stream` awaits `interval.tick()` in two places per iteration: once at the head of the loop
/// body and once as one arm of the `tokio::select!` at its foot. The second arm is *raced* against
/// `socket.next()`, so a client that sends a control frame cancels that tick and the loop spends
/// one period on that pass instead of two. A silent client therefore gets half the samples a noisy
/// one does, from the same server, over the same exec. An implementation that spends one period per
/// sample gives both clients the same cadence and passes.
#[tokio::test(flavor = "multi_thread")]
async fn the_metrics_sampling_cadence_does_not_depend_on_client_traffic() {
    let harness = Harness::open().await;

    let mut quiet = harness.connect(&metrics_path()).await.upgraded();
    let first = quiet.next_usage_sample().await;
    let second = quiet.next_usage_sample().await;
    let silent_gap = second.saturating_duration_since(first);
    drop(quiet);

    let mut noisy = harness.connect(&metrics_path()).await.upgraded();
    noisy.next_usage_sample().await;
    noisy.send_frame(true, 0x9, &[]).await;
    let first = noisy.next_usage_sample().await;
    noisy.send_frame(true, 0x9, &[]).await;
    let second = noisy.next_usage_sample().await;
    let noisy_gap = second.saturating_duration_since(first);
    drop(noisy);

    let ratio = silent_gap.as_secs_f64() / noisy_gap.as_secs_f64();
    assert!(
        (0.75..=1.25).contains(&ratio),
        "a silent client is sampled every {silent_gap:?} and a client that sends one ping per \
         sample every {noisy_gap:?} (ratio {ratio:.2}); the cadence must not depend on what the \
         client sends"
    );
}

// ---------------------------------------------------------------------------------------------
// Adversary pass 2. Everything below this line is added; nothing above it is changed.
//
// Pass 1's fix for the doubled cadence moved the `tokio::select!` that reads client frames from
// the foot of `run_stream`'s outer loop into an *inner* loop that spins until the next tick
// (`crates/substrate-daemon/src/app/metrics.rs:210-227`). Before that change the outer
// `interval.tick()` admitted at most one client control frame per sample period; now the route
// answers every control frame that arrives while it waits. `EventStreamPolicy` bounds exactly
// this with `max_controls_per_window` / `control_window` (`app/events.rs:36-37`), and
// `MetricsStreamPolicy` — which `app/metrics.rs:27` says holds "the same permit bounds, the same
// client-frame ceiling and the same lifetime" — restates neither.
// ---------------------------------------------------------------------------------------------

/// How the server ended the stream, from the client's side of the socket.
#[derive(Debug, PartialEq, Eq)]
enum StreamEnding {
    /// A close frame carrying this code — the client is told which bound it hit.
    Close(u16),
    /// A close frame with no code.
    ClosedWithoutCode,
    /// The connection was dropped: the client is told nothing at all.
    Eof,
    /// Neither, inside the deadline.
    StillOpen,
}

impl WebSocketClient {
    /// Read until the server ends the stream, counting the pong frames it sends on the way.
    async fn drain_until_end(&mut self, deadline: Duration) -> (StreamEnding, usize) {
        let mut pongs = 0_usize;
        let ending = tokio::time::timeout(deadline, async {
            loop {
                let Some(frame) = self.next_frame().await else {
                    return StreamEnding::Eof;
                };
                match frame.opcode {
                    0xa => pongs += 1,
                    0x8 => {
                        return frame
                            .payload
                            .get(..2)
                            .map_or(StreamEnding::ClosedWithoutCode, |bytes| {
                                StreamEnding::Close(u16::from_be_bytes([bytes[0], bytes[1]]))
                            });
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap_or(StreamEnding::StillOpen);
        (ending, pongs)
    }
}

/// The control-frame budget a client gets on the sibling stream:
/// `EventStreamPolicy::production().max_controls_per_window`
/// (`crates/substrate-daemon/src/app/events.rs:52`) over its `control_window` of one minute,
/// asserted on the served route by `control_frame_flood_closes_1008_and_releases_the_subject_
/// permit` (`crates/substrate-daemon/tests/websocket.rs:354`), which sends exactly one frame more
/// than the budget and requires close `1008`.
const EVENT_STREAM_CONTROL_BUDGET: u32 = 120;

/// The metrics stream answers client control frames without any budget, and after the cadence fix
/// it answers them as fast as they arrive.
///
/// This is `control_frame_flood_closes_1008_and_releases_the_subject_permit`
/// (`crates/substrate-daemon/tests/websocket.rs:354`) pointed at the other bounded stream: the
/// same flood, the same length, the same expected close. `run_stream` matches `Message::Ping` and
/// replies `Message::Pong` inside a loop with no `ControlRate`
/// (`crates/substrate-daemon/src/app/metrics.rs:213-226`), so a client that spends nothing but
/// bandwidth makes the daemon spend a read, a match and a write per frame, on a permit it holds
/// for up to the one-hour lifetime.
///
/// A correct implementation makes this green the way the event stream already does: count control
/// frames against `max_controls_per_window` over `control_window` and close `1008` when the budget
/// is gone.
#[tokio::test(flavor = "multi_thread")]
async fn a_metrics_control_frame_flood_earns_the_close_the_event_stream_gives() {
    let harness = Harness::open().await;
    let mut client = harness.connect(&metrics_path()).await.upgraded();

    let flood = EVENT_STREAM_CONTROL_BUDGET + 1;
    for _ in 0..flood {
        client.send_frame(true, 0x9, &[]).await;
    }
    let started = Instant::now();
    let (ending, pongs) = client.drain_until_end(Duration::from_secs(6)).await;
    let elapsed = started.elapsed();

    assert_eq!(
        ending,
        StreamEnding::Close(1008),
        "the event stream spends a client's {EVENT_STREAM_CONTROL_BUDGET} control frames per \
         window and then closes 1008 (tests/websocket.rs:354); the metrics stream answered \
         {pongs} pongs to {flood} pings in {elapsed:?} and ended as {ending:?}"
    );

    drop(client);
    assert_full_subject_capacity(&harness).await;
}

/// Half the declared client-frame ceiling — a frame no bound rejects.
const IN_BOUNDS_DATA_FRAME: usize = 512;

/// A data frame ends the metrics stream whatever its size, so "the stream ended" is not an
/// observation of the frame ceiling.
///
/// `every_websocket_upgrade_declares_its_frame_message_and_lifetime_bounds`
/// (`crates/substrate-daemon/src/app/metrics.rs:944-949`) names
/// `an_oversized_client_frame_ends_the_metrics_stream_and_returns_its_permit` (line 527 of this
/// file) as the case that "observes one firing" of the ceiling. That case sends 1 025 bytes and
/// asserts the stream ends — but `run_stream`'s `_ => return`
/// (`crates/substrate-daemon/src/app/metrics.rs:222`) ends the stream on *any* data frame, and
/// ends it the same way: by dropping the socket. This case sends a frame at half the ceiling and
/// asks the socket what it was told.
///
/// The sibling stream answers the identical frame with close `1003` and a reason
/// (`crates/substrate-daemon/src/app/events.rs:505-511`, asserted by
/// `data_frame_closes_1003_and_releases_the_subject_permit`,
/// `crates/substrate-daemon/tests/websocket.rs:314`), which both tells the client what it did and
/// makes the two endings distinguishable — so `1009`-or-EOF for the oversized frame then means
/// something. `website/docs/guides/storage-and-metrics.md:139` promises "a client is told when it
/// hits a bound".
#[tokio::test(flavor = "multi_thread")]
async fn a_metrics_data_frame_earns_the_named_close_the_event_stream_gives() {
    let harness = Harness::open().await;
    let mut client = harness.connect(&metrics_path()).await.upgraded();

    client
        .send_frame(true, 0x2, &[0_u8; IN_BOUNDS_DATA_FRAME])
        .await;
    let (ending, _pongs) = client.drain_until_end(Duration::from_secs(6)).await;

    assert_eq!(
        ending,
        StreamEnding::Close(1003),
        "a {IN_BOUNDS_DATA_FRAME} byte data frame is half the declared 1024 byte ceiling, and the \
         metrics stream ended as {ending:?} — the same ending the 1 025 byte frame at line 527 \
         gets, so that case does not observe the ceiling; the event stream answers this frame \
         with close 1003 (tests/websocket.rs:314)"
    );

    drop(client);
    assert_full_subject_capacity(&harness).await;
}

/// The first sample is immediate and the second is a whole interval later.
///
/// `website/docs/guides/storage-and-metrics.md:134` — the route "emits one immediate observation,
/// then the latest sample approximately once per second". The cadence fix spends the interval's
/// immediate first tick *before* the loop
/// (`crates/substrate-daemon/src/app/metrics.rs:179`), which is the only placement that satisfies
/// both halves: spending it inside the loop delays the immediate observation by a full period,
/// and spending it nowhere sends the first two samples back to back.
/// `a_metrics_stream_samples_at_the_interval_its_contract_advertises` measures gaps between
/// samples 1, 2 and 3 and cannot see either mistake at the head of the stream.
#[tokio::test(flavor = "multi_thread")]
async fn the_first_metrics_sample_is_immediate_and_the_second_is_one_interval_later() {
    let harness = Harness::open().await;
    let requested = Instant::now();
    let mut client = harness.connect(&metrics_path()).await.upgraded();
    let first = client.next_usage_sample().await;
    let second = client.next_usage_sample().await;

    let advertised = Duration::from_millis(substrate_wire::RESOURCE_USAGE_SAMPLE_INTERVAL_MS);
    let half = advertised.mul_f64(0.5);
    let to_first = first.saturating_duration_since(requested);
    let gap = second.saturating_duration_since(first);

    assert!(
        to_first < half,
        "the guide promises one immediate observation and the first sample arrived {to_first:?} \
         after the upgrade was requested (ceiling {half:?})"
    );
    assert!(
        gap >= half,
        "the second sample arrived {gap:?} after the first; an immediate first observation must \
         not be followed by a second one inside the advertised {advertised:?} interval"
    );
}
