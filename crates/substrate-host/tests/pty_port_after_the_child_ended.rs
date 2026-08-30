#![forbid(unsafe_code)]
//! **The class**, not the instance: every `Driver` port method that touches a pty session after its
//! child can have exited, and what each one must do about it.
//!
//! Round 1 corrected `resize_pty` for reporting a window as applied to a finished child. Round 3
//! found `write_pipe` making the same claim on the same file. Two instances of one class is a
//! signal that the class was never enumerated, so this enumerates it and pins every member — the
//! ones that must refuse *and* the ones that must not, because "refuses after exit" is wrong for
//! half of them and an unpinned decision is how the next one drifts.
//!
//! | port method | after the child ended | why |
//! |---|---|---|
//! | `write_pipe_session` | refuses `session.pty-ended` | the master accepts bytes no slave will read |
//! | `resize_pty_session` | refuses `session.pty-ended` | `TIOCSWINSZ` still lands; nothing observes it |
//! | `close_pipe_session_input` | refuses `session.input-close-unserved` | a pty has no half-close, in any state |
//! | `read_pipe_session` | **succeeds** | the tail and the end-of-file are how a client finishes |
//! | `signal` | **succeeds** | returns the terminal observation it already holds |
//! | `observe_exec` | **succeeds** | an observation is a fact about the past |
//! | `output` | **succeeds** | the durable transcript outlives the child by design |
//!
//! `set_exec_lease`, `acknowledge_exec`, `discard_superseded_exec` and `completed_execs` are the
//! remaining session-touching members. They are bookkeeping between the driver and the store, make
//! no claim to a client about a live child, and are not asserted here.
//!
//! Delegated lane only: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, the case is **absent, never reported as passed** (invariant 3).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use substrate_host::{DispatchOutcome, Driver as _, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecOutputQuery, ExecSignalInput,
    ExecStartInput, ExecState, NetworkMode, OutputStream, PipeSessionStartInput, PtyWindow,
    SandboxProfile, SessionMode,
};

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    _directory: tempfile::TempDir,
}

fn lane() -> Option<Lane> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        return None;
    }
    let directory = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.cgroup_root = Some(root);
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

fn pty_start(snapshot: &str) -> PipeSessionStartInput {
    PipeSessionStartInput {
        exec: ExecStartInput {
            workspace: "ws_test".to_owned(),
            argv: vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                "printf 'done\\n'".to_owned(),
            ],
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

#[tokio::test(flavor = "multi_thread")]
async fn every_port_method_states_its_own_answer_for_a_finished_pty_session() {
    let Some(lane) = lane() else {
        eprintln!("absent: no delegated cgroup root or no bubblewrap");
        return;
    };
    let id = "ex_ptyportclass";
    let DispatchOutcome::Observed(_) = lane
        .driver
        .start_pipe_session(id, "ws_test", &pty_start(&lane.snapshot))
        .await
    else {
        panic!("the delegated lane must start a pty session");
    };

    // Drain to end of file. This is the first member of the class and it must *succeed*: the tail
    // of the transcript and the end-of-file are how a client learns the session is over.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut transcript = Vec::new();
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
            Ok(Some(frame)) => transcript.extend_from_slice(&frame.bytes),
            Err(_timeout) => {}
        }
    }
    let observed = lane
        .driver
        .observe_exec(id)
        .await
        .expect("observe_exec answers for a finished session");
    assert_eq!(observed.resource.state, ExecState::Exited);

    // A read after end of file still answers, and still says end of file.
    assert!(
        lane.driver
            .read_pipe_session(id, Duration::from_millis(500))
            .await
            .expect("read_pipe_session answers for a finished session")
            .is_none()
    );

    // The two that make a claim about a live child, and must not.
    assert_eq!(
        lane.driver
            .write_pipe_session(id, b"nobody reads this\n")
            .await
            .expect_err("input to a finished pty session is refused")
            .code,
        substrate_wire::SESSION_PTY_ENDED
    );
    assert_eq!(
        lane.driver
            .resize_pty_session(
                id,
                PtyWindow {
                    columns: 132,
                    rows: 43
                }
            )
            .expect_err("a resize on a finished pty session is refused")
            .code,
        substrate_wire::SESSION_PTY_ENDED
    );

    // A pty has no half-close in any state, so this one answers the same before and after.
    assert_eq!(
        lane.driver
            .close_pipe_session_input(id)
            .await
            .expect_err("a pty session has no half-close")
            .code,
        substrate_wire::SESSION_INPUT_CLOSE_UNSERVED
    );

    // And the three that report the past, which is still there.
    let terminal = lane
        .driver
        .signal(
            id,
            &ExecSignalInput {
                signal: substrate_wire::Signal::Kill,
                grace_ms: 0,
            },
        )
        .await
        .expect("signal answers with the terminal observation it already holds");
    assert_eq!(terminal.resource.state, ExecState::Exited);
    let slice = lane
        .driver
        .output(
            id,
            &ExecOutputQuery {
                stream: OutputStream::Stdout,
                offset: 0,
                limit_bytes: 4_096,
            },
        )
        .await
        .expect("the durable transcript outlives the child");
    assert!(
        slice.returned_bytes > 0,
        "the child printed before it exited: {transcript:?}"
    );
}
