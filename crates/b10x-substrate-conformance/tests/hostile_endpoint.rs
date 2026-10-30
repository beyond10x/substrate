//! Integrity conformance cases for design 31 against an endpoint that does not behave: the runner
//! certifies an endpoint it does not trust, so a malformed or slow answer must end in a named
//! harness failure or a failed case within `--deadline`, and nothing the endpoint sends may reach
//! the closed report as free text (design 31 § 3, § 5 point 5, § 6).
//!
//! The endpoint here is an in-process TLS 1.3 listener with an `rcgen` certificate; the runner is
//! the shipped binary, and its token command is `/bin/echo` printing a fixed sentinel credential.
//! Every case first asserts that the endpoint read at least one request, so a runner that stopped
//! before dialling cannot pass a case vacuously.

#![cfg(unix)]

use std::io::Write as _;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use rcgen::{CertificateParams, KeyPair, date_time_ymd};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};

const RUNNER: &str = env!("CARGO_BIN_EXE_b10x-substrate-conformance");
const SCHEMA: &str = include_str!("../schema/remote-conformance-report.v1.schema.json");
const SERVER_NAME: &str = "hostile.substrate.test";
const CONTRACT: &str = "substrate-wire/0.25.0";
const CONTRACT_SHA256: &str = "853d7365a56f0d8dc97fe5feb87ce57285a34bbf7a0f6eb05616d6010fc4fc00";
const ECHO: &str = "/bin/echo";
const PREFIX: &str = "identity_access_v1_";
/// The credential the token command prints: the bundle's format, a fixed 43-character body.
const SENTINEL: &str = "identity_access_v1_SENTINELsentinelSENTINELsentinelSENTINELsen";

/// What the endpoint answers.
#[derive(Clone, Copy)]
enum Behaviour {
    /// Every answer is chunked with a first chunk size of `usize::MAX`, then one body byte.
    ChunkSizeOverflow,
    /// Every answer declares a large body and sends one byte of it every 300 ms, each well
    /// within `--io-timeout`.
    Drip,
    /// `GET /v1/machine` is a well-formed snapshot whose `snapshot` member is the bearer the
    /// request carried; every other request is a 404 with an empty object.
    EchoBearerAsSnapshot,
    /// `GET /v1/machine` is a well-formed snapshot with a `sha256:` snapshot id whose
    /// `driver_version` is the bearer the request carried; every other request is a 404.
    EchoBearerAsDriverVersion,
}

struct Shared {
    stop: AtomicBool,
    /// Requests the endpoint read in full.
    requests: AtomicUsize,
}

struct Endpoint {
    address: SocketAddr,
    roots: PathBuf,
    shared: Arc<Shared>,
}

impl Endpoint {
    fn requests(&self) -> usize {
        self.shared.requests.load(Ordering::SeqCst)
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
    }
}

fn server_config(directory: &Path) -> (Arc<ServerConfig>, PathBuf) {
    let mut params =
        CertificateParams::new([SERVER_NAME.to_owned()]).expect("certificate parameters");
    params.not_before = date_time_ymd(2025, 1, 1);
    params.not_after = date_time_ymd(2035, 1, 1);
    let key = KeyPair::generate().expect("private key");
    let certificate = params.self_signed(&key).expect("self-signed certificate");
    let roots = directory.join("roots.pem");
    std::fs::write(&roots, certificate.pem()).expect("trust roots");
    let certificates = rustls_pemfile::certs(&mut certificate.pem().as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("certificate");
    let private = rustls_pemfile::private_key(&mut key.serialize_pem().as_bytes())
        .expect("private key parse")
        .expect("private key");
    let mut config = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(certificates, private)
        .expect("TLS config");
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    (Arc::new(config), roots)
}

fn serve(directory: &Path, behaviour: Behaviour) -> Endpoint {
    let (config, roots) = server_config(directory);
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let shared = Arc::new(Shared {
        stop: AtomicBool::new(false),
        requests: AtomicUsize::new(0),
    });
    let serving = Arc::clone(&shared);
    thread::spawn(move || {
        for tcp in listener.incoming() {
            if serving.stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(tcp) = tcp else { continue };
            let config = Arc::clone(&config);
            let shared = Arc::clone(&serving);
            thread::spawn(move || connection(tcp, &config, behaviour, &shared));
        }
    });
    Endpoint {
        address,
        roots,
        shared,
    }
}

/// One request's line, and the bearer it carried.
fn read_request(stream: &mut impl std::io::Read) -> Option<(String, Option<String>)> {
    let mut received = Vec::new();
    let mut block = [0_u8; 4096];
    let end = loop {
        if let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let read = stream.read(&mut block).ok()?;
        if read == 0 {
            return None;
        }
        received.extend_from_slice(&block[..read]);
    };
    let head = String::from_utf8_lossy(&received[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?.to_owned();
    let mut bearer = None;
    let mut length = 0_usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "authorization" => {
                bearer = value.trim().strip_prefix("Bearer ").map(str::to_owned);
            }
            "content-length" => length = value.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    let mut body = received.len() - (end + 4);
    while body < length {
        let read = stream.read(&mut block).ok()?;
        if read == 0 {
            return None;
        }
        body += read;
    }
    Some((request_line, bearer))
}

fn head(status: &str, extra: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nx-b10x-contract: {CONTRACT}\r\nx-b10x-contract-bundle-sha256: \
         {CONTRACT_SHA256}\r\ncontent-type: application/json\r\nconnection: close\r\n{extra}\r\n"
    )
}

/// A machine envelope the wire crate decodes, with `snapshot` and `driver_version` as given.
fn machine(snapshot: &str, driver_version: &str) -> Value {
    json!({
        "api_version": "v1",
        "request_id": "req-hostile",
        "result": {
            "snapshot": snapshot,
            "driver": "host",
            "driver_version": driver_version,
            "config_generation": 1,
            "probed_at": "2026-10-30T00:00:00Z",
            "facts": {
                "operation.ledger-subject-max-rows": 1,
                "operation.ledger-subject-max-bytes": 1,
                "operation.ledger-global-max-rows": 1,
                "operation.ledger-global-max-bytes": 1
            }
        }
    })
}

/// A schema-valid snapshot id.
fn snapshot_id() -> String {
    format!("sha256:{}", "a".repeat(64))
}

fn answer(stream: &mut impl std::io::Write, status: &str, body: &str) {
    let reply = format!(
        "{}{body}",
        head(status, &format!("content-length: {}\r\n", body.len()))
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

fn connection(tcp: TcpStream, config: &Arc<ServerConfig>, behaviour: Behaviour, shared: &Shared) {
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(server) = ServerConnection::new(Arc::clone(config)) else {
        return;
    };
    let mut stream = StreamOwned::new(server, tcp);
    let Some((request_line, bearer)) = read_request(&mut stream) else {
        return;
    };
    shared.requests.fetch_add(1, Ordering::SeqCst);
    let is_machine = request_line.starts_with("GET /v1/machine ");
    let bearer = bearer.unwrap_or_else(|| "none".to_owned());
    match behaviour {
        Behaviour::ChunkSizeOverflow => {
            let reply = format!(
                "{}ffffffffffffffff\r\nX",
                head("200 OK", "transfer-encoding: chunked\r\n")
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
        Behaviour::Drip => {
            let _ = stream.write_all(head("200 OK", "content-length: 1000000\r\n").as_bytes());
            let _ = stream.flush();
            while !shared.stop.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(300));
                if stream
                    .write_all(b" ")
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    return;
                }
            }
        }
        Behaviour::EchoBearerAsSnapshot if is_machine => {
            answer(
                &mut stream,
                "200 OK",
                &machine(&bearer, "1.0.0").to_string(),
            );
        }
        Behaviour::EchoBearerAsDriverVersion if is_machine => {
            answer(
                &mut stream,
                "200 OK",
                &machine(&snapshot_id(), &bearer).to_string(),
            );
        }
        Behaviour::EchoBearerAsSnapshot | Behaviour::EchoBearerAsDriverVersion => {
            answer(&mut stream, "404 Not Found", "{}");
        }
    }
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

/// How one runner process ended.
struct Ended {
    code: Option<i32>,
    killed: bool,
    elapsed: Duration,
    output: String,
}

fn run(
    directory: &Path,
    endpoint: &Endpoint,
    out: &Path,
    extra: &[&str],
    bound: Duration,
) -> Ended {
    assert!(Path::new(ECHO).is_file(), "{ECHO} is the token command");
    let log = directory.join("runner.log");
    let file = std::fs::File::create(&log).expect("log");
    let endpoint_url = format!("https://{}/", endpoint.address);
    let mut command = Command::new(RUNNER);
    command
        .args(["--endpoint", &endpoint_url, "--trust-roots"])
        .arg(&endpoint.roots)
        .args(["--server-name", SERVER_NAME, "--token-command", ECHO])
        .args(["--token-command-arg", SENTINEL, "--out"])
        .arg(out)
        .args(extra)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(file.try_clone().expect("log"))
        .stderr(file);
    let started = Instant::now();
    let mut child = command.spawn().expect("runner starts");
    let (code, killed) = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break (status.code(), false);
        }
        if started.elapsed() >= bound {
            let _ = child.kill();
            let _ = child.wait();
            break (None, true);
        }
        thread::sleep(Duration::from_millis(100));
    };
    let ended = Ended {
        code,
        killed,
        elapsed: started.elapsed(),
        output: std::fs::read_to_string(&log).unwrap_or_default(),
    };
    assert!(
        endpoint.requests() > 0,
        "precondition: the runner reached the endpoint (exit {:?}): {}",
        ended.code,
        ended.output
    );
    ended
}

/// The report the run wrote, its text and its schema errors (at most three).
fn written_report(out: &Path, ended: &Ended) -> (String, Vec<String>) {
    let bytes = std::fs::read(out.join("report.json")).unwrap_or_else(|_| {
        panic!(
            "precondition: the runner wrote a report (exit {:?}): {}",
            ended.code, ended.output
        )
    });
    let schema: Value = serde_json::from_str(SCHEMA).expect("schema");
    let report: Value = serde_json::from_slice(&bytes).expect("report is JSON");
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    let errors = validator
        .iter_errors(&report)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .take(3)
        .collect();
    (String::from_utf8_lossy(&bytes).into_owned(), errors)
}

/// Design 31 § 3: a refused value or a harness failure exits 2 or 3 with a named code. A chunk
/// size line of `ffffffffffffffff` is a malformed response (`response-malformed`, exit 3); it must
/// not panic the runner (`dechunk`'s `size + 2` and `rest[..size]`, `src/probe.rs`).
#[test]
fn a_chunk_size_of_usize_max_is_a_harness_failure_not_a_panic() {
    let directory = tempfile::tempdir().expect("scratch");
    let endpoint = serve(directory.path(), Behaviour::ChunkSizeOverflow);
    let out = directory.path().join("out");
    let ended = run(
        directory.path(),
        &endpoint,
        &out,
        &["--deadline", "5", "--io-timeout", "2"],
        Duration::from_secs(30),
    );
    assert!(!ended.killed, "the runner did not end: {}", ended.output);
    assert!(
        !ended.output.contains("panicked"),
        "story:remote-clean-room-conformance: the runner panicked on the endpoint's chunk size \
         (exit {:?}): {}",
        ended.code,
        ended.output
    );
    assert_eq!(
        ended.code,
        Some(3),
        "story:remote-clean-room-conformance: a malformed chunked answer is harness failure exit \
         3; the runner printed: {}",
        ended.output
    );
}

/// Design 31 § 3: the run is bounded by `--deadline`. An endpoint that answers the first
/// `GET /v1/machine` one byte at a time, each byte inside `--io-timeout`, must not hold the runner
/// past its deadline: here 2 s, with a 25 s allowance before the case gives up and kills it.
#[test]
fn an_endpoint_that_drips_its_first_answer_does_not_outlast_the_deadline() {
    let directory = tempfile::tempdir().expect("scratch");
    let endpoint = serve(directory.path(), Behaviour::Drip);
    let out = directory.path().join("out");
    let ended = run(
        directory.path(),
        &endpoint,
        &out,
        &["--deadline", "2", "--io-timeout", "1"],
        Duration::from_secs(25),
    );
    assert!(
        !ended.killed,
        "story:remote-clean-room-conformance: with --deadline 2 and --io-timeout 1 the runner was \
         still reading the endpoint's first answer after {:?} and was killed; output: {}",
        ended.elapsed, ended.output
    );
}

/// Design 31 § 5 point 5 and § 6: the report is closed, so no endpoint-supplied free text can
/// carry a credential or tenant data into it. The endpoint here answers `GET /v1/machine` with
/// the bearer it was sent as the snapshot id; the report must validate against its own schema
/// (`snapshot` is `^sha256:[0-9a-f]{64}$`) and hold no byte of the credential's body.
#[test]
fn an_endpoint_supplied_snapshot_cannot_carry_the_credential_into_the_report() {
    let decoded: substrate_wire::Success<substrate_wire::CapabilitySnapshot> =
        serde_json::from_value(machine(SENTINEL, "1.0.0")).expect("a wire machine envelope");
    assert_eq!(decoded.result.snapshot, SENTINEL);

    let directory = tempfile::tempdir().expect("scratch");
    let endpoint = serve(directory.path(), Behaviour::EchoBearerAsSnapshot);
    let out = directory.path().join("out");
    let ended = run(
        directory.path(),
        &endpoint,
        &out,
        &[
            "--deadline",
            "20",
            "--io-timeout",
            "2",
            "--restart-timeout",
            "1",
        ],
        Duration::from_secs(90),
    );
    assert!(!ended.killed, "the runner did not end: {}", ended.output);
    let (text, errors) = written_report(&out, &ended);
    assert!(
        !text.contains(&SENTINEL[PREFIX.len()..]),
        "story:remote-clean-room-conformance: report.json (exit {:?}) carries the credential the \
         endpoint echoed as capability_snapshot.snapshot; schema errors: {errors:?}",
        ended.code
    );
    assert!(
        errors.is_empty(),
        "story:remote-clean-room-conformance: the report the runner wrote violates its own closed \
         schema: {errors:?}"
    );
}

/// Design 31 § 5 point 5: "the report has no free-text field a credential could land in". The
/// `word` pattern `driver_version` is held to, `^[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}$`, admits a whole
/// `identity_access_v1_` credential (62 bytes). The endpoint echoes the bearer as
/// `driver_version`; the report must not carry it, whether or not it validates.
#[test]
fn an_endpoint_supplied_driver_version_cannot_carry_the_credential_into_the_report() {
    let directory = tempfile::tempdir().expect("scratch");
    let endpoint = serve(directory.path(), Behaviour::EchoBearerAsDriverVersion);
    let out = directory.path().join("out");
    let ended = run(
        directory.path(),
        &endpoint,
        &out,
        &[
            "--deadline",
            "20",
            "--io-timeout",
            "2",
            "--restart-timeout",
            "1",
        ],
        Duration::from_secs(90),
    );
    assert!(!ended.killed, "the runner did not end: {}", ended.output);
    let (text, errors) = written_report(&out, &ended);
    assert!(
        !text.contains(&SENTINEL[PREFIX.len()..]),
        "story:remote-clean-room-conformance: report.json (exit {:?}, schema errors {errors:?}) \
         carries the credential the endpoint echoed as capability_snapshot.driver_version",
        ended.code
    );
}
