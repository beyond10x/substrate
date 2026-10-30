//! Design 31 § 7: the clean-room runner against the shipped daemon on its production TLS listener.
//!
//! The fixture is a copy of `crates/b10x-substrate-sdk/tests/remote.rs`: an `rcgen` certificate for
//! the daemon, an in-process Identity stub, and the shipped `substrate-daemon` found as an
//! executable (`SUBSTRATE_TEST_DAEMON`, or the sibling of this test executable), never linked. It
//! adds `--event-retention 32`, a loopback `--ops-listen` and a supervisor that starts the daemon
//! again, on the same state, certificate and port, whenever it dies.
//!
//! This target has its own `main` (`harness = false`), because the runner's token command and
//! restart hook are this same executable started with `helper-token` or `helper-hook`, and a token
//! command's standard output is its credential line, which libtest's own output would precede.
//! The scenarios run one after another and the binary ends with libtest's summary line.

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{BufRead as _, Write as _};
use std::net::SocketAddr;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rcgen::{CertificateParams, KeyPair, date_time_ymd};
use rustls::ServerConfig;
use serde_json::Value;
use sha2::Digest as _;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio_rustls::TlsAcceptor;

#[path = "../../../test-support/readiness.rs"]
mod readiness;

const SERVER_NAME: &str = "substrate.test";
/// The name a headless Service would carry (ADR 0046 K2): the daemon's certificate must not.
const FORBIDDEN_NAME: &str = "substrate-headless.test";
const RUNNER: &str = env!("CARGO_BIN_EXE_b10x-substrate-conformance");
const SCHEMA: &str = include_str!("../schema/remote-conformance-report.v1.schema.json");
const OPERATIONS: &str = include_str!("../../../contracts/substrate-wire/0.25.0/operations.json");
/// A variable only the token command may receive.
const TOKEN_MARK: &str = "B10X_CONFORMANCE_TEST_TOKEN_MARK";
/// A variable only the restart hook may receive.
const HOOK_MARK: &str = "B10X_CONFORMANCE_TEST_HOOK_MARK";
/// The bound design 31 § 7 sets on one runner process.
const RUNNER_BOUND: Duration = Duration::from_secs(90);
const CREDENTIAL_PREFIX: &str = "identity_access_v1_";

/// Pinned portable totals: every case the portable daemon's facts select passed, and exactly the
/// exec, resource, session and restart-exec cases are absent (design 31 § 7).
const PORTABLE_PASSED: u64 = 42;
const PORTABLE_ABSENT: [&str; 11] = [
    "exec.no-egress",
    "exec.retire",
    "exec.run-output",
    "exec.signal",
    "resources.metrics-names-exec",
    "resources.usage-observed",
    "restart.exec-no-redispatch",
    "restart.exec-unknown",
    "sessions.pipe-attach",
    "sessions.pty-resize",
    "sessions.second-attach-refused",
];

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().collect();
    match arguments.get(1).map(String::as_str) {
        Some("helper-token") => return helper_token(&arguments[2..]),
        Some("helper-hook") => return helper_hook(&arguments[2..]),
        _ => {}
    }
    let scenarios: [(&str, fn()); 2] = [
        (
            "portable_runner_certifies_the_shipped_daemon_and_keeps_no_credential",
            portable,
        ),
        (
            "delegated_runner_executes_the_exec_session_resource_and_restart_groups",
            delegated,
        ),
    ];
    println!("\nrunning {} tests", scenarios.len());
    let mut failed = Vec::new();
    for (name, scenario) in scenarios {
        let started = Instant::now();
        let outcome = std::panic::catch_unwind(scenario);
        if outcome.is_ok() {
            println!("test {name} ... ok ({} ms)", started.elapsed().as_millis());
        } else {
            println!("test {name} ... FAILED");
            failed.push(name);
        }
    }
    let passed = scenarios.len() - failed.len();
    println!(
        "\ntest result: {}. {passed} passed; {} failed; 0 ignored; 0 measured; 0 filtered out\n",
        if failed.is_empty() { "ok" } else { "FAILED" },
        failed.len()
    );
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(101)
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("test runtime")
}

// ---------------------------------------------------------------------------------------------
// Helpers: this executable as the runner's token command and restart hook.
// ---------------------------------------------------------------------------------------------

/// The descriptors this process holds, without the one that lists them.
fn open_descriptors() -> Vec<String> {
    let own = format!("/proc/{}/fd", std::process::id());
    let mut descriptors = Vec::new();
    for entry in std::fs::read_dir("/proc/self/fd").expect("descriptor list") {
        let entry = entry.expect("descriptor entry");
        let target = std::fs::read_link(entry.path())
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        if target == own || target.is_empty() {
            continue;
        }
        descriptors.push(entry.file_name().to_string_lossy().into_owned());
    }
    descriptors.sort();
    descriptors
}

fn record(records: &Path, role: &str) {
    let environment: BTreeMap<String, String> = std::env::vars().collect();
    let document = serde_json::json!({
        "environment": environment,
        "descriptors": open_descriptors(),
    });
    std::fs::write(
        records.join(format!("{role}-{}.json", std::process::id())),
        serde_json::to_vec(&document).expect("record JSON"),
    )
    .expect("helper record");
}

/// `helper-token <records> <serve|malformed>`: one request line in, one credential line out.
fn helper_token(arguments: &[String]) -> ExitCode {
    let records = PathBuf::from(&arguments[0]);
    record(&records, "token");
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .expect("token request");
    let request: Value = serde_json::from_str(&line).expect("token request JSON");
    if arguments[1] == "malformed" {
        println!("{CREDENTIAL_PREFIX}short");
        return ExitCode::SUCCESS;
    }
    let credentials: Value = serde_json::from_slice(
        &std::fs::read(records.join("credentials.json")).expect("credentials"),
    )
    .expect("credentials JSON");
    let purpose = request["purpose"].as_str().expect("purpose");
    println!("{}", credentials[purpose].as_str().expect("credential"));
    ExitCode::SUCCESS
}

/// `helper-hook <records>`: the crash design 31 § 7 asks for, `kill -KILL` of the daemon.
fn helper_hook(arguments: &[String]) -> ExitCode {
    let records = PathBuf::from(&arguments[0]);
    record(&records, "hook");
    let pid: i32 = std::fs::read_to_string(records.join("daemon.pid"))
        .expect("daemon pid file")
        .trim()
        .parse()
        .expect("daemon pid");
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("kill the daemon");
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------------------------
// The fixture: Identity stub and a supervised daemon.
// ---------------------------------------------------------------------------------------------

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

fn random_body() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bytes = [0_u8; 43];
    getrandom::fill(&mut bytes).expect("randomness");
    bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect()
}

fn server_config(identity: &TestIdentity) -> Arc<ServerConfig> {
    let certificates = rustls_pemfile::certs(&mut identity.certificate_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("certificate");
    let key = rustls_pemfile::private_key(&mut identity.private_key_pem.as_bytes())
        .expect("private key parse")
        .expect("private key");
    let mut config = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .expect("TLS config");
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

/// The in-process Identity stub: the primary sentinel resolves to all three hosted scopes, the
/// narrowed one to `observe` only, anything else is unknown.
struct Authority {
    origin: String,
    primary: String,
    narrowed: String,
    task: tokio::task::JoinHandle<()>,
}

impl Authority {
    async fn start(root: &Path) -> Self {
        let identity = identity("127.0.0.1");
        std::fs::write(root.join("identity-ca.pem"), &identity.certificate_pem)
            .expect("Identity CA");
        let acceptor = TlsAcceptor::from(server_config(&identity));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Identity listener");
        let origin = format!(
            "https://{}",
            listener.local_addr().expect("Identity address")
        );
        let primary = format!("{CREDENTIAL_PREFIX}{}", random_body());
        let narrowed = format!("{CREDENTIAL_PREFIX}{}", random_body());
        let (task_origin, task_primary, task_narrowed) =
            (origin.clone(), primary.clone(), narrowed.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                let origin = task_origin.clone();
                let primary = format!("authorization: bearer {}", task_primary.to_lowercase());
                let narrowed = format!("authorization: bearer {}", task_narrowed.to_lowercase());
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let Ok(request) = read_head(&mut stream).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
                    let (status, body) = if request.contains(&primary) {
                        ("200 OK", authority_body(&origin, "exec observe workspaces"))
                    } else if request.contains(&narrowed) {
                        ("200 OK", authority_body(&origin, "observe"))
                    } else {
                        ("401 Unauthorized", Vec::new())
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        Self {
            origin,
            primary,
            narrowed,
            task,
        }
    }
}

fn authority_body(origin: &str, scope: &str) -> Vec<u8> {
    let now = chrono::Utc::now().timestamp();
    serde_json::to_vec(&serde_json::json!({
        "iss": origin,
        "sub": "conformance-subject",
        "aud": "urn:b10x:substrate",
        "iat": now,
        "nbf": now,
        "exp": now + 300,
        "jti": format!("conformance-jti-{}", random_body()),
        "act": {"sub": "conformance-actor"},
        "scope": scope,
        "principal_kind": "human",
        "tenant_id": "conformance-tenant",
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

fn private_directory(path: &Path) {
    std::fs::create_dir_all(path).expect("directory");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("private mode");
}

/// The shipped daemon on its production TLS listener, started again whenever it dies.
struct Fixture {
    root: tempfile::TempDir,
    records: PathBuf,
    authority: Authority,
    endpoint: String,
    ops_origin: String,
    daemon_ca: PathBuf,
    starts: Arc<AtomicUsize>,
    stopping: Arc<AtomicBool>,
    supervisor: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start(cgroup_root: Option<&Path>) -> Self {
        let binary = daemon_binary();
        assert!(
            binary.is_file(),
            "the shipped daemon is missing at {}: build it with `cargo build -p b10x-substrate-daemon --bin substrate-daemon` or set SUBSTRATE_TEST_DAEMON",
            binary.display()
        );
        let root = tempfile::TempDir::new().expect("fixture root");
        private_directory(root.path());
        let records = root.path().join("records");
        private_directory(&records);
        let authority = Authority::start(root.path()).await;
        std::fs::write(
            records.join("credentials.json"),
            serde_json::to_vec(&serde_json::json!({
                "primary": authority.primary,
                "narrowed": authority.narrowed,
            }))
            .expect("credentials JSON"),
        )
        .expect("credentials file");
        std::fs::set_permissions(
            records.join("credentials.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("private credentials");
        let daemon = identity(SERVER_NAME);
        let daemon_ca = root.path().join("daemon-ca.pem");
        let certificate = root.path().join("daemon.pem");
        let private_key = root.path().join("daemon.key");
        std::fs::write(&daemon_ca, &daemon.certificate_pem).expect("daemon CA");
        std::fs::write(&certificate, &daemon.certificate_pem).expect("daemon certificate");
        std::fs::write(&private_key, &daemon.private_key_pem).expect("daemon key");
        std::fs::set_permissions(&private_key, std::fs::Permissions::from_mode(0o600))
            .expect("private daemon key");
        let address = unused_address();
        let ops = unused_address();
        let mut arguments: Vec<std::ffi::OsString> = vec![
            "--socket".into(),
            root.path().join("unused.sock").into(),
            "--state".into(),
            root.path().join("state.sqlite").into(),
            "--workspaces".into(),
            root.path().join("workspaces").into(),
            "--deployment".into(),
            "conformance-test".into(),
            "--event-retention".into(),
            "32".into(),
            "--tls-listen".into(),
            address.to_string().into(),
            "--tls-certificate-chain".into(),
            certificate.into(),
            "--tls-private-key".into(),
            private_key.into(),
            "--hosted-identity-origin".into(),
            authority.origin.clone().into(),
            "--hosted-identity-ca-bundle".into(),
            root.path().join("identity-ca.pem").into(),
            "--ops-listen".into(),
            ops.to_string().into(),
        ];
        if let Some(cgroup) = cgroup_root {
            arguments.push("--cgroup-root".into());
            arguments.push(cgroup.into());
        }
        let log = root.path().join("daemon.log");
        let starts = Arc::new(AtomicUsize::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let mut child = spawn_daemon(&binary, &arguments, &log, &records);
        starts.fetch_add(1, Ordering::SeqCst);
        wait_ready(&mut child, address, &log).await;
        let supervisor = {
            let (starts, stopping, records) =
                (Arc::clone(&starts), Arc::clone(&stopping), records.clone());
            // The child is `kill_on_drop`: a panicking scenario drops the runtime, the task and
            // with it the daemon.
            tokio::spawn(async move {
                loop {
                    let _ = child.wait().await;
                    if stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    child = spawn_daemon(&binary, &arguments, &log, &records);
                    starts.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        Self {
            root,
            records,
            authority,
            endpoint: format!("https://{address}/"),
            ops_origin: format!("http://{ops}"),
            daemon_ca,
            starts,
            stopping,
            supervisor,
        }
    }

    fn credentials(&self) -> [&str; 2] {
        [&self.authority.primary, &self.authority.narrowed]
    }

    async fn stop(self) {
        self.stopping.store(true, Ordering::SeqCst);
        let pid: i32 = std::fs::read_to_string(self.records.join("daemon.pid"))
            .expect("daemon pid")
            .trim()
            .parse()
            .expect("pid");
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        );
        tokio::time::timeout(Duration::from_secs(20), self.supervisor)
            .await
            .expect("daemon shutdown deadline")
            .expect("supervisor");
        self.authority.task.abort();
        let log = std::fs::read_to_string(self.root.path().join("daemon.log")).unwrap_or_default();
        for secret in [&self.authority.primary, &self.authority.narrowed] {
            assert!(
                !log.contains(secret.as_str()),
                "credential leaked to daemon diagnostics"
            );
        }
    }
}

fn spawn_daemon(
    binary: &Path,
    arguments: &[std::ffi::OsString],
    log: &Path,
    records: &Path,
) -> tokio::process::Child {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .expect("daemon log");
    let child = Command::new(binary)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .kill_on_drop(true)
        .spawn()
        .expect("spawn the shipped daemon");
    std::fs::write(
        records.join("daemon.pid"),
        child.id().expect("daemon pid").to_string(),
    )
    .expect("daemon pid file");
    child
}

async fn wait_ready(child: &mut tokio::process::Child, address: SocketAddr, log: &Path) {
    let deadline = tokio::time::Instant::now() + readiness::DAEMON_READINESS;
    loop {
        if let Some(status) = child.try_wait().expect("inspect daemon") {
            panic!(
                "daemon exited before readiness: {status}: {}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
        }
        if TcpStream::connect(address).await.is_ok() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon readiness timeout: {}",
            std::fs::read_to_string(log).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------------------------
// Running the runner.
// ---------------------------------------------------------------------------------------------

/// The fresh, empty directories the runner gets as its home, temporary and XDG directories and as
/// its working directory; each must still be empty afterwards.
struct Clean {
    _root: tempfile::TempDir,
    directories: Vec<(String, PathBuf)>,
    cwd: PathBuf,
}

impl Clean {
    fn new() -> Self {
        let root = tempfile::TempDir::new().expect("clean root");
        let mut directories = Vec::new();
        for name in [
            "HOME",
            "TMPDIR",
            "XDG_CACHE_HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_RUNTIME_DIR",
        ] {
            let path = root.path().join(name.to_lowercase());
            private_directory(&path);
            directories.push((name.to_owned(), path));
        }
        let cwd = root.path().join("cwd");
        private_directory(&cwd);
        Self {
            _root: root,
            directories,
            cwd,
        }
    }

    fn assert_empty(&self) {
        for path in self
            .directories
            .iter()
            .map(|(_, path)| path)
            .chain([&self.cwd])
        {
            let entries: Vec<_> = std::fs::read_dir(path)
                .expect("clean directory")
                .map(|entry| entry.expect("entry").file_name())
                .collect();
            assert!(
                entries.is_empty(),
                "the runner wrote outside --out: {} holds {entries:?}",
                path.display()
            );
        }
    }
}

struct Outcome {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run_runner(clean: &Clean, arguments: &[String]) -> Outcome {
    wait_runner(spawn_runner(clean, arguments)).await
}

fn spawn_runner(clean: &Clean, arguments: &[String]) -> tokio::process::Child {
    let mut command = Command::new(RUNNER);
    command
        .args(arguments)
        .env_clear()
        .env(TOKEN_MARK, "token-only")
        .env(HOOK_MARK, "hook-only")
        .current_dir(&clean.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (name, path) in &clean.directories {
        command.env(name, path);
    }
    command.spawn().expect("spawn the runner")
}

async fn wait_runner(child: tokio::process::Child) -> Outcome {
    let output = tokio::time::timeout(RUNNER_BOUND, child.wait_with_output())
        .await
        .expect("the runner did not exit within 90 s and was killed")
        .expect("runner output");
    Outcome {
        code: output.status.code().expect("runner exit code"),
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

fn token_arguments(fixture: &Fixture, mode: &str) -> Vec<String> {
    let me = std::env::current_exe().expect("this executable");
    vec![
        "--token-command".into(),
        me.display().to_string(),
        "--token-command-arg".into(),
        "helper-token".into(),
        "--token-command-arg".into(),
        fixture.records.display().to_string(),
        "--token-command-arg".into(),
        mode.into(),
        "--token-env".into(),
        TOKEN_MARK.into(),
    ]
}

fn hook_arguments(fixture: &Fixture) -> Vec<String> {
    let me = std::env::current_exe().expect("this executable");
    vec![
        "--restart-hook".into(),
        me.display().to_string(),
        "--restart-hook-arg".into(),
        "helper-hook".into(),
        "--restart-hook-arg".into(),
        fixture.records.display().to_string(),
        "--hook-env".into(),
        HOOK_MARK.into(),
    ]
}

fn endpoint_arguments(fixture: &Fixture, out: &Path) -> Vec<String> {
    vec![
        "--endpoint".into(),
        fixture.endpoint.clone(),
        "--trust-roots".into(),
        fixture.daemon_ca.display().to_string(),
        "--server-name".into(),
        SERVER_NAME.into(),
        "--out".into(),
        out.display().to_string(),
        "--deadline".into(),
        "60".into(),
        "--io-timeout".into(),
        "5".into(),
        "--restart-timeout".into(),
        "30".into(),
    ]
}

/// No byte the runner produced names a credential, its 43-character body or the format prefix.
fn assert_no_credential(fixture: &Fixture, label: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    for credential in fixture.credentials() {
        let body = &credential[CREDENTIAL_PREFIX.len()..];
        assert!(!text.contains(credential), "{label} carries a credential");
        assert!(!text.contains(body), "{label} carries a credential body");
    }
    assert!(
        !text.contains(CREDENTIAL_PREFIX),
        "{label} carries the credential format prefix"
    );
}

/// `--out` holds exactly the report and its digest file, in a 0700 directory; the digest file is
/// `sha256sum` form over the report's bytes. Returns the report.
fn read_report(fixture: &Fixture, out: &Path) -> Value {
    let mode = std::fs::metadata(out).expect("--out").permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "--out is created private");
    let names: BTreeSet<String> = std::fs::read_dir(out)
        .expect("--out listing")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        BTreeSet::from(["report.json".to_owned(), "report.json.sha256".to_owned()])
    );
    let bytes = std::fs::read(out.join("report.json")).expect("report");
    assert_no_credential(fixture, "report.json", &bytes);
    let digest = sha2::Sha256::digest(&bytes)
        .iter()
        .fold(String::new(), |mut text, byte| {
            write!(text, "{byte:02x}").expect("hex digit");
            text
        });
    assert_eq!(
        std::fs::read_to_string(out.join("report.json.sha256")).expect("digest file"),
        format!("{digest}  report.json\n")
    );
    let report: Value = serde_json::from_slice(&bytes).expect("report JSON");
    let mut canonical = serde_json::to_string_pretty(&sorted(&report)).expect("canonical");
    canonical.push('\n');
    assert_eq!(
        String::from_utf8(bytes).expect("UTF-8 report"),
        canonical,
        "the report is written in deterministic form: sorted keys, two-space indent, newline"
    );
    let schema: Value = serde_json::from_str(SCHEMA).expect("schema JSON");
    jsonschema::draft202012::meta::validate(&schema).expect("the schema meta-validates");
    let validator = jsonschema::validator_for(&schema).expect("schema validator");
    let errors: Vec<String> = validator
        .iter_errors(&report)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "report is outside its schema: {errors:?}"
    );
    assert_every_operation_accounted(&report);
    report
}

/// Every operation of the pinned registry is named by a case of the report or listed under
/// `not_covered`, and none is both.
fn assert_every_operation_accounted(report: &Value) {
    let registry: Value = serde_json::from_str(OPERATIONS).expect("pinned registry");
    let named: BTreeSet<&str> = report["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .flat_map(|case| {
            case["requires"]["operations"]
                .as_array()
                .expect("requires.operations")
                .iter()
                .filter_map(Value::as_str)
        })
        .collect();
    let listed: BTreeSet<&str> = report["not_covered"]
        .as_array()
        .expect("not_covered")
        .iter()
        .filter_map(|entry| entry["operation"].as_str())
        .collect();
    for operation in registry["operations"].as_array().expect("operations") {
        let id = operation["id"].as_str().expect("operation id");
        assert!(
            named.contains(id) != listed.contains(id),
            "story:remote-clean-room-conformance: registry operation {id} is named by a case: \
             {}, listed under not_covered: {}",
            named.contains(id),
            listed.contains(id)
        );
    }
}

fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), sorted(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// What each helper process recorded, by role.
fn records(fixture: &Fixture, role: &str) -> Vec<Value> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&fixture.records).expect("records") {
        let entry = entry.expect("record");
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!("{role}-")) {
            found.push(
                serde_json::from_slice(&std::fs::read(entry.path()).expect("record file"))
                    .expect("record JSON"),
            );
            std::fs::remove_file(entry.path()).expect("consume record");
        }
    }
    found
}

fn assert_helper_records(fixture: &Fixture, role: &str, forbidden_variable: &str) {
    let found = records(fixture, role);
    assert!(!found.is_empty(), "the {role} never ran");
    for record in &found {
        assert_eq!(
            record["descriptors"],
            serde_json::json!(["0", "1", "2"]),
            "the {role} inherited a descriptor beyond 0 to 2: {record}"
        );
        let environment = record["environment"].as_object().expect("environment");
        assert!(
            !environment.contains_key(forbidden_variable),
            "the {role} received {forbidden_variable}"
        );
        assert_no_credential(fixture, role, record.to_string().as_bytes());
    }
}

fn case_ids(report: &Value, status: &str) -> BTreeSet<String> {
    report["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|case| case["status"] == status)
        .map(|case| case["id"].as_str().expect("case id").to_owned())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Scenarios.
// ---------------------------------------------------------------------------------------------

fn portable() {
    runtime().block_on(portable_scenarios());
}

#[allow(clippy::too_many_lines)]
async fn portable_scenarios() {
    let fixture = Fixture::start(None).await;
    let scratch = tempfile::TempDir::new().expect("scratch");

    // The full run: every group the daemon's facts select, one crash restart.
    let clean = Clean::new();
    let out = scratch.path().join("full");
    let mut arguments = endpoint_arguments(&fixture, &out);
    arguments.extend(token_arguments(&fixture, "serve"));
    arguments.extend(hook_arguments(&fixture));
    arguments.extend([
        "--forbidden-name".into(),
        FORBIDDEN_NAME.into(),
        "--ops-origin".into(),
        fixture.ops_origin.clone(),
    ]);
    let started = Instant::now();
    let outcome = run_runner(&clean, &arguments).await;
    eprintln!(
        "conformance portable run: {} ms",
        started.elapsed().as_millis()
    );
    assert_no_credential(&fixture, "stdout", &outcome.stdout);
    assert_no_credential(&fixture, "stderr", &outcome.stderr);
    let report = read_report(&fixture, &out);
    assert_eq!(
        outcome.code,
        0,
        "runner exit; stderr: {}; failed: {:?}",
        String::from_utf8_lossy(&outcome.stderr),
        report["cases"].as_array().map(|cases| cases
            .iter()
            .filter(|case| case["status"] == "failed")
            .collect::<Vec<_>>())
    );
    assert_eq!(report["verdict"], "passed");
    assert_eq!(report["run"]["partial"], false);
    assert_eq!(report["totals"]["failed"], 0);
    assert_eq!(
        report["totals"]["passed"],
        PORTABLE_PASSED,
        "passed: {:?}; absent: {:?}",
        case_ids(&report, "passed"),
        case_ids(&report, "absent")
    );
    let absent = case_ids(&report, "absent");
    assert_eq!(
        absent,
        PORTABLE_ABSENT
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<BTreeSet<_>>()
    );
    for case in report["cases"].as_array().expect("cases") {
        if case["status"] == "absent" {
            assert_eq!(case["absent"]["reason"], "fact-absent", "{case}");
        }
    }
    assert_eq!(
        report["totals"]["absent"],
        u64::try_from(PORTABLE_ABSENT.len()).expect("count")
    );
    assert!(
        fixture.starts.load(Ordering::SeqCst) >= 2,
        "the hook restarted the daemon"
    );
    clean.assert_empty();
    assert_helper_records(&fixture, "hook", TOKEN_MARK);
    assert_helper_records(&fixture, "token", HOOK_MARK);

    // A partial run over `--token-fd`: a socketpair whose far end this test serves.
    let clean = Clean::new();
    let out = scratch.path().join("partial");
    let (theirs, ours) = std::os::unix::net::UnixStream::pair().expect("socketpair");
    let descriptor = theirs.as_raw_fd();
    nix::fcntl::fcntl(
        &theirs,
        nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()),
    )
    .expect("inheritable token socket");
    let served = Arc::new(AtomicUsize::new(0));
    let answering = {
        let (served, primary, narrowed) = (
            Arc::clone(&served),
            fixture.authority.primary.clone(),
            fixture.authority.narrowed.clone(),
        );
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(ours.try_clone().expect("socket clone"));
            let mut writer = ours;
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|count| count > 0) {
                let request: Value = serde_json::from_str(&line).expect("token request");
                let credential = if request["purpose"] == "narrowed" {
                    &narrowed
                } else {
                    &primary
                };
                writeln!(writer, "{credential}").expect("token reply");
                served.fetch_add(1, Ordering::SeqCst);
                line.clear();
            }
        })
    };
    let mut arguments = endpoint_arguments(&fixture, &out);
    arguments.extend([
        "--token-fd".into(),
        descriptor.to_string(),
        "--only".into(),
        "machine,auth,restart".into(),
    ]);
    arguments.extend(hook_arguments(&fixture));
    let child = spawn_runner(&clean, &arguments);
    // The runner holds its copy from the spawn on; this process closes its own at once, so the
    // serving thread sees end-of-file when the runner exits.
    drop(theirs);
    let outcome = wait_runner(child).await;
    answering.join().expect("token server");
    assert!(
        served.load(Ordering::SeqCst) > 0,
        "the token socket answered no request"
    );
    assert_no_credential(&fixture, "stdout", &outcome.stdout);
    assert_no_credential(&fixture, "stderr", &outcome.stderr);
    let report = read_report(&fixture, &out);
    assert_eq!(
        outcome.code,
        4,
        "a partial run is incomplete; stderr: {}",
        String::from_utf8_lossy(&outcome.stderr)
    );
    assert_eq!(report["verdict"], "incomplete");
    assert_eq!(report["run"]["partial"], true);
    assert_eq!(report["totals"]["failed"], 0);
    assert_eq!(report["declared"]["token_provider"], "fd");
    clean.assert_empty();
    assert_helper_records(&fixture, "hook", TOKEN_MARK);

    // Exit 3: a provider that prints a malformed credential.
    let clean = Clean::new();
    let out = scratch.path().join("malformed");
    let mut arguments = endpoint_arguments(&fixture, &out);
    arguments.extend(token_arguments(&fixture, "malformed"));
    let outcome = run_runner(&clean, &arguments).await;
    assert_eq!(
        outcome.code,
        3,
        "{}",
        String::from_utf8_lossy(&outcome.stderr)
    );
    assert!(!out.exists(), "a harness failure writes no report");
    let _ = records(&fixture, "token");
    clean.assert_empty();

    // Exit 2: an `--out` that already exists.
    let clean = Clean::new();
    let out = scratch.path().join("exists");
    std::fs::create_dir(&out).expect("existing --out");
    let mut arguments = endpoint_arguments(&fixture, &out);
    arguments.extend(token_arguments(&fixture, "serve"));
    let outcome = run_runner(&clean, &arguments).await;
    assert_eq!(
        outcome.code,
        2,
        "{}",
        String::from_utf8_lossy(&outcome.stderr)
    );
    assert_eq!(
        std::fs::read_dir(&out).expect("existing --out").count(),
        0,
        "an existing --out is left untouched"
    );
    let _ = records(&fixture, "token");

    // Exit 2: an endpoint that advertises a contract the runner does not pin.
    let stub = ForeignContract::start(&fixture).await;
    let clean = Clean::new();
    let out = scratch.path().join("foreign");
    let mut arguments = endpoint_arguments(&fixture, &out);
    let endpoint = arguments
        .iter()
        .position(|argument| argument == "--endpoint")
        .expect("endpoint flag");
    arguments[endpoint + 1].clone_from(&stub.endpoint);
    arguments.extend(token_arguments(&fixture, "serve"));
    let outcome = run_runner(&clean, &arguments).await;
    assert_eq!(
        outcome.code,
        2,
        "{}",
        String::from_utf8_lossy(&outcome.stderr)
    );
    assert!(!out.exists(), "an unknown contract writes no report");
    stub.task.abort();
    let _ = records(&fixture, "token");

    fixture.stop().await;
}

/// A TLS endpoint, trusted under the daemon's certificate, that advertises another contract.
struct ForeignContract {
    endpoint: String,
    task: tokio::task::JoinHandle<()>,
}

impl ForeignContract {
    async fn start(fixture: &Fixture) -> Self {
        let pem = std::fs::read_to_string(fixture.root.path().join("daemon.pem"))
            .expect("daemon certificate");
        let key =
            std::fs::read_to_string(fixture.root.path().join("daemon.key")).expect("daemon key");
        let acceptor = TlsAcceptor::from(server_config(&TestIdentity {
            certificate_pem: pem,
            private_key_pem: key,
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("stub listener");
        let endpoint = format!("https://{}/", listener.local_addr().expect("stub address"));
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let _ = read_head(&mut stream).await;
                    let body = br#"{"api_version":"v1","request_id":"req_stub","result":{}}"#;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nx-b10x-contract: substrate-wire/0.0.1\r\nx-b10x-contract-bundle-sha256: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        "0".repeat(64),
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                });
            }
        });
        Self { endpoint, task }
    }
}

fn delegated() {
    let Some(cgroup) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") else {
        println!("conformance delegated: ABSENT, SUBSTRATE_VECTORS_CGROUP_ROOT is not set");
        return;
    };
    runtime().block_on(delegated_scenario(PathBuf::from(cgroup)));
}

/// The groups the delegated lane requires, each by the line it prints once every case of the
/// group passed.
const DELEGATED_GROUPS: [(&str, &str); 4] = [
    ("exec", "conformance delegated exec: executed"),
    ("resources", "conformance delegated resources: executed"),
    ("sessions", "conformance delegated sessions: executed"),
    (
        "restart-exec",
        "conformance delegated restart exec: executed",
    ),
];

async fn delegated_scenario(cgroup: PathBuf) {
    let fixture = Fixture::start(Some(&cgroup)).await;
    let scratch = tempfile::TempDir::new().expect("scratch");
    let clean = Clean::new();
    let out = scratch.path().join("delegated");
    let mut arguments = endpoint_arguments(&fixture, &out);
    arguments.extend(token_arguments(&fixture, "serve"));
    arguments.extend(hook_arguments(&fixture));
    arguments.extend([
        "--forbidden-name".into(),
        FORBIDDEN_NAME.into(),
        "--ops-origin".into(),
        fixture.ops_origin.clone(),
    ]);
    let outcome = run_runner(&clean, &arguments).await;
    assert_no_credential(&fixture, "stdout", &outcome.stdout);
    assert_no_credential(&fixture, "stderr", &outcome.stderr);
    let report = read_report(&fixture, &out);
    let failed = case_ids(&report, "failed");
    assert_eq!(
        outcome.code,
        0,
        "delegated runner exit; failed: {failed:?}; stderr: {}",
        String::from_utf8_lossy(&outcome.stderr)
    );
    assert_eq!(report["verdict"], "passed");
    clean.assert_empty();
    for (group, line) in DELEGATED_GROUPS {
        let cases: Vec<&Value> = report["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .filter(|case| case["group"] == group)
            .collect();
        assert!(!cases.is_empty(), "the {group} group has no case");
        if cases.iter().all(|case| case["status"] == "passed") {
            println!("{line}");
        } else {
            let statuses: Vec<String> = cases
                .iter()
                .map(|case| format!("{} {}", case["id"], case["status"]))
                .collect();
            panic!("the {group} group did not pass on the delegated lane: {statuses:?}");
        }
    }
    fixture.stop().await;
}
