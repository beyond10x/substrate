//! Adversary pass 2 for `story:an-unknown-exec-is-killed-again-before-restart`, wave 2026-10-23.
//!
//! A real run is driven to an `unknown` observation by `run_child`'s own `reconcile_cgroup`: an
//! empty nested cgroup created from outside the sandbox while the exec runs keeps the exec's
//! cgroup directory from being removed (the technique of `unproven_exec_tree_retry_security.rs`).
//!
//! Delegated only: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, and without bubblewrap, each case returns early and is **absent, never
//! passed** (invariant 3). `bash scripts/delegated-lane.sh` runs them.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use substrate_host::{DispatchOutcome, Driver as _, DriverErrorClass, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecSignalInput, ExecStartInput, ExecState,
    NetworkMode, SandboxProfile, Signal, WorkspaceAccess,
};

const WORKSPACE: &str = "ws_test";

fn delegated_cgroup_root() -> Option<PathBuf> {
    let Some(root) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT").map(PathBuf::from) else {
        eprintln!("ABSENT: SUBSTRATE_VECTORS_CGROUP_ROOT is unset");
        return None;
    };
    if !Path::new("/usr/bin/bwrap").is_file() {
        eprintln!("ABSENT: no bubblewrap");
        return None;
    }
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

/// Removes the pinned nested group and the exec cgroup above it however the case ends.
struct Pin(PathBuf);

impl Drop for Pin {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
        if let Some(parent) = self.0.parent() {
            let _ = std::fs::write(parent.join("cgroup.kill"), "1");
            for _ in 0..40 {
                if std::fs::remove_dir(parent).is_ok() || !parent.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn exec_input(snapshot: &str, argv: &[&str]) -> ExecStartInput {
    ExecStartInput {
        writable_roots: Vec::new(),
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workspace: WORKSPACE.to_owned(),
        argv: argv.iter().map(|part| (*part).to_owned()).collect(),
        env: ExecEnvironment {
            allow: vec![],
            set: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        },
        sandbox: ConfinementRequest {
            capability_snapshot: snapshot.to_owned(),
            network: NetworkMode::None,
            aperture: None,
            apertures: Vec::new(),
            profile: SandboxProfile::Workspace,
            required: true,
        },
        limits: ExecLimits {
            timeout_ms: 30_000,
            output_bytes: 65_536,
            processes: 16,
            memory_bytes: 134_217_728,
            cpu_millis: 10_000,
        },
        wait: true,
        workspace_access: WorkspaceAccess::ReadWrite,
        scratch: None,
        measurements: BTreeSet::new(),
        capsule: None,
        lease_ttl_ms: Some(60_000),
        workdir: None,
    }
}

/// The daemon's workspace-lease cleanup (`crates/substrate-daemon/src/app/service.rs`, the
/// `LeaseResource::Workspace` arm of `cleanup_expired`) sends KILL to every `unknown` exec holding
/// the workspace, treats a `NotFound` answer as "nothing left", and then destroys the workspace,
/// releases its project quota and deletes its row. Correction round 1 made `observe` refuse to
/// answer `NotFound` for a carried tree for exactly that reason (its comment: "which releases its
/// workspace"); `signal`, the call the workspace path actually makes, still answers `NotFound`
/// while the tree is not proven gone.
#[tokio::test(flavor = "multi_thread")]
async fn delegated_signal_does_not_answer_absent_for_a_tree_not_proven_gone() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let workspaces = tempfile::tempdir().expect("workspaces");
    let mut config = HostConfig::minimum(workspaces.path().join("workspaces"));
    config.cgroup_root = Some(cgroup_root.clone());
    let driver: Arc<HostDriver> = HostDriver::open(config).expect("host driver");
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");
    let snapshot = driver.machine().snapshot;

    let id = "ex_adv2signal";
    let exec_cgroup = cgroup_root.join(format!("substrate-{id}"));
    let pin = Pin(exec_cgroup.join("pinned"));
    let pinned = pin.0.clone();
    let watched = exec_cgroup.clone();
    let creator = tokio::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if watched.join("cgroup.procs").is_file() && std::fs::create_dir(&pinned).is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        false
    });
    let observation = match driver
        .start_exec(
            id,
            WORKSPACE,
            &exec_input(&snapshot, &["/usr/bin/sleep", "0.5"]),
        )
        .await
    {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!(
                "the delegated lane must dispatch: {} {}",
                error.code, error.message
            )
        }
    };
    assert!(creator.await.expect("creator"), "nested group created");
    assert_eq!(observation.resource.state, ExecState::Unknown);
    driver.acknowledge_exec(&observation);
    assert!(exec_cgroup.is_dir(), "the exec cgroup is still there");

    let observed = driver.observe_exec(id).await.expect_err("untracked");
    assert_ne!(
        observed.class,
        DriverErrorClass::NotFound,
        "{}",
        observed.code
    );

    let signalled = driver
        .signal(
            id,
            &ExecSignalInput {
                signal: Signal::Kill,
                grace_ms: 0,
            },
        )
        .await;
    assert!(exec_cgroup.is_dir(), "the tree is still not proven gone");
    let class = signalled.as_ref().err().map(|error| error.class);
    assert_ne!(
        class,
        Some(DriverErrorClass::NotFound),
        "KILL on a tree not proven gone answers `NotFound`, which workspace-lease cleanup reads as \
         absent and destroys the workspace under it; observe answers {}: {signalled:?}",
        observed.code
    );
    drop(pin);
}
