//! The raw-pipe truncation statement, delivered into a terminal transcript that has none.
//!
//! Design 13 removed the truncation statement from the pty vocabulary on purpose: reaching the
//! declared output bound **ends** the session and names itself on the exec observation's refusal
//! field, because design 05 gave the pty no `truncated` frame and a terminal stream has no offset
//! to resume from. `contracts/substrate-wire/0.10.0/schemas/pty-channel-frame.json` carries
//! `x-b10x-no-truncated` and no `truncated` branch, and `xtask/src/bundle.rs:754-760` refuses a
//! bundle that grows one.
//!
//! `drain_capped` is shared with raw pipes and does not know which it is draining
//! (`crates/substrate-host/src/process.rs:1979-1986`): on truncation it discards the last
//! `TRUNCATION_MARKER.len()` bytes the child actually wrote *inside* the bound and appends
//! `b"\n[substrate: output truncated]\n"` in their place. So the durable transcript of a terminal
//! session ends with a sentence substrate wrote, in a vocabulary the bundle says this session does
//! not speak, and 31 bytes of what the child wrote are gone.
//!
//! Delegated lane only: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
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

const TRUNCATION_MARKER: &[u8] = b"\n[substrate: output truncated]\n";

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
    if machine.facts.sessions_pty != Some(true) {
        return None;
    }
    std::fs::create_dir_all(driver.root().join("ws_test")).expect("workspace directory");
    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        _directory: directory,
    })
}

fn pty_start(snapshot: &str, argv: &[&str], output_bytes: u64) -> PipeSessionStartInput {
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
                output_bytes,
                processes: 16,
                memory_bytes: 67_108_864,
                cpu_millis: 5_000,
            },
            wait: false,
            scratch: None,
            measurements: std::collections::BTreeSet::new(),
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
            columns: 97,
            rows: 37,
        }),
    }
}

/// A terminal session that ended at its declared bound reports what the child wrote, and says which
/// bound ended it on the refusal field — not by writing a sentence of its own into the transcript.
#[tokio::test(flavor = "multi_thread")]
async fn the_durable_terminal_transcript_carries_no_raw_pipe_truncation_statement() {
    let Some(lane) = lane() else {
        return;
    };
    let id = "ex_ptymarker";
    let start = pty_start(
        &lane.snapshot,
        &[
            "/bin/sh",
            "-c",
            "/usr/bin/head -c 65536 /dev/zero | /usr/bin/tr '\\0' 'a'",
        ],
        4_096,
    );
    match lane.driver.start_pipe_session(id, "ws_test", &start).await {
        DispatchOutcome::Observed(_) => {}
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "the delegated lane must dispatch a pty session: {} {}",
            error.code, error.message
        ),
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match lane
            .driver
            .read_pipe_session(id, Duration::from_secs(5))
            .await
        {
            Ok(Some(_frame)) => {}
            Ok(None) => break,
            Err(_deadline) => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the terminal never reached end of file"
        );
    }
    let observed = lane
        .driver
        .observe_exec(id)
        .await
        .expect("the session is still tracked");
    assert_eq!(
        observed
            .resource
            .refusal
            .as_ref()
            .map(|refusal| refusal.code.as_str()),
        Some("session.output-limit"),
        "this case only makes its claim when the bound actually ended the session"
    );
    let tail_start = observed
        .stdout
        .len()
        .saturating_sub(TRUNCATION_MARKER.len());
    let tail = String::from_utf8_lossy(&observed.stdout[tail_start..]).into_owned();
    assert!(
        !observed.stdout.ends_with(TRUNCATION_MARKER),
        "a terminal has no truncation statement (design 13, \
         schemas/pty-channel-frame.json x-b10x-no-truncated), and this transcript ends with \
         substrate's own raw-pipe one in place of {} bytes the child wrote: {tail:?}",
        TRUNCATION_MARKER.len()
    );
}
