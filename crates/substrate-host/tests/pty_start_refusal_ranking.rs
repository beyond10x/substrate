#![forbid(unsafe_code)]
//! **The class**: the four checks that exist at *both* session-start entry points, and their rank.
//!
//! Round 1 found the two ports disagreeing about which of two applicable refusals answers, and
//! fixed the `fact`/`window` pair. Round 3 found the same defect surviving in the `fact`/`wait`
//! pair. A pair at a time is how this keeps recurring, so this pins the whole overlap.
//!
//! `ProcessRuntime::start_pipe` and `validate_pipe_session_input` overlap on exactly four checks:
//!
//! | check | refusal | rank |
//! |---|---|---|
//! | the `sessions.pty` fact | `session.pty-unserved` | 1 |
//! | the window shape | `session.window-invalid` | 2 |
//! | `exec.wait` | `session.wait-invalid` / schema-invalid | 3 |
//! | the session byte and queue bounds | `session.limit-unserved` / schema-invalid | 3 |
//!
//! `wait` and the bounds share a rank because the daemon tests them in one condition; the driver
//! splits them and keeps both below the window. Everything *outside* those four is each port's own
//! business — the daemon owns request shape, the driver owns host capability — and the daemon is
//! always reached first, so a driver refusal is only ever visible for something the daemon
//! admitted. That is why the invariant worth pinning is the rank of the overlap, not the whole
//! sequence.
//!
//! Portable lane: `HostConfig::minimum` on a temporary directory is a host with no terminals and no
//! confinement, which is the deployment the ordering decision is about.

use std::collections::BTreeMap;

use substrate_host::{DispatchOutcome, Driver as _, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode,
    PipeSessionStartInput, PtyWindow, SandboxProfile, SessionMode,
};

fn start(
    snapshot: &str,
    wait: bool,
    window: Option<PtyWindow>,
    queued_frames: u32,
) -> PipeSessionStartInput {
    PipeSessionStartInput {
        exec: ExecStartInput {
            workspace: "ws_test".to_owned(),
            argv: vec!["/bin/sh".to_owned()],
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
                output_bytes: 65_536,
                processes: 16,
                memory_bytes: 67_108_864,
                cpu_millis: 5_000,
            },
            wait,
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            lease_ttl_ms: Some(60_000),
        },
        input_limit_bytes: 65_536,
        frame_limit_bytes: 4_096,
        queued_frames,
        mode: SessionMode::Pty,
        window,
    }
}

/// Every pair in the overlap, each request earning both members, and the higher rank answering.
#[tokio::test(flavor = "multi_thread")]
async fn the_overlapping_checks_rank_the_same_at_the_driver_port() {
    let directory = tempfile::tempdir().expect("temporary host root");
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    let machine = driver.machine();
    assert_eq!(
        machine.facts.sessions_pty, None,
        "this case is about a deployment that never proved it can give a terminal"
    );
    std::fs::create_dir_all(driver.root().join("ws_test")).expect("workspace directory");
    let good = PtyWindow {
        columns: 80,
        rows: 24,
    };

    for (name, input, expected) in [
        // fact beats window
        (
            "no fact, no window",
            start(&machine.snapshot, false, None, 8),
            substrate_wire::SESSION_PTY_UNSERVED,
        ),
        // fact beats wait
        (
            "no fact, wait",
            start(&machine.snapshot, true, Some(good), 8),
            substrate_wire::SESSION_PTY_UNSERVED,
        ),
        // fact beats the session bounds
        (
            "no fact, bad queue",
            start(&machine.snapshot, false, Some(good), 0),
            substrate_wire::SESSION_PTY_UNSERVED,
        ),
        // fact beats all three at once
        (
            "no fact, no window, wait, bad queue",
            start(&machine.snapshot, true, None, 0),
            substrate_wire::SESSION_PTY_UNSERVED,
        ),
    ] {
        let DispatchOutcome::NotDispatched(error) = driver
            .start_pipe_session("ex_rank", "ws_test", &input)
            .await
        else {
            panic!("{name}: a terminal must never be served as a pipe session instead");
        };
        assert_eq!(error.code, expected, "{name}");
    }

    // With the fact absent the gate answers everything above, so the window/wait and window/bounds
    // ranks are asserted in the mode the fact does not gate: a `pipes` start carrying a window
    // earns the window refusal, and must earn it before `wait` and before the bounds.
    for (name, mut input) in [
        ("pipes with a window and wait", {
            let mut value = start(&machine.snapshot, true, Some(good), 8);
            value.mode = SessionMode::Pipes;
            value
        }),
        ("pipes with a window and a bad queue", {
            let mut value = start(&machine.snapshot, false, Some(good), 0);
            value.mode = SessionMode::Pipes;
            value
        }),
    ] {
        input.window = Some(good);
        let DispatchOutcome::NotDispatched(error) = driver
            .start_pipe_session("ex_rank", "ws_test", &input)
            .await
        else {
            panic!("{name}: a start outside the closed shape must be refused");
        };
        assert_eq!(error.code, substrate_wire::SESSION_WINDOW_INVALID, "{name}");
    }
}
