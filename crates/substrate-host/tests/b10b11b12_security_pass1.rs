//! Security review pass 1 for wave 2026-10-23 units b11 and b12.
//!
//! b12, `story:host-admit-checks-the-read-only-roots-fact`: the driver refuses `read_only_roots`
//! with the unserved `exec.sandbox-unavailable` refusal when its snapshot does not publish
//! `exec.read-only-roots`. The daemon checks the lexical rules and then the fact, and touches no
//! host path (`check_read_only_roots` in `crates/substrate-daemon/src/app/operations.rs`); the
//! driver is held here to the same answer. Portable: a host with neither delegation nor
//! confinement is exactly the host whose fact is absent.
//!
//! b11, `story:nested-exec-cgroups-are-removed-depth-first`: the removal walk's two bounds at their
//! exact values, driven through the public driver against a real cgroup. Delegated only: without
//! `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this process is inside, and without
//! bubblewrap, each delegated case returns early and is **absent, never passed** (invariant 3).

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use substrate_host::{DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, ExecState, NetworkMode,
    ReadOnlyRoot, SandboxProfile, WorkspaceAccess,
};

const WORKSPACE: &str = "ws_test";
/// `MAX_EXEC_CGROUP_DEPTH` in `src/process.rs`.
const WALK_DEPTH: usize = 32;
/// `MAX_EXEC_NESTED_CGROUPS` in `src/process.rs`.
const WALK_COUNT: usize = 256;

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
            requires: None,
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

// ---------------------------------------------------------------------------------------------
// b12
// ---------------------------------------------------------------------------------------------

/// On a host whose snapshot does not publish `exec.read-only-roots`, every lexically valid start
/// naming a root is answered `exec.sandbox-unavailable` (class unserved, address
/// `read_only_roots`), whatever is or is not at the declared host path: the daemon's answer for the
/// same request. A per-root filesystem code here means the driver resolved a caller-named host
/// path on a host that cannot bind it, and answered differently from the daemon.
#[tokio::test(flavor = "multi_thread")]
async fn b12_an_unpublished_host_answers_every_valid_root_unserved_before_resolving_it() {
    let host_root = tempfile::tempdir().expect("temporary host root");
    let driver = HostDriver::open(HostConfig::minimum(host_root.path().join("workspaces")))
        .expect("host driver");
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace directory");
    let machine = driver.machine();
    assert!(
        machine.facts.exec_read_only_roots.is_none(),
        "the portable driver must lack exec.read-only-roots, or this case measures nothing"
    );
    let fixture = std::fs::canonicalize(host_root.path()).expect("canonical fixture");
    let real = fixture.join("real");
    std::fs::create_dir(&real).expect("real directory");
    std::fs::write(fixture.join("file"), "file\n").expect("regular file");

    let mut wrong = Vec::new();
    for (id, declared) in [
        ("ex_b12_real", real.clone()),
        ("ex_b12_absent", fixture.join("absent")),
        ("ex_b12_file", fixture.join("file")),
    ] {
        let mut input = exec_input(&machine.snapshot, &["/usr/bin/true"]);
        input.read_only_roots = vec![ReadOnlyRoot {
            host_path: declared.to_str().expect("UTF-8").to_owned(),
            mount: "/opt/root".to_owned(),
        }];
        match driver.start_exec(id, WORKSPACE, &input).await {
            DispatchOutcome::NotDispatched(error) => {
                if error.code != "exec.sandbox-unavailable"
                    || error.address.as_deref() != Some("read_only_roots")
                {
                    wrong.push(format!(
                        "{id}: expected exec.sandbox-unavailable at read_only_roots, got {} at \
                         {:?}",
                        error.code, error.address
                    ));
                }
            }
            _ => panic!("{id} must be refused before dispatch"),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

// ---------------------------------------------------------------------------------------------
// b11
// ---------------------------------------------------------------------------------------------

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

/// Every directory beneath `top`, parents before children, never following a symlink.
fn nested_dirs(top: &Path) -> Vec<PathBuf> {
    let mut found = vec![top.to_owned()];
    let mut next = 0;
    while let Some(path) = found.get(next).cloned() {
        next += 1;
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                found.push(entry.path());
            }
        }
    }
    found.remove(0);
    found
}

/// Removes every nested group a case created, deepest first, then kills and removes the exec
/// cgroup, however the case ends.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::write(self.0.join("cgroup.kill"), "1");
        for _ in 0..40 {
            for dir in nested_dirs(&self.0).iter().rev() {
                let _ = std::fs::remove_dir(dir);
            }
            if std::fs::remove_dir(&self.0).is_ok() || !self.0.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

fn driver(cgroup_root: &Path, workspaces: &Path) -> Arc<HostDriver> {
    let mut config = HostConfig::minimum(workspaces.join("workspaces"));
    config.cgroup_root = Some(cgroup_root.to_owned());
    let driver = HostDriver::open(config).expect("host driver");
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");
    driver
}

/// The groups a case nests: a chain `depth` deep, plus flat siblings until `total` groups exist.
fn layout(exec_cgroup: &Path, depth: usize, total: usize) -> Vec<PathBuf> {
    let mut groups = Vec::new();
    let mut chain = exec_cgroup.to_owned();
    for _ in 0..depth {
        chain = chain.join("n");
        groups.push(chain.clone());
    }
    let mut index = 0;
    while groups.len() < total {
        groups.push(exec_cgroup.join(format!("s{index}")));
        index += 1;
    }
    groups
}

/// Runs `id` while `groups` are created inside its cgroup from outside the sandbox.
async fn run_nested(
    driver: &HostDriver,
    cgroup_root: &Path,
    snapshot: &str,
    id: &str,
    groups: Vec<PathBuf>,
) -> ExecObservation {
    let exec_cgroup = cgroup_root.join(format!("substrate-{id}"));
    let creator = tokio::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if exec_cgroup.join("cgroup.procs").is_file() {
                return groups
                    .iter()
                    .all(|group| std::fs::create_dir(group).is_ok());
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        false
    });
    let observation = match driver
        .start_exec(
            id,
            WORKSPACE,
            &exec_input(snapshot, &["/usr/bin/sleep", "1.5"]),
        )
        .await
    {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "the delegated lane must dispatch {id}: {} {}",
            error.code, error.message
        ),
    };
    assert!(
        creator.await.expect("creator task"),
        "{id}: every nested group was created while the exec ran"
    );
    observation
}

/// At both bounds at once — a chain exactly 32 deep and exactly 256 nested groups — the walk
/// still removes everything and the run is proven gone. Catches `>` relaxed to `>=` in either
/// bound check, which no portable case pins (they test 33 and 257 only).
#[tokio::test(flavor = "multi_thread")]
async fn delegated_b11_nested_groups_exactly_at_both_bounds_are_removed() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let workspaces = tempfile::tempdir().expect("workspaces");
    let driver = driver(&cgroup_root, workspaces.path());
    let snapshot = driver.machine().snapshot;
    let id = "ex_b11atbound";
    let exec_cgroup = cgroup_root.join(format!("substrate-{id}"));
    let _cleanup = Cleanup(exec_cgroup.clone());

    let groups = layout(&exec_cgroup, WALK_DEPTH, WALK_COUNT);
    assert_eq!(groups.len(), WALK_COUNT);
    let observation = run_nested(&driver, &cgroup_root, &snapshot, id, groups).await;
    assert_eq!(
        observation.resource.state,
        ExecState::Exited,
        "a subtree exactly at both bounds is removed and the run proven gone"
    );
    assert!(!exec_cgroup.exists(), "the exec cgroup is removed");
    driver.acknowledge_exec(&observation);
    eprintln!("delegated_b11_nested_groups_exactly_at_both_bounds_are_removed: executed");
}

/// One group past the count bound, at a depth within it: refused as a whole, so the run is
/// `unknown`, never reported terminated, and every nested group is kept.
#[tokio::test(flavor = "multi_thread")]
async fn delegated_b11_one_group_past_the_count_bound_keeps_the_run_unknown() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let workspaces = tempfile::tempdir().expect("workspaces");
    let driver = driver(&cgroup_root, workspaces.path());
    let snapshot = driver.machine().snapshot;
    let id = "ex_b11pastcount";
    let exec_cgroup = cgroup_root.join(format!("substrate-{id}"));
    let _cleanup = Cleanup(exec_cgroup.clone());

    let groups = layout(&exec_cgroup, 1, WALK_COUNT + 1);
    let observation = run_nested(&driver, &cgroup_root, &snapshot, id, groups.clone()).await;
    assert_eq!(
        observation.resource.state,
        ExecState::Unknown,
        "a subtree past the count bound is never reported terminated"
    );
    assert!(
        groups.iter().all(|group| group.is_dir()),
        "nothing is removed past the bound"
    );
    driver.acknowledge_exec(&observation);
    eprintln!("delegated_b11_one_group_past_the_count_bound_keeps_the_run_unknown: executed");
}
