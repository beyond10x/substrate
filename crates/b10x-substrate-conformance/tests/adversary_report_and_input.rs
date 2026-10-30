//! Adversary cases for `story:remote-clean-room-conformance` (wave 2026-10-30, unit u1).
//!
//! The runner is run as a binary against an in-process TLS stub that advertises the pinned
//! contract pair and answers `GET /v1/machine` with a capability snapshot the wire crate decodes.
//! No daemon is involved. The cases hold the runner to design 31 § 6 (the report is closed at
//! every level and validates against its schema on every exit path that writes one) and to the
//! command line's own promise (`src/cli.rs`): a refused value is invalid input and exits 2.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rcgen::{CertificateParams, KeyPair, date_time_ymd};
use rustls::ServerConfig;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio_rustls::TlsAcceptor;

const RUNNER: &str = env!("CARGO_BIN_EXE_b10x-substrate-conformance");
const SCHEMA: &str = include_str!("../schema/remote-conformance-report.v1.schema.json");
const SERVER_NAME: &str = "substrate.test";
const PINNED_CONTRACT: &str = "substrate-wire/0.25.0";
const PINNED_SHA256: &str = "853d7365a56f0d8dc97fe5feb87ce57285a34bbf7a0f6eb05616d6010fc4fc00";
const PRINTF: &str = "/usr/bin/printf";
const BOUND: Duration = Duration::from_secs(90);

fn scratch() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("adv-u1-")
        .tempdir()
        .expect("scratch directory")
}

/// A self-signed certificate for [`SERVER_NAME`], written as the runner's trust roots.
struct Identity {
    certificate_pem: String,
    private_key_pem: String,
}

fn identity() -> Identity {
    let mut params =
        CertificateParams::new([SERVER_NAME.to_owned()]).expect("certificate parameters");
    params.not_before = date_time_ymd(2025, 1, 1);
    params.not_after = date_time_ymd(2035, 1, 1);
    let key = KeyPair::generate().expect("private key");
    let certificate = params.self_signed(&key).expect("self-signed certificate");
    Identity {
        certificate_pem: certificate.pem(),
        private_key_pem: key.serialize_pem(),
    }
}

fn server_config(identity: &Identity) -> Arc<ServerConfig> {
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

/// A machine body the wire crate decodes: only the four required facts, so every workspace,
/// exec, session and event case selects absent, and `snapshot` as given.
fn machine_body(snapshot: &str) -> Vec<u8> {
    let body = json!({
        "api_version": "v1",
        "request_id": "req_stub",
        "result": {
            "snapshot": snapshot,
            "driver": "host",
            "driver_version": "0.0.0",
            "config_generation": 1,
            "probed_at": "2026-10-30T00:00:00Z",
            "facts": {
                "operation.ledger-subject-max-rows": 1,
                "operation.ledger-subject-max-bytes": 1,
                "operation.ledger-global-max-rows": 1,
                "operation.ledger-global-max-bytes": 1
            }
        }
    });
    let bytes = serde_json::to_vec(&body).expect("machine body");
    serde_json::from_slice::<substrate_wire::Success<substrate_wire::CapabilitySnapshot>>(&bytes)
        .expect("the stub's machine body decodes into the wire crate's closed types");
    bytes
}

fn failure_body(class: &str, code: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "api_version": "v1",
        "request_id": "req_stub",
        "error": { "class": class, "code": code, "message": "stub", "retriable": false }
    }))
    .expect("failure body")
}

/// What the stub answers to every request other than `GET /v1/machine`.
#[derive(Clone)]
struct Other {
    status: u16,
    body: Arc<Vec<u8>>,
}

/// A TLS endpoint advertising the pinned contract pair.
struct Stub {
    endpoint: String,
    roots: PathBuf,
    mutations: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

async fn read_request(stream: &mut (impl AsyncRead + Unpin)) -> Option<String> {
    let mut bytes = Vec::new();
    let end = loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let mut block = [0_u8; 4096];
        let count = stream.read(&mut block).await.ok()?;
        if count == 0 || bytes.len() > 64 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&block[..count]);
    };
    let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut have = bytes.len() - end - 4;
    while have < length {
        let mut block = [0_u8; 4096];
        let count = stream.read(&mut block).await.ok()?;
        if count == 0 {
            break;
        }
        have += count;
    }
    head.lines().next().map(str::to_owned)
}

impl Stub {
    async fn start(directory: &Path, snapshot: &str, other: Other) -> Self {
        Self::start_with_machine(directory, machine_body(snapshot), other).await
    }

    /// A stub whose `GET /v1/machine` answers `machine` as given, decodable or not.
    async fn start_with_machine(directory: &Path, machine: Vec<u8>, other: Other) -> Self {
        let identity = identity();
        let roots = directory.join("roots.pem");
        std::fs::write(&roots, &identity.certificate_pem).expect("trust roots");
        let acceptor = TlsAcceptor::from(server_config(&identity));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("stub listener");
        let endpoint = format!("https://{}/", listener.local_addr().expect("stub address"));
        let machine = Arc::new(machine);
        let mutations = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&mutations);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                let machine = Arc::clone(&machine);
                let other = other.clone();
                let counted = Arc::clone(&counted);
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let Some(line) = read_request(&mut stream).await else {
                        return;
                    };
                    if !line.starts_with("GET ") {
                        counted.fetch_add(1, Ordering::SeqCst);
                    }
                    let (status, body) = if line.starts_with("GET /v1/machine ") {
                        (200, machine.as_slice())
                    } else {
                        (other.status, other.body.as_slice())
                    };
                    let head = format!(
                        "HTTP/1.1 {status} Stub\r\ncontent-type: application/json\r\nx-b10x-contract: {PINNED_CONTRACT}\r\nx-b10x-contract-bundle-sha256: {PINNED_SHA256}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self {
            endpoint,
            roots,
            mutations,
            task,
        }
    }
}

fn not_found() -> Other {
    Other {
        status: 404,
        body: Arc::new(failure_body("refused", "resource.not-found")),
    }
}

fn credential_line() -> String {
    format!("identity_access_v1_{}\\n", "A".repeat(43))
}

fn arguments(endpoint: &str, roots: &Path, out: &Path, token_command: &str) -> Vec<String> {
    [
        "--endpoint",
        endpoint,
        "--trust-roots",
        &roots.display().to_string(),
        "--server-name",
        SERVER_NAME,
        "--out",
        &out.display().to_string(),
        "--token-command",
        token_command,
        "--token-command-arg",
        &credential_line(),
        "--deadline",
        "60",
        "--io-timeout",
        "2",
        "--restart-timeout",
        "5",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn run(arguments: &[String], cwd: &Path) -> Outcome {
    let child = tokio::process::Command::new(RUNNER)
        .args(arguments)
        .env_clear()
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn the runner");
    let output = tokio::time::timeout(BOUND, child.wait_with_output())
        .await
        .expect("the runner exits within its bound")
        .expect("runner output");
    Outcome {
        code: output.status.code().expect("runner exit code"),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// The schema errors of `out/report.json`, each with its instance path.
fn schema_errors(out: &Path) -> Vec<String> {
    let report: Value =
        serde_json::from_slice(&std::fs::read(out.join("report.json")).expect("report.json"))
            .expect("report JSON");
    let schema: Value = serde_json::from_str(SCHEMA).expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("schema validator");
    validator
        .iter_errors(&report)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect()
}

/// Control: against a stub whose snapshot is well formed, an exit-1 run writes a report that
/// validates. The red cases below differ from this one only in the value they name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_run_against_a_well_formed_stub_writes_a_report_inside_its_schema() {
    let directory = scratch();
    let stub = Stub::start(
        directory.path(),
        &format!("sha256:{}", "a".repeat(64)),
        not_found(),
    )
    .await;
    let out = directory.path().join("out");
    let outcome = run(
        &arguments(&stub.endpoint, &stub.roots, &out, PRINTF),
        directory.path(),
    )
    .await;
    stub.task.abort();
    assert_eq!(
        outcome.code, 1,
        "a stub that refuses every route fails the run: {}{}",
        outcome.stdout, outcome.stderr
    );
    let errors = schema_errors(&out);
    assert!(
        errors.is_empty(),
        "story:remote-clean-room-conformance: report outside its schema: {errors:?}"
    );
}

/// Design 31 § 6: the report is closed at every level and carries no free text; the schema pins
/// `capability_snapshot.first.snapshot` to `sha256:<64 hex>`. The runner copies the server's
/// `snapshot` string into the report unchecked (`src/lib.rs` `snapshot_read`), so a non-conforming
/// endpoint gets server-chosen text into a report that then fails its own schema.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_chosen_snapshot_string_never_leaves_the_report_outside_its_schema() {
    let directory = scratch();
    let stub = Stub::start(directory.path(), "tenant-a/namespace-b", not_found()).await;
    let out = directory.path().join("out");
    let outcome = run(
        &arguments(&stub.endpoint, &stub.roots, &out, PRINTF),
        directory.path(),
    )
    .await;
    stub.task.abort();
    assert_eq!(
        outcome.code, 1,
        "the stub fails the run: {}{}",
        outcome.stdout, outcome.stderr
    );
    let report = std::fs::read_to_string(out.join("report.json")).expect("report.json");
    assert!(
        !report.contains("tenant-a/namespace-b"),
        "story:remote-clean-room-conformance: the report carries server-chosen free text from the \
         machine snapshot (design 31 § 6)"
    );
    let errors = schema_errors(&out);
    assert!(
        errors.is_empty(),
        "story:remote-clean-room-conformance: report outside its schema: {errors:?}"
    );
}

/// Design 31 § 6: an unserved case records the observed HTTP status, which the schema bounds to
/// 100..=599. The raw probe accepts any three-digit status (`src/probe.rs` `parse_reply`) and the
/// unserved case records it unchecked (`src/cases.rs` `unserved_answer`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observed_status_outside_http_never_leaves_the_report_outside_its_schema() {
    let directory = scratch();
    let stub = Stub::start(
        directory.path(),
        &format!("sha256:{}", "a".repeat(64)),
        Other {
            status: 999,
            body: Arc::new(failure_body("unserved", "stub.unserved")),
        },
    )
    .await;
    let out = directory.path().join("out");
    let outcome = run(
        &arguments(&stub.endpoint, &stub.roots, &out, PRINTF),
        directory.path(),
    )
    .await;
    stub.task.abort();
    assert!(
        out.join("report.json").exists(),
        "a report is written: exit {} {}{}",
        outcome.code,
        outcome.stdout,
        outcome.stderr
    );
    let errors = schema_errors(&out);
    assert!(
        errors.is_empty(),
        "story:remote-clean-room-conformance: report outside its schema: {errors:?}"
    );
}

/// `src/cli.rs`: "Every value is checked here or at start; a refused value is invalid input and
/// exits 2 without a report." An `--out` whose parent does not exist is only found after the
/// whole inventory, mutations included, has run against the endpoint, and then exits 3.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_out_directory_that_cannot_be_created_is_invalid_input_before_any_request() {
    let directory = scratch();
    let stub = Stub::start(
        directory.path(),
        &format!("sha256:{}", "a".repeat(64)),
        not_found(),
    )
    .await;
    let out = directory.path().join("no-such-parent").join("out");
    let outcome = run(
        &arguments(&stub.endpoint, &stub.roots, &out, PRINTF),
        directory.path(),
    )
    .await;
    stub.task.abort();
    assert_eq!(
        outcome.code, 2,
        "story:remote-clean-room-conformance: an uncreatable --out is invalid input (exit 2); \
         stderr: {}",
        outcome.stderr
    );
    assert_eq!(
        stub.mutations.load(Ordering::SeqCst),
        0,
        "story:remote-clean-room-conformance: invalid input sends no mutation to the endpoint"
    );
}

/// Design 31 § 5, item 4: "The token command has no descriptor beyond 0 to 2" (and § 4: the hook
/// "inherits no other descriptor"). The runner marks only the descriptors it opens close-on-exec;
/// one its own parent left inheritable (as `flock(1)` does for the lock it holds, which is how
/// `build-slot.sh` runs every cargo command in this wave) reaches the token command and the hook.
/// `local_endpoint`'s portable scenario fails for the same reason when run under `flock`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_descriptor_the_runner_inherited_never_reaches_the_token_command() {
    let directory = scratch();
    let stub = Stub::start(
        directory.path(),
        &format!("sha256:{}", "a".repeat(64)),
        not_found(),
    )
    .await;
    let out = directory.path().join("out");
    let record = directory.path().join("token-descriptors.txt");
    let script = format!(
        "ls /proc/$$/fd > '{}'; printf '{}'",
        record.display(),
        credential_line()
    );
    let mut arguments = arguments(&stub.endpoint, &stub.roots, &out, "/bin/sh");
    let index = arguments
        .iter()
        .position(|argument| argument == "--token-command-arg")
        .expect("token argument");
    arguments.splice(
        index..index + 2,
        [
            "--token-command-arg".to_owned(),
            "-c".to_owned(),
            "--token-command-arg".to_owned(),
            script,
        ],
    );
    // A descriptor the runner's parent leaves inheritable, as a supervisor or `flock` does.
    let inherited = std::fs::File::open(&stub.roots).expect("an inheritable descriptor");
    nix::fcntl::fcntl(
        &inherited,
        nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()),
    )
    .expect("clear close-on-exec");
    let inherited_number = std::os::fd::AsRawFd::as_raw_fd(&inherited).to_string();
    let outcome = run(&arguments, directory.path()).await;
    drop(inherited);
    stub.task.abort();
    assert_eq!(
        outcome.code, 1,
        "the token command answered and the stub failed the run: {}{}",
        outcome.stdout, outcome.stderr
    );
    let descriptors: Vec<String> = std::fs::read_to_string(&record)
        .expect("the token command recorded its descriptors")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        descriptors,
        ["0", "1", "2"],
        "story:remote-clean-room-conformance: the token command holds a descriptor beyond 0 to 2 \
         (design 31 § 5); the runner's parent left descriptor {inherited_number} inheritable"
    );
}

/// The brief's rule: invalid input (a bad URL, missing trust roots, an unreadable token fd) is
/// exit 2. A `--token-command` that does not exist is refused only when the first credential is
/// fetched, as a harness failure (exit 3, `token-unavailable`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_command_that_does_not_exist_is_invalid_input() {
    let directory = scratch();
    let stub = Stub::start(
        directory.path(),
        &format!("sha256:{}", "a".repeat(64)),
        not_found(),
    )
    .await;
    let out = directory.path().join("out");
    let missing = directory.path().join("no-such-token-command");
    let outcome = run(
        &arguments(
            &stub.endpoint,
            &stub.roots,
            &out,
            &missing.display().to_string(),
        ),
        directory.path(),
    )
    .await;
    stub.task.abort();
    assert_eq!(
        outcome.code, 2,
        "story:remote-clean-room-conformance: a token command that does not exist is invalid \
         input (exit 2); stderr: {}",
        outcome.stderr
    );
}

/// The same rule for every program the runner would start: a token command that exists but is
/// not executable, and a restart hook that does not exist, are invalid input found at start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_program_the_runner_cannot_start_is_invalid_input_before_any_request() {
    let directory = scratch();
    let stub = Stub::start(
        directory.path(),
        &format!("sha256:{}", "a".repeat(64)),
        not_found(),
    )
    .await;
    let unexecutable = directory.path().join("not-executable");
    std::fs::write(&unexecutable, "#!/bin/sh\n").expect("a plain file");
    let out = directory.path().join("out-a");
    let outcome = run(
        &arguments(
            &stub.endpoint,
            &stub.roots,
            &out,
            &unexecutable.display().to_string(),
        ),
        directory.path(),
    )
    .await;
    assert_eq!(
        outcome.code, 2,
        "story:remote-clean-room-conformance: a token command that is not executable is invalid \
         input (exit 2); stderr: {}",
        outcome.stderr
    );
    let out = directory.path().join("out-b");
    let mut hooked = arguments(&stub.endpoint, &stub.roots, &out, PRINTF);
    hooked.extend([
        "--restart-hook".to_owned(),
        directory.path().join("no-such-hook").display().to_string(),
    ]);
    let outcome = run(&hooked, directory.path()).await;
    stub.task.abort();
    assert_eq!(
        outcome.code, 2,
        "story:remote-clean-room-conformance: a restart hook that does not exist is invalid \
         input (exit 2); stderr: {}",
        outcome.stderr
    );
    assert_eq!(
        stub.mutations.load(Ordering::SeqCst),
        0,
        "story:remote-clean-room-conformance: invalid input sends no mutation to the endpoint"
    );
    assert!(
        !directory.path().join("out-a").exists() && !out.exists(),
        "story:remote-clean-room-conformance: a run refused as invalid input leaves no --out"
    );
}

/// Finding `undecodable-machine-is-harness-failure`: a machine body the wire types refuse is the
/// endpoint's failure, not the runner's. The core case `machine.envelope-decodes` fails, the
/// report is written inside its schema with no server text, and the run exits 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undecodable_machine_body_fails_the_envelope_case_and_writes_the_report() {
    let directory = scratch();
    let machine = serde_json::to_vec(&json!({
        "api_version": "v1",
        "request_id": "req_stub",
        "result": { "snapshot": "tenant-a/namespace-b", "driver": "host" }
    }))
    .expect("machine body");
    assert!(
        serde_json::from_slice::<substrate_wire::Success<substrate_wire::CapabilitySnapshot>>(
            &machine
        )
        .is_err(),
        "the stub's machine body is one the wire crate refuses"
    );
    let stub = Stub::start_with_machine(directory.path(), machine, not_found()).await;
    let out = directory.path().join("out");
    let outcome = run(
        &arguments(&stub.endpoint, &stub.roots, &out, PRINTF),
        directory.path(),
    )
    .await;
    stub.task.abort();
    assert_eq!(
        outcome.code, 1,
        "story:remote-clean-room-conformance: an undecodable machine body fails the run (exit \
         1), it is not a harness failure; stderr: {}",
        outcome.stderr
    );
    let errors = schema_errors(&out);
    assert!(
        errors.is_empty(),
        "story:remote-clean-room-conformance: report outside its schema: {errors:?}"
    );
    let text = std::fs::read_to_string(out.join("report.json")).expect("report.json");
    assert!(
        !text.contains("tenant-a/namespace-b"),
        "story:remote-clean-room-conformance: the report carries server text"
    );
    let report: Value = serde_json::from_str(&text).expect("report JSON");
    let envelope = report["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == "machine.envelope-decodes")
        .expect("story:remote-clean-room-conformance: the report records machine.envelope-decodes");
    assert_eq!(envelope["status"], "failed");
    assert_eq!(envelope["diagnostics"], json!(["envelope-undecodable"]));
    assert_eq!(report["verdict"], "failed");
    assert_eq!(
        stub.mutations.load(Ordering::SeqCst),
        0,
        "story:remote-clean-room-conformance: no case runs on facts the runner could not read"
    );
}
