//! Security review cases for `story:an-unknown-exec-is-killed-again-before-restart`, wave
//! 2026-10-23.
//!
//! These drive a real run to an `unknown` observation through the host driver's own
//! `reconcile_cgroup`, not by installing a flagged entry by hand: an empty nested cgroup created
//! from outside the sandbox while the exec runs keeps its cgroup directory from being removed, so
//! the whole tree cannot be proven gone. That measures the line in `run_child` that records the
//! cgroup as unproven, which the unit's own cases set directly.
//!
//! Delegated only: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, and without bubblewrap, each case returns early and is **absent, never
//! passed** (invariant 3). `bash scripts/delegated-lane.sh` runs them.

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use substrate_host::{DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, ExecState, NetworkMode,
    SandboxProfile, WorkspaceAccess,
};

const WORKSPACE: &str = "ws_test";

/// The delegated cgroup v2 subtree this process is inside, or nothing.
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

/// Removes every nested group a case pinned, and the exec cgroup above it, however the case ends,
/// so a failed case never leaves the shared delegation root refusing the next driver's start.
struct Pins(Vec<PathBuf>);

impl Drop for Pins {
    fn drop(&mut self) {
        for pinned in &self.0 {
            let _ = std::fs::remove_dir(pinned);
            if let Some(parent) = pinned.parent() {
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
}

fn driver(cgroup_root: &Path, workspaces: &Path, max_tracked_execs: usize) -> Arc<HostDriver> {
    let mut config = HostConfig::minimum(workspaces.join("workspaces"));
    config.cgroup_root = Some(cgroup_root.to_owned());
    config.max_tracked_execs = max_tracked_execs;
    let driver = HostDriver::open(config).expect("host driver");
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");
    driver
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

async fn observed(driver: &HostDriver, id: &str, input: &ExecStartInput) -> ExecObservation {
    match driver.start_exec(id, WORKSPACE, input).await {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "the delegated lane must dispatch {id}: {} {}",
            error.code, error.message
        ),
    }
}

/// Run `id` to its terminal observation while a nested group is created inside its cgroup from
/// outside the sandbox. Records the nested group in `pins` before anything can fail.
async fn run_pinned(
    driver: &HostDriver,
    cgroup_root: &Path,
    snapshot: &str,
    id: &str,
    pins: &mut Pins,
) -> ExecObservation {
    let exec_cgroup = cgroup_root.join(format!("substrate-{id}"));
    let pinned = exec_cgroup.join("pinned");
    pins.0.push(pinned.clone());
    let creator = tokio::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if exec_cgroup.join("cgroup.procs").is_file() && std::fs::create_dir(&pinned).is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        false
    });
    let observation = observed(
        driver,
        id,
        &exec_input(snapshot, &["/usr/bin/sleep", "0.5"]),
    )
    .await;
    assert!(
        creator.await.expect("creator task"),
        "{id}: the nested group was created while the exec ran"
    );
    observation
}

/// A run whose cgroup could not be proven empty is recorded `unknown` by `run_child` itself; once
/// acknowledged it keeps its tracking slot, is retried every round, stays in place while it
/// cannot be proven gone, and frees its slot as soon as a round proves it gone.
#[tokio::test(flavor = "multi_thread")]
async fn delegated_a_run_left_unproven_by_run_child_holds_its_slot_until_a_round_proves_it_gone() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let workspaces = tempfile::tempdir().expect("workspaces");
    let driver = driver(&cgroup_root, workspaces.path(), 1);
    assert_eq!(
        driver.machine().facts.exec_cgroup_kill,
        Some(true),
        "a delegated root must serve cgroup.kill; an absent fact here is a broken harness"
    );
    let snapshot = driver.machine().snapshot;
    let mut pins = Pins(Vec::new());

    let observation = run_pinned(&driver, &cgroup_root, &snapshot, "ex_secpin", &mut pins).await;
    assert_eq!(
        observation.resource.state,
        ExecState::Unknown,
        "a cgroup that cannot be removed is never reported terminated"
    );
    let exec_cgroup = cgroup_root.join("substrate-ex_secpin");
    assert!(exec_cgroup.is_dir());

    driver.acknowledge_exec(&observation);
    let crowded = driver
        .start_exec(
            "ex_second",
            WORKSPACE,
            &exec_input(&snapshot, &["/usr/bin/true"]),
        )
        .await;
    match crowded {
        DispatchOutcome::NotDispatched(error) => assert_eq!(
            error.code, "exec.tracking-capacity",
            "the acknowledged unproven tree holds the only tracking slot"
        ),
        _ => panic!("a start beyond max_tracked_execs must be refused before dispatch"),
    }

    driver
        .retry_unproven_exec_trees()
        .await
        .expect("a retry round");
    assert!(
        exec_cgroup.is_dir(),
        "a cgroup that still cannot be proven empty is left for the next round"
    );

    std::fs::remove_dir(&pins.0[0]).expect("the nested group is empty");
    driver
        .retry_unproven_exec_trees()
        .await
        .expect("a retry round");
    assert!(
        !exec_cgroup.exists(),
        "a round removes the cgroup it proves empty"
    );
    let second = observed(
        &driver,
        "ex_second",
        &exec_input(&snapshot, &["/usr/bin/true"]),
    )
    .await;
    assert_eq!(
        second.resource.state,
        ExecState::Exited,
        "the slot is free once the tree is proven gone"
    );
    driver.acknowledge_exec(&second);
}

/// A round over many trees that cannot be proven gone stays short: its proof attempts are shared
/// across all of them, not made per tree, so a hostile run cannot stretch the maintenance sweep
/// that also expires leases. Shutdown, which also visits each still-tracked one through `signal`,
/// stays short too, and the next start's refusal is unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn delegated_a_round_over_many_unproven_trees_stays_bounded() {
    const STUCK: usize = 8;
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let workspaces = tempfile::tempdir().expect("workspaces");
    let driver = driver(&cgroup_root, workspaces.path(), STUCK);
    assert_eq!(
        driver.machine().facts.exec_cgroup_kill,
        Some(true),
        "a delegated root must serve cgroup.kill; an absent fact here is a broken harness"
    );
    let snapshot = driver.machine().snapshot;
    let mut pins = Pins(Vec::new());

    for index in 0..STUCK {
        let id = format!("ex_secbound{index}");
        let observation = run_pinned(&driver, &cgroup_root, &snapshot, &id, &mut pins).await;
        assert_eq!(observation.resource.state, ExecState::Unknown, "{id}");
        // Half acknowledged (carried), half still tracked: both kinds are retried.
        if index % 2 == 0 {
            driver.acknowledge_exec(&observation);
        }
    }

    let started = Instant::now();
    driver
        .retry_unproven_exec_trees()
        .await
        .expect("a retry round");
    let round = started.elapsed();
    assert!(
        round < Duration::from_secs(1),
        "one round over {STUCK} unproven trees took {round:?}"
    );

    let started = Instant::now();
    driver.shutdown().await.expect("shutdown");
    let shutdown = started.elapsed();
    assert!(
        shutdown < Duration::from_secs(3),
        "shutdown over {STUCK} unproven trees took {shutdown:?}"
    );
    eprintln!("round {round:?}, shutdown {shutdown:?}");
    drop(driver);

    let second_workspaces = tempfile::tempdir().expect("workspaces");
    let mut config = HostConfig::minimum(second_workspaces.path().join("workspaces"));
    config.cgroup_root = Some(cgroup_root.clone());
    let refused = HostDriver::open(config.clone())
        .err()
        .map(|error| error.code);
    assert_eq!(
        refused,
        Some("exec.cgroup-not-empty"),
        "the next start still refuses a tree it cannot prove gone"
    );

    for pinned in &pins.0 {
        std::fs::remove_dir(pinned).expect("the nested group is empty");
    }
    HostDriver::open(config).expect("a start after the nested groups are gone");
}
