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

use std::collections::BTreeSet;
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

/// The shipped binary, built by cargo before this integration test runs.
const DAEMON: &str = env!("CARGO_BIN_EXE_substrate-daemon");
const CGROUP_EXEC: &str = env!("CARGO_BIN_EXE_substrate-cgroup-exec");
const DAEMON_OVERRIDE_VARIABLE: &str = "SUBSTRATE_VECTORS_DAEMON";

fn daemon_binary() -> PathBuf {
    std::env::var_os(DAEMON_OVERRIDE_VARIABLE).map_or_else(|| PathBuf::from(DAEMON), PathBuf::from)
}

/// Selects the delegated lane, mirroring the predecessor's `--cgroup-root` argument.
const CGROUP_ROOT_VARIABLE: &str = "SUBSTRATE_VECTORS_CGROUP_ROOT";

struct DelegatedCgroup(PathBuf);

impl DelegatedCgroup {
    fn acquire() -> Option<Self> {
        let parent = std::env::var_os(CGROUP_ROOT_VARIABLE).map(PathBuf::from)?;
        let path = parent.join(format!("substrate-test-{}", ulid::Ulid::new()));
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

impl Drop for DelegatedCgroup {
    fn drop(&mut self) {
        std::fs::remove_dir(self.0.join("daemon")).unwrap_or_else(|error| {
            panic!(
                "per-test daemon cgroup {} was not empty at teardown: {error}",
                self.0.join("daemon").display()
            )
        });
        std::fs::remove_dir(&self.0).unwrap_or_else(|error| {
            panic!(
                "per-test delegated cgroup {} was not empty at teardown: {error}",
                self.0.display()
            )
        });
    }
}

/// The RFC 6455 sample key and the accept value it forces, as in `tests/websocket.rs`.
const HANDSHAKE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const HANDSHAKE_ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

const READINESS_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------------------------
// Wire client
// ---------------------------------------------------------------------------------------------

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn read_response(stream: &mut UnixStream) -> (u16, Vec<u8>) {
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
    let mut length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        assert_ne!(
            name, "transfer-encoding",
            "the clean-room client reads content-length responses only"
        );
        if name == "content-length" {
            length = Some(value.trim().parse::<usize>().expect("content-length value"));
        }
    }
    let length = length.expect("content-length response header");
    while buffer.len() < boundary + length {
        let mut chunk = [0_u8; 8192];
        let read = stream.read(&mut chunk).await.expect("read response body");
        assert!(read > 0, "connection closed mid-body");
        buffer.extend_from_slice(&chunk[..read]);
    }
    (status, buffer[boundary..boundary + length].to_vec())
}

/// One request per connection, exactly as the predecessor's `http.client` harness did.
async fn request(
    socket: &Path,
    method: &str,
    path: &str,
    request_id: &str,
    body: Option<&[u8]>,
) -> (u16, Value) {
    let mut stream = UnixStream::connect(socket)
        .await
        .unwrap_or_else(|error| panic!("connect {}: {error}", socket.display()));
    let mut head =
        format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nx-request-id: {request_id}\r\n");
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
    let (status, body) = read_response(&mut stream).await;
    let payload: Value = serde_json::from_slice(&body).expect("JSON response body");
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
        let accept = head
            .split("\r\n")
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("sec-websocket-accept"))
            .map(|(_, value)| value.trim().to_owned())
            .expect("sec-websocket-accept header");
        assert_eq!(accept, HANDSHAKE_ACCEPT);
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
    socket: PathBuf,
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
        let socket = root.join("substrate.sock");
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
        let deadline = Instant::now() + READINESS_TIMEOUT;
        while !socket.exists() {
            if let Some(status) = child.try_wait().expect("daemon status") {
                let error = (&mut stderr).await.unwrap_or_default();
                panic!("substrate-daemon exited before readiness ({status}): {error}");
            }
            assert!(
                Instant::now() < deadline,
                "substrate-daemon did not create its Unix socket"
            );
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
        }
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
        let sandbox = match self.aperture {
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
        input
    }
}

const NO_EGRESS_PROGRAM: &str = "import socket;socket.create_connection(('1.1.1.1',53),1)";

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

#[allow(clippy::too_many_lines)] // One sequential journey; splitting it would lose the ordering.
async fn check_confined_execs(
    daemon: &Daemon,
    workspace: &str,
    snapshot: &str,
    cgroup_root: &Path,
) -> usize {
    let mut passed = 0;

    let (status, executed) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_exec",
            Some(&mutation(
                "01JPHASE2CLEANEXEC0001",
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
            "req_clean_environment",
            Some(&mutation(
                "01JPHASE2CLEANENV000001",
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

    let (status, pwd_exec) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_pwd",
            Some(&mutation(
                "01JPHASE3CLEANPWD000001",
                &ExecInput::new(
                    workspace,
                    snapshot,
                    &["/usr/bin/test", "/workspace", "=", "/workspace"],
                    true,
                )
                .environment(json!({ "PWD": "/workspace" }))
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 200, "{pwd_exec}");
    assert_eq!(pwd_exec["result"]["exit"], EXITED_CLEANLY(), "{pwd_exec}");
    passed += 1;

    let (status, no_egress) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_no_egress",
            Some(&mutation(
                "01JPHASE2CLEANNOEGRESS1",
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

    let (status, pids_exec) = daemon
        .call(
            "POST",
            "/v1/execs",
            "req_clean_pids",
            Some(&mutation(
                "01JPHASE2CLEANPIDS00001",
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
            "req_clean_memory",
            Some(&mutation(
                "01JPHASE2CLEANMEMORY0001",
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
            "req_clean_truncation",
            Some(&mutation(
                "01JPHASE2CLEANFILL00001",
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
            "req_clean_timeout",
            Some(&mutation(
                "01JPHASE2CLEANTIMEOUT001",
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
            "req_clean_tree_start",
            Some(&mutation(
                "01JPHASE2CLEANTREE00001",
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
    wait_for_workspace_file(daemon, workspace, ".tree-ready", "req_clean_tree_ready").await;
    let (status, cancelled) = daemon
        .call(
            "POST",
            &format!("/v1/execs/{exec_id}/signal"),
            "req_clean_tree_signal",
            Some(&mutation(
                "01JPHASE2CLEANTREESIGNAL",
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
            "req_clean_trap_start",
            Some(&mutation(
                "01JPHASE3CLEANTRAPSTART1",
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
    wait_for_workspace_file(daemon, workspace, ".trap-ready", "req_clean_trap_ready").await;
    let (status, trapped) = daemon
        .call(
            "POST",
            &format!("/v1/execs/{}/signal", text(&trapping["result"]["id"])),
            "req_clean_trap_signal",
            Some(&mutation(
                "01JPHASE3CLEANTRAPSIGNAL",
                &json!({ "signal": "TERM", "grace_ms": 5000 }),
            )),
        )
        .await;
    assert_eq!(status, 200, "{trapped}");
    assert_eq!(trapped["result"]["state"], "exited", "{trapped}");
    assert_eq!(trapped["result"]["exit"], EXITED_CLEANLY(), "{trapped}");
    passed += 1;

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
        .call("GET", "/v1/pipe-sessions", "req_clean_pty_modes", None)
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
                "/v1/pipe-sessions",
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
            "/v1/pipe-sessions",
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

    let mut channel = SessionChannel::open(
        &daemon.socket,
        &format!("/v1/pipe-sessions/{session_id}/attach"),
    )
    .await;
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

#[allow(clippy::too_many_lines)] // One sequential journey; splitting it would lose the ordering.
async fn check_http_journey(
    daemon: &Daemon,
    cgroup_root: Option<&Path>,
    inside: u16,
    outside: u16,
    firehose: u16,
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

    expect_error(
        &daemon
            .call(
                "GET",
                &format!(
                    "/v1/workspaces/{workspace}/files/%2e%2e%2fetc%2fpasswd\
                     ?mode=file&offset=0&limit_bytes=16"
                ),
                "req_clean_escape",
                None,
            )
            .await,
        422,
        "workspace.path-escape",
    );
    passed += 1;

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

    if let Some(cgroup_root) = cgroup_root {
        passed += check_confined_execs(daemon, &workspace, &snapshot, cgroup_root).await;
        passed += check_confined_apertures(daemon, &workspace, &snapshot, inside, outside).await;
        passed += check_confined_aperture_ceiling(daemon, &workspace, &snapshot, firehose).await;
        passed += check_confined_secret_slots(daemon, &workspace, &snapshot).await;
        passed += check_confined_pty(daemon, &workspace, &snapshot).await;
    } else {
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
            .call("GET", "/v1/pipe-sessions", "req_clean_pty_modes", None)
            .await;
        assert_eq!(status, 501, "{capabilities}");
        expect_error(
            &daemon
                .call(
                    "POST",
                    "/v1/pipe-sessions",
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
    assert_eq!(status, 200);
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
// The lane
// ---------------------------------------------------------------------------------------------

/// The predecessor printed its case count; the port asserts it, so a case cannot vanish quietly.
const PORTABLE_CASES: usize = 35;
const DELEGATED_CASES: usize = 62;

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
    ];
    // The declared slot file: one bounded, owner-private regular file, written before the daemon
    // that must be able to read it exists. Its path is argv; its bytes never are.
    let slot_file = root.join("declared.slot");
    std::fs::write(&slot_file, SECRET_SLOT_VALUE).expect("write the declared slot file");
    std::fs::set_permissions(&slot_file, std::fs::Permissions::from_mode(0o600))
        .expect("restrict the declared slot file");
    let slots = vec![format!("{SECRET_SLOT_NAME}={}", slot_file.display())];
    let daemon = Daemon::start(
        root,
        delegated.as_ref().map(DelegatedCgroup::path),
        &declared,
        &slots,
        &[delegated_key_flag()],
        false,
    )
    .await;
    check_dual_daemon_refusal(&daemon).await;
    let passed = check_http_journey(
        &daemon,
        delegated.as_ref().map(DelegatedCgroup::path),
        inside.port(),
        outside.port(),
        firehose.port(),
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
