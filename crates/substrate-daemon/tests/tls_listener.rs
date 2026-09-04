#![cfg(unix)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::Utc;
use ed25519_dalek::{Signer as _, SigningKey};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use rcgen::{CertificateParams, KeyPair, date_time_ymd};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use substrate_store::{LeaseClock, NewLease, NewOperation, Reservation, Scope, Store, StoredExec};
use substrate_wire::{
    ConfinementRequest, Exec, ExecKind, ExecState, NetworkMode, PipeSession, PipeSessionLimits,
    SandboxProfile, SessionAttachmentState, SessionKind, SessionMode, SessionState, Workspace,
    WorkspaceKind, WorkspaceState, session_authority_transcript,
};
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

fn identity(not_before: (i32, u8, u8), not_after: (i32, u8, u8)) -> TestIdentity {
    identity_for(SERVER_NAME, not_before, not_after)
}

fn identity_for(name: &str, not_before: (i32, u8, u8), not_after: (i32, u8, u8)) -> TestIdentity {
    let mut params = CertificateParams::new([name.to_owned()]).expect("certificate params");
    params.not_before = date_time_ymd(not_before.0, not_before.1, not_before.2);
    params.not_after = date_time_ymd(not_after.0, not_after.1, not_after.2);
    let key = KeyPair::generate().expect("private key");
    let certificate = params.self_signed(&key).expect("self-signed certificate");
    TestIdentity {
        certificate_pem: certificate.pem(),
        private_key_pem: key.serialize_pem(),
        certificate_der: certificate.der().clone(),
    }
}

fn current_identity() -> TestIdentity {
    identity((2025, 1, 1), (2035, 1, 1))
}

fn write_identity(root: &Path, identity: &TestIdentity) -> (PathBuf, PathBuf) {
    let certificate = root.join("identity.pem");
    let private_key = root.join("identity.key");
    atomic_replace(&certificate, identity.certificate_pem.as_bytes(), 0o644);
    atomic_replace(&private_key, identity.private_key_pem.as_bytes(), 0o600);
    (certificate, private_key)
}

fn atomic_replace(path: &Path, bytes: &[u8], mode: u32) {
    let temporary = path.with_extension(format!("new-{}", std::process::id()));
    std::fs::write(&temporary, bytes).expect("write replacement");
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode))
        .expect("replacement permissions");
    std::fs::rename(temporary, path).expect("replace identity file");
}

fn unused_address() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral listener");
    let address = listener.local_addr().expect("ephemeral address");
    drop(listener);
    address
}

fn daemon_command(
    root: &Path,
    listen: SocketAddr,
    certificate: &Path,
    private_key: &Path,
    identity_origin: &str,
    identity_ca: &Path,
) -> Command {
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
        .expect("private daemon root");
    let mut command = Command::new(env!("CARGO_BIN_EXE_substrate-daemon"));
    command
        .arg("--socket")
        .arg(root.join("unused.sock"))
        .arg("--state")
        .arg(root.join("state.sqlite"))
        .arg("--workspaces")
        .arg(root.join("workspaces"))
        .arg("--deployment")
        .arg("tls-test")
        .arg("--tls-listen")
        .arg(listen.to_string())
        .arg("--tls-certificate-chain")
        .arg(certificate)
        .arg("--tls-private-key")
        .arg(private_key)
        .arg("--hosted-identity-origin")
        .arg(identity_origin)
        .arg("--hosted-identity-ca-bundle")
        .arg(identity_ca)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cgroup_root) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") {
        command.arg("--cgroup-root").arg(cgroup_root);
    }
    command
}

#[derive(Clone)]
struct AuthorityState {
    origin: String,
    observe: String,
    workspace: String,
    exec: String,
    unavailable: String,
    revoked: Arc<AtomicBool>,
}

fn access_credential(fill: char) -> String {
    format!("identity_{}_v1_{}", "access", fill.to_string().repeat(43))
}

async fn resolve_authority(State(state): State<AuthorityState>, headers: HeaderMap) -> Response {
    if headers
        .get("x-b10x-audience")
        .and_then(|value| value.to_str().ok())
        != Some("urn:b10x:substrate")
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let credential = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if credential == Some(state.unavailable.as_str()) {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let scope =
        if credential == Some(state.observe.as_str()) && !state.revoked.load(Ordering::SeqCst) {
            "observe"
        } else if credential == Some(state.workspace.as_str()) {
            "workspaces"
        } else if credential == Some(state.exec.as_str()) {
            "exec"
        } else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall time")
        .as_secs();
    let now = i64::try_from(now).expect("wall time fits");
    Json(json!({
        "iss": state.origin,
        "sub": "sensitive-subject-marker",
        "aud": "urn:b10x:substrate",
        "iat": now,
        "nbf": now,
        "exp": now + 300,
        "jti": "sensitive-jti-marker",
        "act": {"sub": "sensitive-actor-marker"},
        "scope": scope,
        "principal_kind": "human",
        "tenant_id": "sensitive-tenant-marker",
        "email": null,
        "groups": []
    }))
    .into_response()
}

async fn start_identity_authority(
    identity: &TestIdentity,
) -> (AuthorityState, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Identity authority");
    let address = listener.local_addr().expect("Identity address");
    let origin = format!("https://{address}");
    let state = AuthorityState {
        origin,
        observe: access_credential('o'),
        workspace: access_credential('w'),
        exec: access_credential('e'),
        unavailable: access_credential('u'),
        revoked: Arc::new(AtomicBool::new(false)),
    };
    let mut certificates = rustls_pemfile::certs(&mut identity.certificate_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("Identity certificate");
    let private_key = rustls_pemfile::private_key(&mut identity.private_key_pem.as_bytes())
        .expect("Identity private key parse")
        .expect("Identity private key");
    let mut tls = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(std::mem::take(&mut certificates), private_key)
        .expect("Identity TLS config");
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let service = Router::new()
        .route("/v1/access-authority", get(resolve_authority))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let service = service.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(stream).await else {
                    return;
                };
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), TowerToHyperService::new(service))
                    .await;
            });
        }
    });
    (state, task)
}

fn connector(roots: impl IntoIterator<Item = CertificateDer<'static>>) -> TlsConnector {
    let mut store = RootCertStore::empty();
    for root in roots {
        store.add(root).expect("test trust root");
    }
    let mut config = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(store)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    TlsConnector::from(std::sync::Arc::new(config))
}

async fn connect(
    address: SocketAddr,
    connector: &TlsConnector,
    name: &str,
) -> Result<TlsStream<TcpStream>, Box<dyn std::error::Error + Send + Sync>> {
    let tcp = TcpStream::connect(address).await?;
    let name = ServerName::try_from(name.to_owned())?;
    Ok(connector.connect(name, tcp).await?)
}

async fn connect_when_ready(
    child: &mut Child,
    address: SocketAddr,
    connector: &TlsConnector,
) -> TlsStream<TcpStream> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("inspect daemon") {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                pipe.read_to_string(&mut stderr)
                    .await
                    .expect("read early daemon error");
            }
            panic!("TLS daemon exited before readiness: {status}: {stderr}");
        }
        if let Ok(stream) = connect(address, connector, SERVER_NAME).await {
            return stream;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "TLS daemon readiness timed out"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn request(stream: &mut TlsStream<TcpStream>, request: &[u8]) -> Vec<u8> {
    stream
        .write_all(request)
        .await
        .expect("write HTTPS request");
    let mut response = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut block = [0_u8; 1024];
        let count = tokio::time::timeout_at(deadline, stream.read(&mut block))
            .await
            .expect("HTTPS response timeout")
            .expect("read HTTPS response");
        assert!(count > 0, "HTTPS response ended before a complete body");
        response.extend_from_slice(&block[..count]);
        let Some(header_end) = response.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            continue;
        };
        let header_end = header_end + 4;
        let headers = std::str::from_utf8(&response[..header_end]).expect("ASCII response headers");
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .expect("bounded response content length");
        if response.len() >= header_end + content_length {
            return response;
        }
    }
}

async fn upgrade_request(stream: &mut TlsStream<TcpStream>, request: &[u8]) -> Vec<u8> {
    stream.write_all(request).await.expect("write WSS request");
    let mut response = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut byte = [0_u8; 1];
        let count = tokio::time::timeout_at(deadline, stream.read(&mut byte))
            .await
            .expect("WSS response timeout")
            .expect("read WSS response");
        assert!(count > 0, "WSS response ended before complete headers");
        response.push(byte[0]);
        if response.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            return response;
        }
    }
}

fn response_json(response: &[u8]) -> serde_json::Value {
    let body = response
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .map(|offset| &response[offset + 4..])
        .expect("HTTP response headers");
    serde_json::from_slice(body).expect("JSON response body")
}

fn hosted_subject(origin: &str) -> String {
    let mut digest = Sha256::new();
    for field in [
        origin,
        "sensitive-tenant-marker",
        "sensitive-subject-marker",
    ] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field.as_bytes());
    }
    format!("hosted:{}", URL_SAFE_NO_PAD.encode(digest.finalize()))
}

#[allow(clippy::too_many_lines)]
fn seed_ready_pipe_session(state_path: &Path, origin: &str) {
    let store = Store::open(state_path).expect("open live daemon state");
    let scope = Scope {
        deployment: "tls-test".to_owned(),
        subject: hosted_subject(origin),
    };
    let now = Utc::now();
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .expect("Linux boot id")
        .trim()
        .to_owned();
    let uptime = std::fs::read_to_string("/proc/uptime").expect("Linux uptime");
    let uptime = uptime.split_whitespace().next().expect("uptime seconds");
    let (seconds, fraction) = uptime.split_once('.').unwrap_or((uptime, "0"));
    let mut millis = fraction.chars().take(3).collect::<String>();
    while millis.len() < 3 {
        millis.push('0');
    }
    let boottime_ms = seconds
        .parse::<u64>()
        .expect("numeric uptime seconds")
        .saturating_mul(1_000)
        .saturating_add(millis.parse::<u64>().expect("numeric uptime milliseconds"));
    let workspace = Workspace {
        id: "ws_network_authority".to_owned(),
        kind: WorkspaceKind::Workspace,
        labels: BTreeMap::default(),
        observed_at: now,
        state: WorkspaceState::Ready,
        storage: None,
        lease: None,
    };
    let workspace_operation = NewOperation {
        scope: scope.clone(),
        operation: "01JNETWORKAUTHORITYWORKSP1".to_owned(),
        operation_kind: "workspace.create".to_owned(),
        request_hash: "b".repeat(64),
        accepted_at: now.to_rfc3339(),
        capability_snapshot: Some(format!("sha256:{}", "7".repeat(64))),
        actor: "sensitive-actor-marker".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(workspace.id.clone()),
    };
    assert_eq!(
        store
            .reserve_workspace_create(
                &workspace_operation,
                "ws-network-authority",
                &workspace,
                None
            )
            .expect("reserve network-authority workspace"),
        Reservation::Accepted
    );
    store
        .complete_workspace(
            &scope,
            &workspace_operation.operation,
            &now.to_rfc3339(),
            201,
            "ws-network-authority",
            &workspace,
        )
        .expect("complete network-authority workspace");
    let operation = NewOperation {
        scope: scope.clone(),
        operation: "01JNETWORKAUTHORITY000001".to_owned(),
        operation_kind: "session.start".to_owned(),
        request_hash: "c".repeat(64),
        accepted_at: now.to_rfc3339(),
        capability_snapshot: Some(format!("sha256:{}", "7".repeat(64))),
        actor: "sensitive-actor-marker".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some("ses_network_authority".to_owned()),
    };
    let lease = NewLease {
        ttl_ms: 60_000,
        clock: LeaseClock {
            wall: now,
            boot_id,
            boottime_ms,
        },
        authorizing_operation: operation.operation.clone(),
        actor: operation.actor.clone(),
        principal: None,
    };
    let mut exec = StoredExec {
        resource: Exec {
            id: "ex_network_authority".to_owned(),
            kind: ExecKind::Exec,
            workspace: workspace.id,
            state: ExecState::Accepted,
            observed_at: now,
            requested: ConfinementRequest {
                capability_snapshot: format!("sha256:{}", "7".repeat(64)),
                network: NetworkMode::None,
                aperture: None,
                profile: SandboxProfile::Workspace,
                required: true,
            },
            applied: None,
            exit: None,
            usage: None,
            lease: Some(lease.observation()),
            refusal: None,
        },
        stdout: Vec::new(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        output_complete: false,
        cgroup: None,
        leader_pid: None,
    };
    let mut session = PipeSession {
        id: "ses_network_authority".to_owned(),
        kind: SessionKind::Session,
        mode: SessionMode::Pipes,
        exec: exec.resource.id.clone(),
        workspace: exec.resource.workspace.clone(),
        state: SessionState::Accepted,
        attachment: SessionAttachmentState::Pending,
        observed_at: now,
        capability_snapshot: format!("sha256:{}", "7".repeat(64)),
        limits: PipeSessionLimits {
            input_bytes: 1_024,
            frame_bytes: 256,
            queued_frames: 4,
        },
        exit: None,
        lease: lease.observation(),
    };
    assert_eq!(
        store
            .reserve_pipe_session_start(&operation, &session, &exec, &lease, None)
            .expect("reserve network-authority session"),
        Reservation::Accepted
    );
    exec.resource.state = ExecState::Running;
    session.state = SessionState::Ready;
    session.attachment = SessionAttachmentState::Available;
    store
        .complete_pipe_session_start(
            &scope,
            &operation.operation,
            &now.to_rfc3339(),
            202,
            &session,
            &exec,
            &lease,
        )
        .expect("complete network-authority session");
}

fn channel_exporter(stream: &TlsStream<TcpStream>) -> [u8; 32] {
    let mut exporter = [0_u8; 32];
    stream
        .get_ref()
        .1
        .export_keying_material(
            &mut exporter,
            substrate_wire::SESSION_AUTHORITY_EXPORTER_LABEL,
            None,
        )
        .expect("TLS exporter");
    exporter
}

fn attachment_request(
    credential: &str,
    session_id: &str,
    authority_id: &str,
    authority: &str,
    timestamp_ms: i64,
    proof: &[u8],
) -> String {
    format!(
        "GET /v1/sessions/{session_id}/attach HTTP/1.1\r\nHost: substrate.test\r\nAuthorization: Bearer {credential}\r\nX-Substrate-Session-Authority-Id: {authority_id}\r\nX-Substrate-Session-Authority: {authority}\r\nX-Substrate-Session-Timestamp: {timestamp_ms}\r\nX-Substrate-Session-Proof: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: AAAAAAAAAAAAAAAAAAAAAA==\r\n\r\n",
        URL_SAFE_NO_PAD.encode(proof)
    )
}

async fn hosted_json(
    address: SocketAddr,
    trusted: &TlsConnector,
    credential: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut stream = connect(address, trusted, SERVER_NAME)
        .await
        .expect("hosted request connection");
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: substrate.test\r\nAuthorization: Bearer {credential}\r\nConnection: close\r\n"
    );
    if let Some(body) = body {
        write!(
            head,
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        )
        .expect("format hosted request headers");
    }
    head.push_str("\r\n");
    if let Some(body) = body {
        head.push_str(body);
    }
    let response = request(&mut stream, head.as_bytes()).await;
    let status = std::str::from_utf8(&response)
        .expect("hosted response text")
        .split_whitespace()
        .nth(1)
        .expect("hosted response status")
        .parse::<u16>()
        .expect("numeric hosted response status");
    (status, response_json(&response))
}

async fn provision_confined_pipe_session(
    address: SocketAddr,
    trusted: &TlsConnector,
    authority: &AuthorityState,
) -> String {
    let (status, machine) = hosted_json(
        address,
        trusted,
        &authority.observe,
        "GET",
        "/v1/machine",
        None,
    )
    .await;
    assert_eq!(status, 200, "machine capability: {machine}");
    let snapshot = machine["result"]["snapshot"]
        .as_str()
        .expect("capability snapshot");
    let workspace_body = json!({
        "op": "01JNETWORKAUTHORITYWORKSP2",
        "input": {"source": "empty", "labels": {}}
    })
    .to_string();
    let (status, workspace) = hosted_json(
        address,
        trusted,
        &authority.workspace,
        "POST",
        "/v1/workspaces",
        Some(&workspace_body),
    )
    .await;
    assert_eq!(status, 201, "workspace creation: {workspace}");
    let workspace_id = workspace["result"]["id"].as_str().expect("workspace id");
    let session_body = json!({
        "op": "01JNETWORKAUTHORITYSESSION2",
        "input": {
            "exec": {
                "argv": ["/bin/sh", "-c", "printf hosted-channel-ready; cat"],
                "env": {"allow": [], "set": {}},
                "lease_ttl_ms": 60000,
                "limits": {
                    "cpu_millis": 1000,
                    "memory_bytes": 67_108_864,
                    "output_bytes": 65536,
                    "processes": 16,
                    "timeout_ms": 30000
                },
                "sandbox": {
                    "capability_snapshot": snapshot,
                    "network": "none",
                    "profile": "workspace",
                    "require": true
                },
                "wait": false,
                "workspace": workspace_id
            },
            "frame_limit_bytes": 65536,
            "input_limit_bytes": 1_048_576,
            "queued_frames": 16
        }
    })
    .to_string();
    let (status, session) = hosted_json(
        address,
        trusted,
        &authority.exec,
        "POST",
        "/v1/sessions",
        Some(&session_body),
    )
    .await;
    assert_eq!(status, 202, "pipe-session start: {session}");
    session["result"]["id"]
        .as_str()
        .expect("session id")
        .to_owned()
}

async fn prove_pipe_bytes(stream: &mut TlsStream<TcpStream>) {
    let payload = json!({
        "kind": "stdin",
        "sequence": 1,
        "content": {"encoding": "base64", "data": STANDARD.encode(b"round-trip-marker\n")}
    })
    .to_string()
    .into_bytes();
    assert!(payload.len() <= 125, "bounded WebSocket client frame");
    let mask = [0x11_u8, 0x22, 0x33, 0x44];
    let mut frame = vec![
        0x81,
        0x80 | u8::try_from(payload.len()).expect("short frame"),
    ];
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % mask.len()]),
    );
    stream
        .write_all(&frame)
        .await
        .expect("write WSS stdin frame");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut transcript = Vec::new();
    while !transcript
        .windows(b"round-trip-marker".len())
        .any(|window| window == b"round-trip-marker")
    {
        let mut header = [0_u8; 2];
        tokio::time::timeout_at(deadline, stream.read_exact(&mut header))
            .await
            .expect("WSS output timeout")
            .expect("WSS frame header");
        assert_eq!(header[1] & 0x80, 0, "server frame is unmasked");
        let mut length = u64::from(header[1] & 0x7f);
        if length == 126 {
            let mut bytes = [0_u8; 2];
            stream
                .read_exact(&mut bytes)
                .await
                .expect("WSS frame length");
            length = u64::from(u16::from_be_bytes(bytes));
        } else if length == 127 {
            let mut bytes = [0_u8; 8];
            stream
                .read_exact(&mut bytes)
                .await
                .expect("WSS frame length");
            length = u64::from_be_bytes(bytes);
        }
        let mut body = vec![0_u8; usize::try_from(length).expect("bounded WSS frame")];
        stream.read_exact(&mut body).await.expect("WSS frame body");
        if header[0] & 0x0f == 1 {
            let frame: serde_json::Value = serde_json::from_slice(&body).expect("session frame");
            if frame["kind"] == "output" && frame["stream"] == "stdout" {
                let bytes = STANDARD
                    .decode(frame["content"]["data"].as_str().expect("stdout data"))
                    .expect("base64 stdout");
                transcript.extend_from_slice(&bytes);
            }
        }
    }
}

fn peer_leaf(stream: &TlsStream<TcpStream>) -> Vec<u8> {
    stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        .expect("server certificate")
        .as_ref()
        .to_vec()
}

fn signal(child: &Child, signal: Signal) {
    let pid = i32::try_from(child.id().expect("daemon pid")).expect("pid fits i32");
    kill(Pid::from_raw(pid), signal).expect("signal daemon");
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one process lifetime proves handshake, rotation, snapshot retention, and shutdown"
)]
async fn production_tls_refuses_unverified_routes_and_rotates_atomically() {
    let root = TempDir::new().expect("temporary root");
    let first = current_identity();
    let second = current_identity();
    let authority_identity = identity_for(IDENTITY_NAME, (2025, 1, 1), (2035, 1, 1));
    let identity_ca = root.path().join("identity-ca.pem");
    std::fs::write(&identity_ca, &authority_identity.certificate_pem).expect("Identity CA");
    let (authority, authority_task) = start_identity_authority(&authority_identity).await;
    let (certificate, private_key) = write_identity(root.path(), &first);
    let address = unused_address();
    let mut child = daemon_command(
        root.path(),
        address,
        &certificate,
        &private_key,
        &authority.origin,
        &identity_ca,
    )
    .spawn()
    .expect("spawn TLS daemon");
    let trusted = connector([
        first.certificate_der.clone(),
        second.certificate_der.clone(),
    ]);
    let mut existing = connect_when_ready(&mut child, address, &trusted).await;
    assert_eq!(
        existing.get_ref().1.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    assert_eq!(
        existing.get_ref().1.alpn_protocol(),
        Some(b"http/1.1".as_slice())
    );
    assert_eq!(peer_leaf(&existing), first.certificate_der.as_ref());

    let mut https = connect(address, &trusted, SERVER_NAME)
        .await
        .expect("trusted HTTPS connection");
    let response = request(
        &mut https,
        b"GET /v1/machine HTTP/1.1\r\nHost: substrate.test\r\nForwarded: for=203.0.113.9\r\nX-Forwarded-For: 203.0.113.9\r\nX-Substrate-Subject: attacker\r\nConnection: keep-alive\r\n\r\n",
    )
    .await;
    let response = String::from_utf8(response).expect("HTTPS response text");
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    assert!(response.contains("auth.credential-absent"), "{response}");
    assert!(
        response
            .to_ascii_lowercase()
            .contains("x-b10x-contract: substrate-wire/0.16.0"),
        "{response}"
    );

    let valid_request = format!(
        "GET /v1/machine HTTP/1.1\r\nHost: substrate.test\r\nAuthorization: Bearer {}\r\nConnection: keep-alive\r\n\r\n",
        authority.observe
    );
    let response = request(&mut https, valid_request.as_bytes()).await;
    let response = String::from_utf8(response).expect("admitted response text");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    let spoofed = format!(
        "GET /v1/machine HTTP/1.1\r\nHost: substrate.test\r\nAuthorization: Bearer {}\r\nX-Substrate-Subject: attacker\r\nX-Substrate-Tenant: attacker\r\nForwarded: for=203.0.113.9\r\nConnection: keep-alive\r\n\r\n",
        authority.workspace
    );
    let response = request(&mut https, spoofed.as_bytes()).await;
    let response = String::from_utf8(response).expect("scope refusal text");
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    assert!(response.contains("auth.scope-denied"), "{response}");

    let workspace_body =
        r#"{"op":"01JPHASE2WORKSPACECREATE","input":{"source":"empty","labels":{}}}"#;
    let denied_mutation = format!(
        "POST /v1/workspaces HTTP/1.1\r\nHost: substrate.test\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{workspace_body}",
        authority.observe,
        workspace_body.len()
    );
    let response = request(&mut https, denied_mutation.as_bytes()).await;
    let response = String::from_utf8(response).expect("pre-durability scope refusal text");
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    assert!(response.contains("auth.scope-denied"), "{response}");

    let invalid = access_credential('x');
    let invalid_request = format!(
        "GET /v1/machine HTTP/1.1\r\nHost: substrate.test\r\nAuthorization: Bearer {invalid}\r\nConnection: keep-alive\r\n\r\n"
    );
    let response = request(&mut https, invalid_request.as_bytes()).await;
    let response = String::from_utf8(response).expect("invalid authority text");
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    assert!(response.contains("auth.authority-invalid"), "{response}");

    let unavailable_request = format!(
        "GET /v1/machine HTTP/1.1\r\nHost: substrate.test\r\nAuthorization: Bearer {}\r\nConnection: keep-alive\r\n\r\n",
        authority.unavailable
    );
    let response = request(&mut https, unavailable_request.as_bytes()).await;
    let response = String::from_utf8(response).expect("unavailable authority text");
    assert!(response.starts_with("HTTP/1.1 503"), "{response}");
    assert!(
        response.contains("auth.authority-unavailable"),
        "{response}"
    );

    authority.revoked.store(true, Ordering::SeqCst);
    let response = request(&mut https, valid_request.as_bytes()).await;
    let response = String::from_utf8(response).expect("revoked authority text");
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    assert!(response.contains("auth.authority-invalid"), "{response}");
    authority.revoked.store(false, Ordering::SeqCst);

    let mut wss = connect(address, &trusted, SERVER_NAME)
        .await
        .expect("trusted WSS transport");
    let wss_request = format!(
        "GET /v1/sessions/ses_test/attach HTTP/1.1\r\nHost: substrate.test\r\nAuthorization: Bearer {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: AAAAAAAAAAAAAAAAAAAAAA==\r\n\r\n",
        authority.exec
    );
    let response = request(&mut wss, wss_request.as_bytes()).await;
    let response = String::from_utf8(response).expect("WSS refusal text");
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    assert!(response.contains("session.authority-absent"), "{response}");

    let mut plaintext = TcpStream::connect(address).await.expect("plaintext socket");
    plaintext
        .write_all(b"GET /v1/machine HTTP/1.1\r\nHost: substrate.test\r\n\r\n")
        .await
        .expect("plaintext attempt");
    let mut plaintext_answer = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(2),
        plaintext.read_to_end(&mut plaintext_answer),
    )
    .await
    .expect("plaintext connection closes")
    .expect("read plaintext refusal");
    assert!(
        !plaintext_answer.windows(5).any(|bytes| bytes == b"HTTP/"),
        "plaintext received an HTTP response"
    );

    let unknown = connector([current_identity().certificate_der]);
    assert!(
        connect(address, &unknown, SERVER_NAME).await.is_err(),
        "unknown root authenticated the daemon"
    );
    assert!(
        connect(address, &trusted, "wrong.test").await.is_err(),
        "wrong server name authenticated the daemon"
    );

    atomic_replace(&certificate, second.certificate_pem.as_bytes(), 0o644);
    atomic_replace(&private_key, second.private_key_pem.as_bytes(), 0o600);
    signal(&child, Signal::SIGHUP);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let rotated = connect(address, &trusted, SERVER_NAME)
            .await
            .expect("connection during rotation");
        if peer_leaf(&rotated) == second.certificate_der.as_ref() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "new connections never received the rotated identity"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let response = request(&mut existing, valid_request.as_bytes()).await;
    assert!(
        String::from_utf8(response)
            .expect("existing response")
            .starts_with("HTTP/1.1 200"),
        "an existing connection did not keep serving after rotation"
    );
    assert_eq!(peer_leaf(&existing), first.certificate_der.as_ref());

    atomic_replace(&private_key, first.private_key_pem.as_bytes(), 0o600);
    signal(&child, Signal::SIGHUP);
    tokio::time::sleep(Duration::from_millis(250)).await;
    let after_invalid = connect(address, &trusted, SERVER_NAME)
        .await
        .expect("connection after invalid reload");
    assert_eq!(peer_leaf(&after_invalid), second.certificate_der.as_ref());

    signal(&child, Signal::SIGTERM);
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .expect("TLS daemon shutdown timed out")
        .expect("wait for TLS daemon");
    assert!(output.status.success(), "TLS daemon: {output:?}");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(logs.contains("tls.identity-reloaded"), "{logs}");
    assert!(logs.contains("tls.reload-invalid"), "{logs}");
    assert!(!logs.contains(&first.private_key_pem), "private key leaked");
    assert!(
        !logs.contains(&second.private_key_pem),
        "private key leaked"
    );
    assert!(!logs.contains(&authority.observe), "credential leaked");
    for claim in [
        "sensitive-subject-marker",
        "sensitive-actor-marker",
        "sensitive-tenant-marker",
        "sensitive-jti-marker",
    ] {
        assert!(!logs.contains(claim), "authority claim leaked: {claim}");
    }
    let state = rusqlite::Connection::open(root.path().join("state.sqlite"))
        .expect("open daemon state after shutdown");
    let operations: i64 = state
        .query_row("SELECT count(*) FROM operations", [], |row| row.get(0))
        .expect("operation count");
    assert_eq!(
        operations, 0,
        "scope refusal reached durable operation admission"
    );
    authority_task.abort();
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one TLS journey proves mint, channel binding, atomic redemption, replay, and secrecy"
)]
async fn hosted_wss_attachment_authority_is_one_use_and_channel_bound() {
    let root = TempDir::new().expect("temporary root");
    let daemon_identity = current_identity();
    let authority_identity = identity_for(IDENTITY_NAME, (2025, 1, 1), (2035, 1, 1));
    let identity_ca = root.path().join("identity-ca.pem");
    std::fs::write(&identity_ca, &authority_identity.certificate_pem).expect("Identity CA");
    let (authority_state, authority_task) = start_identity_authority(&authority_identity).await;
    let (certificate, private_key) = write_identity(root.path(), &daemon_identity);
    let address = unused_address();
    let mut child = daemon_command(
        root.path(),
        address,
        &certificate,
        &private_key,
        &authority_state.origin,
        &identity_ca,
    )
    .spawn()
    .expect("spawn TLS daemon");
    let trusted = connector([daemon_identity.certificate_der.clone()]);
    let ready = connect_when_ready(&mut child, address, &trusted).await;
    drop(ready);
    let delegated = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT").is_some();
    let session_id = if delegated {
        provision_confined_pipe_session(address, &trusted, &authority_state).await
    } else {
        seed_ready_pipe_session(&root.path().join("state.sqlite"), &authority_state.origin);
        "ses_network_authority".to_owned()
    };

    let signing_key = SigningKey::from_bytes(&[42_u8; 32]);
    let public_key = URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes());
    let mint_body = serde_json::to_string(&json!({
        "public_key": public_key
    }))
    .expect("mint body");
    let mint_request = format!(
        "POST /v1/sessions/{session_id}/attachment-authorities HTTP/1.1\r\nHost: substrate.test\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{mint_body}",
        authority_state.exec,
        mint_body.len()
    );
    let mut mint_connection = connect(address, &trusted, SERVER_NAME)
        .await
        .expect("mint HTTPS connection");
    let minted = request(&mut mint_connection, mint_request.as_bytes()).await;
    assert!(
        minted.starts_with(b"HTTP/1.1 201"),
        "{}",
        String::from_utf8_lossy(&minted)
    );
    let minted = response_json(&minted);
    let authority_id = minted["result"]["authority_id"]
        .as_str()
        .expect("authority id");
    let authority = minted["result"]["authority"]
        .as_str()
        .expect("authority bearer");
    let remaining_seconds = minted["result"]["expires_at"]
        .as_str()
        .expect("authority expiry")
        .parse::<chrono::DateTime<Utc>>()
        .expect("expiry timestamp")
        .timestamp()
        - Utc::now().timestamp();
    assert!(
        (59..=60).contains(&remaining_seconds),
        "mint lifetime was {remaining_seconds} seconds"
    );

    let source = connect(address, &trusted, SERVER_NAME)
        .await
        .expect("proof source connection");
    let source_exporter = channel_exporter(&source);
    let mut target = connect(address, &trusted, SERVER_NAME)
        .await
        .expect("attachment target connection");
    let target_exporter = channel_exporter(&target);
    assert_ne!(source_exporter, target_exporter, "distinct TLS channels");
    let timestamp_ms = Utc::now().timestamp_millis();
    let wrong_proof = signing_key.sign(&session_authority_transcript(
        authority_id,
        &source_exporter,
        timestamp_ms,
    ));
    let wrong_request = attachment_request(
        &authority_state.exec,
        &session_id,
        authority_id,
        authority,
        timestamp_ms,
        &wrong_proof.to_bytes(),
    );
    let response = request(&mut target, wrong_request.as_bytes()).await;
    let response_text = String::from_utf8(response).expect("wrong-channel refusal text");
    assert!(response_text.starts_with("HTTP/1.1 401"), "{response_text}");
    assert!(
        response_text.contains("session.authority-unbound"),
        "{response_text}"
    );

    let timestamp_ms = Utc::now().timestamp_millis();
    let proof = signing_key.sign(&session_authority_transcript(
        authority_id,
        &target_exporter,
        timestamp_ms,
    ));
    let valid_request = attachment_request(
        &authority_state.exec,
        &session_id,
        authority_id,
        authority,
        timestamp_ms,
        &proof.to_bytes(),
    );
    let response = upgrade_request(&mut target, valid_request.as_bytes()).await;
    let response_text = String::from_utf8_lossy(&response);
    assert!(response_text.starts_with("HTTP/1.1 101"), "{response_text}");
    if delegated {
        prove_pipe_bytes(&mut target).await;
    }

    let mut replay = connect(address, &trusted, SERVER_NAME)
        .await
        .expect("replay connection");
    let replay_request = attachment_request(
        &authority_state.exec,
        &session_id,
        authority_id,
        authority,
        Utc::now().timestamp_millis(),
        &proof.to_bytes(),
    );
    let response = request(&mut replay, replay_request.as_bytes()).await;
    let response_text = String::from_utf8(response).expect("replay refusal text");
    assert!(response_text.starts_with("HTTP/1.1 409"), "{response_text}");
    assert!(
        response_text.contains("session.authority-redeemed"),
        "{response_text}"
    );

    signal(&child, Signal::SIGTERM);
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .expect("TLS daemon shutdown timed out")
        .expect("wait for TLS daemon");
    assert!(output.status.success(), "TLS daemon: {output:?}");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!logs.contains(authority), "session authority leaked");
    assert!(
        !logs.contains(&URL_SAFE_NO_PAD.encode(proof.to_bytes())),
        "session proof leaked"
    );
    assert!(!logs.contains(&public_key), "session public key leaked");
    assert!(
        !logs.contains(substrate_wire::SESSION_AUTHORITY_TRANSCRIPT_DOMAIN),
        "session transcript marker leaked"
    );
    authority_task.abort();
}

async fn startup_output(identity: &TestIdentity, key: StartupKey<'_>) -> std::process::Output {
    let root = TempDir::new().expect("temporary root");
    let certificate = root.path().join("identity.pem");
    let private_key = root.path().join("identity.key");
    std::fs::write(&certificate, &identity.certificate_pem).expect("certificate");
    match key {
        StartupKey::Missing => {}
        StartupKey::Material { pem, mode } => {
            std::fs::write(&private_key, pem).expect("private key");
            std::fs::set_permissions(&private_key, std::fs::Permissions::from_mode(mode))
                .expect("private-key permissions");
        }
    }
    daemon_command(
        root.path(),
        unused_address(),
        &certificate,
        &private_key,
        "https://127.0.0.1:9",
        &certificate,
    )
    .output()
    .await
    .expect("run invalid TLS daemon")
}

enum StartupKey<'a> {
    Missing,
    Material { pem: &'a str, mode: u32 },
}

#[tokio::test]
async fn invalid_identity_material_refuses_startup_without_leaking_bytes() {
    let valid = current_identity();
    let missing = startup_output(&valid, StartupKey::Missing).await;
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("tls.private-key-unsafe"),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );

    let unsafe_key = startup_output(
        &valid,
        StartupKey::Material {
            pem: &valid.private_key_pem,
            mode: 0o640,
        },
    )
    .await;
    assert!(!unsafe_key.status.success());
    assert!(
        String::from_utf8_lossy(&unsafe_key.stderr).contains("tls.private-key-unsafe"),
        "{}",
        String::from_utf8_lossy(&unsafe_key.stderr)
    );

    for invalid in [
        identity((2020, 1, 1), (2021, 1, 1)),
        identity((2090, 1, 1), (2091, 1, 1)),
    ] {
        let output = startup_output(
            &invalid,
            StartupKey::Material {
                pem: &invalid.private_key_pem,
                mode: 0o600,
            },
        )
        .await;
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("tls.identity-invalid"), "{stderr}");
        assert!(!stderr.contains(&invalid.certificate_pem));
        assert!(!stderr.contains(&invalid.private_key_pem));
    }

    let other = current_identity();
    let mismatch = startup_output(
        &valid,
        StartupKey::Material {
            pem: &other.private_key_pem,
            mode: 0o600,
        },
    )
    .await;
    assert!(!mismatch.status.success());
    let stderr = String::from_utf8_lossy(&mismatch.stderr);
    assert!(stderr.contains("tls.identity-invalid"), "{stderr}");
    assert!(!stderr.contains(&other.private_key_pem));
}

#[tokio::test]
async fn development_static_bearer_refuses_non_loopback_binding() {
    let root = TempDir::new().expect("temporary root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private daemon root");
    let bearer = root.path().join("bearer");
    std::fs::write(&bearer, format!("dl_substrate_v1_{}", "a".repeat(43))).expect("bearer");
    std::fs::set_permissions(&bearer, std::fs::Permissions::from_mode(0o600))
        .expect("bearer permissions");
    let port = unused_address().port();
    let output = Command::new(env!("CARGO_BIN_EXE_substrate-daemon"))
        .arg("--socket")
        .arg(root.path().join("unused.sock"))
        .arg("--state")
        .arg(root.path().join("state.sqlite"))
        .arg("--workspaces")
        .arg(root.path().join("workspaces"))
        .arg("--deployment")
        .arg("tcp-test")
        .arg("--tcp-listen")
        .arg(format!("0.0.0.0:{port}"))
        .arg("--tcp-bearer-file")
        .arg(&bearer)
        .arg("--tcp-subject")
        .arg("development:test")
        .arg("--tcp-actor")
        .arg("test")
        .arg("--tcp-private-overlay")
        .arg("--tcp-development-only")
        .output()
        .await
        .expect("run development TCP daemon");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "tls.listener-config-invalid: development static-bearer TCP must bind loopback"
        ),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
