//! Run as the admitted non-root UID inside an isolated final-image test container.

use std::error::Error;
use std::path::Path;
use std::time::Duration;

use b10x_substrate_sdk::{
    Client, ExecMeasurement, ExecUsage, ExecutionPolicy, MetricsObservation, MetricsResourceKind,
    PipeFrame, PipeSession, PtyWindow, Signal,
};

#[tokio::main(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // One real session owns the observations and cleanup being proved.
async fn main() -> Result<(), Box<dyn Error>> {
    let socket = std::env::args()
        .nth(1)
        .ok_or("expected test daemon socket")?;
    let client = Client::builder().unix_socket(socket).connect().await?;
    require(
        client.machine().facts.sessions_pty == Some(true),
        "PTY capability absent",
    )?;
    let workspace = client.workspace().empty().create().await?;
    let mut owned_session: Option<PipeSession> = None;
    let result: Result<(), Box<dyn Error>> = async {
        workspace.write_file("roundtrip.txt", b"container-backed workspace").await?;
        require(workspace.read_file("roundtrip.txt", 0, 128).await?.bytes == b"container-backed workspace", "workspace bytes differ")?;
        let policy = ExecutionPolicy::builder()
            .timeout(Duration::from_secs(30))
            .cpu_time(Duration::from_secs(5))
            .memory_bytes(64 * 1024 * 1024)
            .processes(16)
            .output_bytes(64 * 1024)
            .build()?;
        let mut session = workspace.pty_session("/bin/sh", PtyWindow { columns: 80, rows: 24 })
            .args(["-c", "sleep 600 & child=$!; kill -0 \"$child\" || exit 1; echo \"$child\" >/workspace/descendant.pid; cat /proc/$$/status >/workspace/worker.status; read line; stty size; printf 'observed:%s\\n' \"$line\"; wait"])
            .policy(policy).lease(Duration::from_secs(20))
            .input_limit_bytes(4096).frame_limit_bytes(4096).queued_frames(16)
            .measure(ExecMeasurement::ResourceUsage).start().await?;
        owned_session = Some(session.clone());
        let exec_id = session.observation().exec_id.clone();
        let sentinel = ulid::Ulid::generate().to_string();
        let mut channel = session.attach().await?;
        channel.resize(PtyWindow { columns: 100, rows: 40 }).await?;
        channel.write(format!("{sentinel}\n").as_bytes()).await?;
        let mut output = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(frame) = channel.next_frame().await? {
                if let PipeFrame::Output { bytes, .. } = frame {
                    output.extend(bytes);
                    let text = String::from_utf8_lossy(&output);
                    if text.contains("40 100") && text.contains(&format!("observed:{sentinel}")) {
                        return Ok::<_, Box<dyn Error>>(());
                    }
                }
            }
            Err("PTY closed before observing input and resize".into())
        }).await.map_err(|_| "PTY input/resize deadline expired")??;
        let worker = String::from_utf8(workspace.read_file("worker.status", 0, 8192).await?.bytes)?;
        for capability in ["CapEff:", "CapPrm:", "CapInh:", "CapAmb:", "CapBnd:"] {
            let value = worker.lines().find_map(|line| line.strip_prefix(capability)).ok_or("missing worker capability")?;
            require(u64::from_str_radix(value.trim(), 16)? == 0, format!("worker retains {capability}"))?;
        }
        require(worker.lines().any(|line| line.strip_prefix("NoNewPrivs:").is_some_and(|value| value.trim() == "1")), "worker no_new_privs is absent")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match client.metrics(MetricsResourceKind::Exec, &exec_id).await? {
                    MetricsObservation::Exec { exec, usage } if exec == exec_id => match usage {
                        ExecUsage::Observed(usage) => {
                            require(usage.memory_current_bytes.is_some_and(|bytes| bytes > 0), "live memory observation missing")?;
                            require(usage.processes_current.is_some_and(|count| count >= 2), "live process tree observation missing")?;
                            break Ok::<_, Box<dyn Error>>(());
                        }
                        ExecUsage::Pending { .. } => tokio::time::sleep(Duration::from_millis(25)).await,
                        ExecUsage::Unavailable { code, .. } => return Err(format!("metrics unavailable: {code}").into()),
                    },
                    _ => return Err("metrics refer to a different resource".into()),
                }
            }
        }).await.map_err(|_| "observed metrics deadline expired")??;
        let exec = client.get_exec(&exec_id).await?;
        let cgroup = Path::new("/sys/fs/cgroup").join(&exec.observation().applied.as_ref().ok_or("missing applied isolation")?.cgroup);
        let pids: Vec<u32> = std::fs::read_to_string(cgroup.join("cgroup.procs"))?.split_whitespace().map(str::parse).collect::<Result<_, _>>()?;
        require(pids.len() >= 2, "running PTY has no process tree")?;
        let descendant: u32 = String::from_utf8(workspace.read_file("descendant.pid", 0, 32).await?.bytes)?.trim().parse()?;
        let observed_descendant = pids.iter().any(|pid| {
            let root = Path::new("/proc").join(pid.to_string());
            let status = std::fs::read_to_string(root.join("status")).unwrap_or_default();
            let namespace_pid = status.lines().find_map(|line| line.strip_prefix("NSpid:")).and_then(|line| line.split_whitespace().last()).and_then(|pid| pid.parse::<u32>().ok());
            namespace_pid == Some(descendant) && std::fs::read_to_string(root.join("comm")).is_ok_and(|comm| comm.trim() == "sleep")
        });
        require(observed_descendant, "the acknowledged background descendant is absent from the exec cgroup")?;
        session.signal(Signal::Kill, Duration::ZERO).await?;
        drop(channel);
        tokio::time::timeout(Duration::from_secs(5), async {
            while cgroup.exists() || pids.iter().any(|pid| Path::new("/proc").join(pid.to_string()).exists()) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }).await.map_err(|_| "whole-tree cleanup deadline expired")?;
        owned_session = None;
        Ok(())
    }.await;
    let session_cleanup: Result<(), Box<dyn Error>> = match owned_session.as_mut() {
        Some(session) => match tokio::time::timeout(
            Duration::from_secs(5),
            session.signal(Signal::Kill, Duration::ZERO),
        )
        .await
        {
            Ok(result) => result.map(|_| ()).map_err(Into::into),
            Err(_) => Err("test session cleanup timed out".into()),
        },
        None => Ok(()),
    };
    let workspace_cleanup: Result<bool, Box<dyn Error>> =
        match tokio::time::timeout(Duration::from_secs(5), workspace.destroy()).await {
            Ok(result) => result.map_err(Into::into),
            Err(_) => Err("test workspace cleanup timed out".into()),
        };
    if let Err(error) = &session_cleanup {
        eprintln!("test session cleanup failed: {error}");
    }
    if let Err(error) = &workspace_cleanup {
        eprintln!("test workspace cleanup failed: {error}");
    }
    result?;
    session_cleanup?;
    require(workspace_cleanup?, "test workspace was not destroyed")?;
    println!(
        "PASS final-image workspace bytes, PTY input/resize, empty worker capabilities, observed metrics, acknowledged descendant and whole-tree cleanup"
    );
    Ok(())
}

fn require(condition: bool, message: impl Into<String>) -> Result<(), Box<dyn Error>> {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}
