use std::process::Stdio;

use base64::Engine as _;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

fn mcp_binary() -> std::path::PathBuf {
    std::env::var_os("SUBSTRATE_MCP_BINARY")
        .map_or_else(|| env!("CARGO_BIN_EXE_substrate-mcp").into(), Into::into)
}

fn mcp_command() -> Command {
    if let Some(image) = std::env::var_os("SUBSTRATE_MCP_DOCKER_IMAGE") {
        let mut command = Command::new("docker");
        command.args([
            "run",
            "--rm",
            "-i",
            "--network=none",
            "--read-only",
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,mode=1777",
            "--pids-limit=128",
            "--memory=512m",
            "--cpus=2",
        ]);
        command.arg(image);
        command
    } else {
        Command::new(mcp_binary())
    }
}

struct Session {
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
}

async fn initialized_native_session() -> Option<(tokio::process::Child, Session)> {
    if std::env::var_os("SUBSTRATE_MCP_CGROUP_ROOT").is_some()
        || std::env::var_os("SUBSTRATE_MCP_DOCKER_IMAGE").is_some()
    {
        return None;
    }
    let mut child = Command::new(mcp_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn native MCP binary");
    let mut session = Session {
        input: child.stdin.take().expect("MCP stdin"),
        output: BufReader::new(child.stdout.take().expect("MCP stdout")),
        next_id: 1,
    };
    let initialized = session
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "lifecycle-test", "version": "1"}
            }),
        )
        .await;
    assert_eq!(initialized["serverInfo"]["name"], "b10x-substrate-mcp");
    Some((child, session))
}

fn direct_child_pid(parent: u32) -> i32 {
    let children = std::fs::read_to_string(format!("/proc/{parent}/task/{parent}/children"))
        .expect("read adapter child list");
    children
        .split_whitespace()
        .next()
        .expect("linked daemon child")
        .parse()
        .expect("numeric daemon pid")
}

async fn await_process_absent(pid: i32) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async move {
        loop {
            if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("process absence deadline");
}

impl Session {
    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.input
            .write_all(
                serde_json::to_string(&request)
                    .expect("encode request")
                    .as_bytes(),
            )
            .await
            .expect("write request");
        self.input.write_all(b"\n").await.expect("write newline");
        self.input.flush().await.expect("flush request");
        let mut line = String::new();
        self.output
            .read_line(&mut line)
            .await
            .expect("read response");
        let response: Value = serde_json::from_str(&line).expect("parse response");
        assert_eq!(response["id"], id);
        response["result"].clone()
    }

    async fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
            .await
    }
}

#[tokio::test]
async fn sigint_and_sigterm_orderly_shutdown_reap_the_linked_daemon() {
    for signal in [
        nix::sys::signal::Signal::SIGTERM,
        nix::sys::signal::Signal::SIGINT,
    ] {
        let Some((mut child, mut session)) = initialized_native_session().await else {
            return;
        };
        let parent = child.id().expect("adapter pid");
        let parent_pid = i32::try_from(parent).expect("adapter pid fits i32");
        let daemon = direct_child_pid(parent);
        let created = session
            .call(
                "workspace_create",
                json!({"operation_id": ulid::Ulid::generate().to_string()}),
            )
            .await;
        assert_eq!(created["isError"], false, "{created}");
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(parent_pid), signal)
            .expect("signal adapter");
        let status = tokio::time::timeout(std::time::Duration::from_secs(20), child.wait())
            .await
            .unwrap_or_else(|_| panic!("orderly {signal:?} shutdown deadline"))
            .expect("wait adapter");
        assert!(status.success(), "orderly signal cleanup failed: {status}");
        await_process_absent(daemon).await;
    }
}

#[tokio::test]
async fn abrupt_parent_death_still_ends_the_linked_daemon() {
    let Some((mut child, _session)) = initialized_native_session().await else {
        return;
    };
    let parent = child.id().expect("adapter pid");
    let parent_pid = i32::try_from(parent).expect("adapter pid fits i32");
    let daemon = direct_child_pid(parent);
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(parent_pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("kill adapter");
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait())
        .await
        .expect("abrupt shutdown deadline")
        .expect("wait adapter");
    assert!(!status.success(), "SIGKILL unexpectedly reported success");
    await_process_absent(daemon).await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One shipped-binary journey proves state and operation ordering.
async fn stdio_adapter_serves_the_portable_or_delegated_clean_room_journey() {
    let delegated_root = std::env::var_os("SUBSTRATE_MCP_CGROUP_ROOT");
    let mut child = mcp_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn shipped MCP binary");
    let mut session = Session {
        input: child.stdin.take().expect("MCP stdin"),
        output: BufReader::new(child.stdout.take().expect("MCP stdout")),
        next_id: 1,
    };
    let initialized = session
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "clean-room", "version": "1"}
            }),
        )
        .await;
    assert_eq!(initialized["serverInfo"]["name"], "b10x-substrate-mcp");

    let machine = session.call("machine_get", json!({})).await;
    assert_eq!(machine["isError"], false);
    assert!(machine["structuredContent"]["facts"].is_object());
    let machine_resource = session
        .request("resources/read", json!({"uri": "substrate://machine"}))
        .await;
    let machine_resource_value: Value = serde_json::from_str(
        machine_resource["contents"][0]["text"]
            .as_str()
            .expect("machine resource text"),
    )
    .expect("machine resource JSON");
    assert_eq!(machine_resource_value, machine["structuredContent"]);

    let create_operation = ulid::Ulid::generate().to_string();
    let created = session
        .call(
            "workspace_create",
            json!({"operation_id": create_operation}),
        )
        .await;
    assert_eq!(created["isError"], false, "{created}");
    let workspace = created["structuredContent"]["observation"]["id"]
        .as_str()
        .expect("workspace id")
        .to_owned();

    let write_operation = ulid::Ulid::generate().to_string();
    let written = session
        .call(
            "workspace_file_write",
            json!({
                "operation_id": write_operation,
                "workspace_id": workspace,
                "path": "hello.txt",
                "content_base64": base64::engine::general_purpose::STANDARD.encode(b"hello MCP")
            }),
        )
        .await;
    assert_eq!(written["isError"], false, "{written}");
    let read = session
        .call(
            "workspace_file_read",
            json!({"workspace_id": workspace, "path": "hello.txt", "offset": 0, "limit_bytes": 32}),
        )
        .await;
    assert_eq!(read["isError"], false, "{read}");
    assert_eq!(
        read["structuredContent"]["content_base64"],
        base64::engine::general_purpose::STANDARD.encode(b"hello MCP")
    );
    assert!(read["structuredContent"].get("bytes").is_none());
    let file_resource = session
        .request(
            "resources/read",
            json!({"uri": format!(
                "substrate://workspaces/{workspace}/files/hello.txt?offset=0&limit=32"
            )}),
        )
        .await;
    let file_resource_value: Value = serde_json::from_str(
        file_resource["contents"][0]["text"]
            .as_str()
            .expect("file resource text"),
    )
    .expect("file resource JSON");
    assert_eq!(
        file_resource_value["content_base64"],
        read["structuredContent"]["content_base64"]
    );
    assert_eq!(
        file_resource_value["sha256"],
        read["structuredContent"]["sha256"]
    );
    assert_eq!(
        file_resource_value["next_offset"],
        read["structuredContent"]["next_offset"]
    );

    let cleanup_identity = if delegated_root.is_some() {
        let metrics_supported = machine["structuredContent"]["facts"]
            .get("exec.resource-usage")
            .is_some_and(Value::is_object);
        let exec_operation = ulid::Ulid::generate().to_string();
        let started = session
            .call(
                "exec_start",
                json!({
                    "operation_id": exec_operation,
                    "workspace_id": workspace,
                    "argv": ["/usr/bin/sha256sum", "/workspace/hello.txt"],
                    "timeout_ms": 5000,
                    "cpu_millis": 5000,
                    "memory_bytes": 64 * 1024 * 1024,
                    "processes": 4,
                    "output_bytes": 4096,
                    "measure_resource_usage": metrics_supported
                }),
            )
            .await;
        assert_eq!(started["isError"], false, "{started}");
        let exec_id = started["structuredContent"]["observation"]["id"]
            .as_str()
            .expect("exec id")
            .to_owned();
        let waited = session
            .call("exec_wait", json!({"exec_id": exec_id, "timeout_ms": 5000}))
            .await;
        assert_eq!(waited["isError"], false, "{waited}");
        assert_eq!(waited["structuredContent"]["state"], "Exited");
        let output = session
            .call(
                "exec_output_read",
                json!({"exec_id": exec_id, "stream": "stdout", "offset": 0, "limit_bytes": 4096}),
            )
            .await;
        assert_eq!(output["isError"], false, "{output}");
        let stdout = base64::engine::general_purpose::STANDARD
            .decode(
                output["structuredContent"]["content_base64"]
                    .as_str()
                    .expect("output base64"),
            )
            .expect("decode output");
        assert_eq!(
            String::from_utf8(stdout).expect("utf8 output"),
            "cd0af7201d1d112b6e788465c310f6af91746a510a1de5b0aa9100bce6274612  /workspace/hello.txt\n"
        );
        if metrics_supported {
            let metrics = session
                .call(
                    "metrics_get",
                    json!({"resource_kind": "exec", "resource_id": exec_id}),
                )
                .await;
            assert_eq!(metrics["isError"], false, "{metrics}");
            assert!(metrics["structuredContent"]["usage"]["wall_ms"].is_number());
        }
        let retired = session
            .call(
                "exec_retire",
                json!({"operation_id": ulid::Ulid::generate().to_string(), "exec_id": exec_id}),
            )
            .await;
        assert_eq!(retired["isError"], false, "{retired}");

        let active = session
            .call(
                "exec_start",
                json!({
                    "operation_id": ulid::Ulid::generate().to_string(),
                    "workspace_id": workspace,
                    "argv": ["/usr/bin/sh", "-c", "echo $$ >/workspace/child.pid; exec sleep 600"],
                    "timeout_ms": 30_000,
                    "cpu_millis": 5000,
                    "memory_bytes": 64 * 1024 * 1024,
                    "processes": 4,
                    "output_bytes": 4096
                }),
            )
            .await;
        assert_eq!(active["isError"], false, "{active}");
        let active_id = active["structuredContent"]["observation"]["id"]
            .as_str()
            .expect("active exec id")
            .to_owned();
        let cgroup = active["structuredContent"]["observation"]["applied"]["cgroup"]
            .as_str()
            .expect("applied cgroup")
            .to_owned();
        let child_pid = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let read = session
                    .call(
                        "workspace_file_read",
                        json!({"workspace_id": workspace, "path": "child.pid", "offset": 0, "limit_bytes": 64}),
                    )
                    .await;
                if read["isError"] == false {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(read["structuredContent"]["content_base64"].as_str().expect("pid base64"))
                        .expect("decode pid");
                    break String::from_utf8(bytes)
                        .expect("pid utf8")
                        .trim()
                        .parse::<i32>()
                        .expect("numeric pid");
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("child pid deadline");
        assert!(!active_id.is_empty());
        Some((child_pid, cgroup))
    } else {
        let exec_operation = ulid::Ulid::generate().to_string();
        let refused = session
            .call(
                "exec_start",
                json!({
                    "operation_id": exec_operation,
                    "workspace_id": workspace,
                    "argv": ["/usr/bin/printf", "should-not-run"],
                    "timeout_ms": 1000,
                    "cpu_millis": 1000,
                    "memory_bytes": 1_048_576,
                    "processes": 1,
                    "output_bytes": 1024
                }),
            )
            .await;
        assert_eq!(refused["isError"], true, "{refused}");
        assert_eq!(
            refused["structuredContent"]["error"]["code"],
            "exec.sandbox-unavailable"
        );
        assert_eq!(
            refused["structuredContent"]["error"]["operation_id"],
            exec_operation
        );
        let operation = session
            .call("operation_get", json!({"operation_id": exec_operation}))
            .await;
        assert_eq!(operation["isError"], false, "{operation}");
        assert_eq!(operation["structuredContent"]["id"], exec_operation);
        assert_eq!(
            operation["structuredContent"]["refusal"]["class"],
            "unserved"
        );
        let operation_resource = session
            .request(
                "resources/read",
                json!({"uri": format!("substrate://operations/{exec_operation}")}),
            )
            .await;
        let operation_resource_value: Value = serde_json::from_str(
            operation_resource["contents"][0]["text"]
                .as_str()
                .expect("operation resource text"),
        )
        .expect("operation resource JSON");
        assert_eq!(operation_resource_value, operation["structuredContent"]);
        let destroyed = session
            .call(
                "workspace_destroy",
                json!({"operation_id": ulid::Ulid::generate().to_string(), "workspace_id": workspace}),
            )
            .await;
        assert_eq!(destroyed["isError"], false, "{destroyed}");
        assert_eq!(destroyed["structuredContent"]["absent"], true);
        None
    };

    drop(session.input);
    let status = tokio::time::timeout(std::time::Duration::from_secs(20), child.wait())
        .await
        .expect("adapter exits before deadline")
        .expect("wait for adapter");
    assert!(status.success(), "adapter cleanup failed: {status}");
    if let (Some(root), Some((pid, cgroup))) = (delegated_root, cleanup_identity) {
        assert!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err(),
            "active workload survived adapter EOF cleanup"
        );
        assert!(
            !std::path::PathBuf::from(root).join(cgroup).exists(),
            "active execution cgroup survived adapter EOF cleanup"
        );
    }
}
