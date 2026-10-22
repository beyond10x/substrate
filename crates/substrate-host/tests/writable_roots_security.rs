//! Security conformance for writable host roots (ADR 0038 kind 1): the descriptors admission pins
//! do not reach the child, and the declared directory is the only writable host mount it gains.
//!
//! Delegated: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this process is
//! inside, or without bubblewrap, each case returns early and is **absent, never passed**.
//! `bash scripts/delegated-lane.sh` runs them.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use substrate_host::{
    DeclaredWritableRoot, DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver,
};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode, SandboxProfile,
    WorkspaceAccess, WritableHostRoot, WritableRoot,
};

const WORKSPACE: &str = "ws_security";
const NAME: &str = "shared";
const MOUNT: &str = "/srv/substrate-writable";

fn fixture(prefix: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("fixture directory under CARGO_TARGET_TMPDIR");
    let canonical = std::fs::canonicalize(directory.path()).expect("canonical fixture directory");
    (directory, canonical)
}

fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        return None;
    }
    Some(root)
}

fn exec_input(snapshot: &str, script: &str) -> ExecStartInput {
    ExecStartInput {
        writable_roots: vec![WritableRoot::Host(WritableHostRoot {
            host: NAME.to_owned(),
            mount: MOUNT.to_owned(),
        })],
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workspace: WORKSPACE.to_owned(),
        argv: vec!["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()],
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

fn delegated_driver(
    cgroup_root: PathBuf,
    workspaces: &Path,
    root: &Path,
) -> std::sync::Arc<HostDriver> {
    let mut config = HostConfig::minimum(workspaces.join("workspaces"));
    config.cgroup_root = Some(cgroup_root);
    config.writable_roots = vec![DeclaredWritableRoot {
        name: NAME.to_owned(),
        path: root.to_owned(),
    }];
    let driver = HostDriver::open(config).expect("host driver");
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");
    driver
}

/// No descriptor the child holds resolves to the declared root's parent or to the host path of
/// the root itself, and no `fd/N/..` walk reaches the parent's sibling.
#[tokio::test(flavor = "multi_thread")]
async fn no_pinned_descriptor_reaches_the_child() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let (_fixture, host) = fixture("rw-root-sec-fd-");
    let parent = host.join("parent");
    let root = parent.join("root");
    std::fs::create_dir_all(&root).expect("declared root");
    std::fs::create_dir_all(parent.join("sibling")).expect("sibling");
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let driver = delegated_driver(cgroup_root, workspaces.path(), &root);
    let machine = driver.machine();
    assert_eq!(
        machine.facts.exec_writable_roots_host,
        Some(vec![NAME.to_owned()])
    );
    let script = "for f in /proc/self/fd/*; do \
                    n=${f##*/}; t=$(readlink \"$f\" 2>/dev/null); echo \"fd $n $t\"; \
                    if [ -d \"$f/..\" ]; then ls -a \"$f/..\" 2>/dev/null | sed \"s|^|up $n |\"; fi; \
                  done";
    let run = observed(
        &driver,
        "ex_rw_sec_fd",
        &exec_input(&machine.snapshot, script),
    )
    .await;
    let listing = String::from_utf8_lossy(&run.stdout).into_owned();
    let host_root = root.to_str().expect("UTF-8");
    let host_parent = parent.to_str().expect("UTF-8");
    assert!(
        listing.lines().any(|line| line.starts_with("fd 0 ")),
        "the child listed no descriptors: {listing}"
    );
    for line in listing.lines() {
        assert!(
            !line.contains(host_root) && !line.contains(host_parent),
            "a child descriptor names the declared root or its parent: {line}\n{listing}"
        );
        assert!(
            !(line.starts_with("up ") && line.ends_with(" sibling")),
            "a child descriptor walks up to the declared root's sibling: {line}\n{listing}"
        );
    }
}

/// The child's mount table holds exactly one read-write mount of the declared root, at the
/// declared mount, nothing is mounted beneath it, and `/usr` stays read-only.
#[tokio::test(flavor = "multi_thread")]
async fn the_declared_root_is_the_only_writable_host_mount() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let (_fixture, host) = fixture("rw-root-sec-mnt-");
    let root = host.join("root");
    std::fs::create_dir_all(&root).expect("declared root");
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let driver = delegated_driver(cgroup_root, workspaces.path(), &root);
    let machine = driver.machine();
    let script = "cat /proc/self/mountinfo; \
                  (touch /usr/escape) 2>/dev/null && echo WROTE-USR; true";
    let run = observed(
        &driver,
        "ex_rw_sec_mnt",
        &exec_input(&machine.snapshot, script),
    )
    .await;
    let listing = String::from_utf8_lossy(&run.stdout).into_owned();
    assert!(!listing.contains("WROTE-USR"), "{listing}");
    let at_mount: Vec<&str> = listing
        .lines()
        .filter(|line| line.split(' ').nth(4) == Some(MOUNT))
        .collect();
    assert_eq!(at_mount.len(), 1, "{listing}");
    let options = at_mount[0].split(' ').nth(5).expect("mount options");
    assert!(options.split(',').any(|o| o == "rw"), "{listing}");
    let beneath_mount = format!("{MOUNT}/");
    assert!(
        !listing.lines().any(|line| line
            .split(' ')
            .nth(4)
            .is_some_and(|m| m.starts_with(&beneath_mount))),
        "a mount appeared beneath the writable root: {listing}"
    );
}
