#![forbid(unsafe_code)]
//! Independent black-box checks for the Unix-socket HTTP runtime.
//!
//! This is the clean-room lane. Every other file in this directory constructs `App` in process
//! and drives the router directly; this one spawns the **shipped binary**
//! (`env!("CARGO_BIN_EXE_substrate-daemon")`), talks to it over its Unix socket with a
//! hand-written HTTP/1.1 and WebSocket client, and asserts only on the wire. Nothing here links
//! the implementation, so a refusal proved here is proved against what ships.
//!
//! **Lanes.** The portable lane runs everywhere and asserts the named refusal
//! `exec.sandbox-unavailable` (501) — invariant 3, a missing isolation guarantee is a named
//! refusal, never silent degradation. The delegated lane is selected by
//! `SUBSTRATE_VECTORS_CGROUP_ROOT`, which must name a delegated cgroup v2 subtree carrying
//! `cpu`/`memory`/`pids` **that this test process is itself inside**; it adds the confined exec,
//! no-egress, pids/memory, timeout, truncation, whole-tree cancellation, egress-aperture and
//! sealed-secret-slot cases. When the variable is unset the delegated cases are *absent*: they are
//! not run and are not counted.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use ed25519_dalek::{Signer as _, SigningKey};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use serde_json::{Value, json};
use sha2::Digest as _;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

#[path = "../../../test-support/readiness.rs"]
mod readiness;
#[path = "../../../test-support/unix_socket.rs"]
mod unix_socket;

/// The shipped binary, built by cargo before this integration test runs.
const DAEMON: &str = env!("CARGO_BIN_EXE_substrate-daemon");
const CGROUP_EXEC: &str = env!("CARGO_BIN_EXE_substrate-cgroup-exec");
const DAEMON_OVERRIDE_VARIABLE: &str = "SUBSTRATE_VECTORS_DAEMON";
/// Pinned independently of the daemon implementation: the clean-room client verifies what ships.
const ADVERTISED_CONTRACT: &str = "substrate-wire/0.25.0";
const ADVERTISED_CONTRACT_SHA256: &str =
    "853d7365a56f0d8dc97fe5feb87ce57285a34bbf7a0f6eb05616d6010fc4fc00";

fn daemon_binary() -> PathBuf {
    std::env::var_os(DAEMON_OVERRIDE_VARIABLE).map_or_else(|| PathBuf::from(DAEMON), PathBuf::from)
}

/// Selects the delegated lane, mirroring the predecessor's `--cgroup-root` argument.
const CGROUP_ROOT_VARIABLE: &str = "SUBSTRATE_VECTORS_CGROUP_ROOT";

struct DelegatedCgroup(PathBuf);

impl DelegatedCgroup {
    fn acquire() -> Option<Self> {
        let parent = std::env::var_os(CGROUP_ROOT_VARIABLE).map(PathBuf::from)?;
        let path = parent.join(format!("substrate-test-{}", ulid::Ulid::generate()));
        std::fs::create_dir(&path).expect("create per-test delegated cgroup root");
        std::fs::create_dir(path.join("daemon")).expect("create daemon cgroup");
        std::fs::write(path.join("cgroup.subtree_control"), "+cpu +memory +pids")
            .expect("delegate cpu, memory and pids to per-exec child cgroups");
        Some(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

/// What a cgroup held when it could not be removed: its `cgroup.procs`, `cgroup.events`, each
/// member process as `/proc` describes it, and every child group, recursively.
fn describe_cgroup(path: &Path, out: &mut String) {
    let read = |name: &str| {
        std::fs::read_to_string(path.join(name)).map_or_else(
            |error| format!("<{error}>"),
            |text| text.trim().replace('\n', " "),
        )
    };
    let _ = writeln!(
        out,
        "  {}: procs=[{}] events=[{}]",
        path.display(),
        read("cgroup.procs"),
        read("cgroup.events")
    );
    if let Ok(procs) = std::fs::read_to_string(path.join("cgroup.procs")) {
        for pid in procs.split_whitespace() {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .unwrap_or_else(|error| format!("<{error}>"));
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).map_or_else(
                |error| format!("<{error}>"),
                |raw| String::from_utf8_lossy(&raw).replace('\0', " "),
            );
            let _ = writeln!(out, "    pid {pid}: stat={} cmdline={cmdline}", stat.trim());
        }
    }
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.filter_map(Result::ok) {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                describe_cgroup(&entry.path(), out);
            }
        }
    }
}

impl DelegatedCgroup {
    /// Removes one per-test group, once, and fails with what it still held if it is busy.
    ///
    /// No wait and no retry: a group that is not empty when the daemon has exited is the finding
    /// itself, and the snapshot taken at that instant is its evidence. A test that is already
    /// panicking reports the snapshot instead of panicking a second time, which would abort the
    /// test binary and lose the first failure's own message.
    fn remove(path: &Path, what: &str) {
        let Err(error) = std::fs::remove_dir(path) else {
            return;
        };
        let mut held = String::new();
        describe_cgroup(path, &mut held);
        let message = format!(
            "per-test {what} cgroup {} was not empty at teardown: {error}\n{held}",
            path.display()
        );
        if std::thread::panicking() {
            eprintln!("{message}");
        } else {
            panic!("{message}");
        }
    }
}

impl Drop for DelegatedCgroup {
    fn drop(&mut self) {
        Self::remove(&self.0.join("daemon"), "daemon");
        Self::remove(&self.0, "delegated");
    }
}

/// A busy per-test group fails its teardown naming the process that held it.
///
/// The teardown failure this harness has shown intermittently said only `Device or resource busy`,
/// which cannot tell a process the daemon left behind from a child group it did not remove. Here
/// a process is placed in the per-test daemon group on purpose, and the failure must carry its pid
/// and the group's `cgroup.events`. Delegated lane only: without a delegated root it is absent.
#[test]
fn a_busy_per_test_cgroup_fails_teardown_naming_what_it_held() {
    let Some(delegated) = DelegatedCgroup::acquire() else {
        return;
    };
    let group = delegated.path().to_path_buf();
    let procs = group.join("daemon/cgroup.procs");
    let mut holder = std::process::Command::new(CGROUP_EXEC)
        .arg(&procs)
        .args(["/bin/sleep", "30"])
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn a process into the per-test daemon group");
    let pid = holder.id().to_string();
    // The fixture's own process, not anything a daemon left: it has joined before teardown runs.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !std::fs::read_to_string(&procs)
        .expect("read the daemon group's cgroup.procs")
        .split_whitespace()
        .any(|member| member == pid)
    {
        assert!(
            Instant::now() < deadline,
            "pid {pid} never joined {}",
            procs.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(delegated)))
        .expect_err("removing a group that holds a process must fail");
    let message = failure
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    holder.kill().expect("kill the holding process");
    holder.wait().expect("reap the holding process");
    std::fs::remove_dir(group.join("daemon")).expect("remove the emptied daemon group");
    std::fs::remove_dir(&group).expect("remove the emptied per-test group");
    assert!(
        message.contains("was not empty at teardown"),
        "teardown failure: {message}"
    );
    assert!(
        message.contains(&format!("procs=[{pid}]")) && message.contains(&format!("pid {pid}: ")),
        "teardown failure does not name the holding pid {pid}: {message}"
    );
    assert!(
        message.contains("populated 1"),
        "teardown failure does not carry cgroup.events: {message}"
    );
}

/// The RFC 6455 sample key and the accept value it forces, as in `tests/websocket.rs`.
const HANDSHAKE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const HANDSHAKE_ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------------------------
// Wire client
// ---------------------------------------------------------------------------------------------

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn read_response(stream: &mut UnixStream) -> (u16, BTreeMap<String, String>, Vec<u8>) {
    let mut buffer = Vec::new();
    let boundary = loop {
        if let Some(index) = find(&buffer, b"\r\n\r\n") {
            break index + 4;
        }
        let mut chunk = [0_u8; 8192];
        let read = stream.read(&mut chunk).await.expect("read response head");
        assert!(read > 0, "connection closed before the response head");
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = std::str::from_utf8(&buffer[..boundary]).expect("ASCII response head");
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .expect("HTTP status line");
    let mut headers = BTreeMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        assert_ne!(
            name, "transfer-encoding",
            "the clean-room client reads content-length responses only"
        );
        headers.insert(name, value.trim().to_owned());
    }
    let length = headers
        .get("content-length")
        .expect("content-length response header")
        .parse::<usize>()
        .expect("content-length value");
    while buffer.len() < boundary + length {
        let mut chunk = [0_u8; 8192];
        let read = stream.read(&mut chunk).await.expect("read response body");
        assert!(read > 0, "connection closed mid-body");
        buffer.extend_from_slice(&chunk[..read]);
    }
    (
        status,
        headers,
        buffer[boundary..boundary + length].to_vec(),
    )
}

fn assert_contract_headers(headers: &BTreeMap<String, String>) {
    assert_eq!(
        headers.get("x-b10x-contract").map(String::as_str),
        Some(ADVERTISED_CONTRACT),
        "response did not carry the promoted contract name"
    );
    assert_eq!(
        headers
            .get("x-b10x-contract-bundle-sha256")
            .map(String::as_str),
        Some(ADVERTISED_CONTRACT_SHA256),
        "response did not carry the promoted inner bundle digest"
    );
}

/// One request per connection, exactly as the predecessor's `http.client` harness did.
async fn request(
    socket: &Path,
    method: &str,
    path: &str,
    request_id: &str,
    body: Option<&[u8]>,
) -> (u16, Value) {
    request_headed(socket, method, path, request_id, &[], body).await
}

/// [`request`], with extra `(name, value)` request headers written verbatim into the head.
async fn request_headed(
    socket: &Path,
    method: &str,
    path: &str,
    request_id: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> (u16, Value) {
    let (status, _, payload) =
        request_with_response_headers(socket, method, path, request_id, headers, body).await;
    (status, payload)
}

/// [`request_headed`], also answering the response head's headers, names lower-cased.
async fn request_with_response_headers(
    socket: &Path,
    method: &str,
    path: &str,
    request_id: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> (u16, BTreeMap<String, String>, Value) {
    let mut stream = UnixStream::connect(socket)
        .await
        .unwrap_or_else(|error| panic!("connect {}: {error}", socket.display()));
    let mut head =
        format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nx-request-id: {request_id}\r\n");
    for (name, value) in headers {
        write!(head, "{name}: {value}\r\n").expect("format the request head");
    }
    if let Some(body) = body {
        let length = body.len();
        head.push_str("content-type: application/json\r\n");
        write!(head, "content-length: {length}\r\n").expect("format the request head");
    }
    head.push_str("\r\n");
    let mut wire = head.into_bytes();
    if let Some(body) = body {
        wire.extend_from_slice(body);
    }
    if let Err(error) = stream.write_all(&wire).await {
        // A refused oversized body is answered before the whole body is read; the peer may have
        // closed the write half already. Any other error is a real failure.
        assert!(
            matches!(
                error.kind(),
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
            ),
            "write {method} {path}: {error}"
        );
    }
    let (status, response_headers, body) = read_response(&mut stream).await;
    assert_contract_headers(&response_headers);
    let payload: Value = serde_json::from_slice(&body).expect("JSON response body");
    let api_version = if path.starts_with("/v2/") { "v2" } else { "v1" };
    assert_eq!(payload["api_version"], api_version, "{payload}");
    assert_eq!(payload["request_id"], request_id, "{payload}");
    (status, response_headers, payload)
}

async fn upgrade_refusal(socket: &Path, path: &str, request_id: &str) -> (u16, Value) {
    let mut stream = UnixStream::connect(socket)
        .await
        .unwrap_or_else(|error| panic!("connect {}: {error}", socket.display()));
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nx-request-id: {request_id}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write websocket refusal probe");
    let (status, headers, body) = read_response(&mut stream).await;
    assert_contract_headers(&headers);
    let payload: Value = serde_json::from_slice(&body).expect("JSON websocket refusal body");
    assert_eq!(payload["api_version"], "v1", "{payload}");
    assert_eq!(payload["request_id"], request_id, "{payload}");
    (status, payload)
}

struct EventStream {
    stream: UnixStream,
}

impl EventStream {
    async fn open(socket: &Path, path: &str) -> Self {
        let mut stream = UnixStream::connect(socket)
            .await
            .expect("connect the event stream");
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write the websocket handshake");
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 16 * 1024, "bounded handshake response");
            let mut byte = [0_u8; 1];
            stream
                .read_exact(&mut byte)
                .await
                .expect("read the websocket handshake");
            head.push(byte[0]);
        }
        let head = std::str::from_utf8(&head).expect("ASCII handshake response");
        assert!(head.starts_with("HTTP/1.1 101 "), "{head}");
        let headers: BTreeMap<String, String> = head
            .split("\r\n")
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let accept = headers
            .get("sec-websocket-accept")
            .expect("sec-websocket-accept header");
        assert_eq!(accept, HANDSHAKE_ACCEPT);
        assert_contract_headers(&headers);
        Self { stream }
    }

    async fn frame(&mut self) -> (u8, Vec<u8>) {
        let mut header = [0_u8; 2];
        self.stream
            .read_exact(&mut header)
            .await
            .expect("websocket frame header");
        let opcode = header[0] & 0x0f;
        assert_eq!(header[1] & 0x80, 0, "server frames must not be masked");
        let mut length = u64::from(header[1] & 0x7f);
        if length == 126 {
            let mut bytes = [0_u8; 2];
            self.stream
                .read_exact(&mut bytes)
                .await
                .expect("medium frame length");
            length = u64::from(u16::from_be_bytes(bytes));
        } else if length == 127 {
            let mut bytes = [0_u8; 8];
            self.stream
                .read_exact(&mut bytes)
                .await
                .expect("large frame length");
            length = u64::from_be_bytes(bytes);
        }
        let mut payload = vec![0_u8; usize::try_from(length).expect("frame length")];
        self.stream
            .read_exact(&mut payload)
            .await
            .expect("websocket frame payload");
        (opcode, payload)
    }
}

/// The session attachment, which unlike the event stream also speaks client to server.
struct SessionChannel {
    stream: EventStream,
    sequence: u64,
    transcript: Vec<u8>,
}

impl SessionChannel {
    async fn open(socket: &Path, path: &str) -> Self {
        Self {
            stream: EventStream::open(socket, path).await,
            sequence: 1,
            transcript: Vec::new(),
        }
    }

    /// One masked client text frame, with the contiguous sequence the attachment requires.
    async fn send(&mut self, frame: &mut Value) {
        frame["sequence"] = json!(self.sequence);
        self.sequence = self.sequence.saturating_add(1);
        let payload = serde_json::to_vec(frame).expect("client frame JSON");
        let mut encoded = vec![0x81_u8];
        assert!(payload.len() <= 125, "bounded client frame");
        encoded.push(0x80 | u8::try_from(payload.len()).expect("short frame length"));
        let mask = [0x11_u8, 0x22, 0x33, 0x44];
        encoded.extend_from_slice(&mask);
        encoded.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % mask.len()]),
        );
        self.stream
            .stream
            .write_all(&encoded)
            .await
            .expect("write the client frame");
    }

    async fn input(&mut self, bytes: &str) {
        self.send(&mut json!({
            "kind": "stdin",
            "content": {"encoding": "base64", "data": BASE64.encode(bytes.as_bytes())}
        }))
        .await;
    }

    /// Reads output frames until the transcript carries `needle`, or the deadline passes.
    ///
    /// A terminal transcript is a stream, not a sequence of answers: the line discipline's echo of
    /// what the client typed is interleaved with what the child printed, so a case waits for a
    /// substring rather than for the n-th frame.
    async fn wait_for(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        while find(&self.transcript, needle.as_bytes()).is_none() {
            assert!(
                Instant::now() < deadline,
                "waiting for {needle:?} in {}",
                self.text()
            );
            let frame = tokio::time::timeout(Duration::from_secs(20), self.stream.frame())
                .await
                .expect("a bounded server frame");
            assert_eq!(
                frame.0, 0x1,
                "the session speaks the closed JSON text encoding"
            );
            let value: Value = serde_json::from_slice(&frame.1).expect("server frame JSON");
            assert_eq!(value["kind"], "output", "{value}");
            assert_eq!(
                value["stream"], "stdout",
                "a terminal has one file: {value}"
            );
            self.transcript.extend_from_slice(
                &BASE64
                    .decode(text(&value["content"]["data"]))
                    .expect("base64 output"),
            );
            assert!(self.transcript.len() < 256 * 1024, "bounded transcript");
        }
        self.text()
    }

    /// The next frame that is not output — the terminal `exit`, or a protocol error.
    async fn wait_for_terminal(&mut self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(Instant::now() < deadline, "waiting for the terminal frame");
            let frame = tokio::time::timeout(Duration::from_secs(20), self.stream.frame())
                .await
                .expect("a bounded server frame");
            if frame.0 != 0x1 {
                continue;
            }
            let value: Value = serde_json::from_slice(&frame.1).expect("server frame JSON");
            if value["kind"] != "output" {
                return value;
            }
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.transcript).into_owned()
    }
}

// ---------------------------------------------------------------------------------------------
// The daemon under test
// ---------------------------------------------------------------------------------------------

struct Daemon {
    socket: unix_socket::SocketPath,
    workspaces: PathBuf,
    command: Vec<String>,
    child: Child,
    stderr: JoinHandle<String>,
}

impl Daemon {
    async fn start(
        root: &Path,
        cgroup_root: Option<&Path>,
        apertures: &[String],
        secret_slots: &[String],
        delegated_context_keys: &[String],
        require_delegated_context: bool,
    ) -> Self {
        Self::start_declaring(
            root,
            cgroup_root,
            apertures,
            secret_slots,
            delegated_context_keys,
            require_delegated_context,
            &[],
        )
        .await
    }

    /// [`Self::start`], also declaring each `name=path` in `writable_roots` (ADR 0038 kind 1).
    async fn start_declaring(
        root: &Path,
        cgroup_root: Option<&Path>,
        apertures: &[String],
        secret_slots: &[String],
        delegated_context_keys: &[String],
        require_delegated_context: bool,
        writable_roots: &[String],
    ) -> Self {
        Self::start_with_arguments(
            root,
            cgroup_root,
            apertures,
            secret_slots,
            delegated_context_keys,
            require_delegated_context,
            writable_roots,
            &[],
        )
        .await
    }

    /// [`Self::start_declaring`], with `extra` appended to the daemon's own argv (a Git source
    /// declaration and its trust root, for the writable Git root cases).
    #[allow(clippy::too_many_arguments)] // One argument per declaration family the daemon takes.
    async fn start_with_arguments(
        root: &Path,
        cgroup_root: Option<&Path>,
        apertures: &[String],
        secret_slots: &[String],
        delegated_context_keys: &[String],
        require_delegated_context: bool,
        writable_roots: &[String],
        extra: &[String],
    ) -> Self {
        let socket = unix_socket::socket_path(root, "substrate.sock");
        let workspaces = root.join("workspaces");
        let mut command = vec![
            daemon_binary().display().to_string(),
            "--socket".to_owned(),
            socket.display().to_string(),
            "--state".to_owned(),
            root.join("state.db").display().to_string(),
            "--workspaces".to_owned(),
            workspaces.display().to_string(),
            "--deployment".to_owned(),
            "dep_cleanroom".to_owned(),
            "--allow-uid".to_owned(),
            nix::unistd::getuid().as_raw().to_string(),
            "--event-retention".to_owned(),
            "64".to_owned(),
        ];
        if let Some(cgroup_root) = cgroup_root {
            command.push("--cgroup-root".to_owned());
            command.push(cgroup_root.display().to_string());
        }
        for aperture in apertures {
            command.push("--egress-aperture".to_owned());
            command.push(aperture.clone());
        }
        // `name=path`, never a value: the daemon's own argv is one of the surfaces the secret-slot
        // cases search, so the declaration has to be the kind of thing that may appear in it.
        for slot in secret_slots {
            command.push("--secret-slot".to_owned());
            command.push(slot.clone());
        }
        // `kid=issuer=base64url-public-key`, and there is no shape of this flag that takes anything
        // else: substrate mints no delegated context and holds no signing key (ADR 0011).
        for key in delegated_context_keys {
            command.push("--delegated-context-key".to_owned());
            command.push(key.clone());
        }
        if require_delegated_context {
            command.push("--require-delegated-context".to_owned());
        }
        for root in writable_roots {
            command.push("--writable-root".to_owned());
            command.push(root.clone());
        }
        command.extend(extra.iter().cloned());
        if let Some(cgroup_root) = cgroup_root {
            let mut wrapped = vec![
                CGROUP_EXEC.to_owned(),
                cgroup_root
                    .join("daemon/cgroup.procs")
                    .display()
                    .to_string(),
            ];
            wrapped.append(&mut command);
            command = wrapped;
        }
        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .env(DAEMON_ONLY_VARIABLE, "must-not-reach-child")
            .kill_on_drop(true)
            .spawn()
            .expect("spawn substrate-daemon");
        let mut pipe = child.stderr.take().expect("daemon stderr pipe");
        let mut stderr = tokio::spawn(async move {
            let mut captured = String::new();
            let _ = pipe.read_to_string(&mut captured).await;
            captured
        });
        let deadline = Instant::now() + readiness::DAEMON_READINESS;
        while !socket.exists() {
            if let Some(status) = child.try_wait().expect("daemon status") {
                let error = (&mut stderr).await.unwrap_or_default();
                panic!("substrate-daemon exited before readiness ({status}): {error}");
            }
            if Instant::now() >= deadline {
                let _ = child.start_kill();
                let error = tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut stderr)
                    .await
                    .map(Result::unwrap_or_default)
                    .unwrap_or_default();
                panic!(
                    "substrate-daemon did not create its Unix socket within {:?}: {error}",
                    readiness::DAEMON_READINESS
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Self {
            socket,
            workspaces,
            command,
            child,
            stderr,
        }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        request_id: &str,
        body: Option<&[u8]>,
    ) -> (u16, Value) {
        request(&self.socket, method, path, request_id, body).await
    }

    async fn call_headed(
        &self,
        method: &str,
        path: &str,
        request_id: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> (u16, Value) {
        request_headed(&self.socket, method, path, request_id, headers, body).await
    }

    /// A bodiless `GET` answering the response headers beside the status and payload.
    async fn get_with_response_headers(
        &self,
        path: &str,
        request_id: &str,
    ) -> (u16, BTreeMap<String, String>, Value) {
        request_with_response_headers(&self.socket, "GET", path, request_id, &[], None).await
    }

    async fn stream(&self, path: &str) -> EventStream {
        EventStream::open(&self.socket, path).await
    }

    /// Every `memfd:substrate-slot-*` descriptor the daemon process still holds.
    ///
    /// Read from outside the daemon, out of `/proc/<pid>/fd`, because "the daemon closed its copy"
    /// is a claim about the process and not about the code that says so (ADR 0012).
    fn held_slot_memfds(&self) -> Vec<String> {
        let pid = self.child.id().expect("daemon process id");
        let directory = format!("/proc/{pid}/fd");
        std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("list {directory}: {error}"))
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_link(entry.path()).ok())
            .map(|target| target.display().to_string())
            .filter(|target| target.contains("memfd:substrate-slot-"))
            .collect()
    }

    /// The daemon's own argv, as the kernel holds it.
    ///
    /// A declaration is `name=path`; a value is not in it, and this is where that is checked
    /// rather than assumed.
    fn cmdline(&self) -> String {
        let pid = self.child.id().expect("daemon process id");
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).expect("read the daemon's argv");
        String::from_utf8_lossy(&raw).into_owned()
    }

    /// Stops the daemon and hands back everything it wrote to stderr.
    ///
    /// Returned rather than dropped: the daemon's own diagnostic stream is one of the surfaces a
    /// secret slot must never appear in, and it is only readable once the pipe has closed.
    async fn close(mut self) -> String {
        if self.child.try_wait().expect("daemon status").is_none() {
            let pid = self.child.id().expect("daemon process id");
            let pid = Pid::from_raw(i32::try_from(pid).expect("process id fits in pid_t"));
            kill(pid, Signal::SIGINT).expect("interrupt substrate-daemon");
        }
        let status = tokio::time::timeout(SHUTDOWN_TIMEOUT, self.child.wait())
            .await
            .expect("substrate-daemon shutdown timed out")
            .expect("substrate-daemon exit status");
        let error = self.stderr.await.unwrap_or_default();
        assert!(
            status.success(),
            "substrate-daemon shutdown failed: {error}"
        );
        error
    }
}

// ---------------------------------------------------------------------------------------------
// Shared assertions
// ---------------------------------------------------------------------------------------------

fn mutation(operation: &str, input: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "op": operation, "input": input })).expect("mutation JSON")
}

fn expect_error(response: &(u16, Value), status: u16, code: &str) {
    let (actual, payload) = response;
    assert_eq!(*actual, status, "{payload}");
    assert_eq!(payload["error"]["code"], code, "{payload}");
}

fn text(value: &Value) -> String {
    value.as_str().expect("string field").to_owned()
}

async fn wait_absent(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while path.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !path.exists(),
        "resource was not cleaned up while idle: {}",
        path.display()
    );
}

// ---------------------------------------------------------------------------------------------
// Refusals that need their own process
// ---------------------------------------------------------------------------------------------

async fn check_startup_refusal(root: &Path) {
    let socket = root.join("unmapped.sock");
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(daemon_binary())
            .args([
                "--socket",
                &socket.display().to_string(),
                "--state",
                &root.join("unmapped.db").display().to_string(),
                "--workspaces",
                &root.join("unmapped-workspaces").display().to_string(),
                "--deployment",
                "dep_unmapped",
            ])
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .expect("unmapped daemon timed out")
    .expect("unmapped daemon output");
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("explicit --allow-uid mapping is required"),
        "{error}"
    );
    assert!(!socket.exists());
    check_aperture_term_refusal(root).await;
}

/// An unrecognised term in an aperture declaration is a **startup** error, never an ignored one.
///
/// The `/tcp` term exists so that a declaration written today cannot be silently reinterpreted by
/// a later slice (design 10 § 9 decision 3); a term the daemon reads past would hand that back,
/// and an operator who wrote `max=` where the daemon expects `/max=` would get an unbounded
/// aperture and no word about it (ADR 0014).
async fn check_aperture_term_refusal(root: &Path) {
    let socket = root.join("unparsed.sock");
    for declaration in [
        "model=127.0.0.1:443/tcp/turbo",
        "model=127.0.0.1:443/tcp/max=1MB",
        "model=127.0.0.1:443/tcp/max=0",
    ] {
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            Command::new(daemon_binary())
                .args([
                    "--socket",
                    &socket.display().to_string(),
                    "--state",
                    &root.join("unparsed.db").display().to_string(),
                    "--workspaces",
                    &root.join("unparsed-workspaces").display().to_string(),
                    "--deployment",
                    "dep_unparsed",
                    "--allow-uid",
                    &nix::unistd::getuid().as_raw().to_string(),
                    "--egress-aperture",
                    declaration,
                ])
                .stdin(Stdio::null())
                .output(),
        )
        .await
        .expect("unparsed daemon timed out")
        .expect("unparsed daemon output");
        assert!(
            !output.status.success(),
            "the daemon started with {declaration}"
        );
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("egress aperture"),
            "the refusal did not name the declaration ({declaration}): {error}"
        );
        assert!(!socket.exists(), "{declaration}");
    }
}

async fn check_dual_daemon_refusal(daemon: &Daemon) {
    let second_socket = daemon.socket.with_file_name("substrate-second.sock");
    let mut command = daemon.command.clone();
    let index = command
        .iter()
        .position(|item| item == "--socket")
        .expect("--socket argument")
        + 1;
    command[index] = second_socket.display().to_string();
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .expect("second daemon timed out")
    .expect("second daemon output");
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("another substrate daemon owns this durable state identity"),
        "{error}"
    );
    assert!(!second_socket.exists());
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_dual_daemon_owner_survives", None)
        .await;
    assert_eq!(status, 200, "{machine}");
}

// ---------------------------------------------------------------------------------------------
// Exec input
// ---------------------------------------------------------------------------------------------

/// The exec request body, with the predecessor's defaults: a 5 s timeout, a 64 KiB output
/// window, 16 processes, 64 MiB of memory and one CPU-second.
struct ExecInput<'a> {
    workspace: &'a str,
    snapshot: &'a str,
    argv: &'a [&'a str],
    wait: bool,
    timeout_ms: u64,
    environment: Value,
    aperture: Option<&'a str>,
    secret_slot: Option<(&'a str, u32)>,
    read_only_files: Option<Value>,
    read_only_roots: Option<Value>,
    workdir: Option<&'a str>,
    apertures: Option<&'a [&'a str]>,
    writable_roots: Option<Value>,
}

impl<'a> ExecInput<'a> {
    fn new(workspace: &'a str, snapshot: &'a str, argv: &'a [&'a str], wait: bool) -> Self {
        Self {
            workspace,
            snapshot,
            argv,
            wait,
            timeout_ms: 5000,
            environment: json!({}),
            aperture: None,
            secret_slot: None,
            read_only_files: None,
            read_only_roots: None,
            workdir: None,
            apertures: None,
            writable_roots: None,
        }
    }

    /// Declares writable roots (ADR 0038): `[{"host", "mount"}]` or `[{"workspace", "mount"}]`.
    fn writable_roots(mut self, roots: Value) -> Self {
        self.writable_roots = Some(roots);
        self
    }

    /// Names the directory the program starts in (design 26 § 1).
    fn workdir(mut self, workdir: &'a str) -> Self {
        self.workdir = Some(workdir);
        self
    }

    /// Selects several declared egress apertures by name (design 26 § 2). Beside [`Self::aperture`]
    /// the request carries both, which is the mode mismatch a list refuses.
    fn apertures(mut self, names: &'a [&'a str]) -> Self {
        self.apertures = Some(names);
        self
    }

    /// Declares single host files read-only (ADR 0033): `[{"host_path", "mount"}]`.
    fn read_only_files(mut self, files: Value) -> Self {
        self.read_only_files = Some(files);
        self
    }

    /// Declares host directories read-only (ADR 0010): `[{"host_path", "mount"}]`.
    fn read_only_roots(mut self, roots: Value) -> Self {
        self.read_only_roots = Some(roots);
        self
    }

    /// Names one operator-declared secret slot and the descriptor it must arrive at (ADR 0012).
    ///
    /// A name and a number, never a value, a path or a length — which is why the request this
    /// builds can be printed in a failure message without printing a credential.
    fn secret_slot(mut self, slot: &'a str, fd: u32) -> Self {
        self.secret_slot = Some((slot, fd));
        self
    }

    /// Selects a declared egress aperture by name — never a destination (ADR 0013).
    fn aperture(mut self, name: &'a str) -> Self {
        self.aperture = Some(name);
        self
    }

    fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    fn environment(mut self, environment: Value) -> Self {
        self.environment = environment;
        self
    }

    fn build(&self) -> Value {
        let mut sandbox = match self.aperture {
            Some(name) => json!({
                "capability_snapshot": self.snapshot,
                "network": "aperture",
                "aperture": name,
                "profile": "workspace",
                "require": true,
            }),
            None => json!({
                "capability_snapshot": self.snapshot,
                "network": "none",
                "profile": "workspace",
                "require": true,
            }),
        };
        if let Some(names) = self.apertures {
            sandbox["network"] = json!("aperture");
            sandbox["apertures"] = json!(names);
        }
        let mut input = json!({
            "workspace": self.workspace,
            "argv": self.argv,
            "env": { "allow": [], "set": self.environment },
            "sandbox": sandbox,
            "limits": {
                "timeout_ms": self.timeout_ms,
                "output_bytes": 65536,
                "processes": 16,
                "memory_bytes": 67_108_864,
                "cpu_millis": 1000,
            },
            "wait": self.wait,
        });
        // Absent when no slot is named, so every request this file already sent stays byte-identical
        // and keeps hashing to what the ledger recorded for it.
        if let Some((slot, fd)) = self.secret_slot {
            input["secret_slots"] = json!([{ "slot": slot, "fd": fd }]);
        }
        if let Some(files) = &self.read_only_files {
            input["read_only_files"] = files.clone();
        }
        if let Some(roots) = &self.read_only_roots {
            input["read_only_roots"] = roots.clone();
        }
        if let Some(workdir) = self.workdir {
            input["workdir"] = json!(workdir);
        }
        if let Some(roots) = &self.writable_roots {
            input["writable_roots"] = roots.clone();
        }
        input
    }
}

const NO_EGRESS_PROGRAM: &str = "import socket;socket.create_connection(('1.1.1.1',53),1)";

/// Exits [`NESTED_USERNS_REFUSED`] only when the kernel refuses a nested user namespace with
/// `ENOSPC`, the answer `--disable-userns` gives (bubblewrap sets the child namespace's
/// `user.max_user_namespaces` to zero), and 0 if one was created. Any other refusal, such as an
/// `EPERM` from a host-level user-namespace restriction, exits 1, so the case cannot pass on a
/// host policy that would hold without `--disable-userns`; any other failure (no `os.unshare`,
/// say) exits 1 with a traceback, so it cannot pass on a program that never asked.
const NESTED_USERNS_PROGRAM: &str = concat!(
    "import errno,os,sys\n",
    "try:\n",
    "    os.unshare(os.CLONE_NEWUSER)\n",
    "except OSError as error:\n",
    "    print(error.errno, flush=True)\n",
    "    sys.exit(7 if error.errno == errno.ENOSPC else 1)\n",
    "sys.exit(0)\n",
);
const NESTED_USERNS_REFUSED: i64 = 7;

const PIDS_PROGRAM: &str = concat!(
    "\n",
    "import os,time\n",
    "children=[]\n",
    "for _ in range(64):\n",
    "    try:\n",
    "        pid=os.fork()\n",
    "    except OSError:\n",
    "        break\n",
    "    if pid == 0:\n",
    "        time.sleep(.2)\n",
    "        os._exit(0)\n",
    "    children.append(pid)\n",
    "print(len(children), flush=True)\n",
    "for pid in children:\n",
    "    os.waitpid(pid,0)\n",
);

const MEMORY_PROGRAM: &str = concat!(
    "x=bytearray(128*1024*1024);",
    "[(x.__setitem__(i,1)) for i in range(0,len(x),4096)];",
    "print(len(x))",
);

const FILL_PROGRAM: &str = concat!(
    "import os;",
    "os.write(1,b'x'*131072);",
    "os.write(2,b'y'*131072)"
);

const TREE_PROGRAM: &str = concat!(
    "import os,signal,time\n",
    "signal.signal(signal.SIGTERM,signal.SIG_IGN)\n",
    "os.fork()\n",
    "with open('/workspace/.tree-ready','w') as ready:\n",
    " ready.write('ready')\n",
    " ready.flush()\n",
    " os.fsync(ready.fileno())\n",
    "time.sleep(60)\n",
);

const TRAP_PROGRAM: &str = concat!(
    "import os,signal,time\n",
    "signal.signal(signal.SIGTERM,lambda _signal,_frame: os._exit(0))\n",
    "with open('/workspace/.trap-ready','w') as ready:\n",
    " ready.write('ready')\n",
    " ready.flush()\n",
    " os.fsync(ready.fileno())\n",
    "while True: time.sleep(1)\n",
);

const EXITED_CLEANLY: fn() -> Value = || json!({ "code": 0, "signal": null });

async fn read_output(daemon: &Daemon, exec_id: &str, stream: &str) -> Value {
    let (status, payload) = daemon
        .call(
            "GET",
            &format!("/v1/execs/{exec_id}/output?stream={stream}&offset=0&limit_bytes=65536"),
            &format!("req_clean_output_{stream}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{payload}");
    payload["result"].clone()
}

fn decoded(output: &Value) -> Vec<u8> {
    BASE64
        .decode(output["content"]["data"].as_str().expect("output data"))
        .expect("base64 output")
}

async fn wait_for_workspace_file(daemon: &Daemon, workspace: &str, path: &str, request: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (status, payload) = daemon
            .call(
                "GET",
                &format!(
                    "/v1/workspaces/{workspace}/files/{path}?mode=file&offset=0&limit_bytes=16"
                ),
                request,
                None,
            )
            .await;
        match status {
            200 => return,
            404 if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            _ => panic!("workspace readiness marker {path}: {status} {payload}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The delegated lane: confined execs
// ---------------------------------------------------------------------------------------------

/// Guarded I/O: a file path that climbs out of the workspace is refused by name, never resolved
/// (`openat2` beneath the workspace root). Runs for each [`WorkspaceOrigin`].
async fn check_path_escape(daemon: &Daemon, workspace: &str, origin: WorkspaceOrigin) -> usize {
    expect_error(
        &daemon
            .call(
                "GET",
                &format!(
                    "/v1/workspaces/{workspace}/files/%2e%2e%2fetc%2fpasswd\
                     ?mode=file&offset=0&limit_bytes=16"
                ),
                &origin.id("req_clean_escape"),
                None,
            )
            .await,
        422,
        "workspace.path-escape",
    );
    1
}

/// How the workspace an isolation case runs in came to be served (ADR 0035).
///
/// The confinement floor is claimed unchanged for an adopted directory, so the isolation cases
/// are one body run once per origin rather than a copy per origin: a case added to the floor is
/// added for both.
#[derive(Clone, Copy)]
enum WorkspaceOrigin {
    /// Minted and made by `workspace.create`.
    Created,
    /// An operator's directory, served after `workspace.adopt`.
    Adopted,
}

impl WorkspaceOrigin {
    /// The operation or request id a case sends for this origin.
    ///
    /// The created origin keeps the exact ids this file always sent, so its ledger rows are
    /// unchanged; the adopted origin suffixes them, so each run is its own ledger row and never a
    /// replay or a conflict of the other's.
    fn id(self, base: &str) -> String {
        match self {
            Self::Created => base.to_owned(),
            Self::Adopted => format!("{base}_ADOPTED"),
        }
    }
}

async fn check_confined_execs(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    cgroup_root: &Path,
) -> usize {
    let mut passed = check_confined_isolation(
        daemon,
        workspace,
        snapshot,
        cgroup_root,
        WorkspaceOrigin::Created,
    )
    .await;
    passed += check_exec_retention(daemon, workspace, snapshot).await;
    passed
}

/// The confinement floor of one confined exec: output, environment shaping, working directory,
/// no egress, no nested user namespace, pids and memory bounds, output truncation, timeout,
/// whole-tree kill and graceful termination.
///
/// Run against a created workspace and again against an adopted one (ADR 0035, consequences: "the
/// delegated lane runs its isolation cases against an adopted workspace").
#[allow(clippy::too_many_lines)] // One case per floor guarantee, in the order the lane has run them.
async fn check_confined_isolation(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    cgroup_root: &Path,
    origin: WorkspaceOrigin,
) -> usize {
    let mut passed = 0;

    let (status, executed) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_exec"),
            Some(&mutation(
                &origin.id("01JPHASE2CLEANEXEC0001"),
                &ExecInput::new(workspace, snapshot, &["/usr/bin/printf", "hello"], true).build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{executed}");
    assert_eq!(executed["result"]["state"], "exited");
    assert_eq!(executed["result"]["exit"], EXITED_CLEANLY());
    assert_eq!(executed["result"]["applied"]["network"], "none");
    assert!(
        !cgroup_root
            .join(text(&executed["result"]["applied"]["cgroup"]))
            .exists()
    );
    let output = read_output(daemon, &text(&executed["result"]["id"]), "stdout").await;
    assert_eq!(decoded(&output), b"hello");
    assert_eq!(output["eof"], true);
    passed += 1;

    let (status, environment_exec) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_environment"),
            Some(&mutation(
                &origin.id("01JPHASE2CLEANENV000001"),
                &ExecInput::new(workspace, snapshot, &["/usr/bin/env"], true)
                    .environment(json!({ "VECTOR_VISIBLE": "yes" }))
                    .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{environment_exec}");
    let environment_output =
        read_output(daemon, &text(&environment_exec["result"]["id"]), "stdout").await;
    let visible = decoded(&environment_output);
    assert_eq!(
        visible,
        b"VECTOR_VISIBLE=yes\n",
        "{}",
        String::from_utf8_lossy(&visible)
    );
    passed += 1;

    // The working directory is observed, not stated: the program prints its physical cwd
    // (`getcwd`, so no `PWD` in the environment can answer for it) and writes a relative file,
    // which must then be served by this workspace's own file route, so the cwd is the mount of
    // the workspace this exec named, not merely a directory called `/workspace`.
    let (status, pwd_exec) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_pwd"),
            Some(&mutation(
                &origin.id("01JPHASE3CLEANPWD000001"),
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/sh", "-c", "pwd -P && printf cwd > cwd-probe.txt"],
                    true,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{pwd_exec}");
    assert_eq!(pwd_exec["result"]["exit"], EXITED_CLEANLY(), "{pwd_exec}");
    let cwd = read_output(daemon, &text(&pwd_exec["result"]["id"]), "stdout").await;
    assert_eq!(
        decoded(&cwd),
        b"/workspace\n",
        "{}",
        String::from_utf8_lossy(&decoded(&cwd))
    );
    let (status, probe) = daemon
        .call(
            "GET",
            &format!(
                "/v1/workspaces/{workspace}/files/cwd-probe.txt?mode=file&offset=0&limit_bytes=16"
            ),
            &origin.id("req_clean_pwd_probe"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{probe}");
    assert_eq!(
        BASE64
            .decode(text(&probe["result"]["content"]["data"]))
            .expect("base64 file content"),
        b"cwd"
    );
    passed += 1;

    let (status, no_egress) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_no_egress"),
            Some(&mutation(
                &origin.id("01JPHASE2CLEANNOEGRESS1"),
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", NO_EGRESS_PROGRAM],
                    true,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{no_egress}");
    assert_eq!(no_egress["result"]["state"], "exited");
    assert_ne!(no_egress["result"]["exit"]["code"], 0, "{no_egress}");
    passed += 1;

    // No nested user namespace: the child's own namespace is made with `--disable-userns`, so it
    // cannot create another one and hold `CAP_SYS_ADMIN` in it (AGENTS.md, safety envelope).
    let (status, nested_userns) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_nested_userns"),
            Some(&mutation(
                &origin.id("01JPHASE4CLEANNESTUSERNS"),
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", NESTED_USERNS_PROGRAM],
                    true,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{nested_userns}");
    assert_eq!(
        nested_userns["result"]["state"], "exited",
        "{nested_userns}"
    );
    assert_eq!(
        nested_userns["result"]["exit"],
        json!({ "code": NESTED_USERNS_REFUSED, "signal": null }),
        "{nested_userns}"
    );
    passed += 1;

    let (status, pids_exec) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_pids"),
            Some(&mutation(
                &origin.id("01JPHASE2CLEANPIDS00001"),
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", PIDS_PROGRAM],
                    true,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{pids_exec}");
    let pids_output = read_output(daemon, &text(&pids_exec["result"]["id"]), "stdout").await;
    let children: u32 = String::from_utf8(decoded(&pids_output))
        .expect("UTF-8 child count")
        .trim()
        .parse()
        .expect("child count");
    assert!(0 < children && children < 16, "{children}");
    passed += 1;

    let (status, memory_exec) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_memory"),
            Some(&mutation(
                &origin.id("01JPHASE2CLEANMEMORY0001"),
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", MEMORY_PROGRAM],
                    true,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{memory_exec}");
    assert_ne!(
        memory_exec["result"]["exit"],
        EXITED_CLEANLY(),
        "{memory_exec}"
    );
    passed += 1;

    let (status, filled) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_truncation"),
            Some(&mutation(
                &origin.id("01JPHASE2CLEANFILL00001"),
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", FILL_PROGRAM],
                    true,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{filled}");
    for stream in ["stdout", "stderr"] {
        let output = read_output(daemon, &text(&filled["result"]["id"]), stream).await;
        assert_eq!(output["returned_bytes"], 65536);
        assert_eq!(output["truncated"], true);
        assert!(decoded(&output).ends_with(b"[substrate: output truncated]\n"));
    }
    passed += 1;

    let (status, timed_out) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_timeout"),
            Some(&mutation(
                &origin.id("01JPHASE2CLEANTIMEOUT001"),
                &ExecInput::new(workspace, snapshot, &["/usr/bin/sleep", "60"], true)
                    .timeout_ms(100)
                    .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{timed_out}");
    assert_eq!(timed_out["result"]["state"], "cancelled");
    assert_eq!(
        timed_out["result"]["exit"],
        json!({ "code": null, "signal": "KILL" })
    );
    assert!(
        !cgroup_root
            .join(text(&timed_out["result"]["applied"]["cgroup"]))
            .exists()
    );
    passed += 1;

    let (status, running) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_tree_start"),
            Some(&mutation(
                &origin.id("01JPHASE2CLEANTREE00001"),
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", TREE_PROGRAM],
                    false,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 202, "{running}");
    let exec_id = text(&running["result"]["id"]);
    let cgroup_name = text(&running["result"]["applied"]["cgroup"]);
    wait_for_workspace_file(
        daemon,
        workspace,
        ".tree-ready",
        &origin.id("req_clean_tree_ready"),
    )
    .await;
    let (status, cancelled) = daemon
        .call(
            "POST",
            &format!("/v1/execs/{exec_id}/signal"),
            &origin.id("req_clean_tree_signal"),
            Some(&mutation(
                &origin.id("01JPHASE2CLEANTREESIGNAL"),
                &json!({ "signal": "TERM", "grace_ms": 100 }),
            )),
        )
        .await;
    assert_eq!(status, 200, "{cancelled}");
    assert_eq!(cancelled["result"]["state"], "cancelled");
    assert_eq!(
        cancelled["result"]["exit"],
        json!({ "code": null, "signal": "KILL" }),
        "{cancelled}"
    );
    assert!(!cgroup_root.join(cgroup_name).exists());
    passed += 1;

    let (status, trapping) = daemon
        .call(
            "POST",
            "/v1/execs",
            &origin.id("req_clean_trap_start"),
            Some(&mutation(
                &origin.id("01JPHASE3CLEANTRAPSTART1"),
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", TRAP_PROGRAM],
                    false,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 202, "{trapping}");
    wait_for_workspace_file(
        daemon,
        workspace,
        ".trap-ready",
        &origin.id("req_clean_trap_ready"),
    )
    .await;
    let (status, trapped) = daemon
        .call(
            "POST",
            &format!("/v1/execs/{}/signal", text(&trapping["result"]["id"])),
            &origin.id("req_clean_trap_signal"),
            Some(&mutation(
                &origin.id("01JPHASE3CLEANTRAPSIGNAL"),
                &json!({ "signal": "TERM", "grace_ms": 5000 }),
            )),
        )
        .await;
    assert_eq!(status, 200, "{trapped}");
    assert_eq!(trapped["result"]["state"], "exited", "{trapped}");
    assert_eq!(trapped["result"]["exit"], EXITED_CLEANLY(), "{trapped}");
    passed += 1;
    passed
}

/// Exec-record retention: waited and abandoned execs past the retained bound. A property of the
/// daemon's exec table, not of the workspace an exec runs in, so it runs once.
async fn check_exec_retention(daemon: &Daemon, workspace: &str, snapshot: &str) -> usize {
    let mut passed = 0;
    for index in 0..129 {
        let (status, completed) = daemon
            .call(
                "POST",
                "/v1/execs",
                &format!("req_clean_waited_{index:03}"),
                Some(&mutation(
                    &format!("01JPHASE3WAITED{index:09}"),
                    &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true).build(),
                )),
            )
            .await;
        assert_eq!(status, 200, "{index} {completed}");
    }
    passed += 1;

    for index in 0..129 {
        let (status, abandoned) = daemon
            .call(
                "POST",
                "/v1/execs",
                &format!("req_clean_abandoned_{index:03}"),
                Some(&mutation(
                    &format!("01JPHASE3ABANDON{index:09}"),
                    &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], false).build(),
                )),
            )
            .await;
        assert_eq!(status, 202, "{index} {abandoned}");
        if index % 16 == 15 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (status, after_abandon) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_after_abandon",
            Some(&mutation(
                "01JPHASE3AFTERABANDON01",
                &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true).build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{after_abandon}");
    passed += 1;
    passed
}

// ---------------------------------------------------------------------------------------------
// Events, reconciliation snapshots and leases
// ---------------------------------------------------------------------------------------------

#[allow(clippy::too_many_lines)] // One sequential journey; splitting it would lose the ordering.
async fn check_phase3_journey(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    cgroup_root: Option<&Path>,
) -> usize {
    let mut passed = 0;

    let (status, before) = daemon
        .call(
            "GET",
            "/v1/events?limit=100",
            "req_phase3_events_before",
            None,
        )
        .await;
    assert_eq!(status, 200, "{before}");
    let source_generation = before["result"]["generation"].as_u64().expect("generation");
    let source_scope = text(&before["result"]["source_scope"]);
    let start_cursor = text(&before["result"]["next_cursor"]);
    let items = before["result"]["items"]
        .as_array()
        .expect("event items")
        .len();
    assert!(
        before["result"]["through_seq"]
            .as_u64()
            .expect("through_seq")
            >= u64::try_from(items).expect("item count")
    );
    passed += 1;

    let mut noise = Vec::new();
    for index in 0..20 {
        let (status, created) = daemon
            .call(
                "POST",
                "/v1/workspaces",
                &format!("req_phase3_noise_{index:02}"),
                Some(&mutation(
                    &format!("01JPHASE3NOISECREATE{index:02}"),
                    &json!({ "source": "empty", "labels": { "noise": format!("{index:02}") } }),
                )),
            )
            .await;
        assert_eq!(status, 201, "{created}");
        noise.push(text(&created["result"]["id"]));
    }

    let mut stream = daemon
        .stream(&format!("/v1/events/stream?cursor={start_cursor}&limit=1"))
        .await;
    let mut last_cursor = start_cursor.clone();
    let mut boundary = None;
    let mut event_frames = 0;
    while boundary.is_none() {
        let (opcode, payload) = stream.frame().await;
        assert_eq!(opcode, 1, "{opcode} {payload:?}");
        let frame: Value = serde_json::from_slice(&payload).expect("event frame JSON");
        if frame["kind"] == "events" {
            event_frames += 1;
            last_cursor = text(&frame["page"]["next_cursor"]);
            assert_eq!(frame["page"]["source_scope"], source_scope);
            for event in frame["page"]["items"].as_array().expect("frame items") {
                assert_eq!(event["generation"], source_generation);
                let cause = text(&event["cause"]["kind"]);
                assert!(cause == "operation" || cause == "control", "{event}");
                assert!(event.get("op").is_none(), "{event}");
            }
        } else {
            boundary = Some(frame);
        }
    }
    assert_eq!(event_frames, 16);
    let boundary = boundary.expect("backpressure boundary frame");
    assert_eq!(
        boundary,
        json!({
            "kind": "backpressure",
            "code": "event.catch-up-limit",
            "last_cursor": last_cursor,
            "recovery": "pull",
        }),
        "{boundary}"
    );
    let (opcode, close_payload) = stream.frame().await;
    assert_eq!(opcode, 8);
    assert_eq!(
        u16::from_be_bytes([close_payload[0], close_payload[1]]),
        1013
    );
    assert_eq!(&close_payload[2..], b"resume with pull from last_cursor");
    drop(stream);
    let (status, recovered) = daemon
        .call(
            "GET",
            &format!("/v1/events?cursor={last_cursor}&limit=100"),
            "req_phase3_pull_recover",
            None,
        )
        .await;
    assert_eq!(status, 200, "{recovered}");
    assert_eq!(recovered["result"]["generation"], source_generation);
    assert_eq!(recovered["result"]["source_scope"], source_scope);
    assert!(
        !recovered["result"]["items"]
            .as_array()
            .expect("recovered items")
            .is_empty()
    );
    passed += 1;

    let (status, created_snapshot) = daemon
        .call(
            "POST",
            "/v1/reconciliation-snapshots",
            "req_phase3_snapshot_create",
            Some(b"{}"),
        )
        .await;
    assert_eq!(status, 201, "{created_snapshot}");
    assert!(created_snapshot.get("operation").is_none());
    let snapshot_id = text(&created_snapshot["result"]["id"]);
    let through_seq = created_snapshot["result"]["through_seq"]
        .as_u64()
        .expect("snapshot through_seq");
    assert_eq!(created_snapshot["result"]["source_scope"], source_scope);
    assert_eq!(
        created_snapshot["result"]["resume_cursor"],
        format!("ev2.{source_scope}.{source_generation}.{through_seq}")
    );
    let partitions = created_snapshot["result"]["partitions"]
        .as_object()
        .expect("snapshot partitions");
    let item_count = created_snapshot["result"]["item_count"]
        .as_u64()
        .expect("snapshot item count");
    assert_eq!(
        partitions.keys().cloned().collect::<BTreeSet<_>>(),
        ["execs", "provenance_events", "workspaces"]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        partitions
            .values()
            .map(|value| value.as_u64().expect("partition count"))
            .sum::<u64>(),
        item_count
    );
    let history = &created_snapshot["result"]["history"];
    let history_items = history["item_count"].as_u64().expect("history item count");
    assert_eq!(history_items, partitions["provenance_events"]);
    if history_items == 0 {
        assert_eq!(history["first_seq"], Value::Null);
        assert_eq!(history["through_seq"], 0);
    } else {
        let first_seq = history["first_seq"].as_u64().expect("history first_seq");
        let history_through = history["through_seq"]
            .as_u64()
            .expect("history through_seq");
        assert!(first_seq <= history_through && history_through < through_seq);
    }
    passed += 1;

    let (status, first_page) = daemon
        .call(
            "GET",
            &format!("/v1/reconciliation-snapshots/{snapshot_id}?limit=2"),
            "req_phase3_snapshot_page_1",
            None,
        )
        .await;
    assert_eq!(status, 200, "{first_page}");
    assert_eq!(first_page["result"]["through_seq"], through_seq);
    let mut cursor = first_page["result"]["next_cursor"].clone();

    let (status, late) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_phase3_late_create",
            Some(&mutation(
                "01JPHASE3LATECREATE0001",
                &json!({ "source": "empty", "labels": { "after": "snapshot" } }),
            )),
        )
        .await;
    assert_eq!(status, 201, "{late}");
    let late_workspace = text(&late["result"]["id"]);
    let mut snapshot_ids = BTreeSet::new();
    for item in first_page["result"]["items"]
        .as_array()
        .expect("page items")
    {
        snapshot_ids.insert(text(&item["id"]));
        let kind = text(&item["kind"]);
        assert!(
            ["workspace", "exec", "provenance-event"].contains(&kind.as_str()),
            "{item}"
        );
        if kind == "provenance-event" {
            let generation = item["value"]["generation"]
                .as_u64()
                .expect("event generation");
            let seq = item["value"]["seq"].as_u64().expect("event seq");
            assert_eq!(item["id"], format!("event:{generation}:{seq}"));
        }
    }
    while !cursor.is_null() {
        let page_cursor = text(&cursor);
        let page_index = snapshot_ids.len();
        let (status, page) = daemon
            .call(
                "GET",
                &format!("/v1/reconciliation-snapshots/{snapshot_id}?cursor={page_cursor}&limit=2"),
                &format!("req_phase3_snapshot_page_{page_index:03}"),
                None,
            )
            .await;
        assert_eq!(status, 200, "{page}");
        assert_eq!(page["result"]["through_seq"], through_seq);
        for item in page["result"]["items"].as_array().expect("page items") {
            snapshot_ids.insert(text(&item["id"]));
        }
        cursor = page["result"]["next_cursor"].clone();
    }
    assert!(!snapshot_ids.contains(&format!("workspace:{late_workspace}")));
    assert_eq!(
        u64::try_from(snapshot_ids.len()).expect("snapshot id count"),
        item_count
    );
    passed += 1;

    let lease_operation = "01JPHASE3LEASECREATE0001";
    let (status, leased) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_phase3_lease_create",
            Some(&mutation(
                lease_operation,
                &json!({
                    "source": "empty",
                    "labels": { "lease": "cleanroom" },
                    "lease_ttl_ms": 1000,
                }),
            )),
        )
        .await;
    assert_eq!(status, 201, "{leased}");
    let leased_workspace = text(&leased["result"]["id"]);
    assert_eq!(leased["result"]["lease"]["state"], "active");
    assert_eq!(
        leased["result"]["lease"]["authorizing_operation"],
        lease_operation
    );
    let (status, renewed) = daemon
        .call(
            "POST",
            &format!("/v1/workspaces/{leased_workspace}/lease/renew"),
            "req_phase3_lease_renew",
            Some(&mutation(
                "01JPHASE3LEASERENEW0001",
                &json!({ "ttl_ms": 1000 }),
            )),
        )
        .await;
    assert_eq!(status, 200, "{renewed}");
    assert_eq!(renewed["result"]["lease"]["ttl_ms"], 1000);
    assert_eq!(
        renewed["result"]["lease"]["authorizing_operation"],
        "01JPHASE3LEASERENEW0001"
    );
    passed += 1;

    wait_absent(&daemon.workspaces.join(&leased_workspace)).await;
    let (status, replayed_renewal) = daemon
        .call(
            "POST",
            &format!("/v1/workspaces/{leased_workspace}/lease/renew"),
            "req_phase3_lease_renew_replay",
            Some(&mutation(
                "01JPHASE3LEASERENEW0001",
                &json!({ "ttl_ms": 1000 }),
            )),
        )
        .await;
    assert_eq!(status, 200, "{replayed_renewal}");
    assert_eq!(replayed_renewal["result"], renewed["result"]);
    expect_error(
        &daemon
            .call(
                "POST",
                &format!("/v1/workspaces/{leased_workspace}/lease/renew"),
                "req_phase3_lease_renew_conflict",
                Some(&mutation(
                    "01JPHASE3LEASERENEW0001",
                    &json!({ "ttl_ms": 2000 }),
                )),
            )
            .await,
        409,
        "operation.request-conflict",
    );
    expect_error(
        &daemon
            .call(
                "GET",
                &format!("/v1/workspaces/{leased_workspace}"),
                "req_phase3_lease_expired",
                None,
            )
            .await,
        404,
        "resource.not-found",
    );
    passed += 1;

    if let Some(cgroup_root) = cgroup_root {
        check_exec_lease(daemon, workspace, snapshot, cgroup_root).await;
    } else {
        expect_error(
            &daemon
                .call(
                    "POST",
                    "/v1/execs/ex_missing/lease/renew",
                    "req_phase3_exec_lease_absent",
                    Some(&mutation(
                        "01JPHASE3EXECLEASEROUTE1",
                        &json!({ "ttl_ms": 1000 }),
                    )),
                )
                .await,
            404,
            "resource.not-found",
        );
    }
    passed += 1;

    for (index, item) in noise
        .iter()
        .chain(std::iter::once(&late_workspace))
        .enumerate()
    {
        let (status, removed) = daemon
            .call(
                "DELETE",
                &format!("/v1/workspaces/{item}"),
                &format!("req_phase3_noise_destroy_{index:02}"),
                Some(&mutation(
                    &format!("01JPHASE3NOISEDESTROY{index:02}"),
                    &json!({}),
                )),
            )
            .await;
        assert_eq!(status, 200, "{removed}");
    }
    expect_error(
        &daemon
            .call(
                "GET",
                &format!("/v1/events?cursor={start_cursor}&limit=10"),
                "req_phase3_retention_gap",
                None,
            )
            .await,
        409,
        "event.retention-gap",
    );
    passed += 1;
    passed
}

/// A `pty` session start body, with the window the caller wants to try.
fn pty_session_input(workspace: &str, snapshot: &str, window: Option<Value>) -> Value {
    let mut exec = ExecInput::new(workspace, snapshot, &["/bin/sh"], false)
        .timeout_ms(30_000)
        .build();
    exec["lease_ttl_ms"] = json!(60_000);
    let mut input = json!({
        "exec": exec,
        "input_limit_bytes": 65_536,
        "frame_limit_bytes": 16_384,
        "queued_frames": 8,
        "mode": "pty"
    });
    if let Some(window) = window {
        input["window"] = window;
    }
    input
}

/// The acceptance of `story:pty-sessions`, over the wire, against the shipped binary.
///
/// An interactive shell on a real terminal inside the confinement floor: the line discipline echoes
/// what the client typed, the child reads the declared window back with `TIOCGWINSZ`, a resize
/// applied on the master is observed by the same call, and the session ends with a terminal `exit`
/// frame. `stty size` *is* `TIOCGWINSZ`; nothing here reads `COLUMNS`, which the sandbox does not
/// have and which would go stale at the first resize anyway (design 13).
#[allow(clippy::too_many_lines)] // One session lifecycle; splitting it would lose the ordering.
async fn check_confined_pty(daemon: &Daemon, workspace: &str, snapshot: &str) -> usize {
    let mut passed = 0;
    let (status, capabilities) = daemon
        .call("GET", "/v1/sessions", "req_clean_pty_modes", None)
        .await;
    assert_eq!(status, 200, "{capabilities}");
    assert_eq!(
        capabilities["result"]["modes"],
        json!(["pipes", "pty"]),
        "a probed terminal is advertised as a served mode"
    );
    assert_eq!(capabilities["result"]["max_window_columns"], json!(1000));
    assert_eq!(capabilities["result"]["max_window_rows"], json!(1000));
    passed += 1;

    // A window is required, never defaulted to 80x24: substrate has nothing to observe here.
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/sessions",
                "req_clean_pty_nowindow",
                Some(&mutation(
                    "01JPHASE2CLEANPTY000003",
                    &pty_session_input(workspace, snapshot, None),
                )),
            )
            .await,
        422,
        "session.window-invalid",
    );
    passed += 1;

    let (status, session) = daemon
        .call(
            "POST",
            "/v1/sessions",
            "req_clean_pty_start",
            Some(&mutation(
                "01JPHASE2CLEANPTY000001",
                &pty_session_input(
                    workspace,
                    snapshot,
                    Some(json!({"columns": 80, "rows": 24})),
                ),
            )),
        )
        .await;
    assert_eq!(status, 202, "{session}");
    assert_eq!(session["result"]["mode"], "pty", "{session}");
    assert_eq!(session["result"]["kind"], "session", "{session}");
    let session_id = text(&session["result"]["id"]);
    let exec_id = text(&session["result"]["exec"]);
    passed += 1;

    let mut channel =
        SessionChannel::open(&daemon.socket, &format!("/v1/sessions/{session_id}/attach")).await;
    // Echo: the line discipline sends the typed bytes back before the child has done anything with
    // them, and the child's own answer follows.
    channel.input("stty size\n").await;
    let transcript = channel.wait_for("24 80").await;
    assert!(
        transcript.contains("stty size"),
        "the terminal echoed what the client typed: {transcript}"
    );
    passed += 1;

    // A resize the child observes, through the ioctl the acceptance names.
    channel
        .send(&mut json!({"kind": "resize", "window": {"columns": 132, "rows": 43}}))
        .await;
    channel.input("stty size\n").await;
    channel.wait_for("43 132").await;
    passed += 1;

    // Out of bounds is a protocol error and the attachment ends; the session is still alive.
    channel
        .send(&mut json!({"kind": "resize", "window": {"columns": 0, "rows": 43}}))
        .await;
    let refusal = channel.wait_for_terminal().await;
    assert_eq!(refusal["kind"], "protocol-error", "{refusal}");
    assert_eq!(refusal["code"], "session.resize-invalid", "{refusal}");
    drop(channel);
    passed += 1;

    // Losing the attachment ends the session and the whole tree with it.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (status, observed) = daemon
            .call(
                "GET",
                &format!("/v1/execs/{exec_id}"),
                "req_clean_pty_observed",
                None,
            )
            .await;
        assert_eq!(status, 200, "{observed}");
        if observed["result"]["state"] == "cancelled" {
            assert_eq!(observed["result"]["exit"]["signal"], "KILL", "{observed}");
            break;
        }
        assert!(Instant::now() < deadline, "{observed}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    passed += 1;

    // One terminal, one file: the durable stderr of a pty session is genuinely empty, because
    // stderr *was* the same descriptor (design 13).
    let (status, slice) = daemon
        .call(
            "GET",
            &format!("/v1/execs/{exec_id}/output?stream=stderr&offset=0&limit_bytes=4096"),
            "req_clean_pty_stderr",
            None,
        )
        .await;
    assert_eq!(status, 200, "{slice}");
    assert_eq!(slice["result"]["returned_bytes"], 0, "{slice}");
    passed += 1;

    passed
}

/// A raw-pipe session around `exec`, with one exec-start field set to `value` (design 27).
fn session_with_exec_field(exec: Value, field: &str, value: Value) -> Value {
    let mut exec = exec;
    exec["lease_ttl_ms"] = json!(60_000);
    exec[field] = value;
    json!({
        "exec": exec,
        "input_limit_bytes": 65_536,
        "frame_limit_bytes": 16_384,
        "queued_frames": 8
    })
}

/// Attaches to a started session, reads it to its `exit` frame, and returns the exec record once
/// the daemon has recorded it terminal with `ready` holding.
async fn run_session_to_exit(
    daemon: &Daemon,
    session: &Value,
    ready: impl Fn(&Value) -> bool,
) -> (Value, Value) {
    let session_id = text(&session["result"]["id"]);
    let exec_id = text(&session["result"]["exec"]);
    let mut channel =
        SessionChannel::open(&daemon.socket, &format!("/v1/sessions/{session_id}/attach")).await;
    // Each program first reads one line, so the session is still ready when the attach arrives.
    channel.input("go\n").await;
    let terminal = channel.wait_for_terminal().await;
    assert_eq!(terminal["kind"], "exit", "{terminal}");
    drop(channel);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (status, observed) = daemon
            .call(
                "GET",
                &format!("/v1/execs/{exec_id}"),
                "req_clean_session_field_exec",
                None,
            )
            .await;
        assert_eq!(status, 200, "{observed}");
        if observed["result"]["state"] == "exited" && ready(&observed["result"]) {
            return (terminal, observed["result"].clone());
        }
        assert!(Instant::now() < deadline, "{observed}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Design 27: each exec-start field the session start publishes in `0.23.0` is served on
/// `POST /v1/sessions` exactly as on exec start — `measurements` and `workspace_access` honoured,
/// `scratch` refused by its own named code before dispatch on a host without project quotas.
#[allow(clippy::too_many_lines)] // Three cases, one per field; splitting them would hide the set.
async fn check_confined_session_exec_fields(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
) -> usize {
    let mut passed = 0;
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_clean_session_fields_facts", None)
        .await;
    assert_eq!(status, 200, "{machine}");
    let facts = &machine["result"]["facts"];
    assert!(
        facts.get("exec.resource-usage").is_some(),
        "the delegated lane delegates cpu, memory and pids, so it proves usage counters: {machine}"
    );
    assert_eq!(
        facts["exec.workspace-scoped-write"], true,
        "the delegated lane proves read-only and scoped workspace mounts: {machine}"
    );
    assert!(
        facts.get("exec.scratch-quota").is_none(),
        "the clean-room daemon declares no project-quota identities: {machine}"
    );

    // `measurements`: the session's exec record carries usage from the start, and the counters
    // are observed once the session has ended.
    let (status, session) = daemon
        .call(
            "POST",
            "/v1/sessions",
            "req_clean_session_measurements",
            Some(&mutation(
                "01JSESSIONFIELDMEASURE01",
                &session_with_exec_field(
                    ExecInput::new(
                        workspace,
                        snapshot,
                        &["/bin/sh", "-c", "read line; exit 0"],
                        false,
                    )
                    .build(),
                    "measurements",
                    json!(["resource-usage"]),
                ),
            )),
        )
        .await;
    assert_eq!(status, 202, "{session}");
    let exec_id = text(&session["result"]["exec"]);
    let (status, started) = daemon
        .call(
            "GET",
            &format!("/v1/execs/{exec_id}"),
            "req_clean_session_measurements_started",
            None,
        )
        .await;
    assert_eq!(status, 200, "{started}");
    assert!(
        started["result"]["usage"]["status"] == "pending"
            || started["result"]["usage"]["status"] == "observed",
        "a session that asked for measurements carries usage: {started}"
    );
    let (_, observed) = run_session_to_exit(daemon, &session, |record| {
        record["usage"]["status"] == "observed"
    })
    .await;
    assert_eq!(observed["exit"]["code"], 0, "{observed}");
    assert_eq!(observed["usage"]["complete"], true, "{observed}");
    assert!(
        observed["usage"]["processes_peak"].as_u64() >= Some(1),
        "{observed}"
    );
    passed += 1;

    // `workspace_access`: the grant is applied and stated, and a write to `/workspace` fails
    // inside the sandbox and leaves the host workspace untouched.
    let probe = "session-read-only-probe";
    let program =
        format!("read line; if echo x > /workspace/{probe}; then exit 0; else exit 9; fi");
    let (status, session) = daemon
        .call(
            "POST",
            "/v1/sessions",
            "req_clean_session_workspace_access",
            Some(&mutation(
                "01JSESSIONFIELDACCESS001",
                &session_with_exec_field(
                    ExecInput::new(workspace, snapshot, &["/bin/sh", "-c", &program], false)
                        .build(),
                    "workspace_access",
                    json!({ "mode": "read-only" }),
                ),
            )),
        )
        .await;
    assert_eq!(status, 202, "{session}");
    let (terminal, observed) = run_session_to_exit(daemon, &session, |_| true).await;
    assert_eq!(terminal["exit"]["code"], 9, "{terminal}");
    assert_eq!(observed["exit"]["code"], 9, "{observed}");
    assert_eq!(
        observed["applied"]["workspace_access"],
        json!({ "mode": "read-only" }),
        "{observed}"
    );
    assert!(
        !daemon.workspaces.join(workspace).join(probe).exists(),
        "a read-only session wrote the workspace"
    );
    passed += 1;

    // `scratch`: the same named refusal exec start gives on a host without the quota fact, before
    // dispatch and recorded as refused.
    let operation = "01JSESSIONFIELDSCRATCH01";
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/sessions",
                "req_clean_session_scratch",
                Some(&mutation(
                    operation,
                    &session_with_exec_field(
                        ExecInput::new(workspace, snapshot, &["/bin/sh", "-c", "exit 0"], false)
                            .build(),
                        "scratch",
                        json!({ "max_bytes": 1_048_576, "max_inodes": 16 }),
                    ),
                )),
            )
            .await,
        501,
        "exec.scratch-quota-unserved",
    );
    assert_eq!(
        ledger_row(daemon, operation, "req_clean_session_scratch_ledger").await["state"],
        "refused"
    );
    passed += 1;
    passed
}

/// The delegated exec-lease sequence: idle-time whole-cgroup expiry, replay, conflict and retire.
#[allow(clippy::too_many_lines)] // One sequential journey; splitting it would lose the ordering.
async fn check_exec_lease(daemon: &Daemon, workspace: &str, snapshot: &str, cgroup_root: &Path) {
    let mut leased_input =
        ExecInput::new(workspace, snapshot, &["/usr/bin/sleep", "60"], false).build();
    leased_input["lease_ttl_ms"] = json!(1000);
    let (status, started) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_phase3_exec_lease_start",
            Some(&mutation("01JPHASE3EXECLEASESTART1", &leased_input)),
        )
        .await;
    assert_eq!(status, 202, "{started}");
    let exec_id = text(&started["result"]["id"]);
    let (status, renewed_exec) = daemon
        .call(
            "POST",
            &format!("/v1/execs/{exec_id}/lease/renew"),
            "req_phase3_exec_lease_renew",
            Some(&mutation(
                "01JPHASE3EXECLEASERENEW1",
                &json!({ "ttl_ms": 1000 }),
            )),
        )
        .await;
    assert_eq!(status, 200, "{renewed_exec}");
    assert_eq!(
        renewed_exec["result"]["lease"]["authorizing_operation"],
        "01JPHASE3EXECLEASERENEW1"
    );
    wait_absent(&cgroup_root.join(text(&started["result"]["applied"]["cgroup"]))).await;
    let (status, replayed_exec_renewal) = daemon
        .call(
            "POST",
            &format!("/v1/execs/{exec_id}/lease/renew"),
            "req_phase3_exec_lease_replay",
            Some(&mutation(
                "01JPHASE3EXECLEASERENEW1",
                &json!({ "ttl_ms": 1000 }),
            )),
        )
        .await;
    assert_eq!(status, 200, "{replayed_exec_renewal}");
    assert_eq!(replayed_exec_renewal["result"], renewed_exec["result"]);
    expect_error(
        &daemon
            .call(
                "POST",
                &format!("/v1/execs/{exec_id}/lease/renew"),
                "req_phase3_exec_lease_conflict",
                Some(&mutation(
                    "01JPHASE3EXECLEASERENEW1",
                    &json!({ "ttl_ms": 2000 }),
                )),
            )
            .await,
        409,
        "operation.request-conflict",
    );
    let (status, expired_exec) = daemon
        .call(
            "GET",
            &format!("/v1/execs/{exec_id}"),
            "req_phase3_exec_lease_expired",
            None,
        )
        .await;
    assert_eq!(status, 200, "{expired_exec}");
    assert_eq!(expired_exec["result"]["state"], "expired");
    let retire_operation = "01JPHASE3EXECRETIRE00001";
    let (status, retired_exec) = daemon
        .call(
            "DELETE",
            &format!("/v1/execs/{exec_id}"),
            "req_phase3_exec_retire",
            Some(&mutation(retire_operation, &json!({}))),
        )
        .await;
    assert_eq!(status, 200, "{retired_exec}");
    assert_eq!(
        retired_exec["result"],
        json!({
            "absent": true,
            "id": exec_id,
            "kind": "exec",
            "observed_at": retired_exec["result"]["observed_at"],
        })
    );
    let (status, retired_replay) = daemon
        .call(
            "DELETE",
            &format!("/v1/execs/{exec_id}"),
            "req_phase3_exec_retire_replay",
            Some(&mutation(retire_operation, &json!({}))),
        )
        .await;
    assert_eq!(status, 200, "{retired_replay}");
    assert_eq!(retired_replay["result"], retired_exec["result"]);
    expect_error(
        &daemon
            .call(
                "GET",
                &format!("/v1/execs/{exec_id}"),
                "req_phase3_exec_retired_absent",
                None,
            )
            .await,
        404,
        "resource.not-found",
    );
}

// ---------------------------------------------------------------------------------------------
// The HTTP journey
// ---------------------------------------------------------------------------------------------

/// One deliberately invalid request per promoted operation. The body/refusal is not a substitute
/// for the positive journeys below; it is the route-inventory proof that the shipped binary
/// reaches every address declared by the promoted bundle and carries the promoted identity on the
/// HTTP response or WebSocket handshake path.
#[allow(clippy::too_many_lines)] // Keeping the complete declarative inventory together makes drift reviewable.
async fn check_promoted_route_inventory(
    daemon: &Daemon,
    workspace: &str,
    process_host: bool,
) -> usize {
    type Probe<'a> = (&'a str, &'a str, String, Option<&'a [u8]>, u16, &'a str);
    let invalid_body = b"{}".as_slice();
    let probes: Vec<Probe<'_>> = vec![
        (
            "machine.get",
            "GET",
            "/v1/machine?unexpected=1".to_owned(),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.create",
            "POST",
            "/v1/workspaces".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.adopt",
            "POST",
            "/v1/workspaces/adopt".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.get",
            "GET",
            format!("/v1/workspaces/{workspace}?unexpected=1"),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.file.read",
            "GET",
            format!("/v1/workspaces/{workspace}/files/probe?mode=invalid"),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.file.write",
            "PUT",
            format!("/v1/workspaces/{workspace}/files/probe"),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.file.delete",
            "DELETE",
            format!("/v1/workspaces/{workspace}/files/probe"),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.destroy",
            "DELETE",
            format!("/v1/workspaces/{workspace}"),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "exec.start",
            "POST",
            "/v1/execs".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "exec.get",
            "GET",
            "/v1/execs/ex_missing?unexpected=1".to_owned(),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "exec.output.get",
            "GET",
            "/v1/execs/ex_missing/output?stream=invalid&offset=0&limit_bytes=1".to_owned(),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "exec.signal",
            "POST",
            "/v1/execs/ex_missing/signal".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "exec.retire",
            "DELETE",
            "/v1/execs/ex_missing".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "session.capabilities",
            "GET",
            "/v1/sessions?unexpected=1".to_owned(),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "session.start",
            "POST",
            "/v1/sessions".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "session.get",
            "GET",
            "/v1/sessions/ses_missing?unexpected=1".to_owned(),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "session.signal",
            "POST",
            "/v1/sessions/ses_missing/signal".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "session.retire",
            "DELETE",
            "/v1/sessions/ses_missing".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "session.lease.renew",
            "POST",
            "/v1/sessions/ses_missing/lease/renew".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "operation.get",
            "GET",
            "/v1/ops/01JROUTEPROBE000000000001?unexpected=1".to_owned(),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "event.list",
            "GET",
            "/v1/events?unexpected=1".to_owned(),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "reconciliation.snapshot.create",
            "POST",
            "/v1/reconciliation-snapshots?unexpected=1".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "reconciliation.snapshot.get",
            "GET",
            "/v1/reconciliation-snapshots/snap_missing?unexpected=1".to_owned(),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.lease.renew",
            "POST",
            format!("/v1/workspaces/{workspace}/lease/renew"),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "exec.lease.renew",
            "POST",
            "/v1/execs/ex_missing/lease/renew".to_owned(),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.file.read-v2",
            "GET",
            format!("/v2/workspaces/{workspace}/files/probe?mode=invalid"),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.tree.read-v2",
            "GET",
            format!("/v2/workspaces/{workspace}/tree?unexpected=1"),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.git.baseline-file.read-v2",
            "GET",
            format!("/v2/workspaces/{workspace}/git/baseline/probe?max_bytes=0"),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.git.changes.read-v2",
            "GET",
            format!(
                "/v2/workspaces/{workspace}/git/changes?max_files=0&max_file_bytes=1&max_total_bytes=1"
            ),
            None,
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.file.replace-v2",
            "PUT",
            format!("/v2/workspaces/{workspace}/files/probe"),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.file.edit-v2",
            "POST",
            format!("/v2/workspaces/{workspace}/file-edits/probe"),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "workspace.file.patch-v2",
            "POST",
            format!("/v2/workspaces/{workspace}/file-patches/probe"),
            Some(invalid_body),
            422,
            "request.schema-invalid",
        ),
        (
            "metrics.get",
            "GET",
            "/v1/metrics?unexpected=1".to_owned(),
            None,
            422,
            "request.schema-invalid",
        ),
    ];
    let upgrade_probes = [
        (
            "session.attach",
            "/v1/sessions/ses_missing/attach",
            404,
            "resource.not-found",
        ),
        (
            "event.stream",
            "/v1/events/stream?unexpected=1",
            422,
            "request.schema-invalid",
        ),
        (
            "metrics.stream",
            "/v1/metrics/stream?unexpected=1",
            422,
            "request.schema-invalid",
        ),
        // ADR 0036 S12: answered on both lanes before the upgrade; no record of this subject
        // exists.
        (
            "process.events",
            "/v1/processes/proc_01JPROBEABSENT0000000001/events",
            404,
            "process.not-found",
        ),
    ];
    // The host-process family (ADR 0036): no probe may start a program. Without a delegated
    // cgroup the daemon serves no host process, so a well-formed start and every mutation of a
    // record answer `process.host-unserved` (501), never a degraded start; the reads still answer,
    // an unknown id is `process.not-found`. With one, the start omits its profile (refused before
    // anything runs) and the mutations name a record that does not exist.
    let process_absent = "proc_01JPROBEABSENT0000000001";
    let process_start = serde_json::to_vec(&json!({
        "op": "01JPROBEPROCESSSTART00001",
        "input": {
            "argv": ["/usr/bin/true"],
            "cwd": "/",
            "env": {},
            "lease": { "ttl_ms": 60_000 },
            "owner": { "name": "probe", "session": "sess-probe" },
            "profile": "host",
            "reason": "Route inventory probe.",
        },
    }))
    .expect("process start JSON");
    let process_start_unprofiled = serde_json::to_vec(&json!({
        "op": "01JPROBEPROCESSSTART00002",
        "input": {
            "argv": ["/usr/bin/true"],
            "cwd": "/",
            "env": {},
            "lease": { "ttl_ms": 60_000 },
            "owner": { "name": "probe", "session": "sess-probe" },
            "reason": "Route inventory probe.",
        },
    }))
    .expect("process start JSON");
    let process_stop = mutation("01JPROBEPROCESSSTOP000001", &json!({}));
    let process_stop_by_owner = br#"{"owner_session":"sess-probe-absent"}"#.as_slice();
    let process_renew = mutation("01JPROBEPROCESSRENEW00001", &json!({ "ttl_ms": 60_000 }));
    let (start_body, start_status, start_code, mutation_status, mutation_code) = if process_host {
        (
            process_start_unprofiled.as_slice(),
            422,
            "process.profile-required",
            404,
            "process.not-found",
        )
    } else {
        (
            process_start.as_slice(),
            501,
            "process.host-unserved",
            501,
            "process.host-unserved",
        )
    };
    let process_probes: Vec<Probe<'_>> = vec![
        (
            "process.start",
            "POST",
            "/v1/processes".to_owned(),
            Some(start_body),
            start_status,
            start_code,
        ),
        (
            "process.get",
            "GET",
            format!("/v1/processes/{process_absent}"),
            None,
            404,
            "process.not-found",
        ),
        (
            "process.stop",
            "POST",
            format!("/v1/processes/{process_absent}/stop"),
            Some(process_stop.as_slice()),
            mutation_status,
            mutation_code,
        ),
        (
            "process.lease.renew",
            "POST",
            format!("/v1/processes/{process_absent}/lease/renew"),
            Some(process_renew.as_slice()),
            mutation_status,
            mutation_code,
        ),
        (
            "process.logs.get",
            "GET",
            format!("/v1/processes/{process_absent}/logs"),
            None,
            404,
            "process.not-found",
        ),
    ];

    let expected: BTreeSet<String> = {
        let registry = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("contracts/substrate-wire/0.25.0/operations.json");
        let document: Value = serde_json::from_slice(
            &std::fs::read(&registry).expect("promoted operation registry bytes"),
        )
        .expect("promoted operation registry JSON");
        document["operations"]
            .as_array()
            .expect("promoted operations array")
            .iter()
            .map(|operation| text(&operation["id"]))
            // The clean-room daemon in this lane is deliberately the Unix listener. The
            // authority mint route is hosted-TLS-only and its absence here is independently
            // asserted by the transport route-set tests.
            .filter(|id| id != "session.authority.mint")
            .collect()
    };
    let covered: BTreeSet<String> = probes
        .iter()
        .map(|(id, ..)| (*id).to_owned())
        .chain(upgrade_probes.iter().map(|(id, ..)| (*id).to_owned()))
        .chain(process_probes.iter().map(|(id, ..)| (*id).to_owned()))
        .chain(["process.list", "process.stop-by-owner"].map(str::to_owned))
        .collect();
    assert_eq!(covered, expected, "shipped-binary route probes drifted");

    for (index, (id, method, path, body, status, code)) in probes.iter().enumerate() {
        let request_id = format!("req_promoted_route_{index:02}");
        let observed = daemon.call(method, path, &request_id, *body).await;
        expect_error(&observed, *status, code);
        assert_eq!(observed.1["error"]["code"], *code, "{id}: {}", observed.1);
    }
    for (index, (id, path, status, code)) in upgrade_probes.iter().enumerate() {
        let request_id = format!("req_promoted_upgrade_{index:02}");
        let observed = upgrade_refusal(&daemon.socket, path, &request_id).await;
        expect_error(&observed, *status, code);
        assert!(observed.1["error"].is_object(), "{id}: {}", observed.1);
    }

    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_promoted_process_fact", None)
        .await;
    assert_eq!(status, 200, "{machine}");
    let published = &machine["result"]["facts"]["process.host"];
    if process_host {
        assert_eq!(
            published,
            &json!(true),
            "process.host on the delegated lane"
        );
    } else {
        assert!(
            published.is_null(),
            "process.host published without a delegated cgroup"
        );
    }
    for (index, (id, method, path, body, status, code)) in process_probes.iter().enumerate() {
        let request_id = format!("req_promoted_process_{index:02}");
        let observed = daemon.call(method, path, &request_id, *body).await;
        expect_error(&observed, *status, code);
        assert_eq!(observed.1["error"]["code"], *code, "{id}: {}", observed.1);
    }
    // The reads answer on both lanes: no record of this subject exists yet.
    let (status, listed) = daemon
        .call(
            "GET",
            "/v1/processes?owner_session=sess-probe",
            "req_promoted_process_list",
            None,
        )
        .await;
    assert_eq!(status, 200, "process.list: {listed}");
    assert_eq!(
        listed["result"]["items"],
        json!([]),
        "process.list: {listed}"
    );
    let stopped_by_owner = daemon
        .call(
            "POST",
            "/v1/processes/stop",
            "req_promoted_process_stop_by_owner",
            Some(process_stop_by_owner),
        )
        .await;
    if process_host {
        assert_eq!(stopped_by_owner.0, 200, "{}", stopped_by_owner.1);
        assert_eq!(
            stopped_by_owner.1["result"],
            json!({ "owner_session": "sess-probe-absent", "stopped": [] }),
            "process.stop-by-owner: {}",
            stopped_by_owner.1
        );
    } else {
        expect_error(&stopped_by_owner, 501, "process.host-unserved");
    }
    covered.len()
}

/// Bundle `0.24.0`'s vector `process-events-beyond-the-readers-bound-is-exhausted`, executed on
/// the delegated lane, which serves host processes: the record holds the vector's `event_readers`
/// open streams, and the next is answered the vector's own status and body before any upgrade.
/// The record is one this daemon started, because the vector's id is a fixture; its request,
/// readers count and expected response are read from the released vector.
async fn check_process_event_readers_vector(daemon: &Daemon) -> usize {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../../contracts/substrate-wire/0.24.0/vectors/http/\
         process-events-beyond-the-readers-bound-is-exhausted.json",
    );
    let vector: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("released readers vector"))
            .expect("readers vector JSON");
    let readers = vector["setup"]
        .as_array()
        .expect("vector setup")
        .iter()
        .find(|step| step["kind"] == "process")
        .and_then(|step| step["state"]["event_readers"].as_u64())
        .expect("the vector's process holds event_readers");
    let request_id = text(&vector["action"]["request"]["headers"]["x-request-id"]);
    let start = mutation(
        "01JVECTORREADERSSTART0001",
        &json!({
            "argv": ["/usr/bin/sleep", "300"],
            "cwd": "/",
            "env": {},
            "lease": { "ttl_ms": 60_000 },
            "owner": { "name": "readers", "session": "sess-vector-readers" },
            "profile": "host",
            "reason": "Hold the readers bound of the process event stream.",
        }),
    );
    let (status, started) = daemon
        .call(
            "POST",
            "/v1/processes",
            "req_vector_readers_start",
            Some(&start),
        )
        .await;
    assert_eq!(status, 200, "{started}");
    let id = text(&started["result"]["id"]);
    let events = format!("/v1/processes/{id}/events");
    let mut open = Vec::new();
    for _ in 0..readers {
        open.push(EventStream::open(&daemon.socket, &events).await);
    }
    let (status, body) = upgrade_refusal(&daemon.socket, &events, &request_id).await;
    assert_eq!(
        json!(status),
        vector["expected"]["response"]["status"],
        "{body}"
    );
    assert_eq!(body, vector["expected"]["response"]["body"]);
    drop(open);
    let stop = mutation("01JVECTORREADERSSTOP00001", &json!({}));
    let (status, stopped) = daemon
        .call(
            "POST",
            &format!("/v1/processes/{id}/stop"),
            "req_vector_readers_stop",
            Some(&stop),
        )
        .await;
    assert_eq!(status, 200, "{stopped}");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (status, record) = daemon
            .call(
                "GET",
                &format!("/v1/processes/{id}"),
                "req_vector_readers_get",
                None,
            )
            .await;
        assert_eq!(status, 200, "{record}");
        if record["result"]["state"] == "ended" {
            break;
        }
        assert!(Instant::now() < deadline, "{id} did not end: {record}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    1
}

/// The caps, lifetime and refusal of each upgraded stream family, as `/v1/machine` publishes them
/// (ADR 0042, bundle `0.23.0`). Pinned here rather than read from the wire crate: the clean-room
/// runner links no implementation.
///
/// `events.stream-bounds` is published on every host, because the event stream is served on
/// every host. `metrics.stream-bounds` is published exactly beside `metrics.stream`, and
/// `sessions.attachment-bounds` exactly beside `exec.argv-only`, because a session is an exec;
/// the delegated lane serves exec, so it must publish the attachment bounds.
async fn check_stream_bound_facts(daemon: &Daemon, delegated: bool) -> usize {
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_clean_stream_bounds", None)
        .await;
    assert_eq!(status, 200);
    let facts = &machine["result"]["facts"];
    assert_eq!(
        facts["events.stream-bounds"],
        json!({
            "per_subject": 4,
            "global": 64,
            "lifetime_ms": 3_600_000,
            "refusal": "event.stream-capacity"
        }),
        "every host publishes the event stream's bounds: {machine}"
    );
    let metrics = facts.get("metrics.stream-bounds");
    assert_eq!(
        metrics.is_some(),
        facts.get("metrics.stream").is_some(),
        "the metrics stream's bounds are published exactly beside metrics.stream: {machine}"
    );
    if let Some(metrics) = metrics {
        assert_eq!(
            metrics,
            &json!({
                "per_subject": 4,
                "global": 64,
                "lifetime_ms": 3_600_000,
                "refusal": "metrics.stream-capacity"
            })
        );
    }
    let attachments = facts.get("sessions.attachment-bounds");
    assert_eq!(
        attachments.is_some(),
        facts.get("exec.argv-only").is_some(),
        "the session attachment's bounds are published exactly when exec is served: {machine}"
    );
    assert_eq!(
        attachments.is_some(),
        delegated,
        "the delegated lane serves exec and the portable lane does not: {machine}"
    );
    if let Some(attachments) = attachments {
        assert_eq!(
            attachments,
            &json!({
                "per_subject": 16,
                "per_session": 1,
                "global": 32,
                "lifetime_ms": 3_600_000,
                "refusal": "session.attachment-capacity"
            })
        );
    }
    1
}

/// The response header naming how many entries a listing omitted (ADR 0043).
const OMITTED_ENTRIES_HEADER: &str = "x-b10x-omitted-entries";

/// A directory holding a FIFO answers the omission in `x-b10x-omitted-entries` on the v1
/// directory page and on the v2 tree; the tree keeps answering `truncated` for it (ADR 0043).
///
/// Run on a workspace that holds nothing else yet, so the tree's items and count are exact; the
/// fixture is removed afterwards, and the same reads then carry no header.
async fn check_omitted_entries(daemon: &Daemon, workspace: &str) -> usize {
    let host = daemon.workspaces.join(workspace);
    let listing = host.join("listing");
    std::fs::create_dir(&listing).expect("listing directory");
    std::fs::write(listing.join("b.txt"), b"b").expect("listed file");
    let fifo = listing.join("a-fifo");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(made.success(), "mkfifo {}", fifo.display());
    let page_path =
        format!("/v1/workspaces/{workspace}/files/listing?mode=directory&limit_items=10");
    let tree_path = |limit: u32| format!("/v2/workspaces/{workspace}/tree?limit_items={limit}");
    let mut passed = 0;

    let (status, headers, page) = daemon
        .get_with_response_headers(&page_path, "req_clean_omitted_page")
        .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(
        page["result"]["items"],
        json!([{ "name": "b.txt", "kind": "file", "size": 1 }])
    );
    assert_eq!(
        headers.get(OMITTED_ENTRIES_HEADER).map(String::as_str),
        Some("1"),
        "the v1 directory page names its omitted FIFO: {headers:?}"
    );
    passed += 1;

    let (status, headers, tree) = daemon
        .get_with_response_headers(&tree_path(100), "req_clean_omitted_tree")
        .await;
    assert_eq!(status, 200, "{tree}");
    let paths: Vec<&str> = tree["result"]["items"]
        .as_array()
        .expect("tree items")
        .iter()
        .map(|item| item["path"].as_str().expect("tree path"))
        .collect();
    assert_eq!(paths, ["listing", "listing/b.txt"], "{tree}");
    assert_eq!(
        tree["result"]["truncated"], true,
        "an omission still sets truncated, for compatibility: {tree}"
    );
    assert_eq!(
        headers.get(OMITTED_ENTRIES_HEADER).map(String::as_str),
        Some("1"),
        "the v2 tree names its omitted FIFO: {headers:?}"
    );
    passed += 1;

    let (status, headers, tree) = daemon
        .get_with_response_headers(&tree_path(1), "req_clean_omitted_tree_limit")
        .await;
    assert_eq!(status, 200, "{tree}");
    assert_eq!(tree["result"]["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        tree["result"]["truncated"], true,
        "the limit is truncation: {tree}"
    );
    assert_eq!(
        headers.get(OMITTED_ENTRIES_HEADER).map(String::as_str),
        Some("1"),
        "the omission is named beside the limit, not folded into it: {headers:?}"
    );
    passed += 1;

    std::fs::remove_file(&fifo).expect("remove the FIFO");
    let (status, page_headers, _) = daemon
        .get_with_response_headers(&page_path, "req_clean_complete_page")
        .await;
    assert_eq!(status, 200);
    let (status, tree_headers, tree) = daemon
        .get_with_response_headers(&tree_path(100), "req_clean_complete_tree")
        .await;
    assert_eq!(status, 200);
    assert_eq!(tree["result"]["truncated"], false, "{tree}");
    assert_eq!(
        (
            page_headers.get(OMITTED_ENTRIES_HEADER),
            tree_headers.get(OMITTED_ENTRIES_HEADER)
        ),
        (None, None),
        "a listing that omits nothing carries no omission header"
    );
    passed += 1;

    std::fs::remove_dir_all(&listing).expect("remove the listing fixture");
    passed
}

#[allow(clippy::too_many_lines)] // One sequential journey; splitting it would lose the ordering.
async fn check_http_journey(
    daemon: &Daemon,
    cgroup_root: Option<&Path>,
    inside: u16,
    outside: u16,
    firehose: u16,
    writable: &WritableFixture,
) -> usize {
    let mut passed = 0;
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_clean_machine", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(machine["result"]["driver"], "host");
    assert_eq!(machine["result"]["facts"]["workspace.guarded-io"], true);
    let snapshot = text(&machine["result"]["snapshot"]);
    passed += 1;

    let operation = "01JPHASE2CLEANCREATE01";
    let create_input = json!({ "source": "empty", "labels": { "runner": "cleanroom" } });
    let (status, created) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_clean_create",
            Some(&mutation(operation, &create_input)),
        )
        .await;
    assert_eq!(status, 201);
    let workspace = text(&created["result"]["id"]);
    assert!(workspace.starts_with("ws_"));
    passed += 1;

    passed += check_promoted_route_inventory(daemon, &workspace, cgroup_root.is_some()).await;
    passed += check_stream_bound_facts(daemon, cgroup_root.is_some()).await;
    if cgroup_root.is_some() {
        passed += check_process_event_readers_vector(daemon).await;
    }
    passed += check_omitted_entries(daemon, &workspace).await;

    let (status, replay) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_clean_replay",
            Some(&mutation(operation, &create_input)),
        )
        .await;
    assert_eq!(status, 201);
    assert_eq!(replay["result"]["id"], workspace);
    passed += 1;

    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/workspaces",
                "req_clean_conflict",
                Some(&mutation(
                    operation,
                    &json!({ "source": "empty", "labels": { "changed": "yes" } }),
                )),
            )
            .await,
        409,
        "operation.request-conflict",
    );
    passed += 1;

    let (status, observed) = daemon
        .call(
            "GET",
            &format!("/v1/workspaces/{workspace}"),
            "req_clean_get",
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(observed["result"]["state"], "ready");
    passed += 1;

    let file_path = format!("/v1/workspaces/{workspace}/files/main.txt");
    let (status, written) = daemon
        .call(
            "PUT",
            &file_path,
            "req_clean_write",
            Some(&mutation(
                "01JPHASE2CLEANWRITE001",
                &json!({ "content": { "encoding": "base64", "data": "aGVsbG8=" } }),
            )),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(written["result"]["atomic_replacement"], true);
    assert_eq!(
        written["result"]["sha256"],
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    passed += 1;

    let maximum_content = vec![b'm'; 1_048_576];
    let (status, maximum_written) = daemon
        .call(
            "PUT",
            &format!("/v1/workspaces/{workspace}/files/maximum.bin"),
            "req_clean_maximum_write",
            Some(&mutation(
                "01JPHASE3MAXIMUMWRITE01",
                &json!({
                    "content": {
                        "encoding": "base64",
                        "data": BASE64.encode(&maximum_content),
                    }
                }),
            )),
        )
        .await;
    assert_eq!(status, 200, "{maximum_written}");
    assert_eq!(maximum_written["result"]["size"], 1_048_576);
    passed += 1;

    let deep_path = ["d"; 65].join("/");
    expect_error(
        &daemon
            .call(
                "DELETE",
                &format!("/v1/workspaces/{workspace}/files/{deep_path}"),
                "req_clean_path_depth",
                Some(&mutation("01JPHASE3PATHDEPTH0001", &json!({}))),
            )
            .await,
        422,
        "workspace.path-depth",
    );
    passed += 1;

    let (status, read) = daemon
        .call(
            "GET",
            &format!("{file_path}?mode=file&offset=0&limit_bytes=5"),
            "req_clean_read",
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(read["result"]["content"]["data"], "aGVsbG8=");
    assert_eq!(read["result"]["eof"], true);
    passed += 1;

    passed += check_path_escape(daemon, &workspace, WorkspaceOrigin::Created).await;

    let float_operation = "01JPHASE3FLOATREFUSAL01";
    let float_input = json!({ "source": "empty", "labels": {}, "priority": 1.5 });
    let (status, float_refused) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_clean_float_refusal",
            Some(&mutation(float_operation, &float_input)),
        )
        .await;
    assert_eq!(status, 422, "{float_refused}");
    assert_eq!(float_refused["error"]["code"], "request.schema-invalid");
    assert_eq!(float_refused["error"]["operation"], float_operation);
    let (status, float_replay) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_clean_float_replay",
            Some(&mutation(float_operation, &float_input)),
        )
        .await;
    assert_eq!(status, 422, "{float_replay}");
    assert_eq!(float_replay["error"], float_refused["error"]);
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/workspaces",
                "req_clean_float_conflict",
                Some(&mutation(
                    float_operation,
                    &json!({ "source": "empty", "labels": {}, "priority": 2.5 }),
                )),
            )
            .await,
        409,
        "operation.request-conflict",
    );
    let (status, float_record) = daemon
        .call(
            "GET",
            &format!("/v1/ops/{float_operation}"),
            "req_clean_float_record",
            None,
        )
        .await;
    assert_eq!(status, 200, "{float_record}");
    assert_eq!(float_record["result"]["state"], "refused");
    passed += 1;

    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/workspaces",
                "req_clean_strict",
                Some(&mutation(
                    "01JPHASE2CLEANSTRICT01",
                    &json!({ "source": "empty", "labels": {}, "secret": "forbidden" }),
                )),
            )
            .await,
        422,
        "request.schema-invalid",
    );
    passed += 1;

    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/workspaces",
                "req_clean_limit",
                Some(&vec![b' '; 2_097_153]),
            )
            .await,
        429,
        "request.body-limit",
    );
    passed += 1;

    // A destination where a name belongs is a rejected escalation in every lane, and it is told
    // apart from an unknown name so an operator is not sent looking for a configuration typo.
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_aperture_destination",
                Some(&mutation(
                    "01JPHASE2CLEANAPERTURE3",
                    &ExecInput::new(&workspace, &snapshot, &["/usr/bin/true"], true)
                        .aperture("127.0.0.1:443")
                        .build(),
                )),
            )
            .await,
        422,
        "exec.aperture-destination-in-request",
    );
    passed += 1;

    // A ceiling is deployment vocabulary, never request data. `ConfinementRequest` is
    // `deny_unknown_fields`, so a conforming client's ceiling field is `schema-invalid` first; this
    // is the one shape the schema cannot see — a *name* carrying one — and it is refused as the
    // rejected escalation it is rather than as a destination or a typo (ADR 0014).
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_aperture_ceiling",
                Some(&mutation(
                    "01JPHASE2CLEANAPERTURE5",
                    &ExecInput::new(&workspace, &snapshot, &["/usr/bin/true"], true)
                        .aperture("model/max=1MiB")
                        .build(),
                )),
            )
            .await,
        422,
        "exec.aperture-ceiling-in-request",
    );
    passed += 1;

    // Shape before capability, in both lanes: a descriptor outside `3..=63` is refused by name
    // whether or not this host could have delivered a slot at all (ADR 0012).
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_secret_slot_descriptor",
                Some(&mutation(
                    "01JPHASE2CLEANSLOT00004",
                    &ExecInput::new(&workspace, &snapshot, &["/usr/bin/true"], true)
                        .secret_slot(SECRET_SLOT_NAME, 2)
                        .build(),
                )),
            )
            .await,
        422,
        "exec.secret-slot-descriptor-invalid",
    );
    passed += 1;

    passed += check_read_only_file_shape(daemon, &workspace, &snapshot).await;
    passed += check_workdir_and_aperture_list_shape(daemon, &workspace, &snapshot).await;
    passed +=
        check_writable_roots_refused(daemon, &workspace, &snapshot, cgroup_root.is_some()).await;

    if let Some(cgroup_root) = cgroup_root {
        passed += check_confined_writable_roots(daemon, &workspace, &snapshot, writable).await;
        passed += check_confined_execs(daemon, &workspace, &snapshot, cgroup_root).await;
        passed += check_confined_apertures(daemon, &workspace, &snapshot, inside, outside).await;
        passed += check_confined_aperture_ceiling(daemon, &workspace, &snapshot, firehose).await;
        passed += check_confined_secret_slots(daemon, &workspace, &snapshot).await;
        passed += check_confined_pty(daemon, &workspace, &snapshot).await;
        passed += check_confined_session_exec_fields(daemon, &workspace, &snapshot).await;
        passed += check_confined_read_only_files(daemon, &workspace, &snapshot).await;
        passed += check_confined_read_only_roots(daemon, &workspace, &snapshot).await;
        passed += check_confined_workdir(daemon, &workspace, &snapshot).await;
        passed +=
            check_confined_aperture_list(daemon, &workspace, &snapshot, inside, firehose).await;
    } else {
        passed += check_read_only_files_unserved(daemon, &workspace, &snapshot).await;
        passed += check_read_only_roots_unserved(daemon, &workspace, &snapshot).await;
        passed += check_workdir_and_aperture_list_unserved(daemon, &workspace, &snapshot).await;
        passed += check_writable_root_unserved(daemon, &workspace, &snapshot, writable).await;
        // No confinement means no verified mechanism, so the capability is absent and every
        // aperture request is `unserved` — never a run that quietly got no network instead.
        expect_error(
            &daemon
                .call(
                    "POST",
                    "/v1/execs",
                    "req_clean_aperture_unserved",
                    Some(&mutation(
                        "01JPHASE2CLEANAPERTURE4",
                        &ExecInput::new(&workspace, &snapshot, &["/usr/bin/true"], true)
                            .aperture("model")
                            .build(),
                    )),
                )
                .await,
            501,
            "exec.egress-apertures-unserved",
        );
        passed += 1;
        // A declared slot this host cannot prove it can seal and pass through is `unserved`, never
        // a run that quietly got the value some weaker way (invariant 3).
        expect_error(
            &daemon
                .call(
                    "POST",
                    "/v1/execs",
                    "req_clean_secret_slot_unserved",
                    Some(&mutation(
                        "01JPHASE2CLEANSLOT00005",
                        &ExecInput::new(&workspace, &snapshot, &["/usr/bin/true"], true)
                            .secret_slot(SECRET_SLOT_NAME, SECRET_SLOT_FD)
                            .build(),
                    )),
                )
                .await,
            501,
            "exec.secret-slots-unserved",
        );
        passed += 1;
        expect_error(
            &daemon
                .call(
                    "POST",
                    "/v1/execs",
                    "req_clean_exec",
                    Some(&mutation(
                        "01JPHASE2CLEANEXEC0001",
                        &ExecInput::new(&workspace, &snapshot, &["/usr/bin/true"], false).build(),
                    )),
                )
                .await,
            501,
            "exec.sandbox-unavailable",
        );
        passed += 1;
        // A terminal this host never proved it can allocate is `unserved` by name, and the
        // capability document does not advertise the mode either. Never a pipe session instead
        // (design 13, invariant 3).
        let (status, capabilities) = daemon
            .call("GET", "/v1/sessions", "req_clean_pty_modes", None)
            .await;
        assert_eq!(status, 501, "{capabilities}");
        expect_error(
            &daemon
                .call(
                    "POST",
                    "/v1/sessions",
                    "req_clean_pty_unserved",
                    Some(&mutation(
                        "01JPHASE2CLEANPTY000002",
                        &pty_session_input(
                            &workspace,
                            &snapshot,
                            Some(json!({
                                "columns": 80,
                                "rows": 24
                            })),
                        ),
                    )),
                )
                .await,
            501,
            "session.pty-unserved",
        );
        passed += 1;
    }

    passed += check_phase3_journey(daemon, &workspace, &snapshot, cgroup_root).await;

    let (status, ledger) = daemon
        .call(
            "GET",
            &format!("/v1/ops/{operation}"),
            "req_clean_operation",
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(ledger["result"]["state"], "terminal");
    assert_eq!(ledger["result"]["resource"], workspace);
    passed += 1;

    let (status, deleted) = daemon
        .call(
            "DELETE",
            &file_path,
            "req_clean_delete",
            Some(&mutation("01JPHASE2CLEANDELETE01", &json!({}))),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(deleted["result"]["absent"], true);
    passed += 1;

    let (status, destroyed) = daemon
        .call(
            "DELETE",
            &format!("/v1/workspaces/{workspace}"),
            "req_clean_destroy",
            Some(&mutation("01JPHASE2CLEANDESTROY1", &json!({}))),
        )
        .await;
    assert_eq!(status, 200, "{destroyed}");
    assert_eq!(destroyed["result"]["absent"], true);
    passed += 1;

    let (status, destroyed_replay) = daemon
        .call(
            "DELETE",
            &format!("/v1/workspaces/{workspace}"),
            "req_clean_destroy_replay",
            Some(&mutation("01JPHASE2CLEANDESTROY1", &json!({}))),
        )
        .await;
    assert_eq!(status, 200, "{destroyed_replay}");
    assert_eq!(destroyed_replay["result"], destroyed["result"]);
    passed += 1;

    let (status, write_replay) = daemon
        .call(
            "PUT",
            &file_path,
            "req_clean_write_replay",
            Some(&mutation(
                "01JPHASE2CLEANWRITE001",
                &json!({ "content": { "encoding": "base64", "data": "aGVsbG8=" } }),
            )),
        )
        .await;
    assert_eq!(status, 200, "{write_replay}");
    assert_eq!(
        write_replay["result"]["sha256"],
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    passed += 1;

    // Delegated context (ADR 0011), on the same daemon every other case ran against: verification
    // is pure computation, so it needs nothing the portable lane does not already have.
    assert_verified_context_is_recorded(
        daemon,
        "01JPHASE2CLEANGRANT0001",
        "req_clean_grant",
        &json!({ "runner": "cleanroom" }),
    )
    .await;
    passed += 1;

    assert_caller_written_identity_is_ignored(
        daemon,
        "01JPHASE2CLEANGRANT0002",
        "req_clean_forged",
    )
    .await;
    passed += 1;

    passed += check_adopted_workspace(daemon, &snapshot, cgroup_root).await;
    passed
}

// ---------------------------------------------------------------------------------------------
// An operator's directory, adopted (ADR 0035)
// ---------------------------------------------------------------------------------------------

/// The directory the operator places under the workspace root. No `ws_` prefix: the daemon did
/// not make it, and only `workspace.adopt` lets it be served.
const ADOPTED_ROOT_NAME: &str = "adopted-repo";
/// A real directory under the workspace root whose name is outside the root-name set: the
/// refusal must come from the name, because a host observation of this entry would succeed.
const OUT_OF_SET_ROOT_NAME: &str = ".adopted-hidden";
const ADOPTED_MARKER: &str = "operator-owned\n";
/// What the confined exec writes into the adopted directory, read back from the host side.
const ADOPTED_EXEC_WRITE: &str = "written-inside";
/// The adopted exec's own exit code, chosen so a default 0 or a confinement failure cannot pass.
const ADOPTED_EXEC_EXIT: i64 = 3;

/// `workspace.adopt` through the shipped daemon: an operator's directory is adopted, its
/// out-of-set neighbour is refused before dispatch, an exec runs in it (delegated) or is refused
/// by the same named refusal as a created workspace's (portable), the isolation cases run in it
/// (delegated), and destroying it leaves the operator's files.
#[allow(clippy::too_many_lines)] // One sequential journey; splitting it would lose the ordering.
async fn check_adopted_workspace(
    daemon: &Daemon,
    snapshot: &str,
    cgroup_root: Option<&Path>,
) -> usize {
    let mut passed = 0;
    let directory = daemon.workspaces.join(ADOPTED_ROOT_NAME);
    std::fs::create_dir(&directory).expect("place the operator directory");
    std::fs::write(directory.join("marker.txt"), ADOPTED_MARKER).expect("place the marker");
    let hidden = daemon.workspaces.join(OUT_OF_SET_ROOT_NAME);
    std::fs::create_dir(&hidden).expect("place the out-of-set directory");

    // Adopted: a minted `ws_` id, the root name and origin published, no lease, and the operator's
    // own file served through the workspace's file route.
    let adopt_input =
        json!({ "root_name": ADOPTED_ROOT_NAME, "labels": { "runner": "cleanroom" } });
    let (status, adopted) = daemon
        .call(
            "POST",
            "/v1/workspaces/adopt",
            "req_clean_adopt",
            Some(&mutation("01JADOPTCLEANADOPT000001", &adopt_input)),
        )
        .await;
    assert_eq!(status, 201, "{adopted}");
    let workspace = &text(&adopted["result"]["id"]);
    assert!(workspace.starts_with("ws_"), "{adopted}");
    assert_ne!(workspace, ADOPTED_ROOT_NAME);
    assert_eq!(adopted["result"]["origin"], "adopted", "{adopted}");
    assert_eq!(
        adopted["result"]["root_name"], ADOPTED_ROOT_NAME,
        "{adopted}"
    );
    assert_eq!(adopted["result"]["state"], "ready", "{adopted}");
    assert!(adopted["result"].get("lease").is_none(), "{adopted}");
    let (status, observed) = daemon
        .call(
            "GET",
            &format!("/v1/workspaces/{workspace}"),
            "req_clean_adopt_get",
            None,
        )
        .await;
    assert_eq!(status, 200, "{observed}");
    assert_eq!(observed["result"]["origin"], "adopted", "{observed}");
    assert_eq!(observed["result"]["root_name"], ADOPTED_ROOT_NAME);
    let (status, marker) = daemon
        .call(
            "GET",
            &format!(
                "/v1/workspaces/{workspace}/files/marker.txt?mode=file&offset=0&limit_bytes=64"
            ),
            "req_clean_adopt_marker",
            None,
        )
        .await;
    assert_eq!(status, 200, "{marker}");
    assert_eq!(
        BASE64
            .decode(text(&marker["result"]["content"]["data"]))
            .expect("base64 file content"),
        ADOPTED_MARKER.as_bytes()
    );
    passed += 1;

    // Out of the published root-name set: refused by the input schema before dispatch, with a
    // directory really there, recorded in the ledger naming no resource, and replayed.
    for (index, name) in [OUT_OF_SET_ROOT_NAME, "../adopted-repo", "-adopted"]
        .into_iter()
        .enumerate()
    {
        let operation = format!("01JADOPTCLEANOUTOFSET{index:03}");
        let input = json!({ "root_name": name, "labels": {} });
        let (status, refused) = daemon
            .call(
                "POST",
                "/v1/workspaces/adopt",
                &format!("req_clean_adopt_out_of_set_{index}"),
                Some(&mutation(&operation, &input)),
            )
            .await;
        assert_eq!(status, 422, "{name:?} {refused}");
        assert_eq!(
            refused["error"]["code"], "request.schema-invalid",
            "{name:?}"
        );
        assert_eq!(refused["error"]["address"], "root_name", "{name:?}");
        assert_eq!(
            refused["error"]["operation"],
            operation.as_str(),
            "{name:?}"
        );
        let (status, replayed) = daemon
            .call(
                "POST",
                "/v1/workspaces/adopt",
                &format!("req_clean_adopt_out_of_set_replay_{index}"),
                Some(&mutation(&operation, &input)),
            )
            .await;
        assert_eq!(status, 422, "{name:?} {replayed}");
        assert_eq!(replayed["error"], refused["error"], "{name:?}");
        let (status, record) = daemon
            .call(
                "GET",
                &format!("/v1/ops/{operation}"),
                &format!("req_clean_adopt_out_of_set_record_{index}"),
                None,
            )
            .await;
        assert_eq!(status, 200, "{name:?} {record}");
        assert_eq!(record["result"]["state"], "refused", "{name:?} {record}");
        assert_eq!(record["result"]["operation_kind"], "workspace.adopt");
        assert!(record["result"]["resource"].is_null(), "{name:?} {record}");
    }
    assert!(hidden.is_dir(), "the out-of-set directory was touched");
    passed += 1;

    // The root name is not a workspace id: an exec addressing the directory by it is refused by
    // the published `workspace-id` pattern before dispatch, in every lane.
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_adopt_exec_by_name",
                Some(&mutation(
                    "01JADOPTCLEANEXECBYNAME1",
                    &ExecInput::new(ADOPTED_ROOT_NAME, snapshot, &["/usr/bin/true"], true).build(),
                )),
            )
            .await,
        422,
        "request.schema-invalid",
    );
    passed += 1;

    passed += check_path_escape(daemon, workspace, WorkspaceOrigin::Adopted).await;

    let exec_script = format!(
        "cat /workspace/marker.txt; printf {ADOPTED_EXEC_WRITE} > /workspace/from-exec.txt; \
         exit {ADOPTED_EXEC_EXIT}"
    );
    let exec_input = ExecInput::new(
        workspace,
        snapshot,
        &["/usr/bin/sh", "-c", exec_script.as_str()],
        true,
    )
    .build();
    if let Some(cgroup_root) = cgroup_root {
        // The exec runs confined in the operator's directory: it reads the operator's file, its
        // write lands in that directory on the host, and its own exit code is observed.
        let (status, executed) = daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_adopt_exec",
                Some(&mutation("01JADOPTCLEANEXEC0000001", &exec_input)),
            )
            .await;
        assert_eq!(status, 200, "{executed}");
        assert_eq!(executed["result"]["state"], "exited", "{executed}");
        assert_eq!(
            executed["result"]["exit"],
            json!({ "code": ADOPTED_EXEC_EXIT, "signal": null }),
            "{executed}"
        );
        assert_eq!(executed["result"]["applied"]["network"], "none");
        let output = read_output(daemon, &text(&executed["result"]["id"]), "stdout").await;
        assert_eq!(decoded(&output), ADOPTED_MARKER.as_bytes());
        assert_eq!(output["eof"], true);
        assert_eq!(
            std::fs::read_to_string(directory.join("from-exec.txt"))
                .expect("the exec's write in the operator directory"),
            ADOPTED_EXEC_WRITE
        );
        passed += 1;

        passed += check_confined_isolation(
            daemon,
            workspace,
            snapshot,
            cgroup_root,
            WorkspaceOrigin::Adopted,
        )
        .await;
    } else {
        // No verified sandbox: the adopted workspace gets the same named refusal a created one
        // does, never an unconfined run in the operator's directory (invariant 3).
        //
        // This exec observes nothing about adoption, and cannot: the daemon decides
        // `exec.sandbox-unavailable` in `validate_exec_input`, before `admit_workspace` reads the
        // workspace row (`crates/substrate-daemon/src/app/execs.rs`), so no exec answer on this
        // lane depends on which workspace it names. That ranking is asserted rather than assumed:
        // an id no adopt ever minted gets the same refusal. What this lane does observe about
        // adoption is the record above (minted `ws_` id, `origin`, `root_name`, the operator's
        // file served) and destroy below; an exec in the adopted directory, and a changed
        // directory refused before any mount, are the delegated lane's and the host crate's
        // (`an_adopted_directory_changed_after_adopt_is_refused_at_exec_before_any_mount`).
        let mut never_adopted = exec_input.clone();
        never_adopted["workspace"] = json!("ws_neveradopted");
        expect_error(
            &daemon
                .call(
                    "POST",
                    "/v1/execs",
                    "req_clean_adopt_exec_unknown",
                    Some(&mutation("01JADOPTCLEANEXECUNKNOWN", &never_adopted)),
                )
                .await,
            501,
            "exec.sandbox-unavailable",
        );
        expect_error(
            &daemon
                .call(
                    "POST",
                    "/v1/execs",
                    "req_clean_adopt_exec",
                    Some(&mutation("01JADOPTCLEANEXEC0000001", &exec_input)),
                )
                .await,
            501,
            "exec.sandbox-unavailable",
        );
        assert!(!directory.join("from-exec.txt").exists());
        passed += 1;
    }

    // Destroy forgets the record and leaves the operator's directory and files (ADR 0035
    // decision 7).
    let (status, destroyed) = daemon
        .call(
            "DELETE",
            &format!("/v1/workspaces/{workspace}"),
            "req_clean_adopt_destroy",
            Some(&mutation("01JADOPTCLEANDESTROY0001", &json!({}))),
        )
        .await;
    assert_eq!(status, 200, "{destroyed}");
    assert_eq!(destroyed["result"]["absent"], true, "{destroyed}");
    assert_eq!(
        std::fs::read_to_string(directory.join("marker.txt")).expect("the marker survives"),
        ADOPTED_MARKER
    );
    let (status, gone) = daemon
        .call(
            "GET",
            &format!("/v1/workspaces/{workspace}"),
            "req_clean_adopt_gone",
            None,
        )
        .await;
    assert_eq!(status, 404, "{gone}");
    passed += 1;
    passed
}

// ---------------------------------------------------------------------------------------------
// The delegated lane: egress apertures
// ---------------------------------------------------------------------------------------------

/// What the model-free fake app-server answers. Not a model and not a protocol: bytes, so the case
/// proves reach and never somebody's API.
const APP_SERVER_BODY: &[u8] = b"substrate-aperture-served";

/// A listener that answers every connection with [`APP_SERVER_BODY`] and nothing else.
async fn fake_app_server() -> (std::net::SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the fake app server");
    let address = listener.local_addr().expect("app server address");
    let handle = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(APP_SERVER_BODY).await;
            let _ = stream.flush().await;
        }
    });
    (address, handle)
}

/// One run, three connects: the aperture serves, and two destinations outside it do not.
///
/// The same installed aperture answers all three, so "reachable" and "unreachable" are proven
/// against one configuration rather than two (design 10 § 7).
fn aperture_program(inside: u16, outside: u16) -> String {
    format!(
        "\nimport socket\n\
         served = socket.create_connection(('127.0.0.1', {inside}), 3).recv(64)\n\
         assert served == b'substrate-aperture-served', served\n\
         for target in (('127.0.0.1', {outside}), ('1.1.1.1', 443)):\n\
         \x20   try:\n\
         \x20       socket.create_connection(target, 3)\n\
         \x20   except OSError:\n\
         \x20       continue\n\
         \x20   raise SystemExit('reached ' + repr(target))\n\
         print('served')\n"
    )
}

/// The delegated aperture cases: reach, refusal of everything else, and the observation.
async fn check_confined_apertures(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    inside: u16,
    outside: u16,
) -> usize {
    let mut passed = 0;
    let program = aperture_program(inside, outside);
    let (status, run) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_aperture",
            Some(&mutation(
                "01JPHASE2CLEANAPERTURE1",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", &program],
                    true,
                )
                .aperture("model")
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["result"]["state"], "exited", "{run}");
    assert_eq!(
        run["result"]["exit"]["code"], 0,
        "the declared destination was not reachable, or one outside it was: {run}"
    );
    passed += 1;

    // Applied, not requested: the name the operator declared and the address it was pinned to.
    let applied = &run["result"]["applied"]["network"];
    assert_eq!(applied["mode"], "aperture", "{run}");
    assert_eq!(applied["name"], "model", "{run}");
    assert_eq!(applied["mechanism"], "loopback-forwarder", "{run}");
    assert_eq!(
        applied["destination"],
        format!("127.0.0.1:{inside}"),
        "the observation is not the pinned destination: {run}"
    );
    assert!(
        applied["bytes"]["from_destination"]
            .as_u64()
            .expect("byte accounting")
            >= APP_SERVER_BODY.len() as u64,
        "the forwarder counted no bytes: {run}"
    );
    let id = text(&run["result"]["id"]);
    let (status, recorded) = daemon
        .call(
            "GET",
            &format!("/v1/execs/{id}"),
            "req_clean_aperture_get",
            None,
        )
        .await;
    assert_eq!(status, 200, "{recorded}");
    assert_eq!(recorded["result"]["applied"]["network"]["name"], "model");
    passed += 1;

    // A name this deployment never declared, named in the refusal.
    let (status, undeclared) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_aperture_undeclared",
            Some(&mutation(
                "01JPHASE2CLEANAPERTURE2",
                &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                    .aperture("registry")
                    .build(),
            )),
        )
        .await;
    assert_eq!(status, 501, "{undeclared}");
    assert_eq!(undeclared["error"]["code"], "exec.aperture-undeclared");
    assert!(
        text(&undeclared["error"]["message"]).contains("registry"),
        "the refusal did not name the aperture: {undeclared}"
    );
    passed += 1;
    passed
}

// ---------------------------------------------------------------------------------------------
// The delegated lane: the declared byte ceiling (ADR 0014)
// ---------------------------------------------------------------------------------------------

/// What the firehose offers one connection. Far more than the declared ceiling, so a run that
/// stops short of it stopped because substrate stopped it and not because the stream ran out.
const FIREHOSE_BYTES: u64 = 1 << 20;
/// The ceiling `capped` carries, and the one `uncapped` does not. A whole number of relay buffers.
const CEILING_BYTES: u64 = 128 * 1024;
/// The apertures the ceiling cases use: one destination, two declarations, one term between them.
const CAPPED_APERTURE: &str = "capped";
const UNCAPPED_APERTURE: &str = "uncapped";

/// A destination that answers every connection with [`FIREHOSE_BYTES`] bytes and then closes.
async fn fake_firehose_server() -> (std::net::SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the firehose");
    let address = listener.local_addr().expect("firehose address");
    let handle = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let chunk = vec![b'x'; 65_536];
                let mut sent = 0_u64;
                while sent < FIREHOSE_BYTES {
                    // A relay that stopped is a closed socket here: that is the ceiling holding,
                    // so the write error ends this connection and nothing else.
                    if stream.write_all(&chunk).await.is_err() {
                        break;
                    }
                    sent += chunk.len() as u64;
                }
                let _ = stream.flush().await;
            });
        }
    });
    (address, handle)
}

/// Reads the aperture until the whole stream has arrived, reconnecting when it has not.
///
/// The child is told nothing: it sees a stream that ends early and tries again, exactly as a
/// harness would (ADR 0014, "the child gets a closed socket"). Under a ceiling it never reaches
/// `expect` and the parent ends the run; without one it reaches it on the first connection and
/// exits 0.
fn firehose_program(port: u16, expect: u64) -> String {
    format!(
        "\nimport socket, time\n\
         total = 0\n\
         while total < {expect}:\n\
         \x20   try:\n\
         \x20       link = socket.create_connection(('127.0.0.1', {port}), 3)\n\
         \x20   except OSError:\n\
         \x20       time.sleep(0.05)\n\
         \x20       continue\n\
         \x20   while True:\n\
         \x20       chunk = link.recv(65536)\n\
         \x20       if not chunk:\n\
         \x20           break\n\
         \x20       total += len(chunk)\n\
         \x20   link.close()\n\
         \x20   time.sleep(0.05)\n\
         print(total)\n"
    )
}

/// The two halves of ADR 0014's acceptance, against one destination and one program.
///
/// The ceiling refuses the run by name, and the same declaration without the term passes the same
/// traffic to completion — which is what "an aperture declared without the term keeps working byte
/// for byte" has to mean if it means anything.
async fn check_confined_aperture_ceiling(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    firehose: u16,
) -> usize {
    let mut passed = 0;
    let program = firehose_program(firehose, FIREHOSE_BYTES);
    let (status, run) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_aperture_capped",
            Some(&mutation(
                "01JPHASE2CLEANCEILING01",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", &program],
                    true,
                )
                .aperture(CAPPED_APERTURE)
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{run}");
    // The state a mid-run bound has always had, and the name it never had before this bundle.
    assert_eq!(run["result"]["state"], "cancelled", "{run}");
    assert_eq!(run["result"]["refusal"]["class"], "exhausted", "{run}");
    assert_eq!(
        run["result"]["refusal"]["code"], "exec.aperture-byte-limit",
        "the run was ended without naming the bound it hit: {run}"
    );
    passed += 1;

    // Reported, not inferred: the ceiling the run ran under, beside the bytes that crossed.
    let applied = &run["result"]["applied"]["network"];
    assert_eq!(applied["name"], CAPPED_APERTURE, "{run}");
    assert_eq!(applied["max_bytes"], CEILING_BYTES, "{run}");
    let crossed = applied["bytes"]["to_destination"].as_u64().expect("bytes")
        + applied["bytes"]["from_destination"]
            .as_u64()
            .expect("bytes");
    assert!(
        crossed >= CEILING_BYTES,
        "the run was ended before the declared ceiling: {run}"
    );
    assert!(
        crossed < FIREHOSE_BYTES,
        "the whole stream crossed a bounded aperture: {run}"
    );
    passed += 1;

    // The negative. Same destination, same program, no declared term: nothing is stopped, nothing
    // is named, and the observation is the one `0.7.0` already produced.
    let (status, open) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_aperture_uncapped",
            Some(&mutation(
                "01JPHASE2CLEANCEILING02",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", &program],
                    true,
                )
                .aperture(UNCAPPED_APERTURE)
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{open}");
    assert_eq!(open["result"]["state"], "exited", "{open}");
    assert_eq!(open["result"]["exit"]["code"], 0, "{open}");
    assert!(
        open["result"].get("refusal").is_none(),
        "an aperture with no declared ceiling named a bound: {open}"
    );
    let applied = &open["result"]["applied"]["network"];
    assert!(
        applied.get("max_bytes").is_none(),
        "an aperture declared without a ceiling published one: {open}"
    );
    assert_eq!(
        applied["bytes"]["from_destination"]
            .as_u64()
            .expect("byte accounting"),
        FIREHOSE_BYTES,
        "the same traffic did not cross an aperture with no ceiling: {open}"
    );
    passed += 1;
    passed
}

// ---------------------------------------------------------------------------------------------
// The delegated lane: sealed secret slots
// ---------------------------------------------------------------------------------------------

/// The slot this deployment declares, and the descriptor a start asks for it at.
const SECRET_SLOT_NAME: &str = "vendor_api_key";
const SECRET_SLOT_FD: u32 = 7;

/// The declared value, and the one string here that must appear in nothing the daemon emits.
///
/// An obvious synthetic — this repository is public, so anything that read like a real credential
/// would be the leak these cases exist to refuse — and high-entropy, so a substring hit in a
/// captured surface is a finding and never a coincidence.
const SECRET_SLOT_VALUE: &str = "not-a-credential-4f2c9a17b6d84e05c3719ad0e46b8f29";

/// Set on the daemon process and on nothing else, so a child that can read it has been handed an
/// environment substrate did not shape. [`SECRET_SLOT_PROGRAM`] repeats the name as a literal,
/// because `concat!` takes literals only; a case asserts the two still agree.
const DAEMON_ONLY_VARIABLE: &str = "SUBSTRATE_TEST_SECRET_SENTINEL";

/// What the confined child reports about its slot: a digest and observations, never the bytes.
///
/// The child is the only observer holding the value, so it is the only one that can search the
/// surfaces only it can see — its own `/proc/self/cmdline` and `/proc/self/environ`. It reports the
/// *position* of a hit, so a leak fails the case without the case printing what leaked.
const SECRET_SLOT_PROGRAM: &str = concat!(
    "import fcntl,hashlib,json,os\n",
    "mapping=os.environ['SUBSTRATE_SECRET_SLOTS']\n",
    "name,number=mapping.split(',')[0].split('=')\n",
    "fd=int(number)\n",
    "value=b''\n",
    "while True:\n",
    "    chunk=os.read(fd,4096)\n",
    "    if not chunk:\n",
    "        break\n",
    "    value+=chunk\n",
    "seals=fcntl.fcntl(fd,fcntl.F_GET_SEALS)\n",
    "try:\n",
    "    os.pwrite(fd,b'x',0)\n",
    "    write_errno=0\n",
    "except OSError as error:\n",
    "    write_errno=error.errno\n",
    "held=[]\n",
    "for entry in os.listdir('/proc/self/fd'):\n",
    "    try:\n",
    "        target=os.readlink('/proc/self/fd/'+entry)\n",
    "    except OSError:\n",
    "        continue\n",
    "    if target.endswith('/fd'):\n",
    "        continue\n",
    "    held.append((int(entry),target))\n",
    "held.sort()\n",
    "print('SLOTREPORT '+json.dumps({\n",
    "    'slot':name,'fd':fd,'mapping':mapping,\n",
    "    'digest':hashlib.sha256(value).hexdigest(),\n",
    "    'seals':seals,'write_errno':write_errno,\n",
    "    'link':os.readlink('/proc/self/fd/%d'%fd),\n",
    "    'fds':[n for n,_ in held],\n",
    "    'memfds':[n for n,t in held if 'memfd:' in t],\n",
    "    'argv_leak':open('/proc/self/cmdline','rb').read().find(value),\n",
    "    'environ_leak':open('/proc/self/environ','rb').read().find(value),\n",
    "    'daemon_variable':'SUBSTRATE_TEST_SECRET_SENTINEL' in os.environ,\n",
    "}))\n",
);

/// The `SLOTREPORT` line the confined child printed.
fn slot_report(stdout: &[u8]) -> Value {
    let captured = String::from_utf8_lossy(stdout);
    let line = captured
        .lines()
        .find_map(|line| line.strip_prefix("SLOTREPORT "))
        .unwrap_or_else(|| panic!("the confined child reported nothing: {captured}"));
    serde_json::from_str(line).expect("the child's report is JSON")
}

/// Lowercase hex SHA-256, so a case can name a value's digest without naming the value.
fn sha256_hex(value: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(value)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").expect("format one digest byte");
            hex
        })
}

/// Polls one exec until it leaves `running`, and refuses to wait forever.
async fn await_exec(daemon: &Daemon, exec_id: &str, request_id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (status, observed) = daemon
            .call("GET", &format!("/v1/execs/{exec_id}"), request_id, None)
            .await;
        assert_eq!(status, 200, "{observed}");
        if observed["result"]["state"] != "running" {
            return observed;
        }
        assert!(Instant::now() < deadline, "{exec_id} never left running");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The delegated secret-slot cases (ADR 0012), proved on the wire against the shipped binary.
#[allow(clippy::too_many_lines)] // One sequential journey; splitting it would lose the ordering.
async fn check_confined_secret_slots(daemon: &Daemon, workspace: &str, snapshot: &str) -> usize {
    let mut passed = 0;
    assert!(
        SECRET_SLOT_PROGRAM.contains(DAEMON_ONLY_VARIABLE),
        "the child looks for a daemon variable this harness no longer sets"
    );

    let operation = "01JPHASE2CLEANSLOT00001";
    let (status, run) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_secret_slot",
            Some(&mutation(
                operation,
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", SECRET_SLOT_PROGRAM],
                    true,
                )
                .secret_slot(SECRET_SLOT_NAME, SECRET_SLOT_FD)
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{run}");
    let exec_id = text(&run["result"]["id"]);
    let stdout = decoded(&read_output(daemon, &exec_id, "stdout").await);
    let stderr = decoded(&read_output(daemon, &exec_id, "stderr").await);
    assert_eq!(run["result"]["state"], "exited", "{run}");
    assert_eq!(
        run["result"]["exit"],
        EXITED_CLEANLY(),
        "the confined child failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    let report = slot_report(&stdout);

    // The child read the declared bytes off the declared descriptor. A digest, so the proof of a
    // successful read is not itself a copy of the value.
    assert_eq!(report["slot"], SECRET_SLOT_NAME, "{report}");
    assert_eq!(report["fd"], SECRET_SLOT_FD, "{report}");
    assert_eq!(
        report["mapping"],
        format!("{SECRET_SLOT_NAME}={SECRET_SLOT_FD}"),
        "the shaped environment carries something other than the mapping: {report}"
    );
    assert_eq!(
        report["digest"],
        sha256_hex(SECRET_SLOT_VALUE.as_bytes()),
        "the child did not read the declared value from its declared descriptor: {report}"
    );
    assert_eq!(
        run["result"]["applied"]["secret_slots"],
        json!([{ "slot": SECRET_SLOT_NAME, "fd": SECRET_SLOT_FD }]),
        "the applied record does not name the slot that was placed: {run}"
    );
    passed += 1;

    // The seal set is exactly ADR 0012's, read back inside the sandbox rather than claimed outside
    // it, and the descriptor is the named anonymous memfd and the only one the child holds.
    assert_eq!(
        report["seals"], 0xf,
        "the child reads back a seal set that is not F_SEAL_WRITE|SHRINK|GROW|SEAL: {report}"
    );
    assert_eq!(
        report["write_errno"],
        nix::errno::Errno::EPERM as i32,
        "a sealed slot accepted a write: {report}"
    );
    assert!(
        text(&report["link"]).contains(&format!("memfd:substrate-slot-{SECRET_SLOT_NAME}")),
        "the descriptor is not the named anonymous memfd: {report}"
    );
    assert_eq!(
        report["memfds"],
        json!([SECRET_SLOT_FD]),
        "the child holds a memfd that is not its declared slot: {report}"
    );
    // Exactly `{0,1,2} ∪ {declared}` — bubblewrap adds none of its own on the far side
    // (`docs/design/11-sealed-secret-slots.md` § 6), so a second start's slot cannot be here.
    assert_eq!(
        report["fds"],
        json!([0, 1, 2, SECRET_SLOT_FD]),
        "the child holds descriptors beyond stdio and its declared slot: {report}"
    );
    passed += 1;

    // A name this deployment never declared, refused by name and never by material.
    let (status, unknown) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_secret_slot_unknown",
            Some(&mutation(
                "01JPHASE2CLEANSLOT00002",
                &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                    .secret_slot("absent_slot", SECRET_SLOT_FD)
                    .build(),
            )),
        )
        .await;
    assert_eq!(status, 422, "{unknown}");
    assert_eq!(
        unknown["error"]["code"], "exec.secret-slot-unknown",
        "{unknown}"
    );
    assert!(
        text(&unknown["error"]["message"]).contains("absent_slot"),
        "the refusal did not name the slot: {unknown}"
    );
    passed += 1;

    // Every surface this run produced, searched for the value. Each is checked for a hit *and* for
    // being the surface it claims to be, because an empty page carries no value either.
    let (status, recorded) = daemon
        .call(
            "GET",
            &format!("/v1/execs/{exec_id}"),
            "req_clean_secret_slot_get",
            None,
        )
        .await;
    assert_eq!(status, 200, "{recorded}");
    let (status, ledger) = daemon
        .call(
            "GET",
            &format!("/v1/ops/{operation}"),
            "req_clean_secret_slot_op",
            None,
        )
        .await;
    assert_eq!(status, 200, "{ledger}");
    assert_eq!(ledger["result"]["resource"], exec_id, "{ledger}");
    let (status, events) = daemon
        .call(
            "GET",
            "/v1/events?limit=100",
            "req_clean_secret_slot_events",
            None,
        )
        .await;
    assert_eq!(status, 200, "{events}");
    let events = events.to_string();
    assert!(
        events.contains(&exec_id),
        "the event page does not cover the run, so finding nothing in it proves nothing"
    );
    let cmdline = daemon.cmdline();
    assert!(
        cmdline.contains("--secret-slot"),
        "this is not the argv of a daemon that declares a slot: {cmdline}"
    );
    for (surface, bytes) in [
        ("the exec response", run.to_string()),
        ("the recorded exec", recorded.to_string()),
        ("the ledger row", ledger.to_string()),
        ("the event page", events),
        ("the refusal body", unknown.to_string()),
        (
            "captured stdout",
            String::from_utf8_lossy(&stdout).into_owned(),
        ),
        (
            "captured stderr",
            String::from_utf8_lossy(&stderr).into_owned(),
        ),
        ("the daemon's argv", cmdline),
    ] {
        assert!(
            !bytes.contains(SECRET_SLOT_VALUE),
            "{surface} carries the declared value"
        );
    }
    // The two surfaces only the child can see, reported as positions rather than substrings.
    assert_eq!(
        report["argv_leak"], -1,
        "the child's argv carries the value"
    );
    assert_eq!(
        report["environ_leak"], -1,
        "the child's environment carries the value"
    );
    assert_eq!(
        report["daemon_variable"], false,
        "the child inherited the daemon's environment instead of a shaped one"
    );
    passed += 1;

    // The daemon has already let go — checked while the child is still running and has not yet
    // read, so the descriptor it later reads from is demonstrably not a copy the daemon held open.
    let deferred = format!("import time\ntime.sleep(2)\n{SECRET_SLOT_PROGRAM}");
    let (status, started) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_secret_slot_closed",
            Some(&mutation(
                "01JPHASE2CLEANSLOT00003",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", &deferred],
                    false,
                )
                .secret_slot(SECRET_SLOT_NAME, SECRET_SLOT_FD)
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 202, "{started}");
    let deferred_id = text(&started["result"]["id"]);
    let (status, running) = daemon
        .call(
            "GET",
            &format!("/v1/execs/{deferred_id}"),
            "req_clean_secret_slot_running",
            None,
        )
        .await;
    assert_eq!(status, 200, "{running}");
    assert_eq!(
        running["result"]["state"], "running",
        "the deferred child was gone before the daemon's descriptors could be read: {running}"
    );
    assert_eq!(
        daemon.held_slot_memfds(),
        Vec::<String>::new(),
        "the daemon still holds a slot memfd after spawn"
    );
    let finished = await_exec(daemon, &deferred_id, "req_clean_secret_slot_closed_get").await;
    assert_eq!(finished["result"]["state"], "exited", "{finished}");
    let deferred_stdout = decoded(&read_output(daemon, &deferred_id, "stdout").await);
    assert_eq!(
        slot_report(&deferred_stdout)["digest"],
        sha256_hex(SECRET_SLOT_VALUE.as_bytes()),
        "the child could not read the value the daemon had already let go of"
    );
    passed += 1;
    passed
}

// ---------------------------------------------------------------------------------------------
// Delegated context and grant attribution (ADR 0011)
// ---------------------------------------------------------------------------------------------
//
// Verification is pure computation over the presented bytes and one configured trusted key, so
// every case here runs in the **portable** lane: no cgroup, no delegation, no privilege. What they
// prove is what a reader of one ledger row can answer — which grant authorised this, on behalf of
// which platform principal — from the shipped binary's own record, over the wire, with nothing of
// the implementation linked.

/// The issuer origin the clean-room key vouches for. RFC 6761 reserved: it names nothing on any
/// network, and nothing here ever calls it — substrate resolves no issuer during a request.
const DELEGATED_ISSUER: &str = "https://issuer.invalid";
const DELEGATED_KID: &str = "cleanroom-key-1";
const DELEGATED_GRANT: &str = "grant:observability-read";
const DELEGATED_PLATFORM_PRINCIPAL: &str = "platform:principal-cleanroom";

/// A test-only signing key, derived from a literal English sentence rather than committed material.
///
/// Substrate never signs a delegated context — it holds a *verifying* key and nothing else — so the
/// signer only exists to stand in for whichever service the deployment configures. Deriving the
/// seed from this sentence means the repository carries no key blob to leak or rotate, and the
/// runtime cases can still mint documents bound to *this* machine's subject and *this* instant,
/// which a committed fixture can never be.
fn delegated_signing_key() -> SigningKey {
    let seed: [u8; 32] =
        sha2::Sha256::digest(b"substrate clean-room delegated-context signing seed").into();
    SigningKey::from_bytes(&seed)
}

/// The `--delegated-context-key kid=issuer=base64url` declaration for that key.
fn delegated_key_flag() -> String {
    format!(
        "{DELEGATED_KID}={DELEGATED_ISSUER}={}",
        BASE64URL.encode(delegated_signing_key().verifying_key().as_bytes())
    )
}

/// The subject the daemon derives from kernel peer credentials, which is what a document binds to.
///
/// Read from the running process rather than written down: the binding a case presents has to be
/// the binding the daemon will actually compute, or the case proves the wrong thing.
fn clean_room_subject() -> String {
    format!("local:{}", nix::unistd::getuid().as_raw())
}

/// The closed claim set of design 09 § 3, as JSON, ready to be perturbed by one member.
fn delegated_claims() -> Value {
    let now = chrono::Utc::now().timestamp();
    json!({
        "act": { "sub": "svc:cleanroom-actor" },
        "aud": "urn:b10x:substrate",
        "bound_deployment": "dep_cleanroom",
        "bound_subject": clean_room_subject(),
        "exp": now + 120,
        "grant_ref": DELEGATED_GRANT,
        "grant_revision": "rev_00000000000000000007",
        "iat": now - 10,
        "iss": DELEGATED_ISSUER,
        "jti": "jti_cleanroom_0000000001",
        "nbf": now - 10,
        "sub": DELEGATED_PLATFORM_PRINCIPAL,
        "tenant": "tenant_cleanroom",
    })
}

/// One compact JWS: `base64url(header).base64url(claims).base64url(signature)`.
fn delegated_context(claims: &Value) -> String {
    let header = json!({
        "alg": "EdDSA",
        "kid": DELEGATED_KID,
        "typ": "substrate-delegated-context+jwt",
    });
    let signing_input = format!(
        "{}.{}",
        BASE64URL.encode(serde_json::to_vec(&header).expect("header JSON")),
        BASE64URL.encode(serde_json::to_vec(claims).expect("claims JSON"))
    );
    let signature = delegated_signing_key().sign(signing_input.as_bytes());
    format!("{signing_input}.{}", BASE64URL.encode(signature.to_bytes()))
}

/// A mutation envelope carrying the optional third member beside `op` and `input`.
fn attributed_mutation(operation: &str, input: &Value, context: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "delegated_context": context,
        "input": input,
        "op": operation,
    }))
    .expect("attributed mutation JSON")
}

/// Starts one clean-room daemon that trusts [`delegated_key_flag`], for a case of its own.
async fn delegated_daemon(root: &Path, required: bool) -> Daemon {
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
        .expect("owner-private clean-room directory");
    Daemon::start(root, None, &[], &[], &[delegated_key_flag()], required).await
}

/// The ledger row `GET /v1/ops/{op}` reports, which is the row an `operation.*` event carries.
async fn ledger_row(daemon: &Daemon, operation: &str, request_id: &str) -> Value {
    let (status, row) = daemon
        .call("GET", &format!("/v1/ops/{operation}"), request_id, None)
        .await;
    assert_eq!(status, 200, "{row}");
    row["result"].clone()
}

/// The acceptance of `story:ledger-rows-carry-the-declared-grant`, on the wire.
async fn assert_verified_context_is_recorded(
    daemon: &Daemon,
    operation: &str,
    request_id: &str,
    labels: &Value,
) {
    let (status, created) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            request_id,
            Some(&attributed_mutation(
                operation,
                &json!({ "source": "empty", "labels": labels }),
                &delegated_context(&delegated_claims()),
            )),
        )
        .await;
    assert_eq!(status, 201, "{created}");

    let row = ledger_row(daemon, operation, &format!("{request_id}_op")).await;
    assert_eq!(
        row["grant_ref"], DELEGATED_GRANT,
        "the ledger row does not carry the declared grant: {row}"
    );
    assert_eq!(
        row["platform_principal"], DELEGATED_PLATFORM_PRINCIPAL,
        "the ledger row does not carry the platform principal: {row}"
    );
    // The existing column keeps its meaning: the process id is not the platform principal, and
    // collapsing them is the confusion design 06 § 2 forbids.
    assert_ne!(
        row["principal"], row["platform_principal"],
        "principal was reused for the platform principal: {row}"
    );
    assert!(
        row["principal"]
            .as_str()
            .is_some_and(|value| value.starts_with("pid:")),
        "principal stopped being the calling process id: {row}"
    );

    // The same two members reach the `operation.*` events, because an operation transition's
    // observation *is* this row (`crates/substrate-store/src/events.rs`).
    let (status, events) = daemon
        .call(
            "GET",
            "/v1/events?limit=100",
            &format!("{request_id}_ev"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{events}");
    let attributed: Vec<&Value> = events["result"]["items"]
        .as_array()
        .expect("event page items")
        .iter()
        .filter(|event| {
            event["cause"]["operation"] == operation
                && text(&event["transition"]).starts_with("operation.")
        })
        .collect();
    assert!(
        !attributed.is_empty(),
        "no operation.* event for {operation}: {events}"
    );
    for event in attributed {
        assert_eq!(
            event["observation"]["grant_ref"], DELEGATED_GRANT,
            "an operation event carries no grant: {event}"
        );
        assert_eq!(
            event["observation"]["platform_principal"], DELEGATED_PLATFORM_PRINCIPAL,
            "an operation event carries no platform principal: {event}"
        );
    }
}

/// Design 06 § 2: caller-written identity strings are not trusted. Two ways of writing one.
async fn assert_caller_written_identity_is_ignored(
    daemon: &Daemon,
    operation: &str,
    request_id: &str,
) {
    // 1. Identity-shaped strings the caller *is* allowed to write — workspace labels are free-form
    //    and are echoed back verbatim — reach the resource and never the attribution.
    let forged = json!({
        "grant_ref": "grant:forged-by-the-caller",
        "platform_principal": "platform:forged-by-the-caller",
    });
    assert_verified_context_is_recorded(daemon, operation, request_id, &forged).await;
    let row = ledger_row(daemon, operation, &format!("{request_id}_row")).await;
    assert_ne!(
        row["platform_principal"], "platform:forged-by-the-caller",
        "a caller-written label became the platform principal: {row}"
    );
    assert_ne!(
        row["grant_ref"], "grant:forged-by-the-caller",
        "a caller-written label became the grant: {row}"
    );

    // 2. Writing one into the envelope itself is refused, not ignored quietly: the request union
    //    stays closed around `op`, `input` and the one signed member.
    let mut envelope = json!({
        "delegated_context": delegated_context(&delegated_claims()),
        "input": { "source": "empty", "labels": {} },
        "op": "01JPHASE2DELEGWRITTEN1",
    });
    envelope["platform_principal"] = json!("platform:forged-by-the-caller");
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/workspaces",
                &format!("{request_id}_env"),
                Some(&serde_json::to_vec(&envelope).expect("envelope JSON")),
            )
            .await,
        422,
        "request.schema-invalid",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ledger_row_records_grant_ref_and_platform_principal() {
    let temporary = TempDir::with_prefix("substrate-delegated-record-").expect("case directory");
    let daemon = delegated_daemon(temporary.path(), false).await;
    assert_verified_context_is_recorded(
        &daemon,
        "01JPHASE2DELEGRECORD001",
        "req_delegated_record",
        &json!({ "runner": "cleanroom" }),
    )
    .await;

    // Omission is untouched: the same daemon, no context, and a row that says so rather than
    // guessing one (invariant 3 — absent, never optimistic).
    let (status, plain) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_delegated_none",
            Some(&mutation(
                "01JPHASE2DELEGRECORD002",
                &json!({ "source": "empty", "labels": {} }),
            )),
        )
        .await;
    assert_eq!(status, 201, "{plain}");
    let row = ledger_row(&daemon, "01JPHASE2DELEGRECORD002", "req_delegated_none_op").await;
    assert!(
        row.get("grant_ref").is_none() && row.get("platform_principal").is_none(),
        "an unattributed row invented an attribution: {row}"
    );
    daemon.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn caller_written_identity_is_ignored_when_context_is_verified() {
    let temporary = TempDir::with_prefix("substrate-delegated-ignore-").expect("case directory");
    let daemon = delegated_daemon(temporary.path(), false).await;
    assert_caller_written_identity_is_ignored(
        &daemon,
        "01JPHASE2DELEGIGNORE001",
        "req_delegated_ignore",
    )
    .await;
    daemon.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_delegated_context_is_refused_by_name_when_required() {
    let temporary = TempDir::with_prefix("substrate-delegated-required-").expect("case directory");
    let daemon = delegated_daemon(temporary.path(), true).await;
    let operation = "01JPHASE2DELEGABSENT001";
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/workspaces",
                "req_delegated_absent",
                Some(&mutation(
                    operation,
                    &json!({ "source": "empty", "labels": {} }),
                )),
            )
            .await,
        422,
        "delegated-context.absent",
    );

    // A named refusal, durable under the operation id — not a missing row (atlas O1).
    let row = ledger_row(&daemon, operation, "req_delegated_absent_op").await;
    assert_eq!(row["state"], "refused", "{row}");
    assert_eq!(row["outcome"]["error"]["code"], "delegated-context.absent");
    assert_eq!(row["outcome"]["error"]["address"], "delegated_context");
    assert!(
        row.get("grant_ref").is_none(),
        "a refused operation recorded a grant: {row}"
    );

    // The same deployment still serves a request that presents one.
    assert_verified_context_is_recorded(
        &daemon,
        "01JPHASE2DELEGABSENT002",
        "req_delegated_present",
        &json!({}),
    )
    .await;
    daemon.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn required_delegated_context_is_verified_before_a_successful_replay() {
    let temporary = TempDir::with_prefix("substrate-delegated-replay-").expect("case directory");
    let daemon = delegated_daemon(temporary.path(), true).await;
    let operation = "01JPHASE2DELEGREPLAY001";
    let input = json!({
        "source": "empty",
        "labels": { "case": "required-context-before-replay" }
    });
    let context = delegated_context(&delegated_claims());

    let (status, created) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_delegated_replay_create",
            Some(&attributed_mutation(operation, &input, &context)),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    let original_row = ledger_row(&daemon, operation, "req_delegated_replay_row").await;

    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/workspaces",
                "req_delegated_replay_absent",
                Some(&mutation(operation, &input)),
            )
            .await,
        422,
        "delegated-context.absent",
    );
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/workspaces",
                "req_delegated_replay_malformed",
                Some(&attributed_mutation(operation, &input, "not-a-compact-jws")),
            )
            .await,
        422,
        "delegated-context.malformed",
    );

    let (status, replayed) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_delegated_replay_valid",
            Some(&attributed_mutation(operation, &input, &context)),
        )
        .await;
    assert_eq!(status, 201, "{replayed}");
    assert_eq!(replayed["result"], created["result"]);
    assert_eq!(
        ledger_row(&daemon, operation, "req_delegated_replay_row_after").await,
        original_row,
        "replay authorization must not rewrite first-write attribution"
    );
    daemon.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn delegated_context_bound_to_another_subject_is_refused() {
    let temporary = TempDir::with_prefix("substrate-delegated-bound-").expect("case directory");
    let daemon = delegated_daemon(temporary.path(), false).await;

    // The binding runs one way. A correctly signed document naming another subject refuses; it
    // never re-subjects the request, because substrate's subject comes from kernel peer credentials
    // and never from HTTP data.
    let mut foreign = delegated_claims();
    foreign["bound_subject"] = json!("local:4242");
    let operation = "01JPHASE2DELEGBOUND0001";
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/workspaces",
                "req_delegated_bound",
                Some(&attributed_mutation(
                    operation,
                    &json!({ "source": "empty", "labels": {} }),
                    &delegated_context(&foreign),
                )),
            )
            .await,
        422,
        "delegated-context.subject-mismatch",
    );
    let row = ledger_row(&daemon, operation, "req_delegated_bound_op").await;
    assert_eq!(row["state"], "refused", "{row}");
    assert_eq!(
        row["actor"],
        format!("unix-peer:{}", nix::unistd::getuid().as_raw()),
        "the request was re-subjected to the document's subject: {row}"
    );
    assert!(
        row.get("grant_ref").is_none(),
        "a subject-bound refusal still recorded a grant: {row}"
    );

    // The same shape, bound to another *deployment*, is the same named refusal.
    let mut elsewhere = delegated_claims();
    elsewhere["bound_deployment"] = json!("dep_elsewhere");
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/workspaces",
                "req_delegated_deployment",
                Some(&attributed_mutation(
                    "01JPHASE2DELEGBOUND0002",
                    &json!({ "source": "empty", "labels": {} }),
                    &delegated_context(&elsewhere),
                )),
            )
            .await,
        422,
        "delegated-context.subject-mismatch",
    );
    daemon.close().await;
}

// ---------------------------------------------------------------------------------------------
// Declared read-only host files (ADR 0033)
// ---------------------------------------------------------------------------------------------

const READ_ONLY_FILE_MOUNT_DIRECTORY: &str = "/opt/gates/bin";
const READ_ONLY_FILE_MOUNT: &str = "/opt/gates/bin/gate";
const READ_ONLY_FILE_OUTPUT: &str = "gate ran from a single bound file";

fn read_only_file(host_path: &str, mount: &str) -> Value {
    json!({ "host_path": host_path, "mount": mount })
}

/// Shape before capability, in both lanes: every lexical rule is refused by name before the daemon
/// asks whether this host could bind a file at all.
async fn check_read_only_file_shape(daemon: &Daemon, workspace: &str, snapshot: &str) -> usize {
    let mut passed = 0;
    let program = "/opt/host/bin/gate";
    let over: Vec<Value> = (0..=16)
        .map(|index| read_only_file(program, &format!("/opt/many/{index}")))
        .collect();
    let root = json!([{ "host_path": "/opt/host", "mount": "/opt/gates" }]);
    for (operation, request, files, roots, code) in [
        (
            "01JROFILECLEANSHAPE0001",
            "req_clean_rofile_exceeded",
            Value::Array(over),
            None,
            "exec.read-only-files-exceeded",
        ),
        (
            "01JROFILECLEANSHAPE0002",
            "req_clean_rofile_path",
            json!([read_only_file(program, "opt/gates/bin/gate")]),
            None,
            "exec.read-only-file-path-invalid",
        ),
        (
            "01JROFILECLEANSHAPE0003",
            "req_clean_rofile_reserved",
            json!([read_only_file(program, "/workspace/gate")]),
            None,
            "exec.read-only-file-mount-reserved",
        ),
        (
            "01JROFILECLEANSHAPE0004",
            "req_clean_rofile_overlap",
            json!([read_only_file(program, READ_ONLY_FILE_MOUNT)]),
            Some(root),
            "exec.read-only-file-mount-overlap",
        ),
        (
            "01JROFILECLEANSHAPE0005",
            "req_clean_rofile_duplicate",
            json!([
                read_only_file(program, READ_ONLY_FILE_MOUNT),
                read_only_file("/opt/host/bin/other", READ_ONLY_FILE_MOUNT),
            ]),
            None,
            "exec.read-only-file-mount-duplicate",
        ),
    ] {
        let mut input = ExecInput::new(workspace, snapshot, &["gate"], true).read_only_files(files);
        if let Some(roots) = roots {
            input = input.read_only_roots(roots);
        }
        expect_error(
            &daemon
                .call(
                    "POST",
                    "/v1/execs",
                    request,
                    Some(&mutation(operation, &input.build())),
                )
                .await,
            422,
            code,
        );
        passed += 1;
    }
    passed
}

/// No proved mechanism, no fact, and a well-formed declaration is `unserved` by name — never a run
/// without the file (invariant 3).
async fn check_read_only_files_unserved(daemon: &Daemon, workspace: &str, snapshot: &str) -> usize {
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_clean_rofile_fact_absent", None)
        .await;
    assert_eq!(status, 200);
    assert!(
        machine["result"]["facts"]
            .get("exec.read-only-files")
            .is_none(),
        "a portable host publishes no read-only file fact: {machine}"
    );
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_rofile_unserved",
                Some(&mutation(
                    "01JROFILECLEANUNSERVED01",
                    &ExecInput::new(workspace, snapshot, &["gate"], true)
                        .read_only_files(json!([read_only_file(
                            "/opt/host/bin/gate",
                            READ_ONLY_FILE_MOUNT
                        )]))
                        .build(),
                )),
            )
            .await,
        501,
        "exec.read-only-files-unserved",
    );
    1
}

/// A host program beside a sibling, in a canonical directory under `CARGO_TARGET_TMPDIR`.
fn read_only_file_fixture() -> (TempDir, PathBuf) {
    let fixture = tempfile::Builder::new()
        .prefix("rofile-cleanroom-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("fixture under CARGO_TARGET_TMPDIR");
    let host = std::fs::canonicalize(fixture.path())
        .expect("canonical fixture")
        .join("bin");
    std::fs::create_dir(&host).expect("host program directory");
    let program = host.join("gate");
    std::fs::write(
        &program,
        format!("#!/bin/sh\necho '{READ_ONLY_FILE_OUTPUT}'\n"),
    )
    .expect("host program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("executable host program");
    std::fs::write(host.join("sibling"), "never visible inside\n").expect("sibling");
    (fixture, host)
}

/// The delegated journey: a host program bound at a mount on `PATH` runs by name and is listed in
/// the applied observation; it cannot be written and has no visible sibling; and a directory or a
/// symlinked path is refused by name before dispatch.
async fn check_confined_read_only_files(daemon: &Daemon, workspace: &str, snapshot: &str) -> usize {
    let mut passed = 0;
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_clean_rofile_fact", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        machine["result"]["facts"]["exec.read-only-files"],
        json!({ "max_files": 16 }),
        "a delegated host proves and publishes the bound: {machine}"
    );

    let (_fixture, host) = read_only_file_fixture();
    let program = host.join("gate");
    let program_text = program.to_str().expect("UTF-8 fixture").to_owned();
    let declared = json!([read_only_file(&program_text, READ_ONLY_FILE_MOUNT)]);
    let path = json!({ "PATH": format!("{READ_ONLY_FILE_MOUNT_DIRECTORY}:/usr/bin:/bin") });

    let (status, run) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_rofile_runs",
            Some(&mutation(
                "01JROFILECLEANRUNS00001",
                &ExecInput::new(workspace, snapshot, &["gate"], true)
                    .environment(path.clone())
                    .read_only_files(declared.clone())
                    .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["result"]["state"], "exited", "{run}");
    assert_eq!(run["result"]["exit"], EXITED_CLEANLY(), "{run}");
    let stdout = decoded(&read_output(daemon, &text(&run["result"]["id"]), "stdout").await);
    assert_eq!(
        String::from_utf8_lossy(&stdout).trim(),
        READ_ONLY_FILE_OUTPUT
    );
    assert_eq!(
        run["result"]["applied"]["read_only_files"], declared,
        "the applied observation lists the bound file: {run}"
    );
    passed += 1;

    let before = std::fs::read(&program).expect("host program");
    let script = format!(
        "test -x {READ_ONLY_FILE_MOUNT} \
         && ! (echo overwritten >{READ_ONLY_FILE_MOUNT}) 2>/dev/null \
         && test ! -e {READ_ONLY_FILE_MOUNT_DIRECTORY}/sibling \
         && test \"$(ls -A {READ_ONLY_FILE_MOUNT_DIRECTORY})\" = gate"
    );
    let (status, run) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_rofile_confined",
            Some(&mutation(
                "01JROFILECLEANCONFINED1",
                &ExecInput::new(workspace, snapshot, &["/bin/sh", "-c", &script], true)
                    .read_only_files(declared)
                    .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(
        run["result"]["exit"],
        EXITED_CLEANLY(),
        "the file is read-only and alone in its mount directory: {run}"
    );
    assert_eq!(
        std::fs::read(&program).expect("host program"),
        before,
        "the host file is unchanged"
    );
    passed += 1;
    passed + check_read_only_file_host_refusals(daemon, workspace, snapshot, &host).await
}

/// The driver's filesystem rules over the wire: a directory and a symlink are refused by name
/// before dispatch.
async fn check_read_only_file_host_refusals(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    host: &Path,
) -> usize {
    let mut passed = 0;
    let link = host.join("gate-link");
    std::os::unix::fs::symlink(host.join("gate"), &link).expect("symlink to the program");
    for (operation, request, host_path, code) in [
        (
            "01JROFILECLEANDIRECTORY",
            "req_clean_rofile_directory",
            host.to_str().expect("UTF-8 fixture").to_owned(),
            "exec.read-only-file-not-a-file",
        ),
        (
            "01JROFILECLEANSYMLINK01",
            "req_clean_rofile_symlink",
            link.to_str().expect("UTF-8 fixture").to_owned(),
            "exec.read-only-file-symlink",
        ),
    ] {
        expect_error(
            &daemon
                .call(
                    "POST",
                    "/v1/execs",
                    request,
                    Some(&mutation(
                        operation,
                        &ExecInput::new(workspace, snapshot, &["gate"], true)
                            .read_only_files(json!([read_only_file(
                                &host_path,
                                READ_ONLY_FILE_MOUNT
                            )]))
                            .build(),
                    )),
                )
                .await,
            422,
            code,
        );
        passed += 1;
    }
    passed
}

// ---------------------------------------------------------------------------------------------
// Declared read-only roots (ADR 0010, bundle 0.23.0)
// ---------------------------------------------------------------------------------------------

/// The served bound the 0.23.0 capability schema publishes as `exec.read-only-roots` `max_roots`,
/// pinned here rather than read from the wire crate: the clean-room runner links no implementation.
const READ_ONLY_ROOTS_BOUND: usize = 4;

/// The portable journey: no confinement floor, so no roots bound is published and a start naming
/// roots is refused by the floor's own name, never run with the roots missing (invariant 3).
async fn check_read_only_roots_unserved(daemon: &Daemon, workspace: &str, snapshot: &str) -> usize {
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_clean_roroot_fact_absent", None)
        .await;
    assert_eq!(status, 200);
    assert!(
        machine["result"]["facts"]
            .get("exec.read-only-roots")
            .is_none(),
        "a portable host publishes no read-only roots fact: {machine}"
    );
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_roroot_unserved",
                Some(&mutation(
                    "01JROROOTCLEANUNSERVED01",
                    &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                        .read_only_roots(json!([{
                            "host_path": "/opt/host/toolchain",
                            "mount": "/opt/toolchain"
                        }]))
                        .build(),
                )),
            )
            .await,
        501,
        "exec.sandbox-unavailable",
    );
    // Shape before capability: one root over the bound is the caller's mistake on any host.
    let over_bound: Vec<Value> = (0..=READ_ONLY_ROOTS_BOUND)
        .map(|index| json!({ "host_path": format!("/opt/host/r{index}"), "mount": format!("/opt/r{index}") }))
        .collect();
    expect_error_at(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_roroot_portable_bound",
                Some(&mutation(
                    "01JROROOTCLEANPORTBOUND1",
                    &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                        .read_only_roots(Value::Array(over_bound))
                        .build(),
                )),
            )
            .await,
        422,
        "exec.read-only-root-invalid",
        "read_only_roots",
    );
    2
}

/// The delegated journey: the host publishes the wire bound, a start naming that many roots runs
/// with each one readable and none writable, the applied observation lists exactly the declared
/// roots, and one root over the bound is refused by name before anything runs.
async fn check_confined_read_only_roots(daemon: &Daemon, workspace: &str, snapshot: &str) -> usize {
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_clean_roroot_fact", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        machine["result"]["facts"]["exec.read-only-roots"],
        json!({ "max_roots": READ_ONLY_ROOTS_BOUND }),
        "a delegated host publishes the served roots bound: {machine}"
    );

    let fixture = tempfile::Builder::new()
        .prefix("roroot-cleanroom-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("fixture under CARGO_TARGET_TMPDIR");
    let base = std::fs::canonicalize(fixture.path()).expect("canonical fixture");
    let bound = READ_ONLY_ROOTS_BOUND;
    let roots: Vec<Value> = (0..=bound)
        .map(|index| {
            let host = base.join(format!("root{index}"));
            std::fs::create_dir(&host).expect("root directory");
            std::fs::write(host.join("marker"), format!("root {index}\n")).expect("marker");
            json!({
                "host_path": host.to_str().expect("UTF-8 fixture"),
                "mount": format!("/opt/ro-root{index}"),
            })
        })
        .collect();
    let declared = Value::Array(roots[..bound].to_vec());
    let script = (0..bound)
        .map(|index| {
            format!(
                "test \"$(cat /opt/ro-root{index}/marker)\" = 'root {index}' \
                 && ! (echo written >/opt/ro-root{index}/marker) 2>/dev/null \
                 && ! touch /opt/ro-root{index}/new 2>/dev/null"
            )
        })
        .collect::<Vec<_>>()
        .join(" && ");

    let (status, run) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_roroot_runs",
            Some(&mutation(
                "01JROROOTCLEANRUNS00001",
                &ExecInput::new(workspace, snapshot, &["/bin/sh", "-c", &script], true)
                    .read_only_roots(declared.clone())
                    .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(
        run["result"]["exit"],
        EXITED_CLEANLY(),
        "every declared root is readable and none is writable: {run}"
    );
    assert_eq!(
        run["result"]["applied"]["read_only_roots"], declared,
        "the applied observation lists every bound root: {run}"
    );
    for index in 0..bound {
        let host = base.join(format!("root{index}"));
        assert_eq!(
            std::fs::read_to_string(host.join("marker")).expect("marker"),
            format!("root {index}\n"),
            "the host root is unchanged"
        );
        assert!(
            !host.join("new").exists(),
            "nothing was created in a host root"
        );
    }

    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_roroot_over_bound",
                Some(&mutation(
                    "01JROROOTCLEANOVERBOUND1",
                    &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                        .read_only_roots(Value::Array(roots))
                        .build(),
                )),
            )
            .await,
        422,
        "exec.read-only-root-invalid",
    );

    2 + check_confined_read_only_root_host_refusals(daemon, workspace, snapshot, &base).await
}

/// The two host refusals, each for a list the wire crate admits, so only the pinned object can
/// answer: a path with nothing at it, a regular file, and a symlink to a real directory.
async fn check_confined_read_only_root_host_refusals(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    base: &std::path::Path,
) -> usize {
    let file = base.join("plain-file");
    std::fs::write(&file, "not a directory\n").expect("plain file");
    let link = base.join("link-to-root0");
    std::os::unix::fs::symlink(base.join("root0"), &link).expect("symlink to a root");
    for (index, (host, code)) in [
        (base.join("absent"), "exec.read-only-root-absent"),
        (file, "exec.read-only-root-not-a-directory"),
        (link, "exec.read-only-root-not-a-directory"),
    ]
    .into_iter()
    .enumerate()
    {
        expect_error_at(
            &daemon
                .call(
                    "POST",
                    "/v1/execs",
                    &format!("req_clean_roroot_host_{index}"),
                    Some(&mutation(
                        &format!("01JROROOTCLEANHOSTREFUSE{index}"),
                        &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                            .read_only_roots(json!([{
                                "host_path": host.to_str().expect("UTF-8 fixture"),
                                "mount": "/opt/ro-refused"
                            }]))
                            .build(),
                    )),
                )
                .await,
            422,
            code,
            "read_only_roots",
        );
    }
    3
}

// ---------------------------------------------------------------------------------------------
// Working directory and several apertures (design 26)
// ---------------------------------------------------------------------------------------------

/// Asserts the refusal's code and the address it names.
fn expect_error_at(response: &(u16, Value), status: u16, code: &str, address: &str) {
    expect_error(response, status, code);
    assert_eq!(response.1["error"]["address"], address, "{}", response.1);
}

/// Shape before capability, in both lanes: every lexical working-directory and aperture-list
/// refusal is answered before any fact is consulted, so a portable host and a delegated one give
/// the same answer (design 26 §§ 1–2).
async fn check_workdir_and_aperture_list_shape(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
) -> usize {
    let mut passed = 0;
    for (operation, request, workdir, code) in [
        (
            "01JWORKDIRCLEANSHAPE001",
            "req_clean_workdir_invalid",
            "workspace/sub",
            "exec.workdir-invalid",
        ),
        (
            "01JWORKDIRCLEANSHAPE002",
            "req_clean_workdir_outside",
            "/tmp",
            "exec.workdir-outside-mounts",
        ),
    ] {
        expect_error_at(
            &daemon
                .call(
                    "POST",
                    "/v1/execs",
                    request,
                    Some(&mutation(
                        operation,
                        &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                            .workdir(workdir)
                            .build(),
                    )),
                )
                .await,
            422,
            code,
            "workdir",
        );
        passed += 1;
    }
    let five = ["model", "capped", "uncapped", "fourth", "fifth"];
    for (operation, request, single, names, code) in [
        (
            "01JAPERTURESCLEANSHAPE1",
            "req_clean_apertures_too_few",
            None,
            &five[..1],
            "exec.apertures-too-few",
        ),
        (
            "01JAPERTURESCLEANSHAPE2",
            "req_clean_apertures_too_many",
            None,
            &five[..],
            "exec.apertures-too-many",
        ),
        (
            "01JAPERTURESCLEANSHAPE3",
            "req_clean_apertures_twice",
            None,
            &["model", "capped", "model"][..],
            "exec.aperture-listed-twice",
        ),
        (
            "01JAPERTURESCLEANSHAPE4",
            "req_clean_apertures_name",
            None,
            &["model", "Not-A-Name"][..],
            "exec.aperture-name-invalid",
        ),
        // One aperture and a list are two selections, and neither is chosen for the caller.
        (
            "01JAPERTURESCLEANSHAPE5",
            "req_clean_apertures_mismatch",
            Some("model"),
            &["model", "capped"][..],
            "exec.aperture-mode-mismatch",
        ),
    ] {
        let input = ExecInput {
            aperture: single,
            ..ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true).apertures(names)
        };
        expect_error_at(
            &daemon
                .call(
                    "POST",
                    "/v1/execs",
                    request,
                    Some(&mutation(operation, &input.build())),
                )
                .await,
            422,
            code,
            "sandbox.apertures",
        );
        passed += 1;
    }
    passed
}

/// Writable roots (ADR 0038), in both lanes: every lexical rule is answered before any fact is
/// consulted. The daemon declares one host root, [`WRITABLE_ROOT_NAME`]; a delegated host
/// publishes exactly that name in `exec.writable-roots.host` once its probe passes, and a portable
/// one publishes nothing. This daemon declares no Git source, so it serves no `workspace.git` and
/// publishes no `exec.writable-roots.git` in either lane; [`check_git_roots`] runs the daemon that
/// does. Either way a root this daemon did not declare (`cache`), and every Git root, is
/// `unserved` with its name — never a run that quietly got no writable mount, and never one that
/// got a different one.
async fn check_writable_roots_refused(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    delegated: bool,
) -> usize {
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_clean_writable_facts", None)
        .await;
    assert_eq!(status, 200);
    let host = machine["result"]["facts"].get("exec.writable-roots.host");
    if delegated {
        assert_eq!(
            host,
            Some(&json!([WRITABLE_ROOT_NAME])),
            "a delegated host proves the bind and publishes the declared name: {machine}"
        );
    } else {
        assert!(
            host.is_none(),
            "without confinement nothing is proved, so nothing is published: {machine}"
        );
    }
    assert!(
        machine["result"]["facts"].get("workspace.git").is_none()
            && machine["result"]["facts"]
                .get("exec.writable-roots.git")
                .is_none(),
        "a daemon that declares no Git source serves no Git workspace and so no Git root: {machine}"
    );
    let mut passed = 0;
    for (operation, request, roots, status, code, message) in writable_root_cases() {
        let response = daemon
            .call(
                "POST",
                "/v1/execs",
                request,
                Some(&mutation(
                    operation,
                    &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                        .writable_roots(roots)
                        .build(),
                )),
            )
            .await;
        expect_error_at(&response, status, code, "writable_roots");
        if let Some(message) = message {
            assert_eq!(response.1["error"]["message"], message, "{}", response.1);
        }
        passed += 1;
    }
    passed
}

/// The name the clean-room daemon declares its one writable host root under.
const WRITABLE_ROOT_NAME: &str = "shared";
/// Where the delegated cases mount it, apart from the case that mounts it at its own host path.
const WRITABLE_ROOT_MOUNT: &str = "/srv/substrate-shared";

/// A declared writable root, `<parent>/shared`, beside an undeclared `<parent>/sibling`, under
/// `CARGO_TARGET_TMPDIR` so it is neither the clean-room state directory nor beneath it. Every
/// path is canonical: the daemon refuses a declared path reached through a symlink.
struct WritableFixture {
    _directory: TempDir,
    parent: PathBuf,
}

impl WritableFixture {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("writable-cleanroom-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .expect("fixture under CARGO_TARGET_TMPDIR");
        let parent = std::fs::canonicalize(directory.path()).expect("canonical fixture");
        for child in [WRITABLE_ROOT_NAME, "sibling"] {
            std::fs::create_dir(parent.join(child)).expect("fixture directory");
        }
        Self {
            _directory: directory,
            parent,
        }
    }

    fn root(&self) -> PathBuf {
        self.parent.join(WRITABLE_ROOT_NAME)
    }

    fn sibling(&self) -> PathBuf {
        self.parent.join("sibling")
    }

    /// The `--writable-root` value the daemon starts with.
    fn declaration(&self) -> String {
        format!("{WRITABLE_ROOT_NAME}={}", self.root().display())
    }

    /// The names directly in `directory`, sorted.
    fn entries(directory: &Path) -> Vec<String> {
        let mut names = std::fs::read_dir(directory)
            .expect("read fixture directory")
            .map(|entry| {
                entry
                    .expect("fixture entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }
}

/// Portable lane: the daemon declares [`WRITABLE_ROOT_NAME`], but without confinement no probe
/// proves the bind, so the fact is absent and a start naming the declared root is `unserved` by
/// name. It is not run without the mount, and nothing is written to the declared directory.
async fn check_writable_root_unserved(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    writable: &WritableFixture,
) -> usize {
    let response = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_writable_declared_unserved",
            Some(&mutation(
                "01JWRITABLECLEANUNSERVED3",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &[
                        "/bin/sh",
                        "-c",
                        "echo written >/srv/substrate-shared/written",
                    ],
                    true,
                )
                .writable_roots(
                    json!([{ "host": WRITABLE_ROOT_NAME, "mount": WRITABLE_ROOT_MOUNT }]),
                )
                .build(),
            )),
        )
        .await;
    expect_error_at(
        &response,
        501,
        "exec.writable-roots-host-unserved",
        "writable_roots",
    );
    assert_eq!(
        response.1["error"]["message"], "Writable host root shared is not served by this host.",
        "{}",
        response.1
    );
    assert!(
        WritableFixture::entries(&writable.root()).is_empty(),
        "an unserved start wrote nothing to the declared root"
    );
    1
}

/// Delegated lane (ADR 0038 kind 1), four cases against the daemon's declared root:
///
/// 1. a write through the mount lands in the declared host directory, and the applied record
///    names exactly what was bound — name, declared host path, mount;
/// 2. mounted at its own host path, the program cannot reach its parent or its sibling: the
///    sibling is not in the sandbox, and on the host neither directory changed;
/// 3. a working directory at and beneath the mount starts the program there;
/// 4. the declared path swapped for a symlink before a start is refused by name, and the
///    symlink's target is not written.
#[allow(clippy::too_many_lines)] // Four cases on one fixture, in order; the swap case restores what it changed.
async fn check_confined_writable_roots(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    writable: &WritableFixture,
) -> usize {
    let mut passed = 0;
    let root = writable.root();
    let host_path = root.to_str().expect("UTF-8 fixture");
    let mounted = json!([{ "host": WRITABLE_ROOT_NAME, "mount": WRITABLE_ROOT_MOUNT }]);

    let (status, written) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_writable_writes",
            Some(&mutation(
                "01JWRITABLECLEANWRITES01",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &[
                        "/bin/sh",
                        "-c",
                        "echo written >/srv/substrate-shared/written",
                    ],
                    true,
                )
                .writable_roots(mounted.clone())
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{written}");
    assert_eq!(written["result"]["exit"], EXITED_CLEANLY(), "{written}");
    assert_eq!(
        std::fs::read_to_string(root.join("written")).expect("the write reached the host"),
        "written\n"
    );
    assert_eq!(
        written["result"]["applied"]["writable_roots"],
        json!([{
            "kind": "host",
            "name": WRITABLE_ROOT_NAME,
            "host_path": host_path,
            "mount": WRITABLE_ROOT_MOUNT,
        }]),
        "{written}"
    );
    passed += 1;

    let parent = writable.parent.to_str().expect("UTF-8 fixture");
    let sibling = writable.sibling();
    let sibling = sibling.to_str().expect("UTF-8 fixture");
    let escape = format!(
        "echo inside >'{host_path}/inside' && test ! -e '{sibling}' && \
         {{ (echo escaped >'{parent}/escaped') 2>/dev/null; \
         (mkdir -p '{sibling}' && echo escaped >'{sibling}/escaped') 2>/dev/null; true; }}"
    );
    let (status, contained) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_writable_contained",
            Some(&mutation(
                "01JWRITABLECLEANCONTAIN1",
                &ExecInput::new(workspace, snapshot, &["/bin/sh", "-c", &escape], true)
                    .writable_roots(json!([{ "host": WRITABLE_ROOT_NAME, "mount": host_path }]))
                    .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{contained}");
    assert_eq!(contained["result"]["exit"], EXITED_CLEANLY(), "{contained}");
    assert_eq!(
        WritableFixture::entries(&writable.parent),
        [WRITABLE_ROOT_NAME, "sibling"],
        "the parent of a writable root is unchanged on the host"
    );
    assert!(
        WritableFixture::entries(&writable.sibling()).is_empty(),
        "a sibling of a writable root is unchanged on the host"
    );
    assert!(root.join("inside").is_file(), "the root itself was written");
    passed += 1;

    std::fs::create_dir_all(root.join("sub")).expect("a directory beneath the root");
    for (operation, request, workdir) in [
        (
            "01JWRITABLECLEANWORKDIR1",
            "req_clean_writable_workdir",
            WRITABLE_ROOT_MOUNT,
        ),
        (
            "01JWRITABLECLEANWORKDIR2",
            "req_clean_writable_workdir_sub",
            "/srv/substrate-shared/sub",
        ),
    ] {
        let (status, ran) = daemon
            .call(
                "POST",
                "/v1/execs",
                request,
                Some(&mutation(
                    operation,
                    &ExecInput::new(
                        workspace,
                        snapshot,
                        &["/bin/sh", "-c", "pwd -P && echo here >here"],
                        true,
                    )
                    .writable_roots(mounted.clone())
                    .workdir(workdir)
                    .build(),
                )),
            )
            .await;
        assert_eq!(status, 200, "{ran}");
        assert_eq!(ran["result"]["exit"], EXITED_CLEANLY(), "{ran}");
        assert_eq!(ran["result"]["applied"]["workdir"], workdir, "{ran}");
        let relative = workdir
            .strip_prefix(WRITABLE_ROOT_MOUNT)
            .expect("beneath the mount")
            .trim_start_matches('/');
        assert!(
            root.join(relative).join("here").is_file(),
            "the program ran in {workdir}: {ran}"
        );
    }
    passed += 1;

    let moved = writable.parent.join("moved");
    std::fs::rename(&root, &moved).expect("move the declared root aside");
    std::os::unix::fs::symlink(&moved, &root).expect("a symlink at the declared path");
    let before = WritableFixture::entries(&moved);
    let response = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_writable_swapped",
            Some(&mutation(
                "01JWRITABLECLEANSWAPPED1",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &[
                        "/bin/sh",
                        "-c",
                        "echo swapped >/srv/substrate-shared/swapped",
                    ],
                    true,
                )
                .writable_roots(mounted)
                .build(),
            )),
        )
        .await;
    std::fs::remove_file(&root).expect("remove the symlink");
    std::fs::rename(&moved, &root).expect("restore the declared root");
    expect_error_at(
        &response,
        422,
        "exec.writable-root-not-a-directory",
        "writable_roots",
    );
    assert!(
        text(&response.1["error"]["message"]).contains(WRITABLE_ROOT_NAME),
        "{}",
        response.1
    );
    assert_eq!(
        WritableFixture::entries(&root),
        before,
        "a refused start wrote nothing through the symlink"
    );
    passed += 1;
    passed
}

/// One refused start per writable-root refusal: (operation, request id, roots, status, code, and
/// the exact message where it names the root).
type WritableRootCase = (
    &'static str,
    &'static str,
    Value,
    u16,
    &'static str,
    Option<&'static str>,
);

/// The six lexical refusals (422) and both unserved kinds (501), in the wire table's order.
fn writable_root_cases() -> [WritableRootCase; 8] {
    let nine: Vec<Value> = (1..=9)
        .map(|index| json!({ "host": format!("r{index}"), "mount": format!("/opt/r{index}") }))
        .collect();
    [
        (
            "01JWRITABLECLEANSHAPE01",
            "req_clean_writable_exceeded",
            json!(nine),
            422,
            "exec.writable-roots-exceeded",
            None,
        ),
        (
            "01JWRITABLECLEANSHAPE02",
            "req_clean_writable_name",
            json!([{ "host": "-cache", "mount": "/opt/cache" }]),
            422,
            "exec.writable-root-name-invalid",
            None,
        ),
        (
            "01JWRITABLECLEANSHAPE03",
            "req_clean_writable_mount_form",
            json!([{ "host": "cache", "mount": "/" }]),
            422,
            "exec.writable-root-mount-invalid",
            None,
        ),
        (
            "01JWRITABLECLEANSHAPE04",
            "req_clean_writable_mount_reserved",
            json!([{ "host": "cache", "mount": "/workspace/cache" }]),
            422,
            "exec.writable-root-mount-reserved",
            None,
        ),
        (
            "01JWRITABLECLEANSHAPE05",
            "req_clean_writable_mount_duplicate",
            json!([
                { "host": "cache", "mount": "/opt/shared" },
                { "workspace": "ws_checkout", "mount": "/opt/shared" }
            ]),
            422,
            "exec.writable-root-mount-duplicate",
            None,
        ),
        (
            "01JWRITABLECLEANSHAPE06",
            "req_clean_writable_mount_overlap",
            json!([
                { "host": "cache", "mount": "/opt/cache" },
                { "workspace": "ws_checkout", "mount": "/opt/cache/checkout" }
            ]),
            422,
            "exec.writable-root-mount-overlap",
            None,
        ),
        (
            "01JWRITABLECLEANUNSERVED1",
            "req_clean_writable_host_unserved",
            json!([{ "host": "cache", "mount": "/opt/cache" }]),
            501,
            "exec.writable-roots-host-unserved",
            Some("Writable host root cache is not served by this host."),
        ),
        (
            "01JWRITABLECLEANUNSERVED2",
            "req_clean_writable_git_unserved",
            json!([{ "workspace": "ws_checkout", "mount": "/opt/checkout" }]),
            501,
            "exec.writable-roots-git-unserved",
            Some("Writable Git root ws_checkout is not served by this host."),
        ),
    ]
}

/// No proved mechanism, no fact: a well-formed working directory is `unserved` by name — never a
/// run started somewhere else — and a well-formed list is `unserved` as one aperture is.
async fn check_workdir_and_aperture_list_unserved(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
) -> usize {
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_clean_workdir_fact_absent", None)
        .await;
    assert_eq!(status, 200);
    assert!(
        machine["result"]["facts"].get("exec.workdir").is_none(),
        "a portable host publishes no working-directory fact: {machine}"
    );
    expect_error_at(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_workdir_unserved",
                Some(&mutation(
                    "01JWORKDIRCLEANUNSERVED1",
                    &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                        .workdir("/workspace")
                        .build(),
                )),
            )
            .await,
        501,
        "exec.workdir-unserved",
        "workdir",
    );
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_apertures_unserved",
                Some(&mutation(
                    "01JAPERTURESCLEANUNSERVED",
                    &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                        .apertures(&["model", "capped"])
                        .build(),
                )),
            )
            .await,
        501,
        "exec.egress-apertures-unserved",
    );
    2
}

/// Runs `pwd -P` in `workdir` and returns the response, asserting nothing.
async fn run_in(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    (operation, request): (&str, &str),
    workdir: &str,
    roots: Option<Value>,
) -> (u16, Value) {
    let mut input =
        ExecInput::new(workspace, snapshot, &["/bin/sh", "-c", "pwd -P"], true).workdir(workdir);
    if let Some(roots) = roots {
        input = input.read_only_roots(roots);
    }
    daemon
        .call(
            "POST",
            "/v1/execs",
            request,
            Some(&mutation(operation, &input.build())),
        )
        .await
}

/// Asserts a run started in `workdir`, and that the applied record names it.
async fn expect_started_in(daemon: &Daemon, response: &(u16, Value), workdir: &str) {
    let (status, run) = response;
    assert_eq!(*status, 200, "{run}");
    assert_eq!(run["result"]["exit"], EXITED_CLEANLY(), "{run}");
    let stdout = decoded(&read_output(daemon, &text(&run["result"]["id"]), "stdout").await);
    assert_eq!(
        String::from_utf8_lossy(&stdout),
        format!("{workdir}\n"),
        "the program starts in the requested directory: {run}"
    );
    assert_eq!(
        run["result"]["applied"]["workdir"], workdir,
        "the applied record names the directory: {run}"
    );
}

/// The delegated journey: a directory beneath the workspace and one beneath a declared root are
/// where the program starts, and an absent directory or a symlink — beneath the workspace or a
/// root — is refused by name before dispatch.
async fn check_confined_workdir(daemon: &Daemon, workspace: &str, snapshot: &str) -> usize {
    let mut passed = 0;
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_clean_workdir_fact", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        machine["result"]["facts"]["exec.workdir"], true,
        "a delegated host proves and publishes the working directory: {machine}"
    );
    let (status, prepared) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_workdir_prepare",
            Some(&mutation(
                "01JWORKDIRCLEANPREPARE1",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &[
                        "/bin/sh",
                        "-c",
                        "mkdir -p workdir-probe/sub && ln -s sub workdir-probe/link",
                    ],
                    true,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{prepared}");
    assert_eq!(prepared["result"]["exit"], EXITED_CLEANLY(), "{prepared}");
    let sub = "/workspace/workdir-probe/sub";
    let run = run_in(
        daemon,
        workspace,
        snapshot,
        ("01JWORKDIRCLEANRUNS0001", "req_clean_workdir_runs"),
        sub,
        None,
    )
    .await;
    expect_started_in(daemon, &run, sub).await;
    passed += 1;
    for (operation, request, workdir) in [
        (
            "01JWORKDIRCLEANABSENT01",
            "req_clean_workdir_absent",
            "/workspace/workdir-probe/absent",
        ),
        (
            "01JWORKDIRCLEANSYMLINK1",
            "req_clean_workdir_symlink",
            "/workspace/workdir-probe/link",
        ),
    ] {
        expect_error_at(
            &run_in(
                daemon,
                workspace,
                snapshot,
                (operation, request),
                workdir,
                None,
            )
            .await,
            422,
            "exec.workdir-missing",
            "workdir",
        );
        passed += 1;
    }
    // The probe goes again: the journey's own destroy expects a workspace one batch can empty, and
    // a directory two levels deep takes a second (`flatten_directory_batch`).
    let (status, removed) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_workdir_remove",
            Some(&mutation(
                "01JWORKDIRCLEANREMOVE01",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/bin/rm", "-rf", "workdir-probe"],
                    true,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{removed}");
    assert_eq!(removed["result"]["exit"], EXITED_CLEANLY(), "{removed}");
    passed + check_confined_workdir_beneath_a_root(daemon, workspace, snapshot).await
}

/// The same journey beneath a declared read-only root: the program starts in a directory of the
/// root, and a symlink inside the root is refused by name before dispatch.
async fn check_confined_workdir_beneath_a_root(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
) -> usize {
    let mut passed = 0;
    let fixture = tempfile::Builder::new()
        .prefix("workdir-cleanroom-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("fixture under CARGO_TARGET_TMPDIR");
    let host = std::fs::canonicalize(fixture.path()).expect("canonical fixture");
    std::fs::create_dir(host.join("tool")).expect("root directory");
    std::os::unix::fs::symlink("tool", host.join("tool-link")).expect("root symlink");
    let roots = json!([{
        "host_path": host.to_str().expect("UTF-8 fixture"),
        "mount": "/opt/workdir-root",
    }]);
    let tool = "/opt/workdir-root/tool";
    let run = run_in(
        daemon,
        workspace,
        snapshot,
        ("01JWORKDIRCLEANROOTRUNS", "req_clean_workdir_root_runs"),
        tool,
        Some(roots.clone()),
    )
    .await;
    expect_started_in(daemon, &run, tool).await;
    passed += 1;
    expect_error_at(
        &run_in(
            daemon,
            workspace,
            snapshot,
            ("01JWORKDIRCLEANROOTLINK", "req_clean_workdir_root_symlink"),
            "/opt/workdir-root/tool-link",
            Some(roots),
        )
        .await,
        422,
        "exec.workdir-missing",
        "workdir",
    );
    passed += 1;
    passed
}

/// The aperture this deployment declares for `localhost`, the one name the generated mapping
/// already gives the first listen address (design 26 § 2). It is the fourth declaration, and
/// `MAX_EGRESS_APERTURES` admits no fifth, so two apertures sharing a name other than `localhost`
/// are held by the host's own tests rather than here.
const LOCALHOST_APERTURE: &str = "local";

/// One run through two listed apertures: `model` at the first listen address and the uncapped
/// firehose at the second, each reached at its own address and each counted on its own record.
fn aperture_list_program(inside: u16, firehose: u16) -> String {
    format!(
        "\nimport socket\n\
         served = socket.create_connection(('127.0.0.1', {inside}), 3).recv(64)\n\
         assert served == b'substrate-aperture-served', served\n\
         link = socket.create_connection(('127.0.0.2', {firehose}), 3)\n\
         total = 0\n\
         while True:\n\
         \x20   chunk = link.recv(65536)\n\
         \x20   if not chunk:\n\
         \x20       break\n\
         \x20   total += len(chunk)\n\
         assert total == {FIREHOSE_BYTES}, total\n\
         print('served')\n"
    )
}

/// The array form of a listed run, in list order: each aperture with the address it listened on,
/// the destination it was pinned to and the bytes that crossed it, and nothing counted twice.
fn expect_listed_run_applied(observed: &Value, inside: u16, firehose: u16) {
    let applied = observed["result"]["applied"]["network"]
        .as_array()
        .unwrap_or_else(|| panic!("a listed run did not record the array form: {observed}"));
    let shape: Vec<[&Value; 4]> = applied
        .iter()
        .map(|one| {
            [
                &one["mode"],
                &one["name"],
                &one["listen"],
                &one["destination"],
            ]
        })
        .collect();
    assert_eq!(
        shape,
        [
            [
                &json!("aperture"),
                &json!("model"),
                &json!(format!("127.0.0.1:{inside}")),
                &json!(format!("127.0.0.1:{inside}")),
            ],
            [
                &json!("aperture"),
                &json!(UNCAPPED_APERTURE),
                &json!(format!("127.0.0.2:{firehose}")),
                &json!(format!("127.0.0.1:{firehose}")),
            ],
        ],
        "{observed}"
    );
    assert_eq!(
        applied[0]["bytes"]["from_destination"].as_u64(),
        Some(APP_SERVER_BODY.len() as u64),
        "{observed}"
    );
    assert_eq!(
        applied[1]["bytes"]["from_destination"].as_u64(),
        Some(FIREHOSE_BYTES),
        "{observed}"
    );
}

/// The delegated half of a list (design 26 § 2): every name must be declared; a list of declared
/// names runs with every aperture, in list order, and records the array form; and a list whose
/// hosts the generated mapping would hold twice is refused by name before anything is installed.
async fn check_confined_aperture_list(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    inside: u16,
    firehose: u16,
) -> usize {
    let mut passed = 0;
    expect_error(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_apertures_undeclared",
                Some(&mutation(
                    "01JAPERTURESCLEANUNDECLARED",
                    &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                        .apertures(&["model", "undeclared"])
                        .build(),
                )),
            )
            .await,
        501,
        "exec.aperture-undeclared",
    );
    passed += 1;

    let program = aperture_list_program(inside, firehose);
    let (status, run) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_apertures_declared",
            Some(&mutation(
                "01JAPERTURESCLEANDECLARED",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/python3", "-c", &program],
                    true,
                )
                .apertures(&["model", UNCAPPED_APERTURE])
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["result"]["state"], "exited", "{run}");
    assert_eq!(
        run["result"]["exit"]["code"], 0,
        "a listed aperture was not reachable at its own listen address: {run}"
    );
    // The array form on the start's answer and on the record read back.
    let id = text(&run["result"]["id"]);
    let (status, recorded) = daemon
        .call(
            "GET",
            &format!("/v1/execs/{id}"),
            "req_clean_apertures_declared_get",
            None,
        )
        .await;
    assert_eq!(status, 200, "{recorded}");
    for observed in [&run, &recorded] {
        expect_listed_run_applied(observed, inside, firehose);
    }
    passed += 1;

    // `localhost` after the first: the generated mapping already gives that name to the first
    // listen address, so the later aperture would be answered by the first one's forwarder and
    // reach its destination; it is refused by name before anything is installed.
    expect_error_at(
        &daemon
            .call(
                "POST",
                "/v1/execs",
                "req_clean_apertures_localhost_second",
                Some(&mutation(
                    "01JAPERTURESCLEANLOCALHOST",
                    &ExecInput::new(workspace, snapshot, &["/usr/bin/true"], true)
                        .apertures(&["model", LOCALHOST_APERTURE])
                        .build(),
                )),
            )
            .await,
        422,
        "exec.aperture-host-listed-twice",
        "sandbox.apertures",
    );
    passed += 1;
    passed
}

// ---------------------------------------------------------------------------------------------
// Writable Git roots (ADR 0038 kind 2)
// ---------------------------------------------------------------------------------------------

/// The Git source name the Git-root daemon declares.
const GIT_SOURCE_NAME: &str = "connector";
/// The provider branch the fixture repository's one commit is on.
const GIT_FIXTURE_BRANCH: &str = "provider-default";
/// Where the cases mount the Git workspace, and the directory beneath it they start in.
const GIT_ROOT_MOUNT: &str = "/srv/checkout";
const GIT_ROOT_WORKDIR: &str = "/srv/checkout/docs";

/// Runs the fixture's own `git` in `directory`, isolated from the operator's configuration.
fn fixture_git(directory: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Substrate Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Substrate Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .expect("git is installed");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("UTF-8 git output")
        .trim()
        .to_owned()
}

/// `git upload-pack --stateless-rpc`, the server half of smart HTTP protocol v2.
fn fixture_upload_pack(repository: &Path, discovery: bool, body: &[u8]) -> Vec<u8> {
    let mut command = std::process::Command::new("git");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_PROTOCOL", "version=2")
        .args(["upload-pack", "--stateless-rpc"]);
    if discovery {
        command.arg("--advertise-refs");
    }
    let mut child = command
        .arg(repository)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git upload-pack is installed");
    std::io::Write::write_all(&mut child.stdin.take().expect("upload-pack stdin"), body)
        .expect("upload-pack input");
    let output = child.wait_with_output().expect("upload-pack output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// Serves one smart-HTTP request on a TLS connection; an error ends the connection.
fn fixture_serve_request(
    stream: &mut std::io::BufReader<
        rustls::StreamOwned<rustls::ServerConnection, std::net::TcpStream>,
    >,
    repository: &Path,
) -> std::io::Result<()> {
    use std::io::{BufRead as _, Read as _, Write as _};
    let mut line = String::new();
    if stream.read_line(&mut line)? == 0 {
        return Err(std::io::Error::from(ErrorKind::UnexpectedEof));
    }
    let discovery = line.starts_with("GET ");
    let mut length = 0;
    loop {
        line.clear();
        stream.read_line(&mut line)?;
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().expect("body length");
        }
    }
    assert!(length < 64 * 1024, "a bounded fixture request body");
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    let response = fixture_upload_pack(repository, discovery, &body);
    let content = if discovery { "advertisement" } else { "result" };
    write!(
        stream.get_mut(),
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-git-upload-pack-{content}\r\n\
         Content-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        response.len()
    )?;
    stream.get_mut().write_all(&response)?;
    stream.get_mut().flush()
}

/// A one-commit repository served over HTTPS on loopback by `git upload-pack`, with the
/// self-signed trust root the daemon is given as `--ca-bundle`.
struct GitSourceFixture {
    _directory: TempDir,
    ca_bundle: PathBuf,
    origin: String,
    commit: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl GitSourceFixture {
    fn start() -> Self {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let directory = tempfile::Builder::new()
            .prefix("git-source-cleanroom-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .expect("fixture under CARGO_TARGET_TMPDIR");
        let parent = std::fs::canonicalize(directory.path()).expect("canonical fixture");
        let repository = parent.join("repository.git");
        std::fs::create_dir_all(repository.join("docs")).expect("fixture repository");
        fixture_git(&repository, &["init", "-q", "-b", GIT_FIXTURE_BRANCH]);
        std::fs::write(repository.join("README.md"), "writable Git root\n").expect("README");
        std::fs::write(repository.join("docs/guide.md"), "guide\n").expect("guide");
        fixture_git(&repository, &["add", "-A"]);
        fixture_git(&repository, &["commit", "-q", "-m", "fixture"]);
        let commit = fixture_git(&repository, &["rev-parse", "HEAD"]);

        let identity = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()])
            .expect("TLS fixture identity");
        let ca_bundle = parent.join("ca.pem");
        std::fs::write(&ca_bundle, identity.cert.pem()).expect("fixture trust root");
        let config = Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![identity.cert.der().clone()],
                    rustls::pki_types::PrivatePkcs8KeyDer::from(
                        identity.signing_key.serialize_der(),
                    )
                    .into(),
                )
                .expect("TLS server config"),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Git listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let origin = format!(
            "https://{}/",
            listener.local_addr().expect("listener address")
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                let _ = socket.set_nonblocking(false);
                let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = socket.set_write_timeout(Some(Duration::from_secs(2)));
                let Ok(tls) = rustls::ServerConnection::new(Arc::clone(&config)) else {
                    continue;
                };
                let mut stream = std::io::BufReader::new(rustls::StreamOwned::new(tls, socket));
                while !stopped.load(Ordering::Relaxed)
                    && fixture_serve_request(&mut stream, &repository).is_ok()
                {}
            }
        });
        Self {
            _directory: directory,
            ca_bundle,
            origin,
            commit,
            stop,
            thread: Some(thread),
        }
    }

    /// The daemon flags that declare this source and trust its certificate.
    fn arguments(&self) -> Vec<String> {
        vec![
            "--git-source".to_owned(),
            format!("{GIT_SOURCE_NAME}={}", self.origin),
            "--ca-bundle".to_owned(),
            self.ca_bundle.display().to_string(),
        ]
    }

    fn create_input(&self) -> Value {
        json!({
            "source": { "git": {
                "source": GIT_SOURCE_NAME,
                "locator": format!("{}repository.git", self.origin),
                "ref": GIT_FIXTURE_BRANCH,
                "commit": self.commit,
                "depth": 1,
            } },
            "labels": { "runner": "git-root" },
        })
    }
}

impl Drop for GitSourceFixture {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn create_empty_workspace(daemon: &Daemon, operation: &str, request_id: &str) -> String {
    let (status, created) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            request_id,
            Some(&mutation(
                operation,
                &json!({ "source": "empty", "labels": {} }),
            )),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    text(&created["result"]["id"])
}

/// Writable Git roots (ADR 0038 kind 2), against a second daemon that declares a Git source.
///
/// Both lanes: the daemon serves `workspace.git` and materializes a Git workspace from the HTTPS
/// fixture. Portable: no confinement proves the bind, so `exec.writable-roots.git` is absent and a
/// start naming the Git workspace is `unserved` by its id, before anything runs. Delegated: the fact
/// is published, a child commits in the mounted checkout from a working directory beneath the mount
/// and the commit is on the host and in the change-set route, and each refusal the request or the
/// store decides is named before dispatch and recorded.
async fn check_git_roots(cgroup_root: Option<&Path>) -> usize {
    let source = GitSourceFixture::start();
    let temporary = TempDir::with_prefix("substrate-gitroots-").expect("clean-room directory");
    let root = temporary.path();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
        .expect("owner-private clean-room directory");
    let daemon = Daemon::start_with_arguments(
        root,
        cgroup_root,
        &[],
        &[],
        &[],
        false,
        &[],
        &source.arguments(),
    )
    .await;
    let delegated = cgroup_root.is_some();
    let mut passed = 0;

    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_git_root_machine", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        machine["result"]["facts"]["workspace.git"], true,
        "a declared Git source is served: {machine}"
    );
    let fact = machine["result"]["facts"].get("exec.writable-roots.git");
    if delegated {
        assert_eq!(
            fact,
            Some(&json!(true)),
            "a delegated host with Git workspaces proves the bind and publishes the fact: {machine}"
        );
    } else {
        assert!(
            fact.is_none(),
            "without confinement nothing is proved, so nothing is published: {machine}"
        );
    }
    let snapshot = text(&machine["result"]["snapshot"]);
    passed += 1;

    let (status, created) = daemon
        .call_headed(
            "POST",
            "/v1/workspaces",
            "req_git_root_checkout",
            &[("x-b10x-workspace-source-authorization", "fixture-authority")],
            Some(&mutation(
                "01JGITROOTCLEANCHECKOUT1",
                &source.create_input(),
            )),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["result"]["state"], "ready", "{created}");
    let checkout = text(&created["result"]["id"]);
    let host_path = daemon.workspaces.join(&checkout);
    assert_eq!(
        fixture_git(&host_path, &["rev-parse", "HEAD"]),
        source.commit,
        "the Git workspace is a checkout of the admitted commit"
    );
    passed += 1;

    let own = create_empty_workspace(&daemon, "01JGITROOTCLEANOWN00001", "req_git_root_own").await;
    passed += if delegated {
        check_confined_git_root(&daemon, &own, &snapshot, &checkout, &source.commit).await
            + check_git_root_refusals(&daemon, &own, &snapshot, &checkout).await
    } else {
        check_git_root_unserved(&daemon, &own, &snapshot, &checkout).await
    };
    daemon.close().await;
    drop(source);
    passed
}

/// Portable lane: a start naming a ready Git workspace is `unserved` by its id, is recorded as
/// refused, and nothing ran in its checkout.
async fn check_git_root_unserved(
    daemon: &Daemon,
    own: &str,
    snapshot: &str,
    checkout: &str,
) -> usize {
    let operation = "01JGITROOTCLEANUNSERVED1";
    let request_id = "req_git_root_unserved";
    let response = daemon
        .call(
            "POST",
            "/v1/execs",
            request_id,
            Some(&mutation(
                operation,
                &ExecInput::new(
                    own,
                    snapshot,
                    &["/bin/sh", "-c", "echo written >/srv/checkout/written"],
                    true,
                )
                .writable_roots(json!([{ "workspace": checkout, "mount": GIT_ROOT_MOUNT }]))
                .build(),
            )),
        )
        .await;
    expect_error_at(
        &response,
        501,
        "exec.writable-roots-git-unserved",
        "writable_roots",
    );
    assert_eq!(
        response.1["error"]["message"],
        format!("Writable Git root {checkout} is not served by this host."),
        "{}",
        response.1
    );
    assert_eq!(
        ledger_row(daemon, operation, "req_git_root_unserved_ledger").await["state"],
        "refused"
    );
    assert!(
        !daemon.workspaces.join(checkout).join("written").exists(),
        "an unserved start wrote nothing to the checkout"
    );
    1
}

/// Delegated lane, two cases: a child commits in the checkout mounted at [`GIT_ROOT_MOUNT`],
/// starting in [`GIT_ROOT_WORKDIR`] beneath it, and the applied record names the bind; the commit
/// is then on the host, on top of the admitted commit, and its file is in the change-set route.
async fn check_confined_git_root(
    daemon: &Daemon,
    own: &str,
    snapshot: &str,
    checkout: &str,
    baseline: &str,
) -> usize {
    let host_path = daemon.workspaces.join(checkout);
    let git = "/usr/bin/git -c safe.directory=* -c user.name=Child \
               -c user.email=child@example.invalid -c commit.gpgsign=false -c gc.auto=0 \
               -c maintenance.auto=false";
    let program = format!(
        "set -e; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null HOME=/workspace; \
         echo child >child.txt; {git} add child.txt; {git} commit -q -m 'child commit'"
    );
    let (status, committed) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_git_root_commit",
            Some(&mutation(
                "01JGITROOTCLEANCOMMIT001",
                &ExecInput::new(own, snapshot, &["/bin/sh", "-c", &program], true)
                    .writable_roots(json!([{ "workspace": checkout, "mount": GIT_ROOT_MOUNT }]))
                    .workdir(GIT_ROOT_WORKDIR)
                    .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{committed}");
    assert_eq!(committed["result"]["exit"], EXITED_CLEANLY(), "{committed}");
    assert_eq!(
        committed["result"]["applied"]["writable_roots"],
        json!([{
            "kind": "git",
            "workspace": checkout,
            "host_path": host_path.to_str().expect("UTF-8 workspace path"),
            "mount": GIT_ROOT_MOUNT,
        }]),
        "{committed}"
    );
    assert_eq!(
        committed["result"]["applied"]["workdir"], GIT_ROOT_WORKDIR,
        "{committed}"
    );
    assert_eq!(
        fixture_git(&host_path, &["log", "-1", "--format=%s"]),
        "child commit",
        "the child's commit is on the host checkout"
    );
    assert_eq!(
        fixture_git(&host_path, &["rev-parse", "HEAD~1"]),
        baseline,
        "the child committed on top of the admitted commit"
    );
    assert_eq!(
        fixture_git(&host_path, &["show", "HEAD:docs/child.txt"]),
        "child",
        "the program ran in the working directory beneath the mount"
    );
    assert!(
        !daemon.workspaces.join(own).join("child.txt").exists(),
        "nothing landed in the start's own workspace"
    );
    let mut passed = 1;

    let (status, changes) = daemon
        .call(
            "GET",
            &format!(
                "/v2/workspaces/{checkout}/git/changes\
                 ?max_files=16&max_file_bytes=65536&max_total_bytes=262144"
            ),
            "req_git_root_changes",
            None,
        )
        .await;
    assert_eq!(status, 200, "{changes}");
    assert_eq!(changes["result"]["commit"], baseline, "{changes}");
    assert!(
        changes["result"]["items"]
            .as_array()
            .expect("change items")
            .iter()
            .any(|item| item["status"] == "added" && item["current"]["path"] == "docs/child.txt"),
        "the committed file is a change against the baseline: {changes}"
    );
    passed += 1;
    passed
}

/// Delegated lane, four cases: an empty workspace, the start's own workspace, one workspace named
/// twice, and a workspace that does not exist are each refused by name at `writable_roots`
/// before dispatch, recorded as refused, and nothing ran.
async fn check_git_root_refusals(
    daemon: &Daemon,
    own: &str,
    snapshot: &str,
    checkout: &str,
) -> usize {
    let plain =
        create_empty_workspace(daemon, "01JGITROOTCLEANPLAIN0001", "req_git_root_plain").await;
    let cases = [
        (
            "01JGITROOTCLEANNOTGIT001",
            "req_git_root_not_git",
            json!([{ "workspace": plain, "mount": GIT_ROOT_MOUNT }]),
            422,
            "exec.writable-root-not-git",
            Some(format!(
                "Writable Git root {plain} is not a workspace created from a Git source."
            )),
        ),
        (
            "01JGITROOTCLEANOWNROOT01",
            "req_git_root_own_root",
            json!([{ "workspace": own, "mount": GIT_ROOT_MOUNT }]),
            422,
            "exec.writable-root-own-workspace",
            None,
        ),
        (
            "01JGITROOTCLEANDUPLICATE",
            "req_git_root_duplicate",
            json!([
                { "workspace": checkout, "mount": "/srv/first" },
                { "workspace": checkout, "mount": "/srv/second" }
            ]),
            422,
            "exec.writable-root-workspace-duplicate",
            Some(format!(
                "Writable Git root {checkout} is named more than once."
            )),
        ),
        (
            "01JGITROOTCLEANUNKNOWN01",
            "req_git_root_unknown",
            json!([{ "workspace": "ws_neverexisted", "mount": GIT_ROOT_MOUNT }]),
            404,
            "resource.not-found",
            Some("Resource was not found.".to_owned()),
        ),
    ];
    let mut passed = 0;
    for (operation, request_id, roots, status, code, message) in cases {
        let response = daemon
            .call(
                "POST",
                "/v1/execs",
                request_id,
                Some(&mutation(
                    operation,
                    &ExecInput::new(
                        own,
                        snapshot,
                        &["/bin/sh", "-c", "echo ran >/workspace/ran"],
                        true,
                    )
                    .writable_roots(roots)
                    .build(),
                )),
            )
            .await;
        expect_error_at(&response, status, code, "writable_roots");
        if let Some(message) = message {
            assert_eq!(response.1["error"]["message"], message, "{}", response.1);
        }
        assert_eq!(
            ledger_row(daemon, operation, &format!("{request_id}_ledger")).await["state"],
            "refused",
            "{code}"
        );
        assert!(
            !daemon.workspaces.join(own).join("ran").exists(),
            "a refused start ran nothing ({code})"
        );
        passed += 1;
    }
    passed
}

// ---------------------------------------------------------------------------------------------
// The lane
// ---------------------------------------------------------------------------------------------

/// The predecessor printed its case count; the port asserts it, so a case cannot vanish quietly.
const PORTABLE_CASES: usize = 119;
const DELEGATED_CASES: usize = 182;

#[tokio::test(flavor = "multi_thread")]
async fn runtime_clean_room_drives_the_shipped_daemon_over_its_unix_socket() {
    let delegated = DelegatedCgroup::acquire();
    let temporary = TempDir::with_prefix("substrate-cleanroom-").expect("clean-room directory");
    let root = temporary.path();
    // `mkdtemp(3)` — and so Python's `tempfile.TemporaryDirectory` — creates 0700; Rust's
    // `TempDir` creates 0777 & ~umask, which the daemon refuses as durable state.
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
        .expect("owner-private clean-room directory");
    check_startup_refusal(root).await;
    // Both endpoints exist before the daemon does, because an aperture is resolved and pinned once
    // at declaration: the daemon must be told a destination it can resolve at startup.
    let (inside, inside_server) = fake_app_server().await;
    let (outside, outside_server) = fake_app_server().await;
    // One destination, two declarations, one optional term between them: the ceiling case and its
    // negative differ by the term and by nothing else (ADR 0014).
    let (firehose, firehose_server) = fake_firehose_server().await;
    let declared = vec![
        format!("model=127.0.0.1:{}/tcp", inside.port()),
        format!(
            "{CAPPED_APERTURE}=127.0.0.1:{}/tcp/max={CEILING_BYTES}",
            firehose.port()
        ),
        format!("{UNCAPPED_APERTURE}=127.0.0.1:{}/tcp", firehose.port()),
        // The name the generated mapping gives the first listen address, which no list may name
        // after its first aperture (design 26 § 2).
        format!("{LOCALHOST_APERTURE}=localhost:{}/tcp", inside.port()),
    ];
    // The declared slot file: one bounded, owner-private regular file, written before the daemon
    // that must be able to read it exists. Its path is argv; its bytes never are.
    let slot_file = root.join("declared.slot");
    std::fs::write(&slot_file, SECRET_SLOT_VALUE).expect("write the declared slot file");
    std::fs::set_permissions(&slot_file, std::fs::Permissions::from_mode(0o600))
        .expect("restrict the declared slot file");
    let slots = vec![format!("{SECRET_SLOT_NAME}={}", slot_file.display())];
    // The declared writable root and a sibling beside it, outside the clean-room root because that
    // root is the daemon's state directory, which no writable root may lie under (ADR 0038).
    let writable = WritableFixture::new();
    let daemon = Daemon::start_declaring(
        root,
        delegated.as_ref().map(DelegatedCgroup::path),
        &declared,
        &slots,
        &[delegated_key_flag()],
        false,
        &[writable.declaration()],
    )
    .await;
    check_dual_daemon_refusal(&daemon).await;
    let passed = check_http_journey(
        &daemon,
        delegated.as_ref().map(DelegatedCgroup::path),
        inside.port(),
        outside.port(),
        firehose.port(),
        &writable,
    )
    .await;
    // The daemon's own diagnostic stream, readable only once its pipe has closed. Uncounted on
    // purpose: it belongs to the process, not to any one HTTP case.
    let diagnostics = daemon.close().await;
    assert!(
        !diagnostics.contains(SECRET_SLOT_VALUE),
        "the daemon's stderr carries the declared value"
    );
    inside_server.abort();
    outside_server.abort();
    firehose_server.abort();
    // A second daemon, after the first has stopped: writable Git roots need a declared Git source,
    // which the journey's daemon deliberately does not have (its Git roots are unserved).
    let passed = passed + check_git_roots(delegated.as_ref().map(DelegatedCgroup::path)).await;
    let (lane, expected) = if delegated.is_some() {
        ("delegated", DELEGATED_CASES)
    } else {
        ("portable", PORTABLE_CASES)
    };
    assert_eq!(passed, expected, "{lane} lane case inventory");
    println!(
        "runtime clean-room: the complete HTTP inventory, startup refusal, \
         and dual-daemon refusal passed ({lane} lane)"
    );
}

// ---------------------------------------------------------------------------------------------
// Adversarial: the bound the observation does not name (ADR 0014)
// ---------------------------------------------------------------------------------------------

/// The ceiling this adversarial case declares. Small enough that the relay stops well inside
/// [`FIREHOSE_BYTES`], so a run that stops has been stopped by substrate.
const ADVERSARIAL_CEILING_BYTES: u64 = 128 * 1024;
const ADVERSARIAL_APERTURE: &str = "bounded";

/// One connection, drained to whatever end it comes to, and then a prompt exit.
///
/// The realistic confined client: it reads until the stream stops and exits. It does not loop
/// reconnecting, because nothing tells it to — ADR 0014, "the child gets a closed socket".
fn exiting_firehose_program(port: u16) -> String {
    format!(
        "\nimport socket, os\n\
         link = socket.create_connection(('127.0.0.1', {port}), 3)\n\
         total = 0\n\
         while True:\n\
         \x20   chunk = link.recv(65536)\n\
         \x20   if not chunk:\n\
         \x20       break\n\
         \x20   total += len(chunk)\n\
         os._exit(0)\n"
    )
}

/// A run stopped at the declared ceiling names the bound that stopped it — whatever the child did
/// next.
///
/// ADR 0014 gives the refusal somewhere to live so that an operator "does not have to tell a
/// ceiling from a client cancel by reading the byte counts"
/// (`crates/substrate-host/src/process.rs:1394-1400`). But the relay stops the bytes
/// (`crates/substrate-host/src/egress.rs:774`) while only the parent's 1 ms supervision tick
/// names the bound (`crates/substrate-host/src/process.rs:1449`), and that tick shares a
/// `tokio::select!` with `child.wait()` (`:1437`). A child that notices its closed socket and
/// exits before the next tick wins that arm, so `aperture_exhausted` is never set and
/// `record_aperture` (`:1401`) writes no refusal.
///
/// Twenty runs rather than one: the window is the phase of a 1 ms interval against the sandbox's
/// teardown, so a single run is a coin toss. Observed on this tree: roughly one run in five comes
/// back `state: "exited"`, `exit.code: 0`, `refusal: null` — indistinguishable from a run that
/// finished on its own — with `applied.network.bytes` summing to exactly the declared ceiling.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_stopped_at_the_ceiling_names_the_bound_even_when_its_child_exits_first() {
    let Some(delegated) = DelegatedCgroup::acquire() else {
        return;
    };
    let temporary = TempDir::with_prefix("substrate-adversarial-").expect("clean-room directory");
    let root = temporary.path();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
        .expect("owner-private clean-room directory");
    let (firehose, firehose_server) = fake_firehose_server().await;
    let declared = vec![format!(
        "{ADVERSARIAL_APERTURE}=127.0.0.1:{}/tcp/max={ADVERSARIAL_CEILING_BYTES}",
        firehose.port()
    )];
    let daemon = Daemon::start(root, Some(delegated.path()), &declared, &[], &[], false).await;

    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_adversarial_machine", None)
        .await;
    assert_eq!(status, 200, "{machine}");
    let snapshot = text(&machine["result"]["snapshot"]);
    assert_eq!(
        machine["result"]["facts"]["exec.egress-apertures"][0]["max_bytes"],
        ADVERSARIAL_CEILING_BYTES,
        "the declared ceiling is not published: {machine}"
    );

    let (status, created) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_adversarial_create",
            Some(&mutation(
                "01JPADVERSARIALCEILINGWS",
                &json!({ "source": "empty", "labels": {} }),
            )),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    let workspace = text(&created["result"]["id"]);

    let program = exiting_firehose_program(firehose.port());
    let mut unnamed = Vec::new();
    for attempt in 0..20 {
        let (status, run) = daemon
            .call(
                "POST",
                "/v1/execs",
                &format!("req_adversarial_run_{attempt}"),
                Some(&mutation(
                    &format!("01JPADVERSARIALCEILING{attempt:02}"),
                    &ExecInput::new(
                        &workspace,
                        &snapshot,
                        &["/usr/bin/python3", "-c", &program],
                        true,
                    )
                    .aperture(ADVERSARIAL_APERTURE)
                    .build(),
                )),
            )
            .await;
        assert_eq!(status, 200, "{run}");
        let applied = &run["result"]["applied"]["network"];
        let crossed = applied["bytes"]["to_destination"].as_u64().unwrap_or(0)
            + applied["bytes"]["from_destination"].as_u64().unwrap_or(0);
        // The premise: substrate stopped this run's egress at the declared ceiling. Without that
        // the case says nothing, so it is asserted rather than assumed.
        assert!(
            (ADVERSARIAL_CEILING_BYTES..FIREHOSE_BYTES).contains(&crossed),
            "attempt {attempt} was not stopped at the ceiling ({crossed} bytes): {run}"
        );
        eprintln!(
            "adversarial attempt {attempt}: state={} exit={} refusal={} crossed={crossed}",
            run["result"]["state"], run["result"]["exit"], run["result"]["refusal"]
        );
        if run["result"]["refusal"]["code"] != "exec.aperture-byte-limit" {
            unnamed.push(format!(
                "attempt {attempt}: state={} refusal={} crossed={crossed}",
                run["result"]["state"], run["result"]["refusal"]
            ));
        }
    }
    let diagnostics = daemon.close().await;
    firehose_server.abort();
    assert!(
        unnamed.is_empty(),
        "a run whose egress substrate stopped at the declared ceiling did not name \
         exec.aperture-byte-limit:\n{}\n(daemon stderr: {diagnostics})",
        unnamed.join("\n")
    );
}

/// A client-supplied aperture *name* is refused over the wire, not answered with a dropped
/// connection.
///
/// ADR 0014's request-side guard `reads_as_ceiling` slices the first four **bytes** of every
/// `/`-separated term of the name (`crates/substrate-wire/src/lib.rs:1942`) and it runs before
/// `valid_aperture_name` (`:1971-1977`), so a name whose byte index 4 lands inside a multi-byte
/// character is a `String` slice panic inside the request handler. This lane is the clean room:
/// it asks the shipped binary over its socket, so what it asserts is what a client sees. Portable
/// — the guard runs before any capability check, so no cgroup delegation is needed to reach it.
#[tokio::test(flavor = "multi_thread")]
async fn a_non_ascii_aperture_name_is_refused_over_the_wire() {
    let temporary = TempDir::with_prefix("substrate-adversarial-utf8-").expect("clean-room");
    let root = temporary.path();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
        .expect("owner-private clean-room directory");
    let daemon = Daemon::start(root, None, &[], &[], &[], false).await;
    let (status, machine) = daemon
        .call("GET", "/v1/machine", "req_adversarial_utf8_machine", None)
        .await;
    assert_eq!(status, 200, "{machine}");
    let snapshot = text(&machine["result"]["snapshot"]);
    let (status, created) = daemon
        .call(
            "POST",
            "/v1/workspaces",
            "req_adversarial_utf8_create",
            Some(&mutation(
                "01JPADVERSARIALUTF8WS001",
                &json!({ "source": "empty", "labels": {} }),
            )),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    let workspace = text(&created["result"]["id"]);

    let answered = tokio::time::timeout(
        Duration::from_secs(10),
        daemon.call(
            "POST",
            "/v1/execs",
            "req_adversarial_utf8_start",
            Some(&mutation(
                "01JPADVERSARIALUTF8RUN01",
                &ExecInput::new(&workspace, &snapshot, &["/usr/bin/true"], false)
                    .aperture("ab\u{20ac}cd")
                    .build(),
            )),
        ),
    )
    .await
    .expect("the daemon answered a request carrying a non-ASCII aperture name");
    expect_error(&answered, 422, "exec.aperture-name-invalid");

    // Still serving: a refusal that takes the connection with it is a different failure from a
    // refusal that takes the daemon with it, and the report has to say which.
    let (status, again) = daemon
        .call("GET", "/v1/machine", "req_adversarial_utf8_again", None)
        .await;
    assert_eq!(status, 200, "{again}");
    daemon.close().await;
}

// ---------------------------------------------------------------------------------------------
// Adversarial: an adopted directory changed under the daemon after adopt (ADR 0035)
// ---------------------------------------------------------------------------------------------

/// Adopts `name` (already placed under the workspace root) and returns the minted id and the
/// capability snapshot an exec start must carry.
async fn adversarial_adopt(daemon: &Daemon, name: &str, tag: &str) -> (String, String) {
    let (status, machine) = daemon
        .call(
            "GET",
            "/v1/machine",
            &format!("req_adv_{tag}_machine"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{machine}");
    let snapshot = text(&machine["result"]["snapshot"]);
    let (status, adopted) = daemon
        .call(
            "POST",
            "/v1/workspaces/adopt",
            &format!("req_adv_{tag}_adopt"),
            Some(&mutation(
                &format!("01JADVADOPT{}ADOPT01", tag.to_uppercase()),
                &json!({ "root_name": name, "labels": {} }),
            )),
        )
        .await;
    assert_eq!(status, 201, "{adopted}");
    (text(&adopted["result"]["id"]), snapshot)
}

/// ADR 0035 decision 5 refuses to adopt an entry that is not a directory, and the host opens a
/// root name with `O_NOFOLLOW` (`crates/substrate-guarded/src/fs.rs`), so a symlink is never
/// adopted and never read through. An exec start pins the same entry the same way
/// (`pin_workspace_directory`, `crates/substrate-guarded/src/fs.rs`) and bubblewrap binds that
/// descriptor (`--bind-fd`), never the path: once bound by path (`--bind <path> /workspace`), an
/// entry the operator replaced with a symlink after adopt mounted the link target read-write,
/// outside the workspace root. The exec is now `workspace.path-escape` before any mount, the
/// answer the file route gives for the same entry.
#[tokio::test(flavor = "multi_thread")]
async fn an_adopted_directory_swapped_for_a_symlink_never_mounts_the_link_target() {
    let Some(delegated) = DelegatedCgroup::acquire() else {
        return;
    };
    let temporary = TempDir::with_prefix("substrate-adversarial-swap-").expect("clean-room");
    let root = temporary.path();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
        .expect("owner-private clean-room directory");
    let daemon = Daemon::start(root, Some(delegated.path()), &[], &[], &[], false).await;
    let outside = root.join("outside-the-workspace-root");
    std::fs::create_dir(&outside).expect("a directory outside the workspace root");
    let adopted = daemon.workspaces.join("adopted-swap");
    std::fs::create_dir(&adopted).expect("the operator directory");
    let (workspace, snapshot) = adversarial_adopt(&daemon, "adopted-swap", "swap").await;

    // The operator replaces the adopted entry with a link to a directory outside the root.
    std::fs::rename(&adopted, daemon.workspaces.join("adopted-swap-before"))
        .expect("move the adopted directory aside");
    std::os::unix::fs::symlink(&outside, &adopted).expect("link in its place");

    // The file route refuses the link (openat2, O_NOFOLLOW).
    let (status, read) = daemon
        .call(
            "GET",
            &format!("/v1/workspaces/{workspace}/files/probe.txt?mode=file&offset=0&limit_bytes=1"),
            "req_adv_swap_list",
            None,
        )
        .await;
    expect_error(&(status, read), 422, "workspace.path-escape");

    let (status, answered) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_adv_swap_exec",
            Some(&mutation(
                "01JADVADOPTSWAPEXEC00001",
                &ExecInput::new(
                    &workspace,
                    &snapshot,
                    &[
                        "/usr/bin/sh",
                        "-c",
                        "printf escaped > /workspace/escaped.txt",
                    ],
                    true,
                )
                .build(),
            )),
        )
        .await;
    let escaped = outside.join("escaped.txt");
    assert!(
        !escaped.exists(),
        "the confined exec wrote outside the workspace root through the swapped entry: \
         {status} {answered}"
    );
    expect_error(&(status, answered), 422, "workspace.path-escape");
    daemon.close().await;
}

/// The directory an adopted row names is the operator's, and the operator may remove it. An exec
/// started afterwards must be a named refusal (invariant 3), never a run somewhere else.
#[tokio::test(flavor = "multi_thread")]
async fn an_adopted_directory_removed_after_adopt_is_a_named_refusal_at_exec() {
    let Some(delegated) = DelegatedCgroup::acquire() else {
        return;
    };
    let temporary = TempDir::with_prefix("substrate-adversarial-gone-").expect("clean-room");
    let root = temporary.path();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
        .expect("owner-private clean-room directory");
    let daemon = Daemon::start(root, Some(delegated.path()), &[], &[], &[], false).await;
    let adopted = daemon.workspaces.join("adopted-gone");
    std::fs::create_dir(&adopted).expect("the operator directory");
    let (workspace, snapshot) = adversarial_adopt(&daemon, "adopted-gone", "gone").await;
    std::fs::remove_dir(&adopted).expect("the operator removes the directory");

    let (status, answered) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_adv_gone_exec",
            Some(&mutation(
                "01JADVADOPTGONEEXEC00001",
                &ExecInput::new(&workspace, &snapshot, &["/usr/bin/true"], true).build(),
            )),
        )
        .await;
    expect_error(&(status, answered), 404, "resource.not-found");
    assert!(
        !adopted.exists(),
        "the exec recreated the operator's directory"
    );
    daemon.close().await;
}
