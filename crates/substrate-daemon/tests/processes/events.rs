//! ADR 0036 S12: a host process's lifecycle in the global event log, and its per-process event
//! stream, `GET /v1/processes/{process_id}/events`.
//!
//! Each case is named for the conformance scenario of `spec/host-processes/` that
//! `story:host-process-event-stream` names as its acceptance. The global-log, readers-bound and
//! cursor cases run portable on the faked driver; the cases about log lines need a real program
//! and run on the delegated lane, absent without it.
//!
//! The stream is a WebSocket, which needs a served connection rather than a router call: each
//! case serves the harness's router on a loopback listener and drives it with a minimal client.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::http::{Method, StatusCode};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use serde_json::{Value, json};
use substrate_daemon::{App, Identity, router};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::{FOREIGN, HANDSHAKE_KEY, Harness, SUBJECT, input, text, with};

/// The harness's router on a loopback listener, serving every connection as `subject`.
struct Served {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(app: &Arc<App>, subject: &str) -> Served {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let app = Arc::clone(app);
    let subject = subject.to_owned();
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let identity = Identity {
                subject: subject.clone(),
                actor: "process-event-test".to_owned(),
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

/// What a stream handshake was answered with.
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

/// One frame the daemon sent.
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
    let query = cursor.map_or_else(String::new, |cursor| format!("?cursor={cursor}"));
    let request = format!(
        "GET /v1/processes/{id}/events{query} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n",
        served.address
    );
    stream
        .write_all(request.as_bytes())
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

    /// Every item up to the close, and the close status.
    async fn drain(&mut self) -> (Vec<Value>, Option<u16>) {
        let mut items = Vec::new();
        loop {
            match self.next().await {
                Frame::Item(item) => items.push(item),
                Frame::Close(code) => return (items, code),
            }
        }
    }

    /// One masked, empty client ping, as RFC 6455 requires of a client (mask key zero).
    async fn ping(&mut self) {
        self.stream
            .write_all(&[0x89, 0x80, 0, 0, 0, 0])
            .await
            .expect("client ping");
    }

    /// Items until one satisfies `wanted`, which is returned last.
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
}

/// The items as one readable shape each: `event:<transition>`, `line:<text>`, `gap`, `ended`.
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

async fn process_events_of_the_log(harness: &Harness) -> Vec<Value> {
    let (status, body) = harness
        .call(SUBJECT, Method::GET, "/v1/events?limit=1000", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["result"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|event| event["resource_kind"] == "process")
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Portable, on the faked driver.
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn host_process_lifecycle_appears_in_the_global_event_log() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("lifecycle");
    let stopped = harness
        .started(
            "op_lifecycle_start_1",
            input(&["/bin/true"], &cwd, "s-1", "a"),
        )
        .await;
    let (status, renewed) = harness
        .renew(
            "op_lifecycle_renew_1",
            &stopped,
            json!({ "ttl_ms": 60_000 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    let (status, body) = harness.stop("op_lifecycle_stop_01", &stopped).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    harness.ended(&stopped).await;
    let expired = harness
        .started(
            "op_lifecycle_start_2",
            input(&["/bin/true"], &cwd, "s-1", "b"),
        )
        .await;
    harness.authority.advance(61_000);
    harness.ended(&expired).await;

    let events = process_events_of_the_log(&harness).await;
    let of = |id: &str| -> Vec<(String, String)> {
        events
            .iter()
            .filter(|event| event["resource"] == id)
            .map(|event| {
                (
                    text(&event["transition"]).to_owned(),
                    text(&event["cause"]["operation"]).to_owned(),
                )
            })
            .collect()
    };
    assert_eq!(
        of(&stopped),
        [
            (
                "process.started".to_owned(),
                "op_lifecycle_start_1".to_owned()
            ),
            (
                "process.lease-renewed".to_owned(),
                "op_lifecycle_renew_1".to_owned()
            ),
            (
                "process.stopped".to_owned(),
                "op_lifecycle_stop_01".to_owned()
            ),
        ]
    );
    let expiry = of(&expired);
    let transitions: Vec<&str> = expiry.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        transitions,
        [
            "process.started",
            "process.lease-expired",
            "process.stopped"
        ]
    );
    assert_eq!(expiry[1].1, expiry[2].1, "the end names the expiry's stop");
    for event in &events {
        assert_eq!(
            event["observation"]["process"], event["resource"],
            "{event}"
        );
    }
    let last = events.last().expect("an end event");
    assert_eq!(last["observation"]["state"], "ended");
    assert_eq!(last["observation"]["ended_by"], "lease-expiry");
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stream_readers_bound_is_refused() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("readers");
    let id = harness
        .started(
            "op_readers_start_01",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app, SUBJECT).await;
    let mut readers = Vec::new();
    for _ in 0..substrate_wire::MAX_PROCESS_EVENT_READERS {
        let mut reader = open(&served, &id, None).await.upgraded();
        let first = reader.until(|item| item["kind"] == "event").await;
        assert_eq!(shapes(&first), ["event:process.started"]);
        readers.push(reader);
    }
    let (status, body) = open(&served, &id, None).await.refused();
    assert_eq!(status, 429, "{body}");
    assert_eq!(body["error"]["code"], "process.events-readers-exceeded");
    assert_eq!(body["error"]["retriable"], true, "{body}");
    // A closed reader returns its place.
    drop(readers.pop());
    let mut again = None;
    for _ in 0..100 {
        match open(&served, &id, None).await {
            Opened::Upgraded(client) => {
                again = Some(client);
                break;
            }
            Opened::Refused(..) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    assert!(again.is_some(), "the place of a closed reader is returned");
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stream_cursor_it_cannot_honour_is_refused() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("cursor");
    let id = harness
        .started(
            "op_cursor_start_001",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app, SUBJECT).await;
    let mut reader = open(&served, &id, None).await.upgraded();
    let first = reader.until(|item| item["kind"] == "event").await;
    let cursor = text(&first[0]["cursor"]).to_owned();
    let parts: Vec<&str> = cursor.split('.').collect();
    assert_eq!(parts.len(), 6, "{cursor}");
    assert_eq!(parts[1], id, "the cursor names its record: {cursor}");
    let (source, generation, seq) = (parts[2], parts[3], parts[4]);
    let refused = |status: u16, body: &Value, code: &str| {
        assert_eq!(status, 409, "{code}: {body}");
        assert_eq!(body["error"]["code"], code, "{body}");
    };
    let cases = [
        ("ev2.scope_a.1.0".to_owned(), "event.cursor-invalid"),
        (
            format!("pe1.{id}.{source}.{generation}.{seq}"),
            "event.cursor-invalid",
        ),
        (
            format!("pe1.{source}.{generation}.{seq}.0"),
            "event.cursor-invalid",
        ),
        (
            format!("pe1.proc_01OTHERRECORD.{source}.{generation}.{seq}.0"),
            "event.cursor-invalid",
        ),
        (
            format!("pe1.{id}.scope_other.{generation}.{seq}.0"),
            "event.source-mismatch",
        ),
        (
            format!("pe1.{id}.{source}.{generation}.999999.0"),
            "event.cursor-invalid",
        ),
        (
            format!("pe1.{id}.{source}.{generation}.{seq}.5"),
            "event.cursor-invalid",
        ),
    ];
    for (bad, code) in cases {
        let (status, body) = open(&served, &id, Some(&bad)).await.refused();
        refused(status, &body, code);
    }
    // The cursor it was given resumes after the item that carried it.
    let mut resumed = open(&served, &id, Some(&cursor)).await.upgraded();
    let (_, foreign) = open(&serve(&harness.app, FOREIGN).await, &id, None)
        .await
        .refused();
    assert_eq!(foreign["error"]["code"], "process.host-subject-mismatch");
    let (status, absent) = open(&served, "proc_absent", None).await.refused();
    assert_eq!(status, 404, "{absent}");
    assert_eq!(absent["error"]["code"], "process.not-found");
    let (status, stopped) = harness.stop("op_cursor_stop_0001", &id).await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    harness.ended(&id).await;
    let (items, close) = resumed.drain().await;
    assert_eq!(shapes(&items), ["event:process.stopped", "ended"]);
    assert_eq!(close, Some(1000));
}

// ---------------------------------------------------------------------------------------------
// Delegated: real programs and their logs.
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn host_process_output_lines_never_enter_the_global_event_log() {
    let Some(harness) = Harness::delegated("ev-lines-not-logged") else {
        return;
    };
    let cwd = harness.workdir("not-logged");
    // The program assembles the marker, so the argv the start records never spells it.
    let marker = "marker-out-of-band-3f9c";
    let id = harness
        .started(
            "op_notlogged_start1",
            input(
                &sh("printf '%s-%s\\n' marker-out-of band-3f9c; printf '%s-%s\\n' marker-out-of band-3f9c >&2"),
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    harness.ended(&id).await;
    assert!(harness.log_line(&id, marker).await.contains(marker));
    let events = process_events_of_the_log(&harness).await;
    let transitions: Vec<&str> = events
        .iter()
        .map(|event| text(&event["transition"]))
        .collect();
    assert_eq!(transitions, ["process.started", "process.exited"]);
    let (_, all) = harness
        .call(SUBJECT, Method::GET, "/v1/events?limit=1000", None)
        .await;
    assert!(!all.to_string().contains(marker), "{all}");
    let durable = harness.durable_bytes();
    assert!(
        !durable
            .windows(marker.len())
            .any(|window| window == marker.as_bytes()),
        "an output line reached the durable state"
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stream_interleaves_events_and_lines_in_order() {
    let Some(harness) = Harness::delegated("ev-interleave") else {
        return;
    };
    let cwd = harness.workdir("interleave");
    let id = harness
        .started(
            "op_interleave_start",
            input(
                &sh("echo before; while [ ! -e go ]; do sleep 0.05; done; echo after; exit 3"),
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    let served = serve(&harness.app, SUBJECT).await;
    let mut reader = open(&served, &id, None).await.upgraded();
    let live = reader.until(|item| is_line(item, "before")).await;
    assert_eq!(shapes(&live), ["event:process.started", "line:before"]);
    let (status, renewed) = harness
        .renew("op_interleave_renew", &id, json!({ "ttl_ms": 60_000 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    std::fs::write(cwd.join("go"), b"").expect("release the program");
    harness.ended(&id).await;
    let (rest, close) = reader.drain().await;
    assert_eq!(
        shapes(&rest),
        [
            "event:process.lease-renewed",
            "line:after",
            "event:process.exited",
            "ended"
        ]
    );
    assert_eq!(close, Some(1000));
    assert_eq!(
        rest[2]["event"]["observation"]["exit"],
        json!({ "code": 3 })
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stream_ends_with_exactly_one_terminal_item() {
    let Some(harness) = Harness::delegated("ev-one-terminal") else {
        return;
    };
    let cwd = harness.workdir("one-terminal");
    let id = harness
        .started(
            "op_oneterminal_strt",
            input(&sh("echo one; echo two"), &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app, SUBJECT).await;
    let mut reader = open(&served, &id, None).await.upgraded();
    harness.ended(&id).await;
    let (items, close) = reader.drain().await;
    assert_eq!(
        shapes(&items),
        [
            "event:process.started",
            "line:one",
            "line:two",
            "event:process.exited",
            "ended"
        ]
    );
    assert_eq!(
        items.iter().filter(|item| item["kind"] == "ended").count(),
        1
    );
    assert_eq!(close, Some(1000));
    let after = tokio::time::timeout(Duration::from_secs(5), reader.frame())
        .await
        .expect("the daemon ends the connection after its close");
    assert!(
        after.is_none(),
        "nothing follows the close of a complete stream: {after:?}"
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stream_terminal_item_names_ended_by() {
    let Some(harness) = Harness::delegated("ev-ended-by") else {
        return;
    };
    let cwd = harness.workdir("ended-by");
    let id = harness
        .started(
            "op_endedby_start_01",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app, SUBJECT).await;
    let mut reader = open(&served, &id, None).await.upgraded();
    let (status, body) = harness.stop("op_endedby_stop_001", &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let record = harness.ended(&id).await;
    let (items, close) = reader.drain().await;
    assert_eq!(
        shapes(&items),
        ["event:process.started", "event:process.stopped", "ended"]
    );
    assert_eq!(
        items[1]["event"]["cause"]["operation"],
        "op_endedby_stop_001"
    );
    let ended = &items[2]["ended"];
    assert_eq!(ended["ended_by"], "stop");
    assert_eq!(ended["ended_by"], record["ended_by"]);
    assert_eq!(ended["exit"], record["exit"]);
    assert!(ended["exit"]["signal"].is_string(), "{ended}");
    assert_eq!(close, Some(1000));
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stream_resumes_from_its_cursor() {
    let Some(harness) = Harness::delegated("ev-resume") else {
        return;
    };
    let cwd = harness.workdir("resume");
    let id = harness
        .started(
            "op_resume_start_001",
            input(
                &sh("echo a; while [ ! -e go ]; do sleep 0.05; done; echo b"),
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    let served = serve(&harness.app, SUBJECT).await;
    let mut first = open(&served, &id, None).await.upgraded();
    let delivered = first.until(|item| is_line(item, "a")).await;
    let cursor = text(&delivered.last().expect("line a")["cursor"]).to_owned();
    drop(first);
    std::fs::write(cwd.join("go"), b"").expect("release the program");
    harness.ended(&id).await;
    let mut resumed = open(&served, &id, Some(&cursor)).await.upgraded();
    let (items, close) = resumed.drain().await;
    assert_eq!(
        shapes(&items),
        ["line:b", "event:process.exited", "ended"],
        "nothing delivered before the cursor is repeated"
    );
    assert_eq!(close, Some(1000));
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stream_reports_a_gap_for_discarded_log_bytes() {
    let Some(harness) = Harness::delegated("ev-gap") else {
        return;
    };
    let cwd = harness.workdir("gap");
    // Nine MiB of 1023-byte lines through a log bounded at eight.
    let id = harness
        .started(
            "op_gap_start_000001",
            input(
                &sh("head -c 9437184 /dev/zero | tr '\\000' a | fold -w 1023; echo; echo last"),
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    harness.ended(&id).await;
    let tail = harness.logs(&id, "").await;
    let discarded = tail["discarded_bytes"].as_u64().expect("discarded");
    assert!(discarded > 0, "{}", tail["discarded_bytes"]);
    let served = serve(&harness.app, SUBJECT).await;
    let mut reader = open(&served, &id, None).await.upgraded();
    let (items, close) = reader.drain().await;
    assert_eq!(close, Some(1000));
    let shapes = shapes(&items);
    assert_eq!(shapes[0], "event:process.started");
    assert_eq!(
        shapes[1], "gap",
        "the discarded bytes are stated, never skipped"
    );
    assert_eq!(items[1]["gap"]["discarded_bytes"], discarded);
    let offset = |item: &Value| -> u64 {
        text(&item["cursor"])
            .rsplit('.')
            .next()
            .and_then(|offset| offset.parse().ok())
            .expect("cursor offset")
    };
    assert_eq!(
        offset(&items[1]),
        discarded,
        "the gap moves to the oldest retained byte"
    );
    assert_eq!(items.iter().filter(|item| item["kind"] == "gap").count(), 1);
    assert_eq!(
        shapes[shapes.len() - 3..],
        ["line:last", "event:process.exited", "ended"]
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_long_line_is_truncated_with_its_length() {
    let Some(harness) = Harness::delegated("ev-long-line") else {
        return;
    };
    let cwd = harness.workdir("long-line");
    let id = harness
        .started(
            "op_longline_start01",
            input(
                &sh("head -c 20000 /dev/zero | tr '\\000' x; echo"),
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    harness.ended(&id).await;
    let served = serve(&harness.app, SUBJECT).await;
    let (items, _) = open(&served, &id, None).await.upgraded().drain().await;
    let line = &items[1]["line"];
    assert_eq!(items[1]["kind"], "line", "{items:?}");
    assert_eq!(
        text(&line["text"]).len(),
        substrate_wire::MAX_PROCESS_EVENT_LINE_BYTES
    );
    assert_eq!(line["truncated"], true);
    assert_eq!(line["full_bytes"], 20_000);
    assert_eq!(line["lossy"], false);
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_non_utf8_line_is_marked_lossy() {
    let Some(harness) = Harness::delegated("ev-lossy") else {
        return;
    };
    let cwd = harness.workdir("lossy");
    let id = harness
        .started(
            "op_lossy_start_0001",
            input(&sh("printf 'ok\\377\\n'"), &cwd, "s-1", "dev"),
        )
        .await;
    harness.ended(&id).await;
    let served = serve(&harness.app, SUBJECT).await;
    let (items, _) = open(&served, &id, None).await.upgraded().drain().await;
    let line = &items[1]["line"];
    assert_eq!(items[1]["kind"], "line", "{items:?}");
    assert_eq!(line["text"], "ok\u{fffd}");
    assert_eq!(line["lossy"], true);
    assert_eq!(line["truncated"], false);
    assert_eq!(line["full_bytes"], 3);
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_slow_reader_never_blocks_the_program() {
    let Some(harness) = Harness::delegated("ev-slow-reader") else {
        return;
    };
    let cwd = harness.workdir("slow-reader");
    let id = harness
        .started(
            "op_slowreader_start",
            input(
                &sh("head -c 4194304 /dev/zero | tr '\\000' s | fold -w 1023; echo; echo done"),
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    let served = serve(&harness.app, SUBJECT).await;
    // Upgraded and never read: the daemon's sends back up against this socket.
    let idle = open(&served, &id, None).await.upgraded();
    let record = harness.ended(&id).await;
    assert_eq!(record["ended_by"], "exit");
    assert_eq!(record["exit"], json!({ "code": 0 }));
    assert_eq!(harness.log_line(&id, "done").await, "done");
    drop(idle);
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stream_on_an_ended_record_replays_and_closes() {
    let Some(mut harness) = Harness::delegated("ev-replay") else {
        return;
    };
    let cwd = harness.workdir("replay");
    let id = harness
        .started(
            "op_replay_start_001",
            with(
                input(&sh("echo hello"), &cwd, "s-1", "dev"),
                "timeout_ms",
                json!(30_000),
            ),
        )
        .await;
    harness.ended(&id).await;
    let expected = [
        "event:process.started",
        "line:hello",
        "event:process.exited",
        "ended",
    ];
    let served = serve(&harness.app, SUBJECT).await;
    let (items, close) = open(&served, &id, None).await.upgraded().drain().await;
    assert_eq!(shapes(&items), expected);
    assert_eq!(close, Some(1000));
    assert_eq!(
        items[3]["ended"],
        json!({ "ended_by": "exit", "exit": { "code": 0 } })
    );
    drop(served);
    // A later run that supervises nothing replays it from the stored log.
    harness.unserve();
    let served = serve(&harness.app, SUBJECT).await;
    let (again, close) = open(&served, &id, None).await.upgraded().drain().await;
    assert_eq!(shapes(&again), expected);
    assert_eq!(close, Some(1000));
    // Nothing runs any more; the record's cgroup went with its end.
    if let Some(cgroup) = &harness.cgroup {
        let _ = std::fs::remove_dir(cgroup);
    }
}

/// A daemon run whose driver finds every earlier record's cgroup gone, and whose log of each holds
/// one line the program wrote before it vanished: `last words`, the thirteen-byte record `o last
/// words\n`.
struct GoneSupervisor;

const LAST_WORDS_END: u64 = 13;

impl super::ProcessLogs for GoneSupervisor {
    fn log_tail(
        &self,
        _id: &str,
        _selection: super::LogStreamSelection,
        _max_bytes: u64,
    ) -> Result<super::LogTail, super::DriverError> {
        Err(super::DriverError::not_found())
    }

    fn log_read(
        &self,
        _id: &str,
        from: u64,
        _max_bytes: u64,
    ) -> Result<Option<substrate_host::LogRead>, super::DriverError> {
        let line = substrate_host::LogLine {
            stream: substrate_host::LogStream::Stdout,
            bytes: b"last words".to_vec(),
            truncated: false,
            full_len: 10,
        };
        Ok(match from {
            0 => Some(substrate_host::LogRead {
                gap: 0,
                lines: vec![(line, LAST_WORDS_END)],
                end: LAST_WORDS_END,
            }),
            LAST_WORDS_END => Some(substrate_host::LogRead {
                gap: 0,
                lines: Vec::new(),
                end: LAST_WORDS_END,
            }),
            _ => None,
        })
    }

    fn log_end(&self, _id: &str) -> Result<u64, super::DriverError> {
        Ok(LAST_WORDS_END)
    }
}

impl super::ProcessSupervisor for GoneSupervisor {
    fn start(
        self: Arc<Self>,
        _id: &str,
        _spec: &super::HostProcessSpec,
    ) -> Result<super::HostProcessStarted, super::DriverError> {
        Err(super::DriverError::not_found())
    }

    fn reattach(self: Arc<Self>, _id: &str) -> Result<super::Reattached, super::DriverError> {
        Ok(super::Reattached::Gone)
    }

    fn begin_stop(&self, _id: &str, _cause: super::StopCause) -> Result<(), super::DriverError> {
        Err(super::DriverError::not_found())
    }

    fn ended(&self, _id: &str) -> Result<Option<super::HostProcessEnd>, super::DriverError> {
        Err(super::DriverError::not_found())
    }

    fn observe(&self, _id: &str) -> Result<super::HostProcessObservation, super::DriverError> {
        Err(super::DriverError::not_found())
    }

    fn forget(&self, _id: &str) -> Result<(), super::DriverError> {
        Ok(())
    }
}

/// `process.vanished` is an end event (S12): it follows the whole log and is the record's last
/// event before the one `ended` item, which names `vanished`. A vanished event placed like a
/// renewal would arrive before the line the program wrote.
#[tokio::test(flavor = "multi_thread")]
async fn host_process_stream_of_a_vanished_record_ends_after_its_log() {
    let (mut harness, _fake) = Harness::faked();
    let cwd = harness.workdir("vanished-stream");
    let id = harness
        .started(
            "op_vanish_start_0001",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    // The next run's cutoff is after the start, so it reconciles the record.
    tokio::time::sleep(Duration::from_millis(5)).await;
    harness.restart();
    Arc::get_mut(&mut harness.app)
        .expect("unshared application")
        .host_processes = Some(Arc::new(GoneSupervisor) as Arc<dyn super::ProcessSupervisor>);
    let record = harness.ended(&id).await;
    assert_eq!(record["ended_by"], "vanished", "{record}");
    let served = serve(&harness.app, SUBJECT).await;
    let (items, close) = open(&served, &id, None).await.upgraded().drain().await;
    assert_eq!(
        shapes(&items),
        [
            "event:process.started",
            "line:last words",
            "event:process.vanished",
            "ended"
        ]
    );
    assert_eq!(items[3]["ended"], json!({ "ended_by": "vanished" }));
    assert_eq!(close, Some(1000));
}

/// The stream's control-frame budget is the event stream's, 120 per minute: the 121st ping closes
/// it 1008, with no `ended` item.
#[tokio::test(flavor = "multi_thread")]
async fn host_process_stream_control_frame_flood_closes_1008() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("control-flood");
    let id = harness
        .started(
            "op_flood_start_00001",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app, SUBJECT).await;
    let mut reader = open(&served, &id, None).await.upgraded();
    let first = reader.until(|item| item["kind"] == "event").await;
    assert_eq!(shapes(&first), ["event:process.started"]);
    for _ in 0..=120 {
        reader.ping().await;
    }
    let (items, close) = reader.drain().await;
    assert!(items.is_empty(), "{items:?}");
    assert_eq!(close, Some(1008));
}
