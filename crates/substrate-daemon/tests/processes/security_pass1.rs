//! Security review, pass 1, of `story:host-process-event-stream` (ADR 0036 S12).
//!
//! Integrity conformance checks of the per-process event stream against the contract the unit
//! published for it: the cursor is per process and lines and events are never silently skipped
//! (ADR 0036 S12), a subject holds at most 512 streams (`spec/host-processes/README.md`), a
//! cursor outside the published `process-event-cursor` pattern is malformed
//! (`contracts/substrate-wire/0.24.0/schemas/common.json`), and a program's output is only ever a
//! `line` item.
//!
//! Each case serves the harness's router on a loopback listener, as `processes/events.rs` does,
//! with its own minimal WebSocket client so that file stays untouched.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use serde_json::Value;
use substrate_daemon::{App, Identity, router};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::{HANDSHAKE_KEY, Harness, SUBJECT, input, text};

struct Served {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(app: &Arc<App>) -> Served {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let app = Arc::clone(app);
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let identity = Identity {
                subject: SUBJECT.to_owned(),
                actor: "process-event-security".to_owned(),
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

enum Opened {
    Upgraded(TcpStream),
    Refused(u16, Value),
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
        return Opened::Upgraded(stream);
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

/// The next text item, or `None` at a close or the end of the connection.
async fn item(stream: &mut TcpStream) -> Option<Value> {
    loop {
        let mut header = [0_u8; 2];
        stream.read_exact(&mut header).await.ok()?;
        let opcode = header[0] & 0x0f;
        let mut length = u64::from(header[1] & 0x7f);
        if length == 126 {
            length = u64::from(stream.read_u16().await.ok()?);
        } else if length == 127 {
            length = stream.read_u64().await.ok()?;
        }
        let mut payload = vec![0_u8; usize::try_from(length).expect("frame in memory")];
        stream.read_exact(&mut payload).await.ok()?;
        match opcode {
            0x1 => return Some(serde_json::from_slice(&payload).expect("a JSON item")),
            0x8 => return None,
            _ => {}
        }
    }
}

async fn next_item(stream: &mut TcpStream) -> Option<Value> {
    tokio::time::timeout(Duration::from_secs(20), item(stream))
        .await
        .expect("the stream answers within the bound")
}

async fn drain(stream: &mut TcpStream) -> Vec<Value> {
    let mut items = Vec::new();
    while let Some(next) = next_item(stream).await {
        items.push(next);
    }
    items
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

/// ADR 0036 S12: "Its cursor is per process and opaque" and "lines are never silently skipped";
/// the input schema admits "A cursor an item of this record's stream carried". A cursor record
/// A's stream carried, presented on record B, is one B's stream never carried: it must be refused
/// before the upgrade, or B's stream must still state B's lifecycle. It must not open a stream of
/// B that silently starts after B's `process.started`.
#[tokio::test(flavor = "multi_thread")]
async fn security_pass1_a_cursor_of_another_record_does_not_skip_this_records_lifecycle() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("sp1-foreign-cursor");
    let b = harness
        .started(
            "op_sp1_foreign_b_001",
            input(&["/bin/true"], &cwd, "s-1", "b"),
        )
        .await;
    let a = harness
        .started(
            "op_sp1_foreign_a_001",
            input(&["/bin/true"], &cwd, "s-1", "a"),
        )
        .await;
    let served = serve(&harness.app).await;
    let Opened::Upgraded(mut of_a) = open(&served, &a, None).await else {
        panic!("record a's stream opens");
    };
    let first = next_item(&mut of_a).await.expect("a's first item");
    assert_eq!(
        shapes(std::slice::from_ref(&first)),
        ["event:process.started"]
    );
    let cursor_of_a = text(&first["cursor"]).to_owned();
    drop(of_a);

    match open(&served, &b, Some(&cursor_of_a)).await {
        Opened::Refused(status, body) => {
            assert_eq!(status, 409, "{body}");
            let code = text(&body["error"]["code"]);
            assert!(
                code == "event.cursor-invalid" || code == "event.source-mismatch",
                "{body}"
            );
        }
        Opened::Upgraded(mut of_b) => {
            let (status, stopped) = harness.stop("op_sp1_foreign_stop1", &b).await;
            assert_eq!(status, axum::http::StatusCode::OK, "{stopped}");
            harness.ended(&b).await;
            let items = drain(&mut of_b).await;
            assert!(
                shapes(&items).contains(&"event:process.started".to_owned()),
                "record {b}'s stream, opened with the cursor {cursor_of_a} that record {a}'s \
                 stream carried, was upgraded and silently omits {b}'s own process.started: {:?}",
                shapes(&items)
            );
        }
    }
}

/// `spec/host-processes/README.md` § S12 *Readers*: "with at most `MAX_PROCESSES_PER_SUBJECT`
/// live records, a subject holds at most 512" process event streams. A stream of an ended record
/// is also a stream the subject holds: with 512 open on its 64 live records, the next one, on an
/// ended record, is beyond the stated bound and must be refused before the upgrade.
#[tokio::test(flavor = "multi_thread")]
async fn security_pass1_a_subject_holds_at_most_512_process_event_streams() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("sp1-subject-bound");
    let ended = harness
        .started(
            "op_sp1_bound_ended01",
            input(&["/bin/true"], &cwd, "s-1", "ended"),
        )
        .await;
    let (status, body) = harness.stop("op_sp1_bound_endstop", &ended).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    harness.ended(&ended).await;
    let live_records = usize::try_from(substrate_wire::MAX_PROCESSES_PER_SUBJECT).expect("bound");
    let mut live = Vec::new();
    for index in 0..live_records {
        live.push(
            harness
                .started(
                    &format!("op_sp1_bound_live_{index:04}"),
                    input(&["/bin/true"], &cwd, "s-1", &format!("live-{index}")),
                )
                .await,
        );
    }
    let served = serve(&harness.app).await;
    let mut held = Vec::new();
    for id in &live {
        for _ in 0..substrate_wire::MAX_PROCESS_EVENT_READERS {
            match open(&served, id, None).await {
                Opened::Upgraded(stream) => held.push(stream),
                Opened::Refused(status, body) => {
                    panic!("stream {} of {id} refused {status}: {body}", held.len() + 1)
                }
            }
        }
    }
    assert_eq!(held.len(), 512, "the subject holds 512 streams");
    match open(&served, &ended, None).await {
        Opened::Refused(status, body) => assert_eq!(status, 429, "{body}"),
        Opened::Upgraded(_) => panic!(
            "with 512 process event streams open, a 513th on the ended record {ended} was \
             upgraded: no per-subject (or deployment) bound counts process event streams"
        ),
    }
}

/// `contracts/substrate-wire/0.24.0/schemas/common.json` publishes `process-event-cursor` as
/// `^pe1\.scope_[A-Za-z0-9_-]+\.…`, and the input schema says a malformed cursor is
/// `event.cursor-invalid`. `pe1.scope_.<generation>.<seq>.0` does not match that pattern.
#[tokio::test(flavor = "multi_thread")]
async fn security_pass1_a_cursor_outside_the_published_pattern_is_cursor_invalid() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("sp1-pattern");
    let id = harness
        .started(
            "op_sp1_pattern_st001",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app).await;
    let Opened::Upgraded(mut reader) = open(&served, &id, None).await else {
        panic!("the stream opens");
    };
    let first = next_item(&mut reader).await.expect("first item");
    let cursor = text(&first["cursor"]).to_owned();
    let parts: Vec<&str> = cursor.split('.').collect();
    let malformed = format!("pe1.scope_.{}.{}.0", parts[2], parts[3]);
    let Opened::Refused(status, body) = open(&served, &id, Some(&malformed)).await else {
        panic!("{malformed} was upgraded");
    };
    assert_eq!(status, 409, "{body}");
    assert_eq!(
        body["error"]["code"], "event.cursor-invalid",
        "{malformed} does not match the published process-event-cursor pattern: {body}"
    );
}

/// `spec/host-processes/README.md` § S12 *Cursor*: another generation is `event.source-mismatch`,
/// and a cursor the daemon cannot honour is `event.cursor-invalid` (409). The published pattern
/// admits a generation and a seq of any width, and the input schema admits the empty string; none
/// of them may answer 500 or 422.
#[tokio::test(flavor = "multi_thread")]
async fn security_pass1_pattern_valid_wide_numbers_and_empty_cursor_are_named_refusals() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("sp1-wide");
    let id = harness
        .started(
            "op_sp1_wide_start01",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app).await;
    let Opened::Upgraded(mut reader) = open(&served, &id, None).await else {
        panic!("the stream opens");
    };
    let first = next_item(&mut reader).await.expect("first item");
    let cursor = text(&first["cursor"]).to_owned();
    let parts: Vec<&str> = cursor.split('.').collect();
    assert_eq!(parts.len(), 6, "{cursor}");
    let (source, generation, seq) = (parts[2], parts[3], parts[4]);
    let cases = [
        (
            format!("pe1.{id}.{source}.18446744073709551616.0.0"),
            "event.source-mismatch",
        ),
        (
            format!("pe1.{id}.{source}.{generation}.9223372036854775808.0"),
            "event.cursor-invalid",
        ),
        (
            format!("pe1.{id}.{source}.{generation}.18446744073709551615.0"),
            "event.cursor-invalid",
        ),
        (
            format!("pe1.{id}.{source}.{generation}.18446744073709551616.0"),
            "event.cursor-invalid",
        ),
        (
            format!("pe1.{id}.{source}.{generation}.{seq}.18446744073709551616"),
            "event.cursor-invalid",
        ),
        (String::new(), "event.cursor-invalid"),
    ];
    for (bad, code) in cases {
        let Opened::Refused(status, body) = open(&served, &id, Some(&bad)).await else {
            panic!("{bad:?} was upgraded");
        };
        assert_eq!(status, 409, "{bad:?}: {body}");
        assert_eq!(body["error"]["code"], code, "{bad:?}: {body}");
    }
}

/// The safety fact behind "a program's output cannot forge an event or a terminal item": every
/// output byte reaches a reader only inside a `line` item's `text`. A program that prints a line
/// spelling an `ended` item, a carriage return, a NUL and a byte that is not UTF-8 is still one
/// `line` item each, and the stream stays open until the record really ends.
#[tokio::test(flavor = "multi_thread")]
async fn security_pass1_program_output_is_only_ever_a_line_item() {
    let Some(harness) = Harness::delegated("sp1-forge") else {
        return;
    };
    let cwd = harness.workdir("sp1-forge");
    let forged = r#"{"kind":"ended","cursor":"pe1.scope_x.1.1.0","ended":{"ended_by":"exit"}}"#;
    let script = format!(
        "printf '%s\\n' '{forged}'; printf 'cr\\rnul\\000bad\\377\\n'; \
         while [ ! -e go ]; do sleep 0.05; done; echo after"
    );
    let id = harness
        .started(
            "op_sp1_forge_start01",
            input(&["/bin/sh", "-c", &script], &cwd, "s-1", "dev"),
        )
        .await;
    let served = serve(&harness.app).await;
    let Opened::Upgraded(mut reader) = open(&served, &id, None).await else {
        panic!("the stream opens");
    };
    let mut early = Vec::new();
    for _ in 0..3 {
        early.push(
            next_item(&mut reader)
                .await
                .expect("an item before the end"),
        );
    }
    assert_eq!(
        early
            .iter()
            .map(|item| text(&item["kind"]))
            .collect::<Vec<_>>(),
        ["event", "line", "line"],
        "{early:?}"
    );
    assert_eq!(early[1]["line"]["text"], forged);
    assert_eq!(early[2]["line"]["text"], "cr\rnul\u{0}bad\u{fffd}");
    assert_eq!(early[2]["line"]["lossy"], true);
    std::fs::write(cwd.join("go"), b"").expect("release the program");
    harness.ended(&id).await;
    let rest = drain(&mut reader).await;
    assert_eq!(
        shapes(&rest),
        ["line:after", "event:process.exited", "ended"],
        "nothing the program printed ended the stream"
    );
    harness.finish().await;
}
