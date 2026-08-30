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
//! no-egress, pids/memory, timeout, truncation and whole-tree cancellation cases. When the
//! variable is unset the delegated cases are *absent*: they are not run and are not counted.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

/// The shipped binary, built by cargo before this integration test runs.
const DAEMON: &str = env!("CARGO_BIN_EXE_substrate-daemon");

/// Selects the delegated lane, mirroring the predecessor's `--cgroup-root` argument.
const CGROUP_ROOT_VARIABLE: &str = "SUBSTRATE_VECTORS_CGROUP_ROOT";

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
    async fn start(root: &Path, cgroup_root: Option<&Path>, apertures: &[String]) -> Self {
        let socket = root.join("substrate.sock");
        let workspaces = root.join("workspaces");
        let mut command = vec![
            DAEMON.to_owned(),
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
        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .env("SUBSTRATE_TEST_SECRET_SENTINEL", "must-not-reach-child")
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

    async fn close(mut self) {
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
        Command::new(DAEMON)
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
        }
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
        json!({
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
        })
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
    "import os,signal,time;",
    "signal.signal(signal.SIGTERM,signal.SIG_IGN);",
    "os.fork();time.sleep(60)",
);

const TRAP_PROGRAM: &str = "trap 'exit 0' TERM; echo ready; while :; do sleep 1; done";

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
    tokio::time::sleep(Duration::from_millis(100)).await;
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
                    &["/usr/bin/sh", "-c", TRAP_PROGRAM],
                    false,
                )
                .build(),
            )),
        )
        .await;
    assert_eq!(status, 202, "{trapping}");
    tokio::time::sleep(Duration::from_millis(100)).await;
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

    if let Some(cgroup_root) = cgroup_root {
        passed += check_confined_execs(daemon, &workspace, &snapshot, cgroup_root).await;
        passed += check_confined_apertures(daemon, &workspace, &snapshot, inside, outside).await;
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
// The lane
// ---------------------------------------------------------------------------------------------

/// The predecessor printed its case count; the port asserts it, so a case cannot vanish quietly.
const PORTABLE_CASES: usize = 29;
const DELEGATED_CASES: usize = 42;

#[tokio::test(flavor = "multi_thread")]
async fn runtime_clean_room_drives_the_shipped_daemon_over_its_unix_socket() {
    let delegated = std::env::var_os(CGROUP_ROOT_VARIABLE).map(PathBuf::from);
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
    let declared = vec![format!("model=127.0.0.1:{}/tcp", inside.port())];
    let daemon = Daemon::start(root, delegated.as_deref(), &declared).await;
    check_dual_daemon_refusal(&daemon).await;
    let passed =
        check_http_journey(&daemon, delegated.as_deref(), inside.port(), outside.port()).await;
    daemon.close().await;
    inside_server.abort();
    outside_server.abort();
    let (lane, expected) = if delegated.is_some() {
        ("delegated", DELEGATED_CASES)
    } else {
        ("portable", PORTABLE_CASES)
    };
    assert_eq!(passed, expected, "{lane} lane case inventory");
    println!(
        "runtime clean-room: {passed} HTTP cases, startup refusal, \
         and dual-daemon refusal passed ({lane} lane)"
    );
}
