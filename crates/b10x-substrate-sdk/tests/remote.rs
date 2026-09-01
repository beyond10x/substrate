#![cfg(unix)]

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use b10x_substrate_sdk::{AccessToken, AccessTokenReason, ExecutionPolicy, PipeFrame, SdkError};
use rcgen::{CertificateParams, KeyPair, date_time_ymd};
use rustls::ServerConfig;
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio_rustls::TlsAcceptor;

const SERVER_NAME: &str = "substrate.test";

struct TestIdentity {
    certificate_pem: String,
    private_key_pem: String,
}

fn identity(name: &str) -> TestIdentity {
    let mut params = CertificateParams::new([name.to_owned()]).expect("certificate parameters");
    params.not_before = date_time_ymd(2025, 1, 1);
    params.not_after = date_time_ymd(2035, 1, 1);
    let key = KeyPair::generate().expect("private key");
    let certificate = params.self_signed(&key).expect("self-signed certificate");
    TestIdentity {
        certificate_pem: certificate.pem(),
        private_key_pem: key.serialize_pem(),
    }
}

fn credential(fill: char) -> String {
    format!("identity_access_v1_{}", fill.to_string().repeat(43))
}

struct Authority {
    origin: String,
    valid: String,
    invalid: String,
    expired: String,
    task: tokio::task::JoinHandle<()>,
}

impl Authority {
    async fn start(root: &Path) -> Self {
        let identity = identity("127.0.0.1");
        let ca = root.join("identity-ca.pem");
        std::fs::write(&ca, &identity.certificate_pem).expect("Identity CA");
        let mut certificates = rustls_pemfile::certs(&mut identity.certificate_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .expect("Identity certificate");
        let key = rustls_pemfile::private_key(&mut identity.private_key_pem.as_bytes())
            .expect("Identity private key parse")
            .expect("Identity private key");
        let mut config = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_no_client_auth()
            .with_single_cert(std::mem::take(&mut certificates), key)
            .expect("Identity TLS config");
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Identity listener");
        let address = listener.local_addr().expect("Identity address");
        let origin = format!("https://{address}");
        let valid = credential('v');
        let invalid = credential('i');
        let expired = credential('x');
        let task_origin = origin.clone();
        let task_valid = valid.clone();
        let task_expired = expired.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                let origin = task_origin.clone();
                let valid = task_valid.clone();
                let expired = task_expired.clone();
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let Ok(request) = read_head(&mut stream).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
                    let valid_header = format!("authorization: bearer {}", valid.to_lowercase());
                    let expired_header =
                        format!("authorization: bearer {}", expired.to_lowercase());
                    let (status, body) = if request.contains(&valid_header) {
                        ("200 OK", authority_body(&origin, false))
                    } else if request.contains(&expired_header) {
                        ("200 OK", authority_body(&origin, true))
                    } else {
                        ("401 Unauthorized", Vec::new())
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        Self {
            origin,
            valid,
            invalid,
            expired,
            task,
        }
    }
}

fn authority_body(origin: &str, is_expired: bool) -> Vec<u8> {
    let now = chrono::Utc::now().timestamp();
    let (issued, expiry) = if is_expired {
        (now - 300, now - 1)
    } else {
        (now, now + 300)
    };
    serde_json::to_vec(&serde_json::json!({
        "iss": origin,
        "sub": "remote-sdk-subject",
        "aud": "urn:b10x:substrate",
        "iat": issued,
        "nbf": issued,
        "exp": expiry,
        "jti": "remote-sdk-jti",
        "act": {"sub": "remote-sdk-actor"},
        "scope": "exec observe workspaces",
        "principal_kind": "human",
        "tenant_id": "remote-sdk-tenant",
        "email": null,
        "groups": []
    }))
    .expect("authority JSON")
}

async fn read_head(stream: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let mut block = [0_u8; 1024];
        let count = stream.read(&mut block).await?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&block[..count]);
        if bytes.len() > 16 * 1024 {
            return Err(std::io::Error::other("request head exceeds test bound"));
        }
    }
    Ok(bytes)
}

fn daemon_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("SUBSTRATE_TEST_DAEMON") {
        return path.into();
    }
    let mut path = std::env::current_exe().expect("current test executable");
    path.pop();
    if path.file_name().is_some_and(|name| name == "deps") {
        path.pop();
    }
    path.push("substrate-daemon");
    path
}

fn unused_address() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral listener");
    let address = listener.local_addr().expect("ephemeral address");
    drop(listener);
    address
}

struct Fixture {
    root: TempDir,
    child: Child,
    authority: Authority,
    endpoint: String,
    daemon_ca: PathBuf,
}

impl Fixture {
    async fn start() -> Self {
        let root = TempDir::new().expect("temporary remote fixture");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private fixture root");
        let authority = Authority::start(root.path()).await;
        let daemon_identity = identity(SERVER_NAME);
        let daemon_ca = root.path().join("daemon-ca.pem");
        let certificate = root.path().join("daemon.pem");
        let private_key = root.path().join("daemon.key");
        std::fs::write(&daemon_ca, &daemon_identity.certificate_pem).expect("daemon CA");
        std::fs::write(&certificate, &daemon_identity.certificate_pem).expect("daemon certificate");
        std::fs::write(&private_key, &daemon_identity.private_key_pem).expect("daemon key");
        std::fs::set_permissions(&private_key, std::fs::Permissions::from_mode(0o600))
            .expect("private daemon key");
        let address = unused_address();
        let mut command = Command::new(daemon_binary());
        command
            .arg("--socket")
            .arg(root.path().join("unused.sock"))
            .arg("--state")
            .arg(root.path().join("state.sqlite"))
            .arg("--workspaces")
            .arg(root.path().join("workspaces"))
            .arg("--deployment")
            .arg("sdk-remote-test")
            .arg("--tls-listen")
            .arg(address.to_string())
            .arg("--tls-certificate-chain")
            .arg(&certificate)
            .arg("--tls-private-key")
            .arg(&private_key)
            .arg("--hosted-identity-origin")
            .arg(&authority.origin)
            .arg("--hosted-identity-ca-bundle")
            .arg(root.path().join("identity-ca.pem"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cgroup) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") {
            command.arg("--cgroup-root").arg(cgroup);
        }
        let mut child = command.spawn().expect("spawn remote daemon");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().expect("inspect remote daemon") {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    pipe.read_to_string(&mut stderr)
                        .await
                        .expect("read daemon startup error");
                }
                panic!("remote daemon exited before readiness: {status}: {stderr}");
            }
            if TcpStream::connect(address).await.is_ok() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "remote daemon readiness timeout"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Self {
            root,
            child,
            authority,
            endpoint: format!("https://{address}/"),
            daemon_ca,
        }
    }

    async fn stop(self) {
        let pid = i32::try_from(self.child.id().expect("daemon pid")).expect("pid fits");
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        )
        .expect("signal daemon");
        let output = tokio::time::timeout(Duration::from_secs(10), self.child.wait_with_output())
            .await
            .expect("daemon shutdown deadline")
            .expect("daemon shutdown");
        assert!(output.status.success(), "remote daemon: {output:?}");
        let diagnostics = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        for secret in [
            &self.authority.valid,
            &self.authority.invalid,
            &self.authority.expired,
        ] {
            assert!(
                !diagnostics.contains(secret),
                "credential leaked to diagnostics"
            );
        }
        self.authority.task.abort();
    }
}

#[tokio::test]
async fn remote_https_requires_exact_tls_and_refreshes_one_invalid_credential() {
    let fixture = Fixture::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let valid = fixture.authority.valid.clone();
    let invalid = fixture.authority.invalid.clone();
    let provider_calls = Arc::clone(&calls);
    let provider_observed = Arc::clone(&observed);
    let provider = move |reason: AccessTokenReason| {
        let call = provider_calls.fetch_add(1, Ordering::SeqCst);
        provider_observed
            .lock()
            .expect("provider observations")
            .push(reason);
        let value = if call == 0 {
            invalid.clone()
        } else {
            valid.clone()
        };
        async move { AccessToken::new(value) }
    };
    let client = b10x_substrate_sdk::Client::builder()
        .https_endpoint(&fixture.endpoint)
        .trust_roots(&fixture.daemon_ca)
        .server_identity(SERVER_NAME)
        .token_provider(provider)
        .connect()
        .await
        .expect("remote SDK connects after one refresh");
    assert_eq!(
        &*observed.lock().expect("provider observations"),
        &[
            AccessTokenReason::Request,
            AccessTokenReason::RefreshAfterAuthorizationFailure,
        ]
    );
    let workspace = client
        .workspace()
        .empty()
        .create()
        .await
        .expect("remote typed workspace create");
    assert!(workspace.id().starts_with("ws_"));
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    let unknown = identity("unknown.test");
    let unknown_ca = fixture.root.path().join("unknown-ca.pem");
    std::fs::write(&unknown_ca, unknown.certificate_pem).expect("unknown CA");
    let valid = fixture.authority.valid.clone();
    let result = b10x_substrate_sdk::Client::builder()
        .https_endpoint(&fixture.endpoint)
        .trust_roots(&unknown_ca)
        .server_identity(SERVER_NAME)
        .token_provider(move |_| {
            let valid = valid.clone();
            async move { AccessToken::new(valid) }
        })
        .connect()
        .await;
    let Err(error) = result else {
        panic!("unknown root must fail");
    };
    assert!(matches!(error, SdkError::Transport(_)), "{error}");

    let valid = fixture.authority.valid.clone();
    let result = b10x_substrate_sdk::Client::builder()
        .https_endpoint(&fixture.endpoint)
        .trust_roots(&fixture.daemon_ca)
        .server_identity("wrong.test")
        .token_provider(move |_| {
            let valid = valid.clone();
            async move { AccessToken::new(valid) }
        })
        .connect()
        .await;
    let Err(error) = result else {
        panic!("wrong server identity must fail");
    };
    assert!(matches!(error, SdkError::Transport(_)), "{error}");

    let expired = fixture.authority.expired.clone();
    let result = b10x_substrate_sdk::Client::builder()
        .https_endpoint(&fixture.endpoint)
        .trust_roots(&fixture.daemon_ca)
        .server_identity(SERVER_NAME)
        .token_provider(move |_| {
            let expired = expired.clone();
            async move { AccessToken::new(expired) }
        })
        .connect()
        .await;
    let Err(error) = result else {
        panic!("expired authority must fail");
    };
    let SdkError::Refusal(refusal) = error else {
        panic!("expired authority lost its named refusal: {error}");
    };
    assert_eq!(refusal.code, "auth.authority-invalid");
    fixture.stop().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn delegated_remote_wss_round_trips_and_a_reconnect_never_reuses_authority() {
    if std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT").is_none() {
        eprintln!("delegated remote SDK WSS lane absent: SUBSTRATE_VECTORS_CGROUP_ROOT is not set");
        return;
    }
    let fixture = Fixture::start().await;
    let valid = fixture.authority.valid.clone();
    let client = b10x_substrate_sdk::Client::builder()
        .https_endpoint(&fixture.endpoint)
        .trust_roots(&fixture.daemon_ca)
        .server_identity(SERVER_NAME)
        .token_provider(move |_| {
            let valid = valid.clone();
            async move { AccessToken::new(valid) }
        })
        .connect()
        .await
        .expect("remote delegated client");
    let workspace = client
        .workspace()
        .empty()
        .create()
        .await
        .expect("remote workspace");
    let policy = ExecutionPolicy::builder()
        .timeout(Duration::from_secs(30))
        .cpu_time(Duration::from_secs(5))
        .memory_bytes(64 * 1024 * 1024)
        .processes(16)
        .output_bytes(64 * 1024)
        .build()
        .expect("session policy");
    let session = workspace
        .pipe_session("/usr/bin/sh")
        .args(["-c", "printf remote-ready; cat"])
        .policy(policy)
        .lease(Duration::from_secs(20))
        .input_limit_bytes(4096)
        .frame_limit_bytes(4096)
        .queued_frames(16)
        .start()
        .await
        .expect("remote pipe session");
    let mut channel = session.attach().await.expect("proof-bound WSS attachment");
    channel
        .write(b"remote-round-trip\n")
        .await
        .expect("WSS input");
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = channel.next_frame().await.expect("WSS frame") {
            if let PipeFrame::Output { bytes, .. } = frame {
                output.extend(bytes);
                if output
                    .windows(b"remote-round-trip".len())
                    .any(|window| window == b"remote-round-trip")
                {
                    break;
                }
            }
        }
    })
    .await
    .expect("remote WSS output deadline");
    drop(channel);
    let result = session.attach().await;
    let Err(second) = result else {
        panic!("one attachment cannot be resumed");
    };
    let SdkError::Refusal(refusal) = &second else {
        panic!("fresh reconnect authority lost its named refusal: {second}");
    };
    assert_eq!(
        refusal.code, "session.not-attachable",
        "a reconnect must mint a fresh authority instead of replaying the redeemed one"
    );
    fixture.stop().await;
}
