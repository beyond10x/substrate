#![cfg(unix)]

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use rcgen::{CertificateParams, KeyPair, date_time_ymd};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

const SERVER_NAME: &str = "substrate.test";
const HOSTED_UNAVAILABLE: &str = "hosted trust envelope admission is not available";

struct TestIdentity {
    certificate_pem: String,
    private_key_pem: String,
    certificate_der: CertificateDer<'static>,
}

fn identity(not_before: (i32, u8, u8), not_after: (i32, u8, u8)) -> TestIdentity {
    let mut params = CertificateParams::new([SERVER_NAME.to_owned()]).expect("certificate params");
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
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
    let (certificate, private_key) = write_identity(root.path(), &first);
    let address = unused_address();
    let mut child = daemon_command(root.path(), address, &certificate, &private_key)
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
    assert!(response.starts_with("HTTP/1.1 503"), "{response}");
    assert!(response.contains(HOSTED_UNAVAILABLE), "{response}");

    let mut wss = connect(address, &trusted, SERVER_NAME)
        .await
        .expect("trusted WSS transport");
    let response = request(
        &mut wss,
        b"GET /v1/pipe-sessions/ses_test/attach HTTP/1.1\r\nHost: substrate.test\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: AAAAAAAAAAAAAAAAAAAAAA==\r\n\r\n",
    )
    .await;
    let response = String::from_utf8(response).expect("WSS refusal text");
    assert!(response.starts_with("HTTP/1.1 503"), "{response}");
    assert!(response.contains(HOSTED_UNAVAILABLE), "{response}");

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

    let response = request(
        &mut existing,
        b"GET /v1/machine HTTP/1.1\r\nHost: substrate.test\r\nConnection: keep-alive\r\n\r\n",
    )
    .await;
    assert!(
        String::from_utf8(response)
            .expect("existing response")
            .starts_with("HTTP/1.1 503"),
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
    daemon_command(root.path(), unused_address(), &certificate, &private_key)
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
