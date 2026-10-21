//! Adversarial cases against ADR 0033 (`story:read-only-host-file-bind`).
//!
//! Each case asserts the story's acceptance — the exec cannot write the declared file, and sees
//! nothing of the host beyond it — through a route the unit's own suite does not try.
//!
//! Delegated lane only: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, or without bubblewrap, these cases return early and are **absent, never
//! passed**. `bash scripts/delegated-lane.sh` runs them.

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::{DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode, ReadOnlyFile,
    SandboxProfile, WorkspaceAccess,
};

const WORKSPACE: &str = "ws_test";
const MOUNT: &str = "/opt/gates/bin/gate";
const ORIGINAL: &str = "#!/bin/sh\necho original\n";

fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    program: PathBuf,
    _workspaces: tempfile::TempDir,
    _fixture: tempfile::TempDir,
}

fn lane() -> Option<Lane> {
    let delegated = delegated_cgroup_root()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        return None;
    }
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(workspaces.path().join("workspaces"));
    config.cgroup_root = Some(delegated);
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    if machine.facts.exec_argv_only != Some(true) {
        return None;
    }
    assert!(
        machine.facts.exec_read_only_files.is_some(),
        "a delegated host publishes the read-only file fact"
    );
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");
    let fixture = tempfile::Builder::new()
        .prefix("read-only-file-adversary-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("fixture directory");
    let host = std::fs::canonicalize(fixture.path()).expect("canonical fixture");
    let program = host.join("gate");
    std::fs::write(&program, ORIGINAL).expect("host program");
    // An ordinary file its owner can write, as a user's own tool or configuration file is.
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("program mode");
    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        program,
        _workspaces: workspaces,
        _fixture: fixture,
    })
}

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8 fixture path").to_owned()
}

fn exec_input(
    snapshot: &str,
    script: &str,
    read_only_files: Vec<ReadOnlyFile>,
    workspace_access: WorkspaceAccess,
) -> ExecStartInput {
    ExecStartInput {
        read_only_roots: Vec::new(),
        read_only_files,
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
        workspace_access,
        scratch: None,
        measurements: BTreeSet::new(),
        capsule: None,
        lease_ttl_ms: Some(60_000),
    }
}

async fn waited(lane: &Lane, id: &str, input: &ExecStartInput) -> ExecObservation {
    match lane.driver.start_exec(id, WORKSPACE, input).await {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("{id} must dispatch: {} {}", error.code, error.message)
        }
    }
}

/// Writes through every descriptor the child holds, then lists the descriptors it found.
const WRITE_EVERY_DESCRIPTOR: &str = "for f in /proc/self/fd/*; do \
     echo \"$f -> $(readlink \"$f\")\"; (printf pwned >\"$f\") 2>/dev/null; done; true";

/// The pinned `O_PATH` descriptor is inherited by the sandboxed child, which re-opens it for writing
/// through `/proc/self/fd/<n>`: the reopen resolves on the host's writable mount, not on the
/// read-only bind, so the host file is rewritten.
#[tokio::test(flavor = "multi_thread")]
async fn no_descriptor_the_child_inherits_writes_the_bound_host_file() {
    let Some(lane) = lane() else {
        return;
    };
    let input = exec_input(
        &lane.snapshot,
        WRITE_EVERY_DESCRIPTOR,
        vec![ReadOnlyFile {
            host_path: text(&lane.program),
            mount: MOUNT.to_owned(),
        }],
        WorkspaceAccess::ReadWrite,
    );
    let observed = waited(&lane, "ex_adv_fd_write", &input).await;
    let after = std::fs::read_to_string(&lane.program).expect("host program");
    let listing = String::from_utf8_lossy(&observed.stdout);
    assert_eq!(
        after, ORIGINAL,
        "the sandboxed child rewrote the declared read-only host file; its descriptors were:\n{listing}"
    );
    assert!(
        !listing.contains(&text(&lane.program)),
        "the sandboxed child holds a descriptor on the declared host file:\n{listing}"
    );
}

/// ADR 0023's scoped workspace: the root is mounted read-only beneath one writable subtree, and
/// both pinned descriptors were retained into the child the same way. A write through either must
/// land nowhere outside the subtree.
#[tokio::test(flavor = "multi_thread")]
async fn no_descriptor_the_child_inherits_writes_outside_a_scoped_subtree() {
    let Some(lane) = lane() else {
        return;
    };
    let workspace = lane.driver.root().join(WORKSPACE);
    std::fs::create_dir_all(workspace.join("out")).expect("scoped subtree");
    std::fs::write(workspace.join("kept"), "kept\n").expect("workspace file");
    let script = "for f in /proc/self/fd/*; do \
         echo \"$f -> $(readlink \"$f\")\"; \
         (printf pwned >\"$f/kept\") 2>/dev/null; (printf new >\"$f/created\") 2>/dev/null; \
         (printf new >\"$f/../created\") 2>/dev/null; done; true";
    let input = exec_input(
        &lane.snapshot,
        script,
        Vec::new(),
        WorkspaceAccess::Scoped {
            writable_subtrees: vec!["out".to_owned()],
        },
    );
    let observed = waited(&lane, "ex_adv_scoped_fd_write", &input).await;
    let listing = String::from_utf8_lossy(&observed.stdout);
    assert_eq!(
        std::fs::read_to_string(workspace.join("kept")).expect("kept"),
        "kept\n",
        "a scoped workspace's read-only root was written through an inherited descriptor:\n{listing}"
    );
    assert!(
        !workspace.join("created").exists(),
        "a scoped workspace's read-only root gained a file through an inherited descriptor:\n{listing}"
    );
    assert!(
        !listing.contains(&text(&workspace)),
        "the sandboxed child holds a descriptor on the workspace:\n{listing}"
    );
}

/// The same route against ADR 0023's read-only workspace, whose pinned root descriptor is retained
/// into the child the same way. Recorded to place the origin of the defect above.
#[tokio::test(flavor = "multi_thread")]
async fn no_descriptor_the_child_inherits_writes_a_read_only_workspace() {
    let Some(lane) = lane() else {
        return;
    };
    let workspace = lane.driver.root().join(WORKSPACE);
    std::fs::write(workspace.join("kept"), "kept\n").expect("workspace file");
    let script = "for f in /proc/self/fd/*; do \
         (printf pwned >\"$f/kept\") 2>/dev/null; (printf new >\"$f/created\") 2>/dev/null; \
         done; true";
    let input = exec_input(
        &lane.snapshot,
        script,
        Vec::new(),
        WorkspaceAccess::ReadOnly,
    );
    let _observed = waited(&lane, "ex_adv_ws_fd_write", &input).await;
    assert_eq!(
        std::fs::read_to_string(workspace.join("kept")).expect("kept"),
        "kept\n",
        "a read-only workspace was written through an inherited descriptor"
    );
    assert!(
        !workspace.join("created").exists(),
        "a read-only workspace gained a file through an inherited descriptor"
    );
}

/// A procfs entry of the substrate process itself is `S_IFREG`, is reached without a symlink, and
/// so passes every ADR 0033 rule: binding it hands the driver's own environment to the child.
#[tokio::test(flavor = "multi_thread")]
async fn the_drivers_own_procfs_entries_do_not_cross_as_a_regular_file() {
    let Some(lane) = lane() else {
        return;
    };
    let environ = format!("/proc/{}/environ", std::process::id());
    let input = exec_input(
        &lane.snapshot,
        "tr '\\0' '\\n' </opt/leak/environ",
        vec![ReadOnlyFile {
            host_path: environ,
            mount: "/opt/leak/environ".to_owned(),
        }],
        WorkspaceAccess::ReadWrite,
    );
    match lane
        .driver
        .start_exec("ex_adv_procfs", WORKSPACE, &input)
        .await
    {
        DispatchOutcome::NotDispatched(_) => {}
        DispatchOutcome::Observed(observed) => {
            let stdout = String::from_utf8_lossy(&observed.stdout);
            assert!(
                !stdout.contains("SUBSTRATE_VECTORS_CGROUP_ROOT="),
                "the child read the driver process's environment through a declared file:\n{stdout}"
            );
        }
        DispatchOutcome::ContainedAbsent(error) | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("unexpected outcome {}", error.code)
        }
    }
}
