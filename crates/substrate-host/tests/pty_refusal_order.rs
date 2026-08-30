//! The refusal order this wave made a decision, asserted at the port that does not obey it.
//!
//! `0.9.0` states the order in a bundle document — `vectors/http/pty-session-unserved-outranks-a-
//! missing-window.json`, coverage requirement `session.pty-refusal-order` — and the daemon obeys it
//! (`crates/substrate-daemon/src/app/operations.rs:558-582`: the fact, then the window shape). The
//! driver port checks the two in the opposite order
//! (`crates/substrate-host/src/process.rs:294-306`: the window shape, then the fact), while its own
//! comment three lines above says "the fact before either".
//!
//! `HostDriver::start_pipe_session` is public, and this is the same argument the round before this
//! one accepted for the 1..=1000 cell bound at the ioctl: not reachable through HTTP because the
//! daemon checks first, a public boundary all the same.
//!
//! Portable lane. Needs no cgroup delegation and no confinement backend: a host with neither is a
//! host whose `sessions.pty` fact is absent, which is exactly the deployment the ordering decision
//! is about.

use std::collections::BTreeMap;

use substrate_host::{DispatchOutcome, Driver as _, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode,
    PipeSessionStartInput, SandboxProfile, SessionMode,
};

fn pty_start(snapshot: &str, window: Option<substrate_wire::PtyWindow>) -> PipeSessionStartInput {
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
        window,
    }
}

/// A windowless `mode: "pty"` start on a driver with no `sessions.pty` fact.
///
/// Both refusals apply. `0.9.0` decided which one answers, and the decision is that the fact
/// outranks the window shape: `session.window-invalid` invites a client on a terminal-less
/// deployment to add a window and retry into a refusal it can never get past, and
/// `session.pty-unserved` says stop. The driver port answers the other one.
#[tokio::test(flavor = "multi_thread")]
async fn the_absent_pty_fact_outranks_a_missing_window_at_the_driver_port() {
    let directory = tempfile::tempdir().expect("temporary host root");
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    let machine = driver.machine();
    assert_eq!(
        machine.facts.sessions_pty, None,
        "this case is about a deployment that never proved it can give a terminal"
    );
    std::fs::create_dir_all(driver.root().join("ws_test")).expect("workspace directory");

    let DispatchOutcome::NotDispatched(error) = driver
        .start_pipe_session(
            "ex_ptyorder",
            "ws_test",
            &pty_start(&machine.snapshot, None),
        )
        .await
    else {
        panic!("a terminal must never be served as a pipe session instead");
    };
    assert_eq!(
        error.code, "session.pty-unserved",
        "the fact outranks the window shape, as the daemon and \
         vectors/http/pty-session-unserved-outranks-a-missing-window.json both state"
    );
}
