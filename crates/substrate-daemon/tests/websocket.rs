#![forbid(unsafe_code)]

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use substrate_daemon::{App, Identity, router};
use substrate_host::{HostConfig, HostDriver};
use substrate_store::{NewOperation, Reservation, Scope, Store};
use substrate_wire::{ErrorClass, ErrorDetail};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const SUBJECT: &str = "local:1000";
const DEPLOYMENT: &str = "dep_websocket_test";
const HANDSHAKE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const SUBJECT_STREAM_LIMIT: usize = 4;

struct Harness {
    _directory: TempDir,
    store: Arc<Store>,
    server: TestServer,
}

impl Harness {
    async fn open() -> Self {
        let directory = tempfile::tempdir().expect("temporary websocket harness");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver");
        let app = App::new(Arc::clone(&store), driver, DEPLOYMENT);
        let server = TestServer::spawn(app).await;
        Self {
            _directory: directory,
            store,
            server,
        }
    }

    async fn connect(&self, path: &str) -> Handshake {
        Handshake::open(self.server.address, path).await
    }

    fn append_event(&self) {
        let operation = "01JWEBSOCKETWAKEEVENT0001";
        let scope = Scope {
            deployment: DEPLOYMENT.to_owned(),
            subject: SUBJECT.to_owned(),
        };
        let new = NewOperation {
            scope,
            operation: operation.to_owned(),
            operation_kind: "test.refusal".to_owned(),
            request_hash: "7".repeat(64),
            accepted_at: "2026-08-14T12:00:00Z".to_owned(),
            capability_snapshot: None,
            actor: "websocket-test".to_owned(),
            principal: None,
            resource: None,
        };
        let detail = ErrorDetail {
            class: ErrorClass::Refused,
            code: "test.refusal".to_owned(),
            message: "Test event.".to_owned(),
            retriable: false,
            address: None,
            operation: Some(operation.to_owned()),
        };
        assert!(matches!(
            self.store
                .record_refusal(&new, "2026-08-14T12:00:00Z", 422, &detail,),
            Ok(Reservation::Replay(_))
        ));
    }
}

struct TestServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl TestServer {
    async fn spawn(app: Arc<App>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind websocket test server");
        let address = listener.local_addr().expect("test server address");
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    return;
                };
                let identity = Identity {
                    subject: SUBJECT.to_owned(),
                    actor: "websocket-test".to_owned(),
                    principal: None,
                };
                let service = router(Arc::clone(&app)).layer(Extension(identity));
                tokio::spawn(async move {
                    let connection = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), TowerToHyperService::new(service))
                        .with_upgrades();
                    let _result = connection.await;
                });
            }
        });
        Self { address, task }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Handshake {
    status: u16,
    stream: TcpStream,
}

impl Handshake {
    async fn open(address: SocketAddr, path: &str) -> Self {
        let mut stream = TcpStream::connect(address)
            .await
            .expect("connect websocket test client");
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write websocket handshake");
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            assert!(response.len() < 16 * 1_024, "bounded handshake response");
            let mut byte = [0_u8; 1];
            stream
                .read_exact(&mut byte)
                .await
                .expect("read websocket handshake");
            response.push(byte[0]);
        }
        let response = std::str::from_utf8(&response).expect("ASCII handshake response");
        let status = response
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .expect("HTTP handshake status");
        Self { status, stream }
    }

    fn upgraded(self) -> WebSocketClient {
        assert_eq!(self.status, 101, "websocket upgrade must succeed");
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

    async fn wait_for_close(&mut self) -> Option<u16> {
        tokio::time::timeout(Duration::from_secs(2), async {
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
        .expect("server must close the bounded websocket")
    }

    async fn close(mut self) {
        self.send_frame(true, 0x8, &1000_u16.to_be_bytes()).await;
        let _closed = tokio::time::timeout(Duration::from_secs(1), self.wait_for_close()).await;
    }
}

async fn assert_full_subject_capacity(harness: &Harness) {
    let path = "/v1/events/stream?limit=1";
    let mut clients = Vec::new();
    for _ in 0..SUBJECT_STREAM_LIMIT {
        clients.push(harness.connect(path).await.upgraded());
    }
    assert_eq!(
        harness.connect(path).await.status,
        429,
        "the fifth live subject stream must hit the exact capacity bound"
    );
    for client in clients {
        client.close().await;
    }
    tokio::task::yield_now().await;
    harness.connect(path).await.upgraded().close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn data_frame_closes_1003_and_releases_the_subject_permit() {
    let harness = Harness::open().await;
    let mut client = harness
        .connect("/v1/events/stream?limit=1")
        .await
        .upgraded();
    client.send_frame(true, 0x1, b"not a control frame").await;
    assert_eq!(client.wait_for_close().await, Some(1003));
    assert_full_subject_capacity(&harness).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn frame_and_message_input_are_bounded_at_1024_bytes_and_release_permits() {
    let harness = Harness::open().await;

    let mut oversized_frame = harness
        .connect("/v1/events/stream?limit=1")
        .await
        .upgraded();
    oversized_frame.send_frame(true, 0x2, &[0; 1_025]).await;
    assert!(
        matches!(oversized_frame.wait_for_close().await, None | Some(1009)),
        "an oversized frame must terminate with EOF or message-too-big"
    );

    let mut oversized_message = harness
        .connect("/v1/events/stream?limit=1")
        .await
        .upgraded();
    oversized_message.send_frame(false, 0x1, &[b'a'; 600]).await;
    oversized_message.send_frame(true, 0x0, &[b'a'; 600]).await;
    assert!(
        matches!(oversized_message.wait_for_close().await, None | Some(1009)),
        "an oversized fragmented message must terminate with EOF or message-too-big"
    );

    assert_full_subject_capacity(&harness).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn control_frame_flood_closes_1008_and_releases_the_subject_permit() {
    let harness = Harness::open().await;
    let mut client = harness
        .connect("/v1/events/stream?limit=1")
        .await
        .upgraded();
    for _ in 0..=120 {
        client.send_frame(true, 0x9, &[]).await;
    }
    assert_eq!(client.wait_for_close().await, Some(1008));
    assert_full_subject_capacity(&harness).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cursor_preflight_failure_releases_permit_and_wakeup_before_reconnect() {
    let harness = Harness::open().await;
    assert_eq!(
        harness
            .connect("/v1/events/stream?limit=1&cursor=ev2.other-scope.1.0")
            .await
            .status,
        409
    );

    assert_full_subject_capacity(&harness).await;

    let mut reconnected = harness
        .connect("/v1/events/stream?limit=1")
        .await
        .upgraded();
    harness.append_event();
    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let frame = reconnected.next_frame().await.expect("live event frame");
            if frame.opcode == 0x1 {
                return frame.payload;
            }
        }
    })
    .await
    .expect("reconnected stream must receive the commit wakeup");
    let event: serde_json::Value = serde_json::from_slice(&event).expect("event frame JSON");
    assert_eq!(event["kind"], "events");
    assert_eq!(event["page"]["items"][0]["transition"], "operation.refused");
    reconnected.close().await;
}
