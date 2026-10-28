//! Adversary pass 1 on `story:host-process-event-stream` (ADR 0036 S12).
//!
//! Each case drives `GET /v1/processes/{process_id}/events` against a statement of ADR 0036 S12,
//! the 0.24.0 bundle or the wire crate's own documentation, or pins an ordering or bound that the
//! unit's own cases leave unobserved. The portable cases run on the faked driver; the delegated
//! ones need a real program and its log, and are absent without the delegated cgroup root.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::http::{Method, StatusCode};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use serde_json::{Value, json};
use substrate_daemon::{Identity, router};
use substrate_store::Store;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::{
    FakeSupervisor, HANDSHAKE_KEY, Harness, MovableAuthority, ProcessSupervisor, SUBJECT, input,
    text,
};

// ---------------------------------------------------------------------------------------------
// A minimal WebSocket client over a loopback listener serving the harness's router.
// ---------------------------------------------------------------------------------------------

struct Served {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(app: &Arc<substrate_daemon::App>) -> Served {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let app = Arc::clone(app);
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let identity = Identity {
                subject: SUBJECT.to_owned(),
                actor: "process-event-adversary".to_owned(),
                principal: None,
            };
            let service = router(Arc::clone(&app)).layer(Extension(identity));
            tokio::spawn(async move {
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), TowerToHyperService::new(service))
                    .with_upgrades()
                    .await;
            });
        }
    });
    Served { address, task }
}

fn handshake(served: &Served, id: &str, cursor: Option<&str>) -> String {
    let query = cursor.map_or_else(String::new, |cursor| format!("?cursor={cursor}"));
    format!(
        "GET /v1/processes/{id}/events{query} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n",
        served.address
    )
}

enum Opened {
    Upgraded(Client),
    Refused(u16, Value),
}

impl Opened {
    fn upgraded(self) -> Client {
        match self {
            Self::Upgraded(client) => client,
            Self::Refused(status, body) => panic!("expected an upgrade, got {status}: {body}"),
        }
    }

    fn refused(self) -> (u16, Value) {
        match self {
            Self::Upgraded(_) => panic!("expected a refusal, the stream was upgraded"),
            Self::Refused(status, body) => (status, body),
        }
    }
}

#[derive(Debug)]
enum Frame {
    Item(Value),
    Close(Option<u16>),
}

struct Client {
    stream: TcpStream,
}

async fn open(served: &Served, id: &str, cursor: Option<&str>) -> Opened {
    let mut stream = TcpStream::connect(served.address).await.expect("connect");
    stream
        .write_all(handshake(served, id, cursor).as_bytes())
        .await
        .expect("handshake");
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 16 * 1_024, "bounded handshake response");
        head.push(stream.read_u8().await.expect("handshake response"));
    }
    let head = String::from_utf8(head).expect("ASCII head");
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .expect("status");
    if status == 101 {
        return Opened::Upgraded(Client { stream });
    }
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .expect("a refusal has a length");
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).await.expect("refusal body");
    Opened::Refused(status, serde_json::from_slice(&body).expect("JSON refusal"))
}

/// Opens a stream, retrying a refusal for up to two seconds: a place returns asynchronously.
async fn open_eventually(served: &Served, id: &str) -> Client {
    let mut last = None;
    for _ in 0..100 {
        match open(served, id, None).await {
            Opened::Upgraded(client) => return client,
            Opened::Refused(status, body) => {
                last = Some((status, body));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
    panic!("no place was returned within two seconds: {last:?}");
}

impl Client {
    async fn frame(&mut self) -> Option<Frame> {
        let mut header = [0_u8; 2];
        self.stream.read_exact(&mut header).await.ok()?;
        let opcode = header[0] & 0x0f;
        let mut length = u64::from(header[1] & 0x7f);
        if length == 126 {
            length = u64::from(self.stream.read_u16().await.ok()?);
        } else if length == 127 {
            length = self.stream.read_u64().await.ok()?;
        }
        let mut payload = vec![0_u8; usize::try_from(length).expect("frame in memory")];
        self.stream.read_exact(&mut payload).await.ok()?;
        match opcode {
            0x1 => Some(Frame::Item(
                serde_json::from_slice(&payload).expect("a JSON item"),
            )),
            0x8 => Some(Frame::Close(
                payload
                    .get(..2)
                    .map(|code| u16::from_be_bytes([code[0], code[1]])),
            )),
            _ => Box::pin(self.frame()).await,
        }
    }

    async fn next(&mut self) -> Frame {
        tokio::time::timeout(Duration::from_secs(20), self.frame())
            .await
            .expect("the stream answers within the bound")
            .unwrap_or(Frame::Close(None))
    }

    async fn drain(&mut self) -> (Vec<Value>, Option<u16>) {
        let mut items = Vec::new();
        loop {
            match self.next().await {
                Frame::Item(item) => items.push(item),
                Frame::Close(code) => return (items, code),
            }
        }
    }

    async fn until(&mut self, wanted: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut items = Vec::new();
        loop {
            match self.next().await {
                Frame::Item(item) => {
                    let done = wanted(&item);
                    items.push(item);
                    if done {
                        return items;
                    }
                }
                Frame::Close(code) => panic!("closed {code:?} before the wanted item: {items:?}"),
            }
        }
    }

    /// One masked client text frame, as RFC 6455 requires of a client (mask key zero).
    async fn send_text(&mut self, payload: &[u8]) {
        assert!(payload.len() < 126, "a short frame");
        let mut frame = vec![0x81, 0x80 | u8::try_from(payload.len()).expect("short")];
        frame.extend_from_slice(&[0, 0, 0, 0]);
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame).await.expect("client frame");
    }
}

fn shapes(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| match text(&item["kind"]) {
            "event" => format!("event:{}", text(&item["event"]["transition"])),
            "line" => format!("line:{}", text(&item["line"]["text"])),
            kind => kind.to_owned(),
        })
        .collect()
}

fn is_line(item: &Value, wanted: &str) -> bool {
    item["kind"] == "line" && item["line"]["text"] == wanted
}

fn sh(script: &str) -> Vec<&str> {
    vec!["/bin/sh", "-c", script]
}

/// Every item a stream delivered is the wire crate's own `ProcessStreamItem`, and every event
/// item a closed event-union variant: what the SDK will decode.
fn assert_wire_items(items: &[Value]) {
    for item in items {
        serde_json::from_value::<substrate_wire::ProcessStreamItem>(item.clone())
            .unwrap_or_else(|error| panic!("not a wire ProcessStreamItem ({error}): {item}"));
        if item["kind"] == "event" {
            let event: substrate_wire::Event =
                serde_json::from_value(item["event"].clone()).expect("a wire event");
            event
                .validate_closed_shape()
                .unwrap_or_else(|error| panic!("not a closed event ({error:?}): {item}"));
        }
    }
}

/// A faked-driver daemon whose event log retains only `retention` events per subject, as
/// `--event-retention` configures it.
fn faked_with_retention(retention: u64) -> Harness {
    let directory = tempfile::tempdir().expect("temporary harness");
    let store = Arc::new(
        Store::open_with_event_retention(directory.path().join("state.db"), retention)
            .expect("state store"),
    );
    let authority = Arc::new(MovableAuthority::default());
    let mut app = Harness::app(&directory, &store, &authority, None);
    let fake = Arc::new(FakeSupervisor::default());
    Arc::get_mut(&mut app)
        .expect("unshared application")
        .host_processes = Some(fake as Arc<dyn ProcessSupervisor>);
    Harness {
        directory,
        store,
        authority,
        app,
        cgroup: None,
    }
}

/// The retained event log's `process.*` transitions of one record.
async fn retained_transitions(harness: &Harness, id: &str) -> Vec<String> {
    let (status, body) = harness
        .call(SUBJECT, Method::GET, "/v1/events?limit=1000", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["result"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|event| event["resource"] == id)
        .map(|event| text(&event["transition"]).to_owned())
        .collect()
}

/// Starts `count` other records, each appending events to the subject's log.
async fn churn_the_event_log(harness: &Harness, cwd: &std::path::Path, count: usize) {
    for index in 0..count {
        harness
            .started(
                &format!("op_adv_churn_start_{index:03}"),
                input(&["/bin/true"], cwd, "s-2", &format!("churn-{index}")),
            )
            .await;
    }
}

// ---------------------------------------------------------------------------------------------
// Portable, on the faked driver.
// ---------------------------------------------------------------------------------------------

/// ADR 0036 S12: "A stream opened on an ended record delivers what the retained log and the event
/// log still hold, the terminal item, and closes." The wire crate's `ProcessEventsQuery` says the
/// same of a stream without a cursor: it "starts with what the event log and the log still hold".
/// A record whose `process.started` the event-log retention already removed is refused
/// `event.retention-gap` instead, and stays refused for as long as it is retained.
#[tokio::test(flavor = "multi_thread")]
async fn adv_ended_record_whose_start_left_event_retention_still_replays_and_closes() {
    let harness = faked_with_retention(4);
    let cwd = harness.workdir("adv-retained-ended");
    let id = harness
        .started(
            "op_adv_retained_start",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    churn_the_event_log(&harness, &cwd, 6).await;
    let (status, body) = harness.stop("op_adv_retained_stop1", &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    harness.ended(&id).await;
    let retained = retained_transitions(&harness, &id).await;
    assert!(
        !retained.iter().any(|t| t == "process.started")
            && retained.contains(&"process.stopped".to_owned()),
        "precondition: the retention removed the start and kept the end: {retained:?}"
    );

    let served = serve(&harness.app).await;
    let (items, close) = match open(&served, &id, None).await {
        Opened::Upgraded(mut client) => client.drain().await,
        Opened::Refused(status, body) => panic!(
            "story:host-process-event-stream: ADR 0036 S12 says a stream of an ended record \
             delivers what the event log still holds and its terminal item; it was refused \
             {status}: {body}"
        ),
    };
    assert_eq!(shapes(&items), ["event:process.stopped", "ended"]);
    assert_eq!(close, Some(1000));
}

/// The stream exists so that a watcher reads it instead of polling (ADR 0036 S12). A running
/// record whose `process.started` left the event-log retention cannot be streamed from its start
/// at all: every new reader is refused `event.retention-gap` (retriable) for the rest of the
/// record's life, though nothing the stream would deliver from now on is lost.
#[tokio::test(flavor = "multi_thread")]
async fn adv_running_record_whose_start_left_event_retention_can_still_be_watched() {
    let harness = faked_with_retention(4);
    let cwd = harness.workdir("adv-retained-live");
    let id = harness
        .started(
            "op_adv_live_start_01",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    churn_the_event_log(&harness, &cwd, 6).await;
    let retained = retained_transitions(&harness, &id).await;
    assert!(
        retained.is_empty(),
        "precondition: the retention removed the start: {retained:?}"
    );

    let served = serve(&harness.app).await;
    let mut watcher = match open(&served, &id, None).await {
        Opened::Upgraded(client) => client,
        Opened::Refused(status, body) => panic!(
            "story:host-process-event-stream: a running record cannot be watched once its \
             start left the event log's retention; refused {status}: {body}"
        ),
    };
    let (status, body) = harness.stop("op_adv_live_stop_0001", &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    harness.ended(&id).await;
    let (items, close) = watcher.drain().await;
    assert_eq!(shapes(&items), ["event:process.stopped", "ended"]);
    assert_eq!(close, Some(1000));
}

/// A cursor whose seq the event-log retention passed is refused before the upgrade with the
/// event log's own code (bundle 0.24.0 compatibility block), never served after it with a 1011.
/// The unit's cases never reach this check in `start_position`.
#[tokio::test(flavor = "multi_thread")]
async fn adv_cursor_older_than_event_retention_is_refused_before_the_upgrade() {
    let harness = faked_with_retention(4);
    let cwd = harness.workdir("adv-retained-cursor");
    let id = harness
        .started(
            "op_adv_cursor_start1",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app).await;
    let mut first = open(&served, &id, None).await.upgraded();
    let delivered = first.until(|item| item["kind"] == "event").await;
    let cursor = text(&delivered[0]["cursor"]).to_owned();
    drop(first);
    churn_the_event_log(&harness, &cwd, 6).await;
    let (status, body) = open(&served, &id, Some(&cursor)).await.refused();
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "event.retention-gap", "{body}");
    assert_eq!(body["error"]["retriable"], true, "{body}");
}

/// The readers bound under churn: a client that sends the handshake and goes away before reading
/// the upgrade answer holds no place. After as many abandoned handshakes as the bound, the bound's
/// worth of readers still opens, and the next is refused.
#[tokio::test(flavor = "multi_thread")]
async fn adv_handshakes_abandoned_before_the_upgrade_return_their_places() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("adv-churn");
    let id = harness
        .started(
            "op_adv_churn_start_1",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app).await;
    for _ in 0..substrate_wire::MAX_PROCESS_EVENT_READERS {
        let mut stream = TcpStream::connect(served.address).await.expect("connect");
        stream
            .write_all(handshake(&served, &id, None).as_bytes())
            .await
            .expect("handshake");
        drop(stream);
    }
    let mut readers = Vec::new();
    for _ in 0..substrate_wire::MAX_PROCESS_EVENT_READERS {
        let mut reader = open_eventually(&served, &id).await;
        let first = reader.until(|item| item["kind"] == "event").await;
        assert_eq!(shapes(&first), ["event:process.started"]);
        readers.push(reader);
    }
    let (status, body) = open(&served, &id, None).await.refused();
    assert_eq!(status, 429, "{body}");
    assert_eq!(body["error"]["code"], "process.events-readers-exceeded");
}

/// A data frame from the reader closes the stream 1003, without an `ended` item: the stream
/// accepts control frames only. None of the unit's cases sends one.
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_data_frame_from_the_reader_closes_1003() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("adv-data-frame");
    let id = harness
        .started(
            "op_adv_data_start_01",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app).await;
    let mut reader = open(&served, &id, None).await.upgraded();
    let first = reader.until(|item| item["kind"] == "event").await;
    assert_eq!(shapes(&first), ["event:process.started"]);
    reader.send_text(b"hello").await;
    let (items, close) = reader.drain().await;
    assert!(items.is_empty(), "{items:?}");
    assert_eq!(close, Some(1003));
}

// ---------------------------------------------------------------------------------------------
// Delegated: real programs and their logs.
// ---------------------------------------------------------------------------------------------

/// A renewal decided between two lines is placed between them, and the cursor of the line before
/// it resumes before the renewal: the cursor's seq stops one short of an event the stream still
/// holds. Two readers of the one record receive the same items. Every item is the wire crate's
/// own type.
#[tokio::test(flavor = "multi_thread")]
async fn adv_renewal_between_two_lines_is_placed_and_resumed_between_them() {
    let Some(harness) = Harness::delegated("adv-renewal-placed") else {
        return;
    };
    let cwd = harness.workdir("adv-renewal-placed");
    let id = harness
        .started(
            "op_adv_placed_start1",
            input(
                &sh("echo a; while [ ! -e go ]; do sleep 0.05; done; echo b"),
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    harness.log_line(&id, "a").await;
    let (status, renewed) = harness
        .renew("op_adv_placed_renew1", &id, json!({ "ttl_ms": 60_000 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    std::fs::write(cwd.join("go"), b"").expect("release the program");
    harness.ended(&id).await;

    let served = serve(&harness.app).await;
    let mut one = open(&served, &id, None).await.upgraded();
    let mut two = open(&served, &id, None).await.upgraded();
    let (items, close) = one.drain().await;
    let (again, _) = two.drain().await;
    assert_eq!(
        shapes(&items),
        [
            "event:process.started",
            "line:a",
            "event:process.lease-renewed",
            "line:b",
            "event:process.exited",
            "ended"
        ]
    );
    assert_eq!(close, Some(1000));
    assert_eq!(
        items, again,
        "two readers of one record receive the same items"
    );
    assert_wire_items(&items);

    let after_a = text(&items[1]["cursor"]).to_owned();
    let (resumed, _) = open(&served, &id, Some(&after_a))
        .await
        .upgraded()
        .drain()
        .await;
    assert_eq!(
        shapes(&resumed),
        [
            "event:process.lease-renewed",
            "line:b",
            "event:process.exited",
            "ended"
        ],
        "the cursor of the line before a held event resumes before that event"
    );
    let after_renewal = text(&items[2]["cursor"]).to_owned();
    let (resumed, _) = open(&served, &id, Some(&after_renewal))
        .await
        .upgraded()
        .drain()
        .await;
    assert_eq!(
        shapes(&resumed),
        ["line:b", "event:process.exited", "ended"]
    );
    harness.finish().await;
    eprintln!("adversary pass1 renewal placed: executed");
}

/// `process.stopped` follows every line of the log, and a lease expiry is placed where the log
/// stood when the clock decided it: after the line the program wrote before it.
#[tokio::test(flavor = "multi_thread")]
async fn adv_stop_and_lease_expiry_are_placed_after_the_lines_before_them() {
    let Some(harness) = Harness::delegated("adv-stop-placed") else {
        return;
    };
    let cwd = harness.workdir("adv-stop-placed");
    let stopped = harness
        .started(
            "op_adv_stopped_start",
            input(&sh("echo up; exec /bin/sleep 300"), &cwd, "s-1", "stopped"),
        )
        .await;
    harness.log_line(&stopped, "up").await;
    let (status, body) = harness.stop("op_adv_stopped_stop01", &stopped).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    harness.ended(&stopped).await;

    let expired = harness
        .started(
            "op_adv_expired_start",
            input(
                &sh("echo before; exec /bin/sleep 300"),
                &cwd,
                "s-1",
                "expired",
            ),
        )
        .await;
    harness.log_line(&expired, "before").await;
    harness.authority.advance(61_000);
    let record = harness.ended(&expired).await;
    assert_eq!(record["ended_by"], "lease-expiry");

    let served = serve(&harness.app).await;
    let (items, close) = open(&served, &stopped, None).await.upgraded().drain().await;
    assert_eq!(
        shapes(&items),
        [
            "event:process.started",
            "line:up",
            "event:process.stopped",
            "ended"
        ]
    );
    assert_eq!(close, Some(1000));
    let (items, close) = open(&served, &expired, None).await.upgraded().drain().await;
    assert_eq!(
        shapes(&items),
        [
            "event:process.started",
            "line:before",
            "event:process.lease-expired",
            "event:process.stopped",
            "ended"
        ]
    );
    assert_eq!(close, Some(1000));
    assert_wire_items(&items);
    harness.finish().await;
    eprintln!("adversary pass1 stop and expiry placed: executed");
}

/// The line bound at its edges: a line of exactly `MAX_PROCESS_EVENT_LINE_BYTES` arrives whole
/// and not truncated, one byte more arrives cut with its full length, and a last line without a
/// newline arrives as written.
#[tokio::test(flavor = "multi_thread")]
async fn adv_line_of_exactly_the_bound_is_whole_and_one_byte_more_is_cut() {
    let Some(harness) = Harness::delegated("adv-line-bound") else {
        return;
    };
    let cwd = harness.workdir("adv-line-bound");
    let id = harness
        .started(
            "op_adv_bound_start01",
            input(
                &sh("head -c 16384 /dev/zero | tr '\\000' x; echo; \
                     head -c 16385 /dev/zero | tr '\\000' y; echo; printf tail"),
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    harness.ended(&id).await;
    let served = serve(&harness.app).await;
    let (items, close) = open(&served, &id, None).await.upgraded().drain().await;
    assert_eq!(close, Some(1000));
    assert_eq!(items.len(), 6, "{:?}", shapes(&items));
    let bound = substrate_wire::MAX_PROCESS_EVENT_LINE_BYTES;
    let exact = &items[1]["line"];
    assert_eq!(text(&exact["text"]), "x".repeat(bound));
    assert_eq!(exact["truncated"], false);
    assert_eq!(exact["full_bytes"], bound);
    let over = &items[2]["line"];
    assert_eq!(text(&over["text"]), "y".repeat(bound));
    assert_eq!(over["truncated"], true);
    assert_eq!(over["full_bytes"], bound + 1);
    let last = &items[3]["line"];
    assert_eq!(last["text"], "tail");
    assert_eq!(last["truncated"], false);
    assert_eq!(last["full_bytes"], 4);
    assert_eq!(shapes(&items[4..]), ["event:process.exited", "ended"]);
    harness.finish().await;
    eprintln!("adversary pass1 line bound: executed");
}

/// A daemon restart between two streams: the cursor the earlier run issued resumes against the
/// log the new run reads from the state directory.
#[tokio::test(flavor = "multi_thread")]
async fn adv_cursor_resumes_across_a_daemon_restart() {
    let Some(mut harness) = Harness::delegated("adv-restart") else {
        return;
    };
    let cwd = harness.workdir("adv-restart");
    let id = harness
        .started(
            "op_adv_restart_start",
            input(
                &sh("echo a; while [ ! -e go ]; do sleep 0.05; done; echo b"),
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    let served = serve(&harness.app).await;
    let mut first = open(&served, &id, None).await.upgraded();
    let delivered = first.until(|item| is_line(item, "a")).await;
    let cursor = text(&delivered.last().expect("line a")["cursor"]).to_owned();
    drop(first);
    drop(served);
    std::fs::write(cwd.join("go"), b"").expect("release the program");
    harness.ended(&id).await;
    harness.restart();
    let served = serve(&harness.app).await;
    let (items, close) = open(&served, &id, Some(&cursor))
        .await
        .upgraded()
        .drain()
        .await;
    assert_eq!(shapes(&items), ["line:b", "event:process.exited", "ended"]);
    assert_eq!(close, Some(1000));
    harness.finish().await;
    eprintln!("adversary pass1 restart resume: executed");
}

/// A cursor whose log offset the bound discarded since it was issued resumes with one `gap` item
/// stating exactly the bytes discarded *after that offset*, at the oldest retained byte (ADR 0036
/// S12: "A reader resuming from a log offset the log bound has already discarded receives one
/// `gap` item stating the discarded byte count"). The unit's gap case starts at offset 0, where
/// the discarded count after the offset and the discarded count since the log began are equal.
#[tokio::test(flavor = "multi_thread")]
async fn adv_resume_into_discarded_bytes_states_the_bytes_after_the_cursor() {
    let Some(harness) = Harness::delegated("adv-resume-gap") else {
        return;
    };
    let cwd = harness.workdir("adv-resume-gap");
    let id = harness
        .started(
            "op_adv_resumegap_st1",
            input(
                &sh("echo a; while [ ! -e go ]; do sleep 0.05; done; \
                     head -c 9437184 /dev/zero | tr '\\000' a | fold -w 1023; echo; echo last"),
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    let served = serve(&harness.app).await;
    let mut first = open(&served, &id, None).await.upgraded();
    let delivered = first.until(|item| is_line(item, "a")).await;
    let cursor = text(&delivered.last().expect("line a")["cursor"]).to_owned();
    drop(first);
    let offset_of = |cursor: &str| -> u64 {
        cursor
            .rsplit('.')
            .next()
            .and_then(|offset| offset.parse().ok())
            .expect("cursor offset")
    };
    let after_a = offset_of(&cursor);
    std::fs::write(cwd.join("go"), b"").expect("release the program");
    harness.ended(&id).await;
    let discarded = harness.logs(&id, "").await["discarded_bytes"]
        .as_u64()
        .expect("discarded");
    assert!(discarded > after_a, "precondition: line a was discarded");

    let (items, close) = open(&served, &id, Some(&cursor))
        .await
        .upgraded()
        .drain()
        .await;
    assert_eq!(close, Some(1000));
    let shapes = shapes(&items);
    assert_eq!(shapes[0], "gap", "{:?}", &shapes[..3]);
    assert_eq!(
        items[0]["gap"]["discarded_bytes"],
        discarded - after_a,
        "the gap states the bytes discarded after the cursor"
    );
    assert_eq!(offset_of(text(&items[0]["cursor"])), discarded);
    assert_eq!(
        shapes[shapes.len() - 3..],
        ["line:last", "event:process.exited", "ended"]
    );
    harness.finish().await;
    eprintln!("adversary pass1 resume into gap: executed");
}

// ---------------------------------------------------------------------------------------------
// A driver whose start the case holds open, behind the same port.
// ---------------------------------------------------------------------------------------------

/// A host-process driver whose program writes one line, `early`, the moment it is spawned, and
/// whose `start` then returns only when the case releases it: the window the real driver has
/// between spawning the program (its log copiers run from then on) and the daemon marking the
/// record running, held open long enough for a reader to attach.
#[derive(Default)]
struct GatedSupervisor {
    log: parking_lot::Mutex<Vec<(substrate_host::LogLine, u64)>>,
    released: (std::sync::Mutex<bool>, std::sync::Condvar),
    stops: parking_lot::Mutex<Vec<String>>,
}

impl GatedSupervisor {
    fn release(&self) {
        let (lock, condvar) = &self.released;
        *lock.lock().expect("gate") = true;
        condvar.notify_all();
    }

    fn end(&self) -> u64 {
        self.log.lock().last().map_or(0, |(_, end)| *end)
    }
}

impl substrate_host::ProcessLogs for GatedSupervisor {
    fn log_tail(
        &self,
        _id: &str,
        _selection: substrate_host::LogStreamSelection,
        _max_bytes: u64,
    ) -> Result<substrate_host::host_process::LogTail, substrate_host::DriverError> {
        Ok(substrate_host::host_process::LogTail {
            lines: Vec::new(),
            discarded_bytes: 0,
            retained_bytes: 0,
            profile: substrate_host::host_process::HostProfile::Host,
            confinement: substrate_host::host_process::Confinement::None,
        })
    }

    /// The S7 log's forward read: whole records after `from`, which must be a record boundary.
    fn log_read(
        &self,
        _id: &str,
        from: u64,
        _max_bytes: u64,
    ) -> Result<Option<substrate_host::LogRead>, substrate_host::DriverError> {
        let log = self.log.lock();
        let end = log.last().map_or(0, |(_, end)| *end);
        if from > end || (from != 0 && !log.iter().any(|(_, at)| *at == from)) {
            return Ok(None);
        }
        Ok(Some(substrate_host::LogRead {
            gap: 0,
            lines: log.iter().filter(|(_, at)| *at > from).cloned().collect(),
            end,
        }))
    }

    fn log_end(&self, _id: &str) -> Result<u64, substrate_host::DriverError> {
        Ok(self.end())
    }
}

impl ProcessSupervisor for GatedSupervisor {
    fn start(
        self: Arc<Self>,
        id: &str,
        spec: &substrate_host::HostProcessSpec,
    ) -> Result<substrate_host::host_process::HostProcessStarted, substrate_host::DriverError> {
        // `o early\n`: the record the S7 log stores for the line, eight bytes.
        self.log.lock().push((
            substrate_host::LogLine {
                stream: substrate_host::LogStream::Stdout,
                bytes: b"early".to_vec(),
                truncated: false,
                full_len: 5,
            },
            8,
        ));
        let (lock, condvar) = &self.released;
        let released = lock.lock().expect("gate");
        let _released = condvar
            .wait_timeout_while(released, Duration::from_secs(10), |released| !*released)
            .expect("gate");
        Ok(substrate_host::host_process::HostProcessStarted {
            id: id.to_owned(),
            pid: 1,
            cgroup: format!("gated-{id}"),
            started_at: chrono::Utc::now(),
            deadline: spec.deadline,
            profile: substrate_host::host_process::HostProfile::Host,
            confinement: substrate_host::host_process::Confinement::None,
        })
    }

    fn reattach(
        self: Arc<Self>,
        _id: &str,
    ) -> Result<substrate_host::Reattached, substrate_host::DriverError> {
        Ok(substrate_host::Reattached::Live)
    }

    fn begin_stop(
        &self,
        id: &str,
        _cause: substrate_host::StopCause,
    ) -> Result<(), substrate_host::DriverError> {
        self.stops.lock().push(id.to_owned());
        Ok(())
    }

    fn ended(
        &self,
        id: &str,
    ) -> Result<Option<substrate_host::HostProcessEnd>, substrate_host::DriverError> {
        Ok(self
            .stops
            .lock()
            .iter()
            .any(|stopped| stopped == id)
            .then(|| substrate_host::HostProcessEnd {
                ended_by: substrate_host::host_process::EndedBy::Stop,
                exit: Some(substrate_host::ProcessExit::Signal(15)),
                stop_step: Some(substrate_host::host_process::StopStep::Term),
                ended_at: chrono::Utc::now(),
            }))
    }

    fn observe(
        &self,
        _id: &str,
    ) -> Result<substrate_host::HostProcessObservation, substrate_host::DriverError> {
        Ok(substrate_host::HostProcessObservation {
            observed_at: chrono::Utc::now(),
            cpu_usec: None,
            memory_bytes: None,
            listening: None,
            profile: substrate_host::host_process::HostProfile::Host,
            confinement: substrate_host::host_process::Confinement::None,
        })
    }

    fn forget(&self, _id: &str) -> Result<(), substrate_host::DriverError> {
        Ok(())
    }
}

/// The unit's own specification: "`process.started` precedes every line"
/// (`spec/host-processes/README.md`, S12 *Order*). A reader that attaches while the record is
/// still `starting` (it is listed by `GET /v1/processes?state=starting`) receives the program's
/// first line before `process.started`, because the stream sends lines as soon as the log holds
/// them and the event is only appended when the daemon marks the record running.
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_reader_attached_while_starting_receives_process_started_first() {
    let mut harness = Harness::portable();
    let gated = Arc::new(GatedSupervisor::default());
    Arc::get_mut(&mut harness.app)
        .expect("unshared application")
        .host_processes = Some(Arc::clone(&gated) as Arc<dyn ProcessSupervisor>);
    let harness = Arc::new(harness);
    let cwd = harness.workdir("adv-early");
    let starter = {
        let harness = Arc::clone(&harness);
        let cwd = cwd.clone();
        tokio::spawn(async move {
            harness
                .started(
                    "op_adv_early_start01",
                    input(&["/bin/true"], &cwd, "s-1", "dev"),
                )
                .await
        })
    };
    let mut id = None;
    for _ in 0..200 {
        let list = harness.list("?state=starting").await;
        if let Some(item) = list["items"].as_array().and_then(|items| items.first()) {
            id = Some(text(&item["id"]).to_owned());
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let Some(id) = id else {
        gated.release();
        panic!("no starting record was listed");
    };
    let served = serve(&harness.app).await;
    let mut reader = open(&served, &id, None).await.upgraded();
    // One stream round while the record is still starting and its log holds the line.
    tokio::time::sleep(Duration::from_millis(300)).await;
    gated.release();
    assert_eq!(starter.await.expect("start task"), id);
    let mut items = Vec::new();
    while !items.iter().any(|item: &Value| item["kind"] == "event")
        || !items.iter().any(|item| is_line(item, "early"))
    {
        match reader.next().await {
            Frame::Item(item) => items.push(item),
            Frame::Close(code) => panic!("closed {code:?}: {items:?}"),
        }
    }
    assert_eq!(
        shapes(&items),
        ["event:process.started", "line:early"],
        "story:host-process-event-stream: process.started precedes every line"
    );
}
