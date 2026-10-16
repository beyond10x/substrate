//! Security conformance cases for `b10x-substrate`, a client of the daemon's Unix socket.
//!
//! The safety envelope in `AGENTS.md` says the daemon derives its subject from kernel peer
//! credentials and never from HTTP data, and that static-bearer TCP is development-only. A client
//! holds its half of that: it sends no header or body field that claims an identity, it connects
//! only to the socket `--socket` or `SUBSTRATE_SOCKET` names, never to a default and never over
//! TCP, it passes `exec run` argv through unchanged, and it prints no environment content.
//!
//! The request cases put a recording relay between the CLI and the shipped daemon and assert on
//! the bytes the CLI actually wrote. The relay connects as this test's uid, which the managed
//! daemon admits, so the daemon still answers every request.
//!
//! The confined-output half of the argv case needs confined exec. Without
//! `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree it says so on stderr and
//! asserts only the request half: absent, not passed.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex, OnceLock};

use b10x_substrate_cli::Cli;
use b10x_substrate_sdk::ManagedDaemon;
use clap::CommandFactory as _;
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};

/// A value no part of the CLI may print or send: it stands for a credential in the environment.
const ENVIRONMENT_SECRET: &str = "env-secret-7f3a9c41d2b84e6f";

/// The shipped daemon binary, found or built as `scenarios.rs` does.
fn daemon_binary() -> PathBuf {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY
        .get_or_init(|| {
            if let Some(path) = std::env::var_os("SUBSTRATE_TEST_DAEMON") {
                return path.into();
            }
            let mut profile_dir = std::env::current_exe().expect("current test executable");
            profile_dir.pop();
            if profile_dir.file_name().is_some_and(|name| name == "deps") {
                profile_dir.pop();
            }
            let sibling = profile_dir.join("substrate-daemon");
            if sibling.is_file() {
                return sibling;
            }
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let mut build = std::process::Command::new(cargo);
            build
                .arg("build")
                .arg("--locked")
                .arg("--manifest-path")
                .arg(&manifest)
                .args(["-p", "b10x-substrate-daemon", "--bin", "substrate-daemon"]);
            if profile_dir
                .file_name()
                .is_some_and(|name| name == "release")
            {
                build.arg("--release");
            }
            let status = build
                .status()
                .expect("starting cargo to build substrate-daemon");
            assert!(
                status.success(),
                "building substrate-daemon failed: {status}"
            );
            let binary = profile_dir.join("substrate-daemon");
            assert!(
                binary.is_file(),
                "{} is absent after the build",
                binary.display()
            );
            binary
        })
        .clone()
}

struct Fixture {
    data: tempfile::TempDir,
    _daemon: ManagedDaemon,
    socket: PathBuf,
}

async fn fixture() -> Fixture {
    let data = tempfile::tempdir().expect("temporary parent");
    let root = data.path().join("owned");
    let mut builder = ManagedDaemon::builder()
        .data_dir(&root)
        .deployment("cli_security")
        .external_binary(daemon_binary());
    if let Some(cgroup) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") {
        builder = builder.cgroup_root(cgroup);
    }
    let daemon = builder.start().await.expect("managed daemon starts");
    Fixture {
        data,
        _daemon: daemon,
        socket: root.join("substrate.sock"),
    }
}

/// Every byte stream a client wrote to the relay, one per connection, appended as it arrives.
type Streams = Arc<Mutex<Vec<Arc<Mutex<Vec<u8>>>>>>;

/// A Unix socket relay to `upstream` that records what the client sends.
struct Relay {
    socket: PathBuf,
    streams: Streams,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn relay(upstream: &Path, dir: &Path) -> Relay {
    let socket = dir.join("relay.sock");
    let listener = UnixListener::bind(&socket).expect("relay socket binds");
    let streams: Streams = Arc::default();
    let recorded = Arc::clone(&streams);
    let upstream = upstream.to_owned();
    let task = tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let seen: Arc<Mutex<Vec<u8>>> = Arc::default();
            recorded.lock().expect("streams").push(Arc::clone(&seen));
            let upstream = upstream.clone();
            tokio::spawn(async move {
                let Ok(server) = UnixStream::connect(&upstream).await else {
                    return;
                };
                let (mut client_read, mut client_write) = client.into_split();
                let (mut server_read, mut server_write) = server.into_split();
                let up = async {
                    let mut buffer = vec![0_u8; 16 * 1024];
                    loop {
                        match client_read.read(&mut buffer).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => {
                                seen.lock()
                                    .expect("stream")
                                    .extend_from_slice(&buffer[..read]);
                                if server_write.write_all(&buffer[..read]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    let _ = server_write.shutdown().await;
                };
                let down = async {
                    let _ = tokio::io::copy(&mut server_read, &mut client_write).await;
                    let _ = client_write.shutdown().await;
                };
                tokio::join!(up, down);
            });
        }
    });
    Relay {
        socket,
        streams,
        task,
    }
}

/// One HTTP request as the client wrote it.
struct Request {
    line: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// The first request on each recorded connection. The SDK writes `Connection: close` and a
/// `content-length` body, so a connection carries one request.
fn requests(relay: &Relay) -> Vec<Request> {
    let streams: Vec<Vec<u8>> = relay
        .streams
        .lock()
        .expect("streams")
        .iter()
        .map(|seen| seen.lock().expect("stream").clone())
        .collect();
    streams
        .iter()
        .filter_map(|bytes| {
            let end = bytes.windows(4).position(|window| window == b"\r\n\r\n")?;
            let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
            let mut lines = head.split("\r\n");
            let line = lines.next().unwrap_or_default().to_owned();
            let headers: Vec<(String, String)> = lines
                .map(|header| {
                    let (name, value) = header.split_once(':').unwrap_or((header, ""));
                    (name.trim().to_ascii_lowercase(), value.trim().to_owned())
                })
                .collect();
            let length = headers
                .iter()
                .find(|(name, _)| name == "content-length")
                .and_then(|(_, value)| value.parse::<usize>().ok())
                .unwrap_or(0);
            let start = end + 4;
            let body = bytes[start..bytes.len().min(start + length)].to_vec();
            Some(Request {
                line,
                headers,
                body,
            })
        })
        .collect()
}

fn all_recorded_bytes(relay: &Relay) -> Vec<u8> {
    relay
        .streams
        .lock()
        .expect("streams")
        .iter()
        .flat_map(|seen| seen.lock().expect("stream").clone())
        .collect()
}

/// The CLI with a scrubbed socket variable and credential-shaped variables set to the secret.
fn command(cwd: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_b10x-substrate"));
    command.current_dir(cwd).env_remove("SUBSTRATE_SOCKET");
    for name in [
        "SUBSTRATE_TOKEN",
        "SUBSTRATE_BEARER",
        "SUBSTRATE_SUBJECT",
        "SUBSTRATE_TENANT",
        "SUBSTRATE_UID",
        "SUBSTRATE_DELEGATED_CONTEXT",
        "B10X_SUBSTRATE_TOKEN",
        "B10X_TOKEN",
        "GITHUB_TOKEN",
    ] {
        command.env(name, ENVIRONMENT_SECRET);
    }
    command
}

async fn cli(socket: &Path, cwd: &Path, args: &[&str]) -> Output {
    let mut command = command(cwd);
    command.arg("--socket").arg(socket).args(args);
    command.output().await.expect("the CLI runs")
}

fn shown(output: &Output) -> String {
    format!(
        "status {:?}, stdout {:?}, stderr {:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json_of(bytes: &[u8], output: &Output) -> Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|error| panic!("not one JSON document ({error}): {}", shown(output)))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn assert_no_secret(output: &Output) {
    let secret = ENVIRONMENT_SECRET.as_bytes();
    assert!(
        !contains(&output.stdout, secret) && !contains(&output.stderr, secret),
        "the CLI printed environment content: {}",
        shown(output)
    );
}

/// Header names a client of the local socket writes. Anything else, `authorization` above all, is
/// a field the daemon would have to ignore to keep deriving the subject from peer credentials.
const CLIENT_HEADERS: [&str; 10] = [
    "host",
    "x-request-id",
    "connection",
    "content-type",
    "content-length",
    "upgrade",
    "sec-websocket-key",
    "sec-websocket-version",
    "x-b10x-contract",
    "x-b10x-contract-bundle-sha256",
];

/// Body members that claim or carry an identity, at any depth.
const IDENTITY_MEMBERS: [&str; 13] = [
    "subject",
    "tenant",
    "uid",
    "gid",
    "user",
    "actor",
    "principal",
    "platform_principal",
    "delegated_context",
    "grant_ref",
    "authorization",
    "token",
    "credentials",
];

fn identity_members(value: &Value, path: &str, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, member) in map {
                let here = format!("{path}.{key}");
                if IDENTITY_MEMBERS.contains(&key.to_ascii_lowercase().as_str()) {
                    found.push(here.clone());
                }
                identity_members(member, &here, found);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                identity_members(item, &format!("{path}[{index}]"), found);
            }
        }
        _ => {}
    }
}

fn find_array(value: &Value, wanted: &[Value]) -> bool {
    match value {
        Value::Array(items) => {
            items.as_slice() == wanted || items.iter().any(|item| find_array(item, wanted))
        }
        Value::Object(map) => map.values().any(|member| find_array(member, wanted)),
        _ => false,
    }
}

fn exec_served(facts: &Value) -> bool {
    facts.get("exec.namespaces").is_some()
        && facts.get("exec.cgroup-limits").is_some()
        && facts.get("exec.cgroup-kill") == Some(&Value::Bool(true))
        && facts.get("exec.no-egress") == Some(&Value::Bool(true))
}

// --- 1. The command line ----------------------------------------------------------------------

fn every_argument(command: &clap::Command, path: &str, out: &mut Vec<(String, clap::Arg)>) {
    for arg in command.get_arguments() {
        out.push((path.to_owned(), arg.clone()));
    }
    for sub in command.get_subcommands() {
        every_argument(sub, &format!("{path} {}", sub.get_name()), out);
    }
}

/// No option or argument anywhere in the tree names an identity or a remote transport, and none
/// reads the environment through clap: the only variable the CLI reads is `SUBSTRATE_SOCKET`, in
/// `main_with`, so a clap `env =` on any argument is a second, unreviewed environment input.
#[test]
fn no_argument_claims_identity_or_reads_the_environment() {
    let mut arguments = Vec::new();
    every_argument(&Cli::command(), "b10x-substrate", &mut arguments);
    assert!(arguments.len() > 10, "the walk saw the command tree");
    let forbidden = [
        "subject",
        "tenant",
        "uid",
        "user",
        "principal",
        "actor",
        "token",
        "bearer",
        "auth",
        "credential",
        "delegat",
        "grant",
        "tcp",
        "host",
        "port",
        "endpoint",
        "url",
        "https",
        "trust",
        "identity",
        "insecure",
    ];
    let mut violations = Vec::new();
    for (path, arg) in &arguments {
        let names: Vec<String> = std::iter::once(arg.get_id().as_str().to_owned())
            .chain(arg.get_long().map(ToOwned::to_owned))
            .chain(
                arg.get_all_aliases()
                    .into_iter()
                    .flatten()
                    .map(ToOwned::to_owned),
            )
            .collect();
        for name in names {
            let lowered = name.to_ascii_lowercase();
            if forbidden.iter().any(|word| lowered.contains(word)) {
                violations.push(format!("{path}: {name}"));
            }
        }
        if let Some(variable) = arg.get_env() {
            violations.push(format!("{path}: {} reads {variable:?}", arg.get_id()));
        }
    }
    assert!(
        violations.is_empty(),
        "identity or transport arguments: {violations:?}"
    );
}

// --- 2. What reaches the socket ---------------------------------------------------------------

/// Every request the CLI writes, over the commands that write a body and those that do not,
/// carries only the client headers and no identity member, and no request or output carries the
/// credential-shaped environment.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One journey over every command that reaches the daemon.
async fn requests_carry_no_identity_header_or_field() {
    let fixture = fixture().await;
    let relay = relay(&fixture.socket, fixture.data.path());
    let cwd = fixture.data.path();
    let socket = relay.socket.clone();
    let mut outputs = Vec::new();

    let machine = cli(&socket, cwd, &["--output", "json", "machine"]).await;
    outputs.push(machine);
    let created = cli(
        &socket,
        cwd,
        &[
            "--output",
            "json",
            "workspace",
            "create",
            "--labels",
            r#"{"subject":"local:0","tenant":"other"}"#,
            "--lease-ttl-ms",
            "60000",
            "--operation-id",
            "op_security_create_0001",
        ],
    )
    .await;
    let workspace = json_of(&created.stdout, &created)["result"]["workspace"]
        .as_str()
        .unwrap_or_else(|| panic!("workspace id: {}", shown(&created)))
        .to_owned();
    outputs.push(created);
    for args in [
        vec!["--output", "json", "workspace", "show", &workspace],
        vec![
            "--output",
            "json",
            "workspace",
            "renew",
            &workspace,
            "--ttl-ms",
            "60000",
        ],
        vec![
            "--output",
            "json",
            "exec",
            "run",
            "--workspace",
            &workspace,
            "--",
            "/bin/sh",
            "-c",
            "exit 0",
        ],
        vec!["--output", "json", "events", "--limit", "5"],
        vec![
            "--output",
            "json",
            "operation",
            "show",
            "op_security_create_0001",
        ],
        vec![
            "--output",
            "json",
            "metrics",
            "--kind",
            "workspace",
            "--id",
            &workspace,
        ],
        vec![
            "--output",
            "json",
            "session",
            "show",
            "ses_01JAAAAAAAAAAAAAAAAAAAAAAA",
        ],
        vec!["workspace", "show", "ws_01JAAAAAAAAAAAAAAAAAAAAAAA"],
        vec!["--output", "json", "workspace", "destroy", &workspace],
    ] {
        outputs.push(cli(&socket, cwd, &args).await);
    }

    for output in &outputs {
        assert_no_secret(output);
    }
    let recorded = requests(&relay);
    assert!(
        recorded.len() >= outputs.len(),
        "every command reached the relay: {} requests for {} commands",
        recorded.len(),
        outputs.len()
    );
    assert!(
        recorded
            .iter()
            .any(|request| request.line.starts_with("POST ")),
        "a body-carrying request was recorded"
    );
    let mut violations = Vec::new();
    for request in &recorded {
        for (name, value) in &request.headers {
            if !CLIENT_HEADERS.contains(&name.as_str()) {
                violations.push(format!("{}: header {name}: {value}", request.line));
            }
        }
        if let Some(body) = request.json() {
            // A label is the caller's data, not a claim; its keys are values of `labels`.
            let mut stripped = body.clone();
            if let Some(labels) = stripped.pointer_mut("/input/labels") {
                *labels = Value::Null;
            }
            let mut found = Vec::new();
            identity_members(&stripped, "$", &mut found);
            for member in found {
                violations.push(format!("{}: body member {member}", request.line));
            }
            if let Some(env) = body.pointer("/input/env") {
                let set_empty = env["set"].as_object().is_none_or(serde_json::Map::is_empty);
                let allow_empty = env["allow"].as_array().is_none_or(Vec::is_empty);
                if !(set_empty && allow_empty) {
                    violations.push(format!("{}: exec environment {env}", request.line));
                }
            }
        } else if !request.body.is_empty() {
            violations.push(format!("{}: a body that is not JSON", request.line));
        }
    }
    assert!(
        violations.is_empty(),
        "identity-bearing request content: {violations:#?}"
    );
    assert!(
        !contains(&all_recorded_bytes(&relay), ENVIRONMENT_SECRET.as_bytes()),
        "environment content reached the socket"
    );
}

/// `exec run` passes each argv word to the daemon as itself: no shell, no splitting, no
/// interpolation, and words that look like CLI options after `--` stay argv. Where confined exec
/// is served, the command sees the same words.
#[tokio::test(flavor = "multi_thread")]
async fn exec_argv_reaches_the_daemon_and_the_command_unchanged() {
    let fixture = fixture().await;
    let relay = relay(&fixture.socket, fixture.data.path());
    let cwd = fixture.data.path();
    let socket = relay.socket.clone();
    let machine = cli(&socket, cwd, &["--output", "json", "machine"]).await;
    let served = exec_served(&json_of(&machine.stdout, &machine)["result"]["observation"]["facts"]);
    let created = cli(&socket, cwd, &["--output", "json", "workspace", "create"]).await;
    let workspace = json_of(&created.stdout, &created)["result"]["workspace"]
        .as_str()
        .expect("workspace id")
        .to_owned();

    // No empty word: the daemon's exec contract refuses one as `request.schema-invalid`
    // (`crates/substrate-daemon/src/app/operations.rs`, the argv shape check), which
    // `empty_argv_word_keeps_the_daemon_refusal` below asserts.
    let words = [
        "--",
        "--output",
        "json",
        "--socket",
        "/elsewhere",
        "$(echo interpolated)",
        "`id`",
        "a b\tc",
        "*",
        "'\"\\",
        "line\nbreak",
        "\u{1b}[31m",
        "ü",
        "-x",
        "${HOME}",
    ];
    let script = r#"for word; do printf '[%s]' "$word"; done"#;
    let mut argv = vec!["/bin/sh", "-c", script, "sh"];
    argv.extend(words);
    let mut cli_words = vec!["exec", "run", "--workspace", workspace.as_str(), "--"];
    cli_words.extend(argv.iter().copied());
    let human = cli(&socket, cwd, &cli_words).await;
    assert_no_secret(&human);

    let wanted: Vec<Value> = argv.iter().map(|word| Value::from(*word)).collect();
    let sent = requests(&relay).into_iter().any(|request| {
        request.line.starts_with("POST /v1/execs")
            && request
                .json()
                .is_some_and(|body| find_array(&body, &wanted))
    });
    if served {
        assert!(
            sent,
            "the exec request carries argv exactly: {}",
            shown(&human)
        );
        let expected: String = words.iter().fold(String::new(), |mut text, word| {
            text.push('[');
            text.push_str(word);
            text.push(']');
            text
        });
        assert_eq!(
            String::from_utf8_lossy(&human.stdout),
            expected,
            "the command saw its words unchanged: {}",
            shown(&human)
        );
        assert_eq!(human.status.code(), Some(0), "{}", shown(&human));
    } else {
        eprintln!(
            "exec_argv_reaches_the_daemon_and_the_command_unchanged: ABSENT, confined exec is \
             unserved; asserting the request half only"
        );
        assert_ne!(human.status.code(), Some(0), "{}", shown(&human));
        assert!(
            String::from_utf8_lossy(&human.stderr).contains("exec.sandbox-unavailable"),
            "{}",
            shown(&human)
        );
        assert!(
            sent || requests(&relay)
                .iter()
                .all(|request| !request.line.starts_with("POST /v1/execs")),
            "an exec request was sent with different argv: {}",
            shown(&human)
        );
    }
}

/// Argv that is not UTF-8 has no faithful value on the JSON wire. The CLI refuses it as a usage
/// error before it connects, rather than sending a replaced word.
#[tokio::test(flavor = "multi_thread")]
async fn non_utf8_argv_is_refused_before_any_request() {
    let fixture = fixture().await;
    let relay = relay(&fixture.socket, fixture.data.path());
    let mut command = command(fixture.data.path());
    command
        .arg("--socket")
        .arg(&relay.socket)
        .args([
            "--output",
            "json",
            "exec",
            "run",
            "--workspace",
            "ws_01JAAAAAAAAAAAAAAAAAAAAAAA",
            "--",
            "/bin/echo",
        ])
        .arg(OsString::from_vec(vec![b'a', 0xff, b'b']));
    let output = command.output().await.expect("the CLI runs");
    assert_eq!(output.status.code(), Some(2), "{}", shown(&output));
    let document = json_of(&output.stderr, &output);
    assert_eq!(document["error"]["code"], "cli_input", "{}", shown(&output));
    assert!(
        relay.streams.lock().expect("streams").is_empty(),
        "the CLI connected with an argv it cannot send"
    );
}

/// An empty argv word goes to the daemon as itself, and the daemon's own refusal comes back with
/// its named code in `error.data.code` and exit 1: the CLI neither drops the word nor invents a
/// refusal of its own.
#[tokio::test(flavor = "multi_thread")]
async fn empty_argv_word_keeps_the_daemon_refusal() {
    let fixture = fixture().await;
    let relay = relay(&fixture.socket, fixture.data.path());
    let cwd = fixture.data.path();
    let socket = relay.socket.clone();
    let created = cli(&socket, cwd, &["--output", "json", "workspace", "create"]).await;
    let workspace = json_of(&created.stdout, &created)["result"]["workspace"]
        .as_str()
        .expect("workspace id")
        .to_owned();
    let args = [
        "--output",
        "json",
        "exec",
        "run",
        "--workspace",
        workspace.as_str(),
        "--",
        "/bin/echo",
        "",
    ];
    let output = cli(&socket, cwd, &args).await;
    assert_eq!(output.status.code(), Some(1), "{}", shown(&output));
    let document = json_of(&output.stderr, &output);
    assert_eq!(document["error"]["code"], "refused", "{}", shown(&output));
    let code = document["error"]["data"]["code"]
        .as_str()
        .unwrap_or_default();
    assert!(
        code == "request.schema-invalid" || code == "exec.sandbox-unavailable",
        "a named daemon code: {}",
        shown(&output)
    );
    let wanted = [Value::from("/bin/echo"), Value::from("")];
    let sent: Vec<Request> = requests(&relay)
        .into_iter()
        .filter(|request| request.line.starts_with("POST /v1/execs"))
        .collect();
    assert!(
        sent.iter().all(|request| request
            .json()
            .is_some_and(|body| find_array(&body, &wanted))),
        "an exec request dropped or changed the empty word"
    );
}

// --- 3. Where the CLI connects ----------------------------------------------------------------

/// A socket value shaped like a network address is a filesystem path. The CLI reports it
/// unavailable and never opens a TCP connection, from the flag or from `SUBSTRATE_SOCKET`.
#[tokio::test(flavor = "multi_thread")]
async fn a_socket_value_is_never_a_network_address() {
    let dir = tempfile::tempdir().expect("temporary cwd");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("tcp listener binds");
    listener
        .set_nonblocking(true)
        .expect("non-blocking listener");
    let port = listener.local_addr().expect("local address").port();
    let values = [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("tcp://127.0.0.1:{port}"),
        format!("http://127.0.0.1:{port}"),
        format!("unix+http://127.0.0.1:{port}"),
    ];
    for value in &values {
        for from_env in [false, true] {
            let mut command = command(dir.path());
            if from_env {
                command.env("SUBSTRATE_SOCKET", value);
            } else {
                command.arg("--socket").arg(value);
            }
            command.args(["--output", "json", "machine"]);
            let output = command.output().await.expect("the CLI runs");
            assert_no_secret(&output);
            assert_eq!(output.status.code(), Some(1), "{value}: {}", shown(&output));
            let document = json_of(&output.stderr, &output);
            assert_eq!(
                document["error"]["code"],
                "unavailable",
                "{value}: {}",
                shown(&output)
            );
            match listener.accept() {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Ok((_, peer)) => panic!("{value} (env {from_env}) opened TCP from {peer}"),
                Err(error) => panic!("accept failed: {error}"),
            }
        }
    }
}

/// With no `--socket` and no `SUBSTRATE_SOCKET`, the CLI connects nowhere, even with a live socket
/// at every conventional location under the working directory, `HOME` and `XDG_RUNTIME_DIR`.
#[tokio::test(flavor = "multi_thread")]
async fn no_socket_selected_connects_nowhere() {
    let dir = tempfile::tempdir().expect("temporary home");
    let home = dir.path().join("home");
    let runtime = dir.path().join("run");
    let state = home.join(".local/state");
    let candidates = [
        dir.path().join("substrate.sock"),
        runtime.join("substrate.sock"),
        runtime.join("substrate/substrate.sock"),
        runtime.join("b10x/substrate.sock"),
        state.join("substrate/substrate.sock"),
        state.join("b10x/substrate/substrate.sock"),
        home.join(".substrate/substrate.sock"),
    ];
    let listeners: Vec<std::os::unix::net::UnixListener> = candidates
        .iter()
        .map(|path| {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("socket parent");
            let listener = std::os::unix::net::UnixListener::bind(path).expect("listener binds");
            listener
                .set_nonblocking(true)
                .expect("non-blocking listener");
            listener
        })
        .collect();
    let mut command = command(dir.path());
    command
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("XDG_STATE_HOME", &state)
        .args(["--output", "json", "machine"]);
    let output = command.output().await.expect("the CLI runs");
    assert_no_secret(&output);
    assert_eq!(output.status.code(), Some(2), "{}", shown(&output));
    assert_eq!(
        json_of(&output.stderr, &output)["error"]["code"],
        "cli_input"
    );
    for (path, listener) in candidates.iter().zip(&listeners) {
        match listener.accept() {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Ok(_) => panic!("the CLI connected to a default socket {}", path.display()),
            Err(error) => panic!("accept failed: {error}"),
        }
    }
}
