//! Production daemon/driver/store privacy evidence. The delegated lane is explicit.
use b10x_substrate_sdk::{
    CaptureMode, Client, ExecMeasurement, ExecState, ExecutionPolicy, PipeFrame, PtyWindow,
    SdkError, Signal,
};
use std::os::unix::fs::PermissionsExt as _;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};

fn binary() -> PathBuf {
    if let Some(path) = std::env::var_os("SUBSTRATE_TEST_DAEMON") {
        return path.into();
    }
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("substrate-daemon")
}
async fn start(root: &Path, cgroup: &Path, allow: bool) -> (Child, Client) {
    let mut command = Command::new(binary());
    command
        .args(["--socket"])
        .arg(root.join("socket"))
        .arg("--state")
        .arg(root.join("state.db"))
        .arg("--workspaces")
        .arg(root.join("workspaces"))
        .args(["--deployment", "privacy112", "--allow-uid"])
        .arg(nix::unistd::geteuid().as_raw().to_string())
        .arg("--cgroup-root")
        .arg(cgroup)
        .stdout(Stdio::from(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("stdout.log"))
                .unwrap(),
        ))
        .stderr(Stdio::from(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("stderr.log"))
                .unwrap(),
        ))
        .kill_on_drop(true);
    if allow {
        command.arg("--allow-unrecorded-sessions");
    }
    let mut child = command.spawn().unwrap();
    let client = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "daemon exited: {}",
                std::fs::read_to_string(root.join("stderr.log")).unwrap()
            );
            if let Ok(client) = Client::builder()
                .unix_socket(root.join("socket"))
                .connect()
                .await
            {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("daemon startup deadline");
    (child, client)
}
fn scan(root: &Path, canary: &[u8]) -> Vec<String> {
    use base64::Engine as _;
    let mut needles = vec![canary.to_vec()];
    for offset in 0..3 {
        let length = (canary.len() - offset) / 3 * 3;
        needles.push(
            base64::engine::general_purpose::STANDARD
                .encode(&canary[offset..offset + length])
                .into_bytes(),
        );
    }
    ["state.db", "state.db-wal", "stdout.log", "stderr.log"]
        .iter()
        .filter_map(|name| {
            let bytes = std::fs::read(root.join(name)).unwrap_or_default();
            needles
                .iter()
                .any(|needle| bytes.windows(needle.len()).any(|w| w == needle))
                .then(|| (*name).to_owned())
        })
        .collect()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn delegated_sdk_streaming_never_records_terminal_canaries() {
    let Some(cgroup) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") else {
        eprintln!("unrecorded SDK lane absent: SUBSTRATE_VECTORS_CGROUP_ROOT is not set");
        return;
    };
    let cgroup = PathBuf::from(cgroup);
    for pty in [false, true] {
        for ending in [
            "policy",
            "success",
            "stderr",
            "failure",
            "cancel",
            "expiry",
            "limit",
            "disconnect",
            "blocked",
            "persistence-error",
            "restart",
            "recorded",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
            let (mut child, mut client) = start(root, &cgroup, ending != "policy").await;
            assert_eq!(client.machine().facts.sessions_unrecorded, Some(true));
            let capabilities = client.session_capabilities().await.unwrap();
            assert_eq!(
                capabilities
                    .capture_modes
                    .contains(&CaptureMode::Unrecorded),
                ending != "policy"
            );
            assert_eq!(capabilities.max_attachments, 32);
            assert_eq!(capabilities.send_timeout_ms, 5_000);
            assert_eq!(capabilities.attachment_lifetime_ms, 3_600_000);
            let workspace = client.workspace().empty().create().await.unwrap();
            let unrecorded = ending != "recorded";
            let capture = if unrecorded {
                CaptureMode::Unrecorded
            } else {
                CaptureMode::Recorded
            };
            let shell = match ending {
                "stderr" => "read line; printf '%s' \"$line\" >&2",
                "failure" => "read line; printf '%s' \"$line\"; exit 7",
                "cancel" | "expiry" | "disconnect" | "restart" | "persistence-error" => {
                    "read line; printf '%s' \"$line\"; sleep 600"
                }
                "limit" => "read line; printf '%s' \"$line\"; /usr/bin/head -c 8192 /dev/zero",
                "blocked" => {
                    "read line; printf '%s' \"$line\"; while :; do printf '0123456789012345678901234567890123456789'; done"
                }
                _ => "read line; printf '%s' \"$line\"",
            };
            let limit = if ending == "limit" { 4096 } else { 1_048_576 };
            let policy = ExecutionPolicy::builder()
                .timeout(Duration::from_secs(20))
                .cpu_time(Duration::from_secs(5))
                .memory_bytes(67_108_864)
                .processes(16)
                .output_bytes(limit)
                .build()
                .unwrap();
            let builder = if pty {
                workspace.pty_session(
                    "/bin/sh",
                    PtyWindow {
                        columns: 80,
                        rows: 24,
                    },
                )
            } else {
                workspace.pipe_session("/bin/sh")
            };
            let metrics_supported = client.machine().facts.exec_resource_usage.is_some();
            let builder = if metrics_supported {
                builder.measure(ExecMeasurement::ResourceUsage)
            } else {
                eprintln!("unrecorded resource measurement lane absent: host counters unserved");
                builder
            };
            let result = builder
                .args(["-c", shell])
                .capture(capture)
                .policy(policy.clone())
                .lease(Duration::from_secs(if ending == "expiry" { 2 } else { 30 }))
                .input_limit_bytes(4096)
                .frame_limit_bytes(4096)
                .queued_frames(16)
                .start()
                .await;
            if ending == "policy" {
                assert!(
                    matches!(result, Err(SdkError::Refusal(ref r)) if r.code == "session.capture-disallowed")
                );
                let db = rusqlite::Connection::open(root.join("state.db")).unwrap();
                let count: i64 = db
                    .query_row("SELECT COUNT(*) FROM execs", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(
                    count, 0,
                    "policy refusal precedes execution reservation/dispatch"
                );
                child.kill().await.unwrap();
                child.wait().await.unwrap();
                continue;
            }
            let mut session = result.unwrap();
            assert_eq!(session.observation().capture, capture);
            let exec_id = session.observation().exec_id.clone();
            let mut channel = session.attach().await.unwrap();
            let canary = format!("SYNTHETIC_LIVE_112_{}", ulid::Ulid::generate());
            channel.write(format!("{canary}\n")).await.unwrap();
            let mut output = Vec::new();
            tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(frame) = channel.next_frame().await.unwrap() {
                    if let PipeFrame::Output { bytes, .. } = frame {
                        output.extend(bytes);
                    }
                    if output.windows(canary.len()).any(|w| w == canary.as_bytes()) {
                        break;
                    }
                }
            })
            .await
            .expect("live canary deadline");
            assert!(
                output.windows(canary.len()).any(|w| w == canary.as_bytes()),
                "{pty} {ending}"
            );
            if ending == "persistence-error" {
                let db = rusqlite::Connection::open(root.join("state.db")).unwrap();
                db.execute_batch("CREATE TRIGGER refuse_exec_update BEFORE UPDATE ON execs BEGIN SELECT RAISE(FAIL, 'synthetic persistence failure'); END;").unwrap();
                channel.signal(Signal::Kill, Duration::ZERO).await.unwrap();
                // Let the driver terminate while the real store persistently refuses the write.
                tokio::time::sleep(Duration::from_millis(750)).await;
                assert!(scan(root, canary.as_bytes()).is_empty());
                db.execute_batch("DROP TRIGGER refuse_exec_update;")
                    .unwrap();
            }
            if ending == "cancel" {
                session.signal(Signal::Kill, Duration::ZERO).await.unwrap();
            }
            if ending == "disconnect" {
                channel.close().await.unwrap();
            } else if ending == "restart" {
                assert!(scan(root, canary.as_bytes()).is_empty());
                child.kill().await.unwrap();
                child.wait().await.unwrap();
                let (next_child, next_client) = start(root, &cgroup, true).await;
                child = next_child;
                client = next_client;
            } else if ending != "blocked" {
                // Drain the terminal frame, bounded independently of the child's timeout.
                let _ = tokio::time::timeout(Duration::from_secs(12), async {
                    while let Ok(Some(_)) = channel.next_frame().await {}
                })
                .await;
            }
            let observed = tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    let exec = client.get_exec(&exec_id).await.unwrap();
                    if matches!(
                        exec.observation().state,
                        ExecState::Exited
                            | ExecState::Cancelled
                            | ExecState::Expired
                            | ExecState::Unknown
                    ) {
                        break exec;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .expect("finite termination deadline");
            let stats = observed.observation().unrecorded_output.as_ref();
            assert_eq!(stats.is_some(), unrecorded);
            if let Some(stats) = stats {
                assert!(stats.stdout_queued_bytes <= limit);
                assert!(stats.stderr_queued_bytes <= limit);
                assert!(stats.queue_high_water_frames <= 16);
                if !pty && ending == "success" {
                    assert_eq!(stats.stdout_observed_bytes, canary.len() as u64);
                    assert_eq!(stats.stdout_queued_bytes, canary.len() as u64);
                    assert_eq!(stats.stderr_observed_bytes, 0);
                    assert_eq!(stats.stderr_queued_bytes, 0);
                }
                if ending != "restart" && (pty || ending != "stderr") {
                    assert!(stats.stdout_observed_bytes >= canary.len() as u64);
                }
                if !pty && ending == "stderr" {
                    assert_eq!(stats.stderr_observed_bytes, canary.len() as u64);
                    assert_eq!(stats.stderr_queued_bytes, canary.len() as u64);
                    assert_eq!(stats.stdout_observed_bytes, 0);
                }
                let error = observed
                    .output_page(b10x_substrate_sdk::OutputStream::Stdout, 0, 1024)
                    .await
                    .unwrap_err();
                assert!(
                    matches!(error, SdkError::Refusal(ref refusal) if refusal.code == "exec.output-unrecorded")
                );
            }
            if ending == "limit" {
                assert_eq!(
                    observed
                        .observation()
                        .refusal
                        .as_ref()
                        .map(|r| r.code.as_str()),
                    Some("session.output-limit")
                );
            }
            if ending == "blocked" {
                assert_eq!(
                    observed
                        .observation()
                        .refusal
                        .as_ref()
                        .map(|r| r.code.as_str()),
                    Some("session.output-backpressure")
                );
            }
            if metrics_supported && ending != "restart" {
                assert!(
                    matches!(
                        observed.observation().usage,
                        Some(substrate_wire::ExecUsage::Observed(_))
                    ),
                    "resource usage survives {ending}"
                );
            }
            if ending == "cancel" {
                assert_eq!(observed.observation().state, ExecState::Cancelled);
                assert_eq!(
                    observed.observation().exit.as_ref().unwrap().signal,
                    Some(Signal::Kill)
                );
            }
            if ending == "expiry" {
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        let exec = client.get_exec(&exec_id).await.unwrap();
                        if exec.observation().lease.as_ref().unwrap().state == substrate_wire::LeaseState::Expired { break; }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                }).await.expect("expiry retains the authoritative expired lease even when cancellation wins the terminal observation race");
            }
            if let Some(applied) = observed.observation().applied.as_ref() {
                assert!(
                    !cgroup.join(&applied.cgroup).exists(),
                    "whole process tree must be reconciled before terminal evidence"
                );
            }
            if ending == "failure" {
                assert_eq!(observed.observation().exit.as_ref().unwrap().code, Some(7));
            }
            if ending == "success" {
                assert_eq!(observed.observation().exit.as_ref().unwrap().code, Some(0));
            }
            let ended = client
                .get_pipe_session(&session.observation().id)
                .await
                .unwrap();
            assert!(
                matches!(ended.attach().await, Err(SdkError::Refusal(ref r)) if r.code == "session.not-attachable")
            );
            // Assert while SQLite and its WAL remain live; shutdown/checkpoint cannot hide recording.
            assert!(root.join("state.db-wal").is_file());
            let hits = scan(root, canary.as_bytes());
            if unrecorded {
                assert!(hits.is_empty(), "{pty} {ending}: payload reached {hits:?}");
            } else {
                assert!(
                    !hits.is_empty(),
                    "recording control must be detected by the same sink scan"
                );
            }
            if ending == "success" {
                // More sequential attachments than the published global capacity: every completed
                // unrecorded attachment must give its permit back.
                for _ in 0..33 {
                    let builder = if pty {
                        workspace.pty_session(
                            "/bin/sh",
                            PtyWindow {
                                columns: 80,
                                rows: 24,
                            },
                        )
                    } else {
                        workspace.pipe_session("/bin/sh")
                    };
                    let next = builder
                        .args(["-c", "read line"])
                        .capture(CaptureMode::Unrecorded)
                        .policy(policy.clone())
                        .lease(Duration::from_secs(10))
                        .input_limit_bytes(1024)
                        .frame_limit_bytes(1024)
                        .queued_frames(2)
                        .start()
                        .await
                        .unwrap();
                    let mut attachment = next
                        .attach()
                        .await
                        .expect("attachment capacity is recovered");
                    attachment.write(b"done\n").await.unwrap();
                    tokio::time::timeout(Duration::from_secs(5), async {
                        // A terminal exec can close the transport without a WS close handshake;
                        // verify the authoritative terminal resource below, not transport EOF.
                        while let Ok(Some(_)) = attachment.next_frame().await {}
                        loop {
                            let exec = client.get_exec(&next.observation().exec_id).await.unwrap();
                            if exec.observation().state == ExecState::Exited {
                                assert_eq!(exec.observation().exit.as_ref().unwrap().code, Some(0));
                                assert!(
                                    !cgroup
                                        .join(&exec.observation().applied.as_ref().unwrap().cgroup)
                                        .exists()
                                );
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .expect("completed attachment releases its slot");
                }
                assert!(scan(root, canary.as_bytes()).is_empty());
            }
            child.kill().await.unwrap();
            child.wait().await.unwrap();
            eprintln!(
                "unrecorded conformance: {} {ending} passed",
                if pty { "pty" } else { "pipes" }
            );
        }
    }
}

#[tokio::test]
async fn unsupported_driver_refuses_unrecorded_before_dispatch() {
    let mut daemon = b10x_substrate_sdk::ManagedDaemon::builder()
        .temporary()
        .deployment("unsupported112")
        .external_binary(binary())
        .allow_unrecorded_sessions(true)
        .start()
        .await
        .unwrap();
    assert_ne!(
        daemon.client().machine().facts.sessions_unrecorded,
        Some(true)
    );
    let workspace = daemon.client().workspace().empty().create().await.unwrap();
    let policy = ExecutionPolicy::builder()
        .timeout(Duration::from_secs(5))
        .cpu_time(Duration::from_secs(1))
        .memory_bytes(67_108_864)
        .processes(16)
        .output_bytes(4096)
        .build()
        .unwrap();
    let result = workspace
        .pipe_session("/bin/true")
        .capture(CaptureMode::Unrecorded)
        .policy(policy)
        .lease(Duration::from_secs(5))
        .input_limit_bytes(1024)
        .frame_limit_bytes(1024)
        .queued_frames(2)
        .start()
        .await;
    assert!(
        matches!(result, Err(SdkError::Refusal(ref r)) if r.code == "session.capture-unserved")
    );
    daemon.shutdown().await.unwrap();
}
