//! Read-only roots are bound by descriptor, not by path
//! (`story:read-only-roots-bound-by-descriptor`, design 26 § 3).
//!
//! Admission opens each declared root once, refusing a symlink at any component, and bubblewrap
//! mounts that descriptor. A root renamed away and replaced by a symlink to another directory
//! after admission therefore still mounts the directory admission inspected.
//!
//! The refusal cases are portable: root admission is the first filesystem question a start is
//! asked, before any confinement fact, so a host without a delegated cgroup answers it too. The
//! swap case is delegated only: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree
//! this process is inside, or without bubblewrap, it is **absent, never reported as passed**
//! (invariant 3). `bash scripts/delegated-lane.sh` runs it.
//!
//! Fixtures live under `CARGO_TARGET_TMPDIR` (this checkout's `target/tmp`), canonicalized, so a
//! declared host path has no symlink component unless a case puts one there on purpose.

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use substrate_host::{DispatchOutcome, Driver as _, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode, ReadOnlyRoot,
    SandboxProfile, WorkspaceAccess,
};

const WORKSPACE: &str = "ws_test";
const MOUNT: &str = "/opt/substrate-swapped-root";
const ADMITTED: &str = "the directory admission inspected";
const DECOY: &str = "a directory swapped in after admission";

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8 fixture path").to_owned()
}

/// A canonical directory under `CARGO_TARGET_TMPDIR`.
fn fixture(prefix: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("fixture directory under CARGO_TARGET_TMPDIR");
    let canonical = std::fs::canonicalize(directory.path()).expect("canonical fixture directory");
    (directory, canonical)
}

fn root(host_path: &Path) -> ReadOnlyRoot {
    ReadOnlyRoot {
        host_path: text(host_path),
        mount: MOUNT.to_owned(),
    }
}

fn exec_input(snapshot: &str, argv: &[&str], read_only_roots: Vec<ReadOnlyRoot>) -> ExecStartInput {
    ExecStartInput {
        read_only_roots,
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

/// The code a start declaring `roots` is refused with before dispatch.
async fn refusal(
    driver: &HostDriver,
    snapshot: &str,
    id: &str,
    roots: Vec<ReadOnlyRoot>,
) -> String {
    match driver
        .start_exec(id, WORKSPACE, &exec_input(snapshot, &["/bin/true"], roots))
        .await
    {
        DispatchOutcome::NotDispatched(error) => error.code.to_owned(),
        DispatchOutcome::Observed(_) => panic!("{id} was dispatched; it must be refused first"),
        DispatchOutcome::ContainedAbsent(error) | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("{id} reached dispatch before its refusal: {}", error.code)
        }
    }
}

/// Each refusal by its existing code, and one admissible root as the control that shows root
/// admission is not simply refusing everything: on a host without confinement it passes root
/// admission and is refused later, by the confinement floor.
///
/// A symlink above the last component was followed before and is refused now (design 26 § 3), as
/// ADR 0033 already refuses one for a single file.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_is_refused_by_its_existing_code_and_a_symlinked_component_is_not_a_directory() {
    let directory = tempfile::tempdir().expect("temporary host root");
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    let snapshot = driver.machine().snapshot;
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace directory");

    let (_fixture, host) = fixture("ro-root-refusal-");
    let real = host.join("real");
    std::fs::create_dir_all(real.join("inner")).expect("real directory");
    std::fs::write(host.join("file"), "not a directory\n").expect("regular file");
    let linked = host.join("linked");
    std::os::unix::fs::symlink(&real, &linked).expect("symlink to a directory");

    let mut refused = Vec::new();
    for (id, declared, code) in [
        (
            "ex_root_parent_link",
            linked.join("inner"),
            "exec.read-only-root-not-a-directory",
        ),
        (
            "ex_root_trailing_link",
            linked.clone(),
            "exec.read-only-root-not-a-directory",
        ),
        (
            "ex_root_file",
            host.join("file"),
            "exec.read-only-root-not-a-directory",
        ),
        (
            "ex_root_absent",
            host.join("absent"),
            "exec.read-only-root-absent",
        ),
        (
            "ex_root_beneath_file",
            host.join("file").join("beneath"),
            "exec.read-only-root-absent",
        ),
    ] {
        let got = refusal(&driver, &snapshot, id, vec![root(&declared)]).await;
        if got != code {
            refused.push(format!(
                "{id} ({}): expected {code}, got {got}",
                declared.display()
            ));
        }
    }
    let control = refusal(&driver, &snapshot, "ex_root_admitted", vec![root(&real)]).await;
    assert!(
        !control.starts_with("exec.read-only-root-"),
        "an admissible root was refused by root admission: {control}"
    );
    assert!(refused.is_empty(), "{refused:#?}");
}

/// The delegated cgroup v2 subtree this process is inside, or nothing.
fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

/// A bubblewrap that, only when handed the exec that mounts [`MOUNT`], records its argv and then
/// renames the admitted root away and puts a symlink to `decoy` at its path, before it runs the
/// real bubblewrap with the same arguments.
///
/// That is the window the story closes: the driver has admitted the root and launched the backend,
/// and the backend has not yet mounted anything. The probes never name [`MOUNT`], so they run the
/// real bubblewrap untouched. Paths are baked in because the driver clears the environment.
fn swapping_backend(directory: &Path, admitted: &Path, decoy: &Path) -> (PathBuf, PathBuf) {
    let backend = directory.join("bwrap");
    let log = directory.join("argv.log");
    let admitted = text(admitted);
    std::fs::write(
        &backend,
        format!(
            "#!/bin/sh\n\
             for argument in \"$@\"; do\n\
             \x20 if [ \"$argument\" = '{MOUNT}' ]; then\n\
             \x20   {{ for each in \"$@\"; do printf '%s\\n' \"$each\"; done; printf '.\\n'; }} >>'{log}'\n\
             \x20   if [ -d '{admitted}' ] && [ ! -L '{admitted}' ]; then\n\
             \x20     /usr/bin/mv '{admitted}' '{admitted}.renamed' || exit 97\n\
             \x20     /usr/bin/ln -s '{decoy}' '{admitted}' || exit 98\n\
             \x20   fi\n\
             \x20   break\n\
             \x20 fi\n\
             done\n\
             exec /usr/bin/bwrap \"$@\"\n",
            log = text(&log),
            decoy = text(decoy),
        ),
    )
    .expect("write the swapping backend");
    std::fs::set_permissions(&backend, std::fs::Permissions::from_mode(0o755))
        .expect("make the swapping backend executable");
    (backend, log)
}

/// The acceptance case: a root swapped for a symlink to another directory after admission and
/// before the mount is still the admitted directory inside, and the argv bubblewrap was handed
/// names the root by descriptor and carries none of the host paths involved.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_swapped_for_a_symlink_after_admission_still_mounts_the_admitted_directory() {
    let Some(delegated) = delegated_cgroup_root() else {
        return;
    };
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        return;
    }
    let (_fixture, host) = fixture("ro-root-swap-");
    let admitted = host.join("admitted");
    let decoy = host.join("decoy");
    std::fs::create_dir(&admitted).expect("admitted directory");
    std::fs::create_dir(&decoy).expect("decoy directory");
    std::fs::write(admitted.join("marker"), format!("{ADMITTED}\n")).expect("admitted marker");
    std::fs::write(decoy.join("marker"), format!("{DECOY}\n")).expect("decoy marker");
    let backend_directory = host.join("backend");
    std::fs::create_dir(&backend_directory).expect("backend directory");
    let (backend, log) = swapping_backend(&backend_directory, &admitted, &decoy);

    let workspaces = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(workspaces.path().join("workspaces"));
    config.cgroup_root = Some(delegated);
    config.bubblewrap = backend;
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    assert_eq!(
        machine.facts.exec_argv_only,
        Some(true),
        "the delegated lane proves confined exec through the swapping backend"
    );
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");

    let input = exec_input(
        &machine.snapshot,
        &["/bin/cat", &format!("{MOUNT}/marker")],
        vec![root(&admitted)],
    );
    let observed = match driver
        .start_exec("ex_root_swapped", WORKSPACE, &input)
        .await
    {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "the delegated lane must dispatch the swapped-root exec: {} {}",
            error.code, error.message
        ),
    };

    // The swap happened, between admission and the mount: the declared path is now a symlink and
    // reading through it on the host reaches the decoy. Without this a green would also be what a
    // backend that never swapped looks like.
    let swapped = std::fs::symlink_metadata(&admitted).expect("the declared path after the run");
    assert!(
        swapped.file_type().is_symlink(),
        "the backend did not swap the root; the case observed nothing"
    );
    assert_eq!(
        std::fs::read_to_string(admitted.join("marker")).expect("the declared path on the host"),
        format!("{DECOY}\n"),
        "on the host the declared path now leads to the decoy"
    );

    let stdout = String::from_utf8_lossy(&observed.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&observed.stderr).into_owned();
    assert_eq!(
        observed.resource.exit.as_ref().and_then(|exit| exit.code),
        Some(0),
        "the swapped-root exec did not run: {stderr}"
    );
    assert_eq!(
        stdout,
        format!("{ADMITTED}\n"),
        "the child saw a directory other than the one admission inspected: {stderr}"
    );

    let recorded = std::fs::read_to_string(&log).expect("the backend recorded the exec argv");
    let argv: Vec<&str> = recorded.lines().take_while(|line| *line != ".").collect();
    let bind = argv
        .windows(3)
        .find(|window| window[2] == MOUNT)
        .expect("the exec argv mounts the declared root");
    assert_eq!(bind[0], "--ro-bind-fd", "{argv:?}");
    assert!(
        bind[1]
            .parse::<u32>()
            .is_ok_and(|fd| fd > substrate_wire::MAX_SECRET_SLOT_FD),
        "the root descriptor {} lies in or below the secret-slot range: {argv:?}",
        bind[1]
    );
    let host_text = text(&host);
    assert!(
        !argv.iter().any(|part| part.contains(&host_text)),
        "a root host path reaches the bubblewrap argv: {argv:?}"
    );
}
