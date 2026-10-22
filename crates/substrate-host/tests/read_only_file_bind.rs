//! A single host file is admitted read-only (`story:read-only-host-file-bind`, ADR 0033).
//!
//! A host program lives in a host directory beside a sibling the exec must never see. Declared as
//! one read-only file at a mount on the exec's `PATH`, it runs by name; writing it fails; the
//! sibling is absent; and a directory, a symlinked path or a symlinked parent is refused before
//! dispatch by name.
//!
//! The fixture lives under `CARGO_TARGET_TMPDIR` (this checkout's `target/tmp`), canonicalized, so
//! the declared host path has no symlink component unless a case puts one there on purpose.
//!
//! Delegated lane only: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, or without bubblewrap, these cases are **absent, never reported as passed**
//! (invariant 3). `bash scripts/delegated-lane.sh` runs them.

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::{DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, MAX_READ_ONLY_FILES,
    NetworkMode, ReadOnlyFile, ReadOnlyFileFacts, ReadOnlyRoot, SandboxProfile, WorkspaceAccess,
};

const WORKSPACE: &str = "ws_test";
const MOUNT_DIRECTORY: &str = "/opt/gates/bin";
const MOUNT: &str = "/opt/gates/bin/gate";
const PROGRAM_OUTPUT: &str = "gate ran from a single bound file";

/// The delegated cgroup v2 subtree this process is inside, or nothing.
fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    facts: Option<ReadOnlyFileFacts>,
    /// The host directory holding the program and its sibling, canonical.
    host: PathBuf,
    _workspaces: tempfile::TempDir,
    _fixture: tempfile::TempDir,
}

impl Lane {
    fn program(&self) -> PathBuf {
        self.host.join("gate")
    }

    fn sibling(&self) -> PathBuf {
        self.host.join("sibling")
    }

    fn declared(&self) -> ReadOnlyFile {
        ReadOnlyFile {
            host_path: text(&self.program()),
            mount: MOUNT.to_owned(),
        }
    }
}

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8 fixture path").to_owned()
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
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");

    let fixture = tempfile::Builder::new()
        .prefix("read-only-file-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("fixture directory under CARGO_TARGET_TMPDIR");
    let host = std::fs::canonicalize(fixture.path())
        .expect("canonical fixture directory")
        .join("bin");
    std::fs::create_dir(&host).expect("host program directory");
    let program = host.join("gate");
    std::fs::write(&program, format!("#!/bin/sh\necho '{PROGRAM_OUTPUT}'\n"))
        .expect("host program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("executable host program");
    std::fs::write(host.join("sibling"), "never visible inside\n").expect("sibling");

    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        facts: machine.facts.exec_read_only_files,
        host,
        _workspaces: workspaces,
        _fixture: fixture,
    })
}

fn exec_input(
    snapshot: &str,
    argv: &[&str],
    read_only_files: Vec<ReadOnlyFile>,
    read_only_roots: Vec<ReadOnlyRoot>,
) -> ExecStartInput {
    ExecStartInput {
        read_only_roots,
        read_only_files,
        secret_slots: Vec::new(),
        workspace: WORKSPACE.to_owned(),
        argv: argv.iter().map(|part| (*part).to_owned()).collect(),
        env: ExecEnvironment {
            allow: vec![],
            set: BTreeMap::from([(
                "PATH".to_owned(),
                format!("{MOUNT_DIRECTORY}:/usr/bin:/bin"),
            )]),
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

async fn waited(lane: &Lane, id: &str, argv: &[&str], files: Vec<ReadOnlyFile>) -> ExecObservation {
    let input = exec_input(&lane.snapshot, argv, files, Vec::new());
    match lane.driver.start_exec(id, WORKSPACE, &input).await {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "the delegated lane must dispatch {id}: {} {}",
            error.code, error.message
        ),
    }
}

async fn refused(lane: &Lane, id: &str, input: &ExecStartInput) -> String {
    match lane.driver.start_exec(id, WORKSPACE, input).await {
        DispatchOutcome::NotDispatched(error) => error.code.to_owned(),
        DispatchOutcome::Observed(_) => panic!("{id} was dispatched; it must be refused first"),
        DispatchOutcome::ContainedAbsent(error) | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("{id} reached dispatch before its refusal: {}", error.code)
        }
    }
}

fn exit_code(observed: &ExecObservation) -> Option<u8> {
    observed
        .resource
        .exit
        .as_ref()
        .expect("a waited exec reports its exit")
        .code
}

/// The fact is published from the probe with the wire bound, and the program runs by name from a
/// mount on `PATH`, with its host executable bits, listed in the applied observation.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_program_bound_at_a_mount_on_path_runs_by_name() {
    let Some(lane) = lane() else {
        return;
    };
    assert_eq!(
        lane.facts,
        Some(ReadOnlyFileFacts {
            max_files: MAX_READ_ONLY_FILES
        }),
        "a delegated host proves and publishes the read-only file bound"
    );
    let observed = waited(&lane, "ex_file_runs", &["gate"], vec![lane.declared()]).await;
    assert_eq!(
        exit_code(&observed),
        Some(0),
        "the bound program runs: {}",
        String::from_utf8_lossy(&observed.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&observed.stdout).trim(),
        PROGRAM_OUTPUT
    );
    assert_eq!(
        observed
            .resource
            .applied
            .as_ref()
            .expect("an admitted exec records what was applied")
            .read_only_files,
        vec![lane.declared()],
        "the observation lists the bound file (ADR 0033)"
    );
}

/// The bind is read-only: neither a write through the mount nor a replacement of it succeeds, and
/// the host file is unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn the_exec_cannot_write_the_bound_file() {
    let Some(lane) = lane() else {
        return;
    };
    let before = std::fs::read(lane.program()).expect("host program");
    // Exit 0 only when the file is there, readable, and every write through it fails: a run in
    // which the file was never bound fails the first clause instead of passing vacuously.
    let script = format!(
        "test -f {MOUNT} && test -r {MOUNT} && test -x {MOUNT} \
         && ! (echo overwritten >{MOUNT}) 2>/dev/null \
         && ! (echo appended >>{MOUNT}) 2>/dev/null \
         && ! chmod u+w {MOUNT} 2>/dev/null"
    );
    let write = waited(
        &lane,
        "ex_file_write",
        &["/bin/sh", "-c", &script],
        vec![lane.declared()],
    )
    .await;
    assert_eq!(
        exit_code(&write),
        Some(0),
        "the declared file is present, readable, executable and never writable: {}",
        String::from_utf8_lossy(&write.stderr)
    );
    assert_eq!(
        std::fs::read(lane.program()).expect("host program"),
        before,
        "the host file is unchanged"
    );
}

/// Only the file crosses: the mount directory holds the file and nothing from its host directory.
#[tokio::test(flavor = "multi_thread")]
async fn no_sibling_of_the_file_is_visible() {
    let Some(lane) = lane() else {
        return;
    };
    assert!(lane.sibling().is_file(), "the sibling exists on the host");
    let sibling_inside = format!("{MOUNT_DIRECTORY}/sibling");
    let host_sibling = text(&lane.sibling());
    let script = format!(
        "test ! -e {sibling_inside} && test ! -e {host_sibling} && ls -A {MOUNT_DIRECTORY}"
    );
    let listed = waited(
        &lane,
        "ex_file_sibling",
        &["/bin/sh", "-c", &script],
        vec![lane.declared()],
    )
    .await;
    assert_eq!(
        exit_code(&listed),
        Some(0),
        "no sibling is reachable: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout).trim(),
        "gate",
        "the mount directory holds the declared file and nothing else"
    );
}

/// Every filesystem rule refuses before dispatch, by name.
#[tokio::test(flavor = "multi_thread")]
async fn a_directory_a_symlink_or_an_absent_path_is_refused_before_dispatch() {
    let Some(lane) = lane() else {
        return;
    };
    let fixture = lane.host.parent().expect("fixture root").to_owned();
    let link = fixture.join("gate-link");
    std::os::unix::fs::symlink(lane.program(), &link).expect("symlink to the program");
    let linked_directory = fixture.join("bin-link");
    std::os::unix::fs::symlink(&lane.host, &linked_directory).expect("symlink to the directory");
    for (id, host_path, code) in [
        (
            "ex_file_directory",
            text(&lane.host),
            "exec.read-only-file-not-a-file",
        ),
        (
            "ex_file_symlink",
            text(&link),
            "exec.read-only-file-symlink",
        ),
        (
            "ex_file_symlink_parent",
            text(&linked_directory.join("gate")),
            "exec.read-only-file-symlink",
        ),
        (
            "ex_file_absent",
            text(&lane.host.join("absent")),
            "exec.read-only-file-absent",
        ),
    ] {
        let input = exec_input(
            &lane.snapshot,
            &["gate"],
            vec![ReadOnlyFile {
                host_path,
                mount: MOUNT.to_owned(),
            }],
            Vec::new(),
        );
        assert_eq!(refused(&lane, id, &input).await, code, "{id}");
    }
}

/// The lexical rules hold in the driver too, independently of the daemon that checks them first.
#[tokio::test(flavor = "multi_thread")]
async fn the_driver_refuses_every_lexical_rule_by_name() {
    let Some(lane) = lane() else {
        return;
    };
    let declared = lane.declared();
    let at = |mount: &str| ReadOnlyFile {
        host_path: declared.host_path.clone(),
        mount: mount.to_owned(),
    };
    let root = ReadOnlyRoot {
        host_path: text(&lane.host),
        mount: "/opt/gates".to_owned(),
    };
    let over: Vec<ReadOnlyFile> = (0..=MAX_READ_ONLY_FILES)
        .map(|index| at(&format!("/opt/many/{index}")))
        .collect();
    for (id, files, roots, code) in [
        (
            "ex_file_over",
            over,
            Vec::new(),
            "exec.read-only-files-exceeded",
        ),
        (
            "ex_file_relative",
            vec![at("opt/gate")],
            Vec::new(),
            "exec.read-only-file-path-invalid",
        ),
        (
            "ex_file_owned",
            vec![at("/workspace/gate")],
            Vec::new(),
            "exec.read-only-file-mount-reserved",
        ),
        (
            "ex_file_under_root",
            vec![at(MOUNT)],
            vec![root.clone()],
            "exec.read-only-file-mount-overlap",
        ),
        (
            "ex_file_over_root",
            vec![at("/opt")],
            vec![root],
            "exec.read-only-file-mount-overlap",
        ),
        (
            "ex_file_duplicate",
            vec![at(MOUNT), at(MOUNT)],
            Vec::new(),
            "exec.read-only-file-mount-duplicate",
        ),
    ] {
        let input = exec_input(&lane.snapshot, &["gate"], files, roots);
        assert_eq!(refused(&lane, id, &input).await, code, "{id}");
    }
}
