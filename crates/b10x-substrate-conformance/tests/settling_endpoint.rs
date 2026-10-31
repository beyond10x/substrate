//! Design 31 § 3 against an endpoint whose workspace destroy settles through reconciliation: the
//! namespace driver's destroy is `WorkspaceDestroyProgress::Pending` until its keeper and its PVC
//! read absent (design 34 § 5), and the daemon then answers the destroy `operation.outcome-unknown`
//! (500, `failed`, retriable), leaves the workspace `destroying` and finishes it in maintenance,
//! which terminalizes the original operation (design 03 decision 8;
//! `bounded_destroy_pending_progress_remains_durable_until_absence_is_proved` in
//! `crates/substrate-daemon/tests/contract_vectors.rs`). The contract's answer for such an outcome
//! is to read the ledger, `GET /v1/ops/{op}`, never to call it a refusal (design 01 § 6).
//!
//! The endpoint here is an in-process TLS 1.3 listener that answers exactly that way: the first
//! `DELETE` of a workspace is `operation.outcome-unknown`, the operation reads `accepted` and the
//! workspace `destroying` until [`SETTLE`] has passed, and then the operation reads `terminal` with
//! the absence and the workspace `resource.not-found`, with one `workspace.destroyed` event caused
//! by the destroy's operation. Everything else a case needs is answered as the daemon answers it,
//! and what no case here needs is a `resource.not-found`.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rcgen::{CertificateParams, KeyPair, date_time_ymd};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};
use sha2::Digest as _;

const RUNNER: &str = env!("CARGO_BIN_EXE_b10x-substrate-conformance");
const SERVER_NAME: &str = "settling.substrate.test";
const CONTRACT: &str = "substrate-wire/0.25.0";
const CONTRACT_SHA256: &str = "853d7365a56f0d8dc97fe5feb87ce57285a34bbf7a0f6eb05616d6010fc4fc00";
const ECHO: &str = "/bin/echo";
/// The credential the token command prints: the bundle's format, a fixed 43-character body.
const SENTINEL: &str = "identity_access_v1_SENTINELsentinelSENTINELsentinelSENTINELsen";
/// How long a destroy stays `destroying` before reconciliation proves the workspace absent.
const SETTLE: Duration = Duration::from_millis(1500);
const OUTCOME_UNKNOWN: &str = "operation.outcome-unknown";

/// One workspace the endpoint holds.
struct Held {
    observed: Value,
    /// The destroy's operation and when its reconciliation finishes, once a destroy was accepted.
    destroy: Option<(String, Instant)>,
    /// The bytes of each file written, by path.
    files: BTreeMap<String, Vec<u8>>,
}

/// One operation the endpoint recorded.
struct Recorded {
    kind: &'static str,
    resource: String,
    /// The answer a replay gets and the ledger reads; `None` while the outcome is unknown.
    outcome: Option<(u16, Value)>,
}

/// Where the endpoint is in a restart the hook asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The hook has not run: the probed snapshot.
    Before,
    /// The process started again and its driver is not open yet: `probed_at` has moved, the
    /// snapshot carries only the daemon's own facts, and `/readyz` is 503, as a namespace-driver
    /// daemon answers until its first probe after the open (design 34 § 6.1, `opening_settles`).
    Opening,
    /// The driver opened and probed: the whole snapshot again, and `/readyz` 200.
    Open,
}

/// How long the driver takes to open after the restart.
const OPENING: Duration = Duration::from_millis(1500);

#[derive(Default)]
struct Model {
    /// The file the restart hook creates; `None` where no restart is declared.
    marker: Option<PathBuf>,
    restarted: Option<Instant>,
    workspaces: BTreeMap<String, Held>,
    operations: BTreeMap<String, Recorded>,
    events: Vec<Value>,
    /// Destroys answered `operation.outcome-unknown`, by operation.
    unknown: Vec<String>,
    /// `GET /v1/ops/{op}` reads of an operation still `accepted`, by operation.
    accepted_reads: Vec<String>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

impl Model {
    fn phase(&mut self) -> Phase {
        if !self.marker.as_ref().is_some_and(|marker| marker.exists()) {
            return Phase::Before;
        }
        let restarted = *self.restarted.get_or_insert_with(Instant::now);
        if restarted.elapsed() < OPENING {
            Phase::Opening
        } else {
            Phase::Open
        }
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "callers hand over the observation they built"
    )]
    fn event(&mut self, resource: &str, transition: &str, operation: &str, observation: Value) {
        let seq = self.events.len() as u64 + 1;
        self.events.push(json!({
            "generation": 1,
            "seq": seq,
            "resource": resource,
            "resource_kind": "workspace",
            "transition": transition,
            "observed_at": now(),
            "actor": "identity:stub",
            "principal": null,
            "cause": {"kind": "operation", "operation": operation},
            "observation": observation,
        }));
    }

    /// Reconciliation: every destroy whose time has come proves the workspace absent and
    /// terminalizes its original operation.
    fn settle(&mut self) {
        let due: Vec<(String, String)> = self
            .workspaces
            .iter()
            .filter_map(|(id, held)| {
                held.destroy
                    .as_ref()
                    .filter(|(_, at)| Instant::now() >= *at)
                    .map(|(operation, _)| (id.clone(), operation.clone()))
            })
            .collect();
        for (id, operation) in due {
            self.workspaces.remove(&id);
            let absence =
                json!({"kind": "workspace", "id": id, "absent": true, "observed_at": now()});
            if let Some(recorded) = self.operations.get_mut(&operation) {
                recorded.outcome = Some((200, absence.clone()));
            }
            self.event(&id, "workspace.destroyed", &operation, absence);
        }
    }

    fn record(&self, operation: &str) -> Value {
        let recorded = &self.operations[operation];
        let (state, terminal_at, outcome) = match &recorded.outcome {
            Some((_, result)) => (
                "terminal",
                json!(now()),
                json!({"kind": "success", "result": result}),
            ),
            None => ("accepted", Value::Null, Value::Null),
        };
        json!({
            "operation": operation,
            "operation_kind": recorded.kind,
            "request_hash": "a".repeat(64),
            "state": state,
            "accepted_at": now(),
            "terminal_at": terminal_at,
            "capability_snapshot": snapshot_id(),
            "actor": "identity:stub",
            "principal": null,
            "resource": recorded.resource,
            "outcome": outcome,
        })
    }
}

struct Endpoint {
    address: SocketAddr,
    /// The plaintext operational listener: `/readyz` only.
    ops: SocketAddr,
    roots: PathBuf,
    marker: PathBuf,
    stop: Arc<AtomicBool>,
    model: Arc<Mutex<Model>>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        let _ = TcpStream::connect(self.ops);
    }
}

fn snapshot_id() -> String {
    format!("sha256:{}", "b".repeat(64))
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

fn serve(directory: &Path) -> Endpoint {
    let (config, roots) = server_config(directory);
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let stop = Arc::new(AtomicBool::new(false));
    let marker = directory.join("restarted");
    let model = Arc::new(Mutex::new(Model {
        marker: Some(marker.clone()),
        ..Model::default()
    }));
    let (stopping, serving) = (Arc::clone(&stop), Arc::clone(&model));
    thread::spawn(move || {
        for tcp in listener.incoming() {
            if stopping.load(Ordering::SeqCst) {
                break;
            }
            let Ok(tcp) = tcp else { continue };
            let config = Arc::clone(&config);
            let model = Arc::clone(&serving);
            thread::spawn(move || connection(tcp, &config, &model));
        }
    });
    let operational = TcpListener::bind("127.0.0.1:0").expect("ops listener");
    let ops = operational.local_addr().expect("ops address");
    let (stopping, serving) = (Arc::clone(&stop), Arc::clone(&model));
    thread::spawn(move || {
        for tcp in operational.incoming() {
            if stopping.load(Ordering::SeqCst) {
                break;
            }
            let Ok(mut tcp) = tcp else { continue };
            let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
            if read_request(&mut tcp).is_none() {
                continue;
            }
            let ready = serving.lock().expect("model").phase() != Phase::Opening;
            let status = if ready {
                "200 OK"
            } else {
                "503 Service Unavailable"
            };
            let _ = tcp.write_all(
                format!("HTTP/1.1 {status}\r\nconnection: close\r\ncontent-length: 0\r\n\r\n")
                    .as_bytes(),
            );
        }
    });
    Endpoint {
        address,
        ops,
        roots,
        marker,
        stop,
        model,
    }
}

/// One request: its method, its target and its body.
fn read_request(stream: &mut impl std::io::Read) -> Option<(String, String, Vec<u8>)> {
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
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let target = request_line.next()?.to_owned();
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0_usize);
    let mut body = received[end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut block).ok()?;
        if read == 0 {
            return None;
        }
        body.extend_from_slice(&block[..read]);
    }
    Some((method, target, body))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "callers hand over the result they built"
)]
fn success(result: Value, operation: Option<&str>) -> Value {
    let mut envelope = json!({"api_version": "v1", "request_id": "req-settling", "result": result});
    if let Some(operation) = operation {
        envelope["operation"] = json!(operation);
    }
    envelope
}

fn failure(class: &str, code: &str, retriable: bool, operation: Option<&str>) -> Value {
    let mut error = json!({
        "class": class,
        "code": code,
        "message": "The stub endpoint answered this.",
        "retriable": retriable,
    });
    if let Some(operation) = operation {
        error["operation"] = json!(operation);
        error["address"] = json!("operation");
    }
    json!({"api_version": "v1", "request_id": "req-settling", "error": error})
}

fn not_found() -> (u16, Value) {
    (404, failure("refused", "resource.not-found", false, None))
}

fn machine(phase: Phase) -> Value {
    let mut facts = json!({
        "operation.ledger-subject-max-rows": 4096,
        "operation.ledger-subject-max-bytes": 67_108_864,
        "operation.ledger-global-max-rows": 4096,
        "operation.ledger-global-max-bytes": 67_108_864,
        "events.pull": true,
        "events.retention-events": 4096
    });
    if phase != Phase::Opening {
        for (fact, value) in [
            ("workspace.guarded-io", json!(true)),
            ("workspace.openat2-beneath", json!(true)),
            ("workspace.atomic-replace", json!(true)),
            ("workspace.max-file-bytes", json!(1_048_576)),
            ("workspace.read-limit-bytes", json!(1_048_576)),
            ("workspace.list-limit-items", json!(1000)),
        ] {
            facts[fact] = value;
        }
    }
    let probed_at = if phase == Phase::Before {
        "2026-10-31T00:00:00Z"
    } else {
        "2026-10-31T00:10:00Z"
    };
    success(
        json!({
            "snapshot": snapshot_id(),
            "driver": "host",
            "driver_version": "1.0.0",
            "config_generation": 1,
            "probed_at": probed_at,
            "facts": facts,
        }),
        None,
    )
}

/// The operation id and labels of a mutation body.
fn mutation(body: &[u8]) -> (String, Value) {
    let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    (
        body["op"].as_str().unwrap_or_default().to_owned(),
        body["input"]["labels"].clone(),
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "one match over every route the stub serves"
)]
fn route(model: &Mutex<Model>, method: &str, target: &str, body: &[u8]) -> (u16, Value) {
    let mut model = model.lock().expect("model");
    model.settle();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        ("GET", ["v1", "machine"]) => {
            let phase = model.phase();
            (200, machine(phase))
        }
        ("POST", ["v1", "workspaces"]) => {
            let (operation, labels) = mutation(body);
            let id = format!("ws_{}", ulid::Ulid::generate());
            let observed = json!({
                "id": id,
                "kind": "workspace",
                "labels": if labels.is_object() { labels } else { json!({}) },
                "observed_at": now(),
                "state": "ready",
                "origin": "created",
                "root_name": id,
            });
            model.workspaces.insert(
                id.clone(),
                Held {
                    observed: observed.clone(),
                    destroy: None,
                    files: BTreeMap::new(),
                },
            );
            model.operations.insert(
                operation.clone(),
                Recorded {
                    kind: "workspace.create",
                    resource: id.clone(),
                    outcome: Some((201, observed.clone())),
                },
            );
            model.event(&id, "workspace.created", &operation, observed.clone());
            (201, success(observed, Some(&operation)))
        }
        ("GET", ["v1", "workspaces", id]) => match model.workspaces.get(*id) {
            Some(held) => (200, success(held.observed.clone(), None)),
            None => not_found(),
        },
        ("PUT", ["v1", "workspaces", id, "files", file]) if model.workspaces.contains_key(*id) => {
            let (operation, _) = mutation(body);
            if let Some(Recorded {
                outcome: Some((status, result)),
                ..
            }) = model.operations.get(&operation)
            {
                return (*status, success(result.clone(), Some(&operation)));
            }
            let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            let bytes = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                parsed["input"]["content"]["data"]
                    .as_str()
                    .unwrap_or_default(),
            )
            .unwrap_or_default();
            let observed = json!({
                "kind": "file",
                "workspace": id,
                "path": file,
                "size": bytes.len(),
                "sha256": hex(&sha2::Sha256::digest(&bytes)),
                "atomic_replacement": true,
                "observed_at": now(),
            });
            if let Some(held) = model.workspaces.get_mut(*id) {
                held.files.insert((*file).to_owned(), bytes);
            }
            model.operations.insert(
                operation.clone(),
                Recorded {
                    kind: "workspace.file.write",
                    resource: (*id).to_owned(),
                    outcome: Some((200, observed.clone())),
                },
            );
            model.event(id, "workspace.file-written", &operation, observed.clone());
            (200, success(observed, Some(&operation)))
        }
        ("GET", ["v2", "workspaces", id, "files", file]) => {
            let Some(bytes) = model
                .workspaces
                .get(*id)
                .and_then(|held| held.files.get(*file))
            else {
                return not_found();
            };
            let size = bytes.len();
            (
                200,
                success(
                    json!({
                        "kind": "file",
                        "workspace": id,
                        "path": file,
                        "size": size,
                        "sha256": hex(&sha2::Sha256::digest(bytes)),
                        "offset": 0,
                        "returned_bytes": size,
                        "next_offset": size,
                        "eof": true,
                        "content": {
                            "encoding": "base64",
                            "data": base64::Engine::encode(
                                &base64::engine::general_purpose::STANDARD,
                                bytes,
                            ),
                        },
                        "observed_at": now(),
                    }),
                    None,
                ),
            )
        }
        ("DELETE", ["v1", "workspaces", id]) => {
            let (operation, _) = mutation(body);
            if let Some(recorded) = model.operations.get(&operation) {
                return match &recorded.outcome {
                    Some((status, result)) => (*status, success(result.clone(), Some(&operation))),
                    None => (
                        500,
                        failure("failed", OUTCOME_UNKNOWN, true, Some(&operation)),
                    ),
                };
            }
            let Some(held) = model.workspaces.get_mut(*id) else {
                return not_found();
            };
            if held.destroy.is_some() {
                return (
                    409,
                    failure("conflict", "workspace.not-ready", false, Some(&operation)),
                );
            }
            held.observed["state"] = json!("destroying");
            held.destroy = Some((operation.clone(), Instant::now() + SETTLE));
            model.operations.insert(
                operation.clone(),
                Recorded {
                    kind: "workspace.destroy",
                    resource: (*id).to_owned(),
                    outcome: None,
                },
            );
            model.unknown.push(operation.clone());
            (
                500,
                failure("failed", OUTCOME_UNKNOWN, true, Some(&operation)),
            )
        }
        ("GET", ["v1", "ops", operation]) if model.operations.contains_key(*operation) => {
            if model.operations[*operation].outcome.is_none() {
                model.accepted_reads.push((*operation).to_owned());
            }
            (200, success(model.record(operation), None))
        }
        ("GET", ["v1", "events"]) => {
            let after = query
                .split('&')
                .find_map(|pair| pair.strip_prefix("cursor="))
                .and_then(|cursor| cursor.strip_prefix("c"))
                .and_then(|seq| seq.parse::<u64>().ok())
                .unwrap_or(0);
            let items: Vec<Value> = model
                .events
                .iter()
                .filter(|event| event["seq"].as_u64().unwrap_or(0) > after)
                .cloned()
                .collect();
            let through = model.events.len() as u64;
            (
                200,
                success(
                    json!({
                        "source_scope": "scope-stub",
                        "generation": 1,
                        "items": items,
                        "next_cursor": format!("c{through}"),
                        "through_seq": through,
                        "first_retained_seq": if through == 0 { Value::Null } else { json!(1) },
                    }),
                    None,
                ),
            )
        }
        _ => not_found(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = std::fmt::Write::write_fmt(&mut text, format_args!("{byte:02x}"));
        text
    })
}

fn connection(tcp: TcpStream, config: &Arc<ServerConfig>, model: &Mutex<Model>) {
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(server) = ServerConnection::new(Arc::clone(config)) else {
        return;
    };
    let mut stream = StreamOwned::new(server, tcp);
    let Some((method, target, body)) = read_request(&mut stream) else {
        return;
    };
    let (status, answer) = route(model, &method, &target, &body);
    let body = answer.to_string();
    let reason = match status {
        200 => "OK",
        201 => "Created",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Internal Server Error",
    };
    let reply = format!(
        "HTTP/1.1 {status} {reason}\r\nx-b10x-contract: {CONTRACT}\r\n\
         x-b10x-contract-bundle-sha256: {CONTRACT_SHA256}\r\ncontent-type: application/json\r\n\
         connection: close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

/// Runs the runner on `groups` against the endpoint and answers each case's status and
/// diagnostics, by id.
fn run(directory: &Path, endpoint: &Endpoint, groups: &str) -> BTreeMap<String, (String, Value)> {
    run_with(directory, endpoint, groups, &[])
}

/// [`run`] with further runner arguments.
fn run_with(
    directory: &Path,
    endpoint: &Endpoint,
    groups: &str,
    extra: &[std::ffi::OsString],
) -> BTreeMap<String, (String, Value)> {
    assert!(Path::new(ECHO).is_file(), "{ECHO} is the token command");
    let out = directory.join(format!("out-{groups}"));
    let log = directory.join(format!("runner-{groups}.log"));
    let file = std::fs::File::create(&log).expect("log");
    let mut child = Command::new(RUNNER)
        .args(["--endpoint", &format!("https://{}/", endpoint.address)])
        .arg("--trust-roots")
        .arg(&endpoint.roots)
        .args(["--server-name", SERVER_NAME, "--token-command", ECHO])
        .args(["--token-command-arg", SENTINEL, "--out"])
        .arg(&out)
        .args(["--only", groups, "--deadline", "60", "--io-timeout", "2"])
        .args(["--restart-timeout", "20"])
        .args(extra)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(file.try_clone().expect("log"))
        .stderr(file)
        .spawn()
        .expect("runner starts");
    let started = Instant::now();
    loop {
        if child.try_wait().expect("wait").is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(90) {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the runner did not end: {}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
    let mut report = String::new();
    std::fs::File::open(out.join("report.json"))
        .unwrap_or_else(|_| {
            panic!(
                "precondition: the runner wrote a report: {}",
                std::fs::read_to_string(&log).unwrap_or_default()
            )
        })
        .read_to_string(&mut report)
        .expect("report");
    let report: Value = serde_json::from_str(&report).expect("report is JSON");
    report["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|case| {
            (
                case["id"].as_str().unwrap_or_default().to_owned(),
                (
                    case["status"].as_str().unwrap_or_default().to_owned(),
                    case["diagnostics"].clone(),
                ),
            )
        })
        .collect()
}

/// The stub's own answers decode as the wire types the SDK and the runner decode them into, so a
/// case cannot fail here on a malformed stub.
#[test]
fn the_stub_answers_in_the_wire_types() {
    let model = Mutex::new(Model::default());
    let created = route(
        &model,
        "POST",
        "/v1/workspaces",
        br#"{"op":"01JSETTLINGCREATE0000000001","input":{"source":"empty","labels":{"k":"v"}}}"#,
    );
    assert_eq!(created.0, 201);
    let workspace: substrate_wire::Success<substrate_wire::Workspace> =
        serde_json::from_value(created.1).expect("a wire workspace");
    let id = workspace.result.id;
    let delete = br#"{"op":"01JSETTLINGDESTROY000000001","input":{}}"#;
    let unknown = route(&model, "DELETE", &format!("/v1/workspaces/{id}"), delete);
    assert_eq!(unknown.0, 500);
    let _: substrate_wire::Failure = serde_json::from_value(unknown.1).expect("a wire failure");
    let accepted = route(&model, "GET", "/v1/ops/01JSETTLINGDESTROY000000001", b"");
    let _: substrate_wire::Success<substrate_wire::OperationRecord> =
        serde_json::from_value(accepted.1).expect("a wire operation record");
    thread::sleep(SETTLE);
    let terminal = route(&model, "GET", "/v1/ops/01JSETTLINGDESTROY000000001", b"");
    let record: substrate_wire::Success<substrate_wire::OperationRecord> =
        serde_json::from_value(terminal.1).expect("a wire operation record");
    assert_eq!(
        record.result.state,
        substrate_wire::OperationState::Terminal
    );
    let page = route(&model, "GET", "/v1/events?limit=1000", b"");
    let _: substrate_wire::Success<substrate_wire::EventPage> =
        serde_json::from_value(page.1).expect("a wire event page");
    let _: substrate_wire::Success<substrate_wire::CapabilitySnapshot> =
        serde_json::from_value(machine(Phase::Before)).expect("a wire machine envelope");
    assert_eq!(
        route(&model, "GET", &format!("/v1/workspaces/{id}"), b"").0,
        404
    );
}

/// story:clean-room-destroy-settles-on-the-namespace-driver: a destroy the endpoint answers
/// `operation.outcome-unknown` and then finishes through reconciliation passes `workspace.destroy`
/// once its operation reads terminal with the absence and the workspace reads not found.
#[test]
fn a_destroy_left_to_reconciliation_passes_once_its_operation_settles() {
    let directory = tempfile::tempdir().expect("scratch");
    let endpoint = serve(directory.path());
    let cases = run(directory.path(), &endpoint, "workspace");
    let model = endpoint.model.lock().expect("model");
    assert!(
        !model.unknown.is_empty(),
        "precondition: the endpoint answered a destroy operation.outcome-unknown"
    );
    assert_eq!(
        cases
            .get("workspace.destroy")
            .map(|(status, _)| status.as_str()),
        Some("passed"),
        "story:clean-room-destroy-settles-on-the-namespace-driver: design 01 § 6 answers a \
         retriable operation.outcome-unknown by reading the ledger, and design 31 § 3 reads the \
         destroy back; the runner reported {:?}",
        cases.get("workspace.destroy")
    );
    assert!(
        !model.accepted_reads.is_empty(),
        "the runner passed the destroy without reading its operation while it was accepted"
    );
    assert!(
        model.workspaces.is_empty(),
        "every workspace the run created is gone at its end, the destroyed one and the ones the \
         closing cleanup removed: {:?}",
        model.workspaces.keys().collect::<Vec<_>>()
    );
}

/// story:clean-room-destroy-settles-on-the-namespace-driver: `events.commit-order` finds the
/// destroy's own event, which the endpoint commits when reconciliation proves the absence.
#[test]
fn commit_order_finds_the_event_of_a_destroy_left_to_reconciliation() {
    let directory = tempfile::tempdir().expect("scratch");
    let endpoint = serve(directory.path());
    let cases = run(directory.path(), &endpoint, "events");
    let model = endpoint.model.lock().expect("model");
    assert!(
        !model.unknown.is_empty(),
        "precondition: the endpoint answered a destroy operation.outcome-unknown"
    );
    assert_eq!(
        cases
            .get("events.commit-order")
            .map(|(status, _)| status.as_str()),
        Some("passed"),
        "story:clean-room-destroy-settles-on-the-namespace-driver: the runner reported {:?}",
        cases.get("events.commit-order")
    );
}

/// story:clean-room-destroy-settles-on-the-namespace-driver: after the restart hook, the runner
/// selects the restart cases on the facts of a snapshot read once `/readyz` answers, not on the
/// first snapshot whose `probed_at` moved. Design 34 § 6.1: a restarted daemon serves its own
/// facts before the driver's first probe settles, and `/readyz` holds until it has.
#[test]
fn the_restart_cases_run_on_the_facts_read_once_the_endpoint_is_ready() {
    let directory = tempfile::tempdir().expect("scratch");
    let endpoint = serve(directory.path());
    let extra: Vec<std::ffi::OsString> = vec![
        "--restart-hook".into(),
        "/usr/bin/touch".into(),
        "--restart-hook-arg".into(),
        endpoint.marker.clone().into_os_string(),
        "--ops-origin".into(),
        format!("http://{}", endpoint.ops).into(),
    ];
    assert!(
        Path::new("/usr/bin/touch").is_file(),
        "/usr/bin/touch is the hook"
    );
    let cases = run_with(directory.path(), &endpoint, "restart", &extra);
    assert_eq!(
        cases
            .get("restart.detected")
            .map(|(status, _)| status.as_str()),
        Some("passed"),
        "precondition: the runner detected the restart: {:?}",
        cases.get("restart.detected")
    );
    for case in [
        "restart.workspace-readable",
        "restart.replay",
        "restart.cursor",
    ] {
        assert_eq!(
            cases.get(case).map(|(status, _)| status.as_str()),
            Some("passed"),
            "design 31 § 4 runs {case} against the restarted endpoint; the runner reported {:?}",
            cases.get(case)
        );
    }
    let model = endpoint.model.lock().expect("model");
    assert!(
        model.workspaces.is_empty(),
        "the restart workspace is gone at the run's end: {:?}",
        model.workspaces.keys().collect::<Vec<_>>()
    );
}
