#![cfg(unix)]
//! Adversary pass 1 against ADR 0040: hosted admission reuses Identity connections through a
//! bounded keep-alive pool. Identity here is a scripted raw HTTP/1.1-over-TLS peer so that each
//! case controls exactly the bytes the resolver reads and the moment a connection closes.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use rcgen::{CertificateParams, KeyPair, date_time_ymd};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio_rustls::client::TlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

const SERVER_NAME: &str = "substrate.test";
const IDENTITY_NAME: &str = "127.0.0.1";

struct TestIdentity {
    certificate_pem: String,
    private_key_pem: String,
    certificate_der: CertificateDer<'static>,
}

fn identity_for(name: &str) -> TestIdentity {
    let mut params = CertificateParams::new([name.to_owned()]).expect("certificate params");
    params.not_before = date_time_ymd(2025, 1, 1);
    params.not_after = date_time_ymd(2035, 1, 1);
    let key = KeyPair::generate().expect("private key");
    let certificate = params.self_signed(&key).expect("self-signed certificate");
    TestIdentity {
        certificate_pem: certificate.pem(),
        private_key_pem: key.serialize_pem(),
        certificate_der: certificate.der().clone(),
    }
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) {
    std::fs::write(path, bytes).expect("write file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("file mode");
}

#[path = "support/ports.rs"]
mod ports;
#[path = "../../../test-support/readiness.rs"]
mod readiness;
use ports::unused_address;

/// What the scripted Identity does with one resolution request.
#[derive(Clone, Copy, Debug)]
enum Answer {
    /// `200` observe authority, `Content-Length`, keep-alive.
    Ok,
    /// As `Ok`, after a delay.
    Delayed(u64),
    /// As `Ok`, then Identity closes the connection (no `Connection: close` header).
    OkThenClose,
    /// As `Ok`, with a second complete `200` observe response appended in the same write.
    OkThenSmuggled,
    /// `HTTP/1.0 200` with `Connection: keep-alive` and `Content-Length`.
    Http10KeepAlive,
    /// `200` observe authority with a chunked body.
    Chunked,
    /// `200` `application/json` whose body is past the 64 KiB bound.
    Oversized,
    /// `200` whose `Content-Length` is one byte short of the authority it sends.
    ShortLength,
    /// `401`.
    Unauthorized,
    /// Read the request, wait, close without answering.
    CloseUnanswered(u64),
}

struct Script {
    origin: String,
    queue: Mutex<VecDeque<Answer>>,
    default: Answer,
    accepts: AtomicUsize,
    open: AtomicUsize,
    resolutions: AtomicUsize,
}

impl Script {
    fn next(&self) -> Answer {
        self.queue
            .lock()
            .expect("script lock")
            .pop_front()
            .unwrap_or(self.default)
    }
    fn accepts(&self) -> usize {
        self.accepts.load(Ordering::SeqCst)
    }
    fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
    fn resolutions(&self) -> usize {
        self.resolutions.load(Ordering::SeqCst)
    }
}

fn authority_json(origin: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall time")
        .as_secs();
    serde_json::json!({
        "iss": origin,
        "sub": "subject",
        "aud": "urn:b10x:substrate",
        "iat": now,
        "nbf": now,
        "exp": now + 300,
        "jti": "jti",
        "act": {"sub": "actor"},
        "scope": "observe",
        "principal_kind": "human",
        "tenant_id": "tenant",
        "email": null,
        "groups": []
    })
    .to_string()
}

fn ok_bytes(origin: &str) -> Vec<u8> {
    let body = authority_json(origin);
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

struct OpenGuard(Arc<Script>);

impl Drop for OpenGuard {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }
}

#[allow(clippy::too_many_lines)]
async fn start_scripted_identity(
    identity: &TestIdentity,
    answers: Vec<Answer>,
    default: Answer,
) -> (Arc<Script>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Identity");
    let origin = format!(
        "https://{}",
        listener.local_addr().expect("Identity address")
    );
    let script = Arc::new(Script {
        origin,
        queue: Mutex::new(answers.into()),
        default,
        accepts: AtomicUsize::new(0),
        open: AtomicUsize::new(0),
        resolutions: AtomicUsize::new(0),
    });
    let certificates = rustls_pemfile::certs(&mut identity.certificate_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("Identity certificate");
    let private_key = rustls_pemfile::private_key(&mut identity.private_key_pem.as_bytes())
        .expect("Identity key parse")
        .expect("Identity key");
    let mut tls = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .expect("Identity TLS config");
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let shared = Arc::clone(&script);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            shared.accepts.fetch_add(1, Ordering::SeqCst);
            shared.open.fetch_add(1, Ordering::SeqCst);
            let guard = OpenGuard(Arc::clone(&shared));
            let acceptor = acceptor.clone();
            let script = Arc::clone(&shared);
            tokio::spawn(async move {
                let _guard = guard;
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut buffer = Vec::new();
                loop {
                    let end = loop {
                        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                            break end + 4;
                        }
                        let mut block = [0_u8; 4096];
                        match tls.read(&mut block).await {
                            Ok(0) | Err(_) => return,
                            Ok(count) => buffer.extend_from_slice(&block[..count]),
                        }
                    };
                    buffer.drain(..end);
                    script.resolutions.fetch_add(1, Ordering::SeqCst);
                    let origin = script.origin.clone();
                    match script.next() {
                        Answer::Ok => {
                            if tls.write_all(&ok_bytes(&origin)).await.is_err() {
                                return;
                            }
                        }
                        Answer::Delayed(ms) => {
                            tokio::time::sleep(Duration::from_millis(ms)).await;
                            if tls.write_all(&ok_bytes(&origin)).await.is_err() {
                                return;
                            }
                        }
                        Answer::OkThenClose => {
                            let _ = tls.write_all(&ok_bytes(&origin)).await;
                            let _ = tls.shutdown().await;
                            return;
                        }
                        Answer::OkThenSmuggled => {
                            let mut bytes = ok_bytes(&origin);
                            bytes.extend_from_slice(&ok_bytes(&origin));
                            if tls.write_all(&bytes).await.is_err() {
                                return;
                            }
                        }
                        Answer::Http10KeepAlive => {
                            let body = authority_json(&origin);
                            let bytes = format!(
                                "HTTP/1.0 200 OK\r\nconnection: keep-alive\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                                body.len()
                            );
                            if tls.write_all(bytes.as_bytes()).await.is_err() {
                                return;
                            }
                        }
                        Answer::Chunked => {
                            let body = authority_json(&origin);
                            let bytes = format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
                                body.len()
                            );
                            if tls.write_all(bytes.as_bytes()).await.is_err() {
                                return;
                            }
                        }
                        Answer::Oversized => {
                            let body = format!("{}{}", authority_json(&origin), " ".repeat(70_000));
                            let bytes = format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                                body.len()
                            );
                            if tls.write_all(bytes.as_bytes()).await.is_err() {
                                return;
                            }
                        }
                        Answer::ShortLength => {
                            let body = authority_json(&origin);
                            let bytes = format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                                body.len() - 1
                            );
                            if tls.write_all(bytes.as_bytes()).await.is_err() {
                                return;
                            }
                        }
                        Answer::Unauthorized => {
                            if tls
                                .write_all(
                                    b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n",
                                )
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Answer::CloseUnanswered(ms) => {
                            tokio::time::sleep(Duration::from_millis(ms)).await;
                            return;
                        }
                    }
                }
            });
        }
    });
    (script, task)
}

fn connector(root: CertificateDer<'static>) -> TlsConnector {
    let mut store = RootCertStore::empty();
    store.add(root).expect("test trust root");
    let mut config = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(store)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    TlsConnector::from(Arc::new(config))
}

struct Daemon {
    _root: TempDir,
    child: Child,
    address: SocketAddr,
    trusted: TlsConnector,
}

impl Daemon {
    async fn start(script: &Script, identity: &TestIdentity) -> Self {
        let root = TempDir::new().expect("temporary root");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private root");
        let server = identity_for(SERVER_NAME);
        let certificate: PathBuf = root.path().join("server.pem");
        let private_key: PathBuf = root.path().join("server.key");
        write_file(&certificate, server.certificate_pem.as_bytes(), 0o644);
        write_file(&private_key, server.private_key_pem.as_bytes(), 0o600);
        let identity_ca = root.path().join("identity-ca.pem");
        write_file(&identity_ca, identity.certificate_pem.as_bytes(), 0o644);
        let address = unused_address();
        let stderr_log = root.path().join("daemon.stderr");
        let mut command = Command::new(env!("CARGO_BIN_EXE_substrate-daemon"));
        command
            .arg("--socket")
            .arg(root.path().join("unused.sock"))
            .arg("--state")
            .arg(root.path().join("state.sqlite"))
            .arg("--workspaces")
            .arg(root.path().join("workspaces"))
            .arg("--deployment")
            .arg("tls-test")
            .arg("--tls-listen")
            .arg(address.to_string())
            .arg("--tls-certificate-chain")
            .arg(&certificate)
            .arg("--tls-private-key")
            .arg(&private_key)
            .arg("--hosted-identity-origin")
            .arg(&script.origin)
            .arg("--hosted-identity-ca-bundle")
            .arg(&identity_ca)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&stderr_log).expect("daemon stderr log"))
            .kill_on_drop(true);
        let child = command.spawn().expect("spawn TLS daemon");
        let mut daemon = Self {
            _root: root,
            child,
            address,
            trusted: connector(server.certificate_der.clone()),
        };
        let deadline = Instant::now() + readiness::DAEMON_READINESS;
        loop {
            if let Some(status) = daemon.child.try_wait().expect("inspect daemon") {
                let stderr = std::fs::read_to_string(&stderr_log).unwrap_or_default();
                panic!("TLS daemon exited before readiness: {status}: {stderr}");
            }
            if daemon.try_stream().await.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = daemon.child.start_kill();
                let _ = daemon.child.wait().await;
                let stderr = std::fs::read_to_string(&stderr_log).unwrap_or_default();
                panic!(
                    "TLS daemon readiness timed out after {:?}: {stderr}",
                    readiness::DAEMON_READINESS
                );
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        daemon
    }

    async fn try_stream(&self) -> Option<TlsStream<TcpStream>> {
        let tcp = TcpStream::connect(self.address).await.ok()?;
        let name = ServerName::try_from(SERVER_NAME.to_owned()).ok()?;
        self.trusted.connect(name, tcp).await.ok()
    }

    async fn stream(&self) -> TlsStream<TcpStream> {
        self.try_stream().await.expect("connect to TLS daemon")
    }

    async fn stop(mut self) {
        let pid = i32::try_from(self.child.id().expect("daemon pid")).expect("pid fits");
        kill(Pid::from_raw(pid), Signal::SIGTERM).expect("signal daemon");
        let status = tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .expect("daemon shutdown timed out")
            .expect("wait for daemon");
        assert!(status.success(), "daemon exit: {status}");
    }
}

fn credential() -> String {
    format!("identity_{}_v1_{}", "access", "o".repeat(43))
}

/// One hosted `GET /v1/machine` on a keep-alive stream; the status code, read within 15 s.
async fn hosted_status(stream: &mut TlsStream<TcpStream>) -> String {
    let request = format!(
        "GET /v1/machine HTTP/1.1\r\nHost: substrate.test\r\nAuthorization: Bearer {}\r\nConnection: keep-alive\r\n\r\n",
        credential()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    let mut response = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let mut block = [0_u8; 4096];
        let count = tokio::time::timeout_at(deadline, stream.read(&mut block))
            .await
            .expect("hosted response timeout")
            .expect("read hosted response");
        assert!(count > 0, "hosted response ended early");
        response.extend_from_slice(&block[..count]);
        let Some(end) = response.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = std::str::from_utf8(&response[..end + 4]).expect("ASCII head");
        let length = head
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(0);
        if response.len() >= end + 4 + length {
            return head[9..12].to_owned();
        }
    }
}

/// ADR 0040 decision 2: "idle timeout 30 s; an older idle connection is closed, never used".
#[tokio::test]
async fn adversary_idle_connection_older_than_the_idle_timeout_is_closed() {
    let identity = identity_for(IDENTITY_NAME);
    let (script, task) = start_scripted_identity(&identity, vec![], Answer::Ok).await;
    let daemon = Daemon::start(&script, &identity).await;
    let mut https = daemon.stream().await;
    assert_eq!(hosted_status(&mut https).await, "200");
    assert_eq!(script.accepts(), 1);
    tokio::time::sleep(Duration::from_millis(31_500)).await;
    let still_open = script.open();
    assert_eq!(
        still_open, 0,
        "ADR 0040: an idle Identity connection older than 30 s is closed; after 31.5 s idle \
         Substrate still holds {still_open} open"
    );
    // The daemon closes an inbound connection idle past its 5 s header read timeout, so the
    // second hosted request needs a fresh client stream; the Identity side is what is under test.
    let mut https = daemon.stream().await;
    assert_eq!(hosted_status(&mut https).await, "200");
    assert_eq!(
        script.accepts(),
        2,
        "an idle connection older than 30 s was used"
    );
    daemon.stop().await;
    task.abort();
}

/// ADR 0040 decision 2: at most 8 idle connections; concurrent resolutions never share one.
#[tokio::test]
async fn adversary_idle_pool_holds_at_most_eight_and_never_shares() {
    let identity = identity_for(IDENTITY_NAME);
    let (script, task) = start_scripted_identity(&identity, vec![], Answer::Delayed(800)).await;
    let daemon = Daemon::start(&script, &identity).await;
    let mut streams = Vec::new();
    for _ in 0..12 {
        streams.push(daemon.stream().await);
    }
    let mut set = tokio::task::JoinSet::new();
    for mut stream in streams {
        set.spawn(async move {
            let status = hosted_status(&mut stream).await;
            (status, stream)
        });
    }
    let mut kept = Vec::new();
    while let Some(joined) = set.join_next().await {
        let (status, stream) = joined.expect("join");
        assert_eq!(status, "200");
        kept.push(stream);
    }
    assert_eq!(
        script.accepts(),
        12,
        "twelve concurrent resolutions did not use twelve connections"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        script.open() <= 8,
        "{} idle Identity connections held, ADR 0040 bounds them at 8",
        script.open()
    );
    daemon.stop().await;
    task.abort();
}

/// ADR 0040 decision 4: Identity closing an idle pooled connection costs a fresh connection,
/// never an `auth.authority-unavailable`.
#[tokio::test]
async fn adversary_connection_identity_closed_while_idle_is_replaced() {
    let identity = identity_for(IDENTITY_NAME);
    let (script, task) =
        start_scripted_identity(&identity, vec![Answer::OkThenClose], Answer::Ok).await;
    let daemon = Daemon::start(&script, &identity).await;
    let mut https = daemon.stream().await;
    assert_eq!(hosted_status(&mut https).await, "200");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(hosted_status(&mut https).await, "200");
    assert_eq!(script.accepts(), 2);
    assert_eq!(script.resolutions(), 2);
    daemon.stop().await;
    task.abort();
}

/// ADR 0040 consequences: "Identity's resolution load still equals the Substrate hosted request
/// rate", and decision 4 retries only a connection that failed "before Identity answers (the peer
/// closed it while it was idle)". A pooled connection on which Identity received the request and
/// then closed is retried, so Identity resolves one hosted request twice.
#[tokio::test]
async fn adversary_retry_never_resends_a_request_identity_received() {
    let identity = identity_for(IDENTITY_NAME);
    let (script, task) = start_scripted_identity(
        &identity,
        vec![Answer::Ok, Answer::CloseUnanswered(0)],
        Answer::Ok,
    )
    .await;
    let daemon = Daemon::start(&script, &identity).await;
    let mut https = daemon.stream().await;
    assert_eq!(hosted_status(&mut https).await, "200");
    let second = hosted_status(&mut https).await;
    let resolutions = script.resolutions();
    assert_eq!(
        resolutions, 2,
        "two hosted requests (second answered {second}) reached Identity as {resolutions} \
         resolutions: the retry re-sent a request Identity had already received"
    );
    daemon.stop().await;
    task.abort();
}

/// Bytes past a complete response must never answer a later resolution: a revoked credential
/// is not admitted from a response Identity sent before the revocation.
#[tokio::test]
async fn adversary_leftover_response_bytes_never_answer_a_later_resolution() {
    let identity = identity_for(IDENTITY_NAME);
    let (script, task) = start_scripted_identity(
        &identity,
        vec![Answer::OkThenSmuggled],
        Answer::Unauthorized,
    )
    .await;
    let daemon = Daemon::start(&script, &identity).await;
    let mut https = daemon.stream().await;
    assert_eq!(hosted_status(&mut https).await, "200");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        hosted_status(&mut https).await,
        "401",
        "a revoked credential was admitted from bytes left over on a pooled connection"
    );
    assert_eq!(script.resolutions(), 2);
    daemon.stop().await;
    task.abort();
}

/// ADR 0040 decision 3: an HTTP/1.0 response is never pooled, even with `Connection: keep-alive`;
/// a chunked clean 200 is pooled.
#[tokio::test]
async fn adversary_http10_is_not_pooled_and_chunked_is() {
    let identity = identity_for(IDENTITY_NAME);
    let (script, task) = start_scripted_identity(
        &identity,
        vec![Answer::Http10KeepAlive, Answer::Chunked],
        Answer::Ok,
    )
    .await;
    let daemon = Daemon::start(&script, &identity).await;
    let mut https = daemon.stream().await;
    assert_eq!(hosted_status(&mut https).await, "200");
    assert_eq!(hosted_status(&mut https).await, "200");
    assert_eq!(
        script.accepts(),
        2,
        "an HTTP/1.0 response's connection was reused"
    );
    assert_eq!(hosted_status(&mut https).await, "200");
    assert_eq!(
        script.accepts(),
        2,
        "a chunked clean 200's connection was not reused"
    );
    daemon.stop().await;
    task.abort();
}

/// ADR 0040 decision 3: an oversized or malformed (short `Content-Length`) body closes the
/// connection.
#[tokio::test]
async fn adversary_oversized_or_short_body_is_not_pooled() {
    let identity = identity_for(IDENTITY_NAME);
    let (script, task) = start_scripted_identity(
        &identity,
        vec![Answer::Oversized, Answer::Ok, Answer::ShortLength],
        Answer::Ok,
    )
    .await;
    let daemon = Daemon::start(&script, &identity).await;
    let mut https = daemon.stream().await;
    assert_ne!(hosted_status(&mut https).await, "200");
    assert_eq!(hosted_status(&mut https).await, "200");
    assert_eq!(
        script.accepts(),
        2,
        "an oversized body's connection was reused"
    );
    assert_ne!(hosted_status(&mut https).await, "200");
    assert_eq!(hosted_status(&mut https).await, "200");
    assert_eq!(
        script.accepts(),
        3,
        "a short Content-Length connection was reused"
    );
    assert_eq!(script.resolutions(), 4);
    daemon.stop().await;
    task.abort();
}

/// ADR 0040 decision 2/4: the 5 s resolution deadline includes the retry.
#[tokio::test]
async fn adversary_deadline_holds_across_the_retry() {
    let identity = identity_for(IDENTITY_NAME);
    let (script, task) = start_scripted_identity(
        &identity,
        vec![
            Answer::Ok,
            Answer::CloseUnanswered(3_000),
            Answer::Delayed(3_000),
        ],
        Answer::Ok,
    )
    .await;
    let daemon = Daemon::start(&script, &identity).await;
    let mut https = daemon.stream().await;
    assert_eq!(hosted_status(&mut https).await, "200");
    let started = Instant::now();
    let status = hosted_status(&mut https).await;
    let elapsed = started.elapsed();
    assert_eq!(
        status, "503",
        "a 6 s pooled-then-retried resolution was not refused"
    );
    assert!(
        elapsed < Duration::from_millis(5_800),
        "the retry ran past the 5 s deadline: {elapsed:?}"
    );
    daemon.stop().await;
    task.abort();
}
