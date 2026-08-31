#![forbid(unsafe_code)]
//! **The class**: the four checks that exist at *both* session-start entry points, and their rank.
//!
//! Round 1 found the two ports disagreeing about which of two applicable refusals answers, and
//! fixed the `fact`/`window` pair. Round 3 found the same defect surviving in the `fact`/`wait`
//! pair. A pair at a time is how this keeps recurring, so this pins the whole overlap.
//!
//! **The criterion, re-derived.** The old one — "the daemon owns request shape" — did not
//! partition: `wait` is request shape too, and it was inside the enumeration while the exec shape
//! was outside it. A check is in the overlap iff **both ports can refuse the same request for the
//! same reason**, a reason being a request field together with the host fact it is read against.
//! Applied to the code, that yields six members, not four:
//!
//! | # | check | daemon | driver |
//! |---|---|---|---|
//! | 1 | the `sessions.pty` fact against `mode` | `session.pty-unserved` | `session.pty-unserved` |
//! | 2 | the window shape against `mode` | `session.window-invalid` | `session.window-invalid` |
//! | 3 | the confinement floor | `session.confinement-unavailable` | `exec.sandbox-unavailable` |
//! | 4 | `exec.wait` | `request.schema-invalid` at `input` | `session.wait-invalid` |
//! | 5 | the session byte and queue bounds | `request.schema-invalid` at `input` | `session.limit-unserved` |
//! | 6 | the exec shape — argv, env, limits, sandbox | `request.schema-invalid` at the field | `exec.*-invalid` |
//!
//! Members 4 and 5 share a rank because the daemon tests them in one condition. Member 6 was the
//! one round 3 missed: the daemon checked it at rank 3 and the driver inside `start_inner`, after
//! everything — opposite ends of the sequence. Member 3 was ranked 3 at the daemon and last at the
//! driver for the same reason. Both are now at the rank the table gives, at both ports.
//!
//! It was invisible because this port is only ever reached for a request the daemon admitted, and
//! because the daemon answers `request.schema-invalid` for members 4, 5 and 6 alike, differing only
//! in `address`. That is exactly why a pin has to read the code rather than the wire.
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
            workspace_access: substrate_wire::WorkspaceAccess::ReadWrite,
            scratch: None,
            measurements: std::collections::BTreeSet::new(),
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
#[allow(clippy::too_many_lines)] // Six members and every pair between them, in one table.
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

    // Members 4, 5 and 6 against each other, with the fact and the window both satisfied. The
    // driver has no confinement floor here (member 3 answers first), so this host is given one.
    let delegated = tempfile::tempdir().expect("temporary cgroup root");
    let mut confined = HostConfig::minimum(directory.path().join("workspaces"));
    confined.cgroup_root = Some(delegated.path().to_path_buf());
    let confined = HostDriver::open(confined).expect("host driver with a cgroup root");
    std::fs::create_dir_all(confined.root().join("ws_test")).expect("workspace directory");
    let snapshot = confined.machine().snapshot.clone();
    for (name, mutate, expected) in [
        // wait beats the exec shape
        (
            "wait and a bad argv",
            Box::new(|value: &mut PipeSessionStartInput| {
                value.exec.wait = true;
                value.exec.argv = vec![String::new()];
            }) as Box<dyn Fn(&mut PipeSessionStartInput)>,
            "session.wait-invalid",
        ),
        // the session bounds beat the exec shape
        (
            "a bad queue and a bad argv",
            Box::new(|value: &mut PipeSessionStartInput| {
                value.queued_frames = 0;
                value.exec.argv = vec![String::new()];
            }),
            "session.limit-unserved",
        ),
        // and wait beats the session bounds, which is the rank they share at the daemon
        (
            "wait and a bad queue",
            Box::new(|value: &mut PipeSessionStartInput| {
                value.exec.wait = true;
                value.queued_frames = 0;
            }),
            "session.wait-invalid",
        ),
    ] {
        let mut input = start(&snapshot, false, Some(good), 8);
        input.mode = SessionMode::Pipes;
        input.window = None;
        mutate(&mut input);
        let DispatchOutcome::NotDispatched(error) = confined
            .start_pipe_session("ex_rank", "ws_test", &input)
            .await
        else {
            panic!("{name}: a start outside the closed shape must be refused");
        };
        assert_eq!(error.code, expected, "{name}");
    }

    // Member 3 outranks 4, 5 and 6: the host with no cgroup root refuses the floor first.
    let mut floorless = start(&machine.snapshot, true, None, 0);
    floorless.mode = SessionMode::Pipes;
    floorless.window = None;
    floorless.exec.argv = vec![String::new()];
    let DispatchOutcome::NotDispatched(error) = driver
        .start_pipe_session("ex_rank", "ws_test", &floorless)
        .await
    else {
        panic!("a host with no confinement floor must refuse");
    };
    assert_eq!(error.code, "exec.sandbox-unavailable");

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
