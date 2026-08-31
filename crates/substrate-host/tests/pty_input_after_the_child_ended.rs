#![forbid(unsafe_code)]
//! Writing to a pty session whose child has finished.
//!
//! Round 1 found that `resize_pty` reported a window as applied after the child exited, because the
//! master descriptor outlives the child and `TIOCSWINSZ` keeps succeeding on it. Round 2 fixed that
//! one call, minting `substrate_wire::SESSION_PTY_ENDED` for the condition and naming it in the
//! bundle: "this exec *is* a pty session, and what is wrong is that nothing is left to observe"
//! (`crates/substrate-host/src/process.rs:797-803`).
//!
//! `write_pipe` sits just above `resize_pty` in the same file and takes the same shortcut. Its
//! raw-pipe branch reads the descriptor before writing and refuses when it is gone
//! (`crates/substrate-host/src/process.rs:715-721`); its terminal branch returns
//! `terminal.write_all(bytes)` directly (`:708-713`), with no reading of the session's state at
//! all. The master outlives the child exactly as it does for the ioctl, and its input queue keeps
//! accepting bytes with no slave left to read them, so the same argument applies word for word:
//! reporting the write as delivered tells a client its bytes reached a process that has finished.
//!
//! Delegated lane only. Without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, the case is **absent, never reported as passed** (invariant 3).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use substrate_host::{DispatchOutcome, Driver as _, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode,
    PipeSessionStartInput, PtyWindow, SandboxProfile, SessionMode,
};

fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    _directory: tempfile::TempDir,
}

fn lane() -> Option<Lane> {
    let delegated = delegated_cgroup_root()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        return None;
    }
    let directory = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.cgroup_root = Some(delegated);
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    assert_eq!(
        machine.facts.sessions_pty,
        Some(true),
        "the delegated lane's own probe must publish sessions.pty"
    );
    std::fs::create_dir_all(driver.root().join("ws_test")).expect("workspace directory");
    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        _directory: directory,
    })
}

fn pty_start(snapshot: &str, argv: &[&str]) -> PipeSessionStartInput {
    PipeSessionStartInput {
        exec: ExecStartInput {
            workspace: "ws_test".to_owned(),
            argv: argv.iter().map(|part| (*part).to_owned()).collect(),
            env: ExecEnvironment {
                allow: vec![],
                set: BTreeMap::new(),
            },
            sandbox: ConfinementRequest {
                capability_snapshot: snapshot.to_owned(),
                network: NetworkMode::None,
                aperture: None,
                profile: SandboxProfile::Workspace,
                required: true,
            },
            limits: ExecLimits {
                timeout_ms: 60_000,
                output_bytes: 1_048_576,
                processes: 16,
                memory_bytes: 67_108_864,
                cpu_millis: 5_000,
            },
            wait: false,
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            lease_ttl_ms: Some(60_000),
        },
        input_limit_bytes: 65_536,
        frame_limit_bytes: 4_096,
        queued_frames: 8,
        mode: SessionMode::Pty,
        window: Some(PtyWindow {
            columns: 80,
            rows: 24,
        }),
    }
}

/// Input written to a pty session whose child has finished is refused, not reported as delivered.
#[tokio::test(flavor = "multi_thread")]
async fn input_after_the_child_ended_is_refused_rather_than_reported_delivered() {
    let Some(lane) = lane() else {
        eprintln!("absent: no delegated cgroup root or no bubblewrap");
        return;
    };
    let id = "ex_ptyinputafterend";
    let DispatchOutcome::Observed(_) = lane
        .driver
        .start_pipe_session(id, "ws_test", &pty_start(&lane.snapshot, &["/bin/true"]))
        .await
    else {
        panic!("the delegated lane must start a pty session");
    };

    // Drain to end of file, which for a terminal is the master reading EIO once every slave has
    // closed — that is, once the child and its whole tree are gone.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the child must reach its end within the deadline"
        );
        match lane
            .driver
            .read_pipe_session(id, Duration::from_millis(500))
            .await
        {
            Ok(None) => break,
            Ok(Some(_frame)) => {}
            Err(_timeout) => {}
        }
    }
    let observation = lane
        .driver
        .observe_exec(id)
        .await
        .expect("a terminal observation");
    assert_eq!(
        observation.resource.state,
        substrate_wire::ExecState::Exited,
        "the child ran to completion"
    );

    let written = lane
        .driver
        .write_pipe_session(id, b"nobody reads this\n")
        .await;
    let error = written.expect_err(
        "a pty session whose child has finished has nothing that can read input; reporting the \
         write as delivered is the same claim `resize_pty` was corrected for making",
    );
    assert_eq!(
        error.code,
        substrate_wire::SESSION_PTY_ENDED,
        "the condition already has a name, minted for it one round ago"
    );
}
