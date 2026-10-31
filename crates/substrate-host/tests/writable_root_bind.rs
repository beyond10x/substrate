//! Writable host roots are bound by descriptor (`story:writable-host-roots`, ADR 0038 kind 1).
//!
//! The operator declares a directory by name (`HostConfig::writable_roots`); a start names it and
//! a mount. Admission opens the declared path once, `openat2` with `RESOLVE_NO_SYMLINKS`, checks
//! the owner on that descriptor, and bubblewrap mounts the descriptor with `--bind-fd`. A path
//! renamed away and replaced by a symlink after admission therefore still mounts the directory
//! admission inspected.
//!
//! The unserved cases are portable: without confinement no probe runs, so the fact is absent and
//! a declared root is refused by name. The rest are delegated: without
//! `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this process is inside, or without
//! bubblewrap, they return early and are **absent, never passed** (invariant 3).
//! `bash scripts/delegated-lane.sh` runs them.
//!
//! Fixtures live under `CARGO_TARGET_TMPDIR` (this checkout's `target/tmp`), canonicalized, so a
//! declared host path has no symlink component unless a case puts one there on purpose.

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use substrate_host::{
    DeclaredWritableRoot, DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver,
};
use substrate_wire::{
    AppliedWritableRoot, ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput,
    NetworkMode, SandboxProfile, WorkspaceAccess, WritableHostRoot, WritableRoot,
};

const WORKSPACE: &str = "ws_test";
const NAME: &str = "shared";
const MOUNT: &str = "/srv/substrate-writable";
const SWAP_MOUNT: &str = "/srv/substrate-writable-swapped";

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

fn declared(name: &str, path: &Path) -> DeclaredWritableRoot {
    DeclaredWritableRoot {
        name: name.to_owned(),
        path: path.to_owned(),
    }
}

fn host_root(name: &str, mount: &str) -> WritableRoot {
    WritableRoot::Host(WritableHostRoot {
        host: name.to_owned(),
        mount: mount.to_owned(),
    })
}

fn exec_input(snapshot: &str, argv: &[&str], writable_roots: Vec<WritableRoot>) -> ExecStartInput {
    ExecStartInput {
        writable_roots,
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

/// The refusal a start is answered with before dispatch: (class, code, message).
async fn refusal(driver: &HostDriver, id: &str, input: &ExecStartInput) -> (String, String) {
    match driver.start_exec(id, WORKSPACE, input).await {
        DispatchOutcome::NotDispatched(error) => (error.code.to_owned(), error.message),
        DispatchOutcome::Observed(_) => panic!("{id} was dispatched; it must be refused first"),
        DispatchOutcome::ContainedAbsent(error) | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("{id} reached dispatch before its refusal: {}", error.code)
        }
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

fn exit_code(observed: &ExecObservation) -> Option<u8> {
    observed.resource.exit.as_ref().and_then(|exit| exit.code)
}

fn stderr(observed: &ExecObservation) -> String {
    String::from_utf8_lossy(&observed.stderr).into_owned()
}

/// The delegated cgroup v2 subtree this process is inside, or nothing.
fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        return None;
    }
    Some(root)
}

/// A delegated driver declaring `roots`, its workspace created.
fn delegated_driver(
    cgroup_root: PathBuf,
    workspaces: &Path,
    roots: Vec<DeclaredWritableRoot>,
    bubblewrap: Option<PathBuf>,
) -> std::sync::Arc<HostDriver> {
    let mut config = HostConfig::minimum(workspaces.join("workspaces"));
    config.cgroup_root = Some(cgroup_root);
    config.writable_roots = roots;
    if let Some(bubblewrap) = bubblewrap {
        config.bubblewrap = bubblewrap;
    }
    let driver = HostDriver::open(config).expect("host driver");
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");
    driver
}

/// Portable: no confinement, no probe, no fact. A declared root is still `unserved` by its name,
/// never a run that quietly got no writable mount.
#[tokio::test(flavor = "multi_thread")]
async fn without_confinement_a_declared_root_is_unserved_by_name() {
    let (_fixture, host) = fixture("rw-root-unserved-");
    let root = host.join("root");
    std::fs::create_dir(&root).expect("declared root");
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(workspaces.path().join("workspaces"));
    config.writable_roots = vec![declared(NAME, &root)];
    let driver = HostDriver::open(config).expect("host driver");
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");
    let machine = driver.machine();
    assert_eq!(
        machine.facts.exec_writable_roots_host, None,
        "a host that cannot confine publishes no writable root"
    );
    let (code, message) = refusal(
        &driver,
        "ex_rw_unserved",
        &exec_input(
            &machine.snapshot,
            &["/bin/true"],
            vec![host_root(NAME, MOUNT)],
        ),
    )
    .await;
    assert_eq!(code, "exec.writable-roots-host-unserved");
    assert!(message.contains(NAME), "{message}");
}

/// The served path, end to end, with the mount at the declared host path itself (ADR 0038
/// decision 4 allows it), so the parent and the sibling are named inside exactly as on the host:
/// the child writes a file in the root and it appears on the host. The sibling is not there inside,
/// and a write to the parent and to the sibling, by their host names, reaches neither on the host:
/// inside, those paths are the sandbox's own discarded root, never the host directories. The
/// applied record names the root by kind, name, host path and mount.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_writes_the_declared_root_and_neither_its_parent_nor_a_sibling() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let (_fixture, host) = fixture("rw-root-served-");
    let parent = host.join("parent");
    let root = parent.join("root");
    let sibling = parent.join("sibling");
    std::fs::create_dir_all(&root).expect("declared root");
    std::fs::create_dir_all(&sibling).expect("sibling");
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let driver = delegated_driver(
        cgroup_root,
        workspaces.path(),
        vec![declared(NAME, &root)],
        None,
    );
    let machine = driver.machine();
    assert_eq!(
        machine.facts.exec_writable_roots_host,
        Some(vec![NAME.to_owned()]),
        "the probe proved the mechanism and the fact lists the declared name"
    );

    let mount = text(&root);
    let script = format!(
        "echo written >'{mount}/written' \
         && test ! -e '{sibling}' \
         && {{ (echo escaped >'{parent}/escaped') 2>/dev/null; \
               (mkdir -p '{sibling}' && echo escaped >'{sibling}/escaped') 2>/dev/null; true; }}",
        parent = text(&parent),
        sibling = text(&sibling),
    );
    let input = exec_input(
        &machine.snapshot,
        &["/bin/sh", "-c", &script],
        vec![host_root(NAME, &mount)],
    );
    let run = observed(&driver, "ex_rw_served", &input).await;
    assert_eq!(exit_code(&run), Some(0), "{}", stderr(&run));
    assert_eq!(
        std::fs::read_to_string(root.join("written")).expect("the file on the host"),
        "written\n"
    );
    let beside: BTreeSet<String> = std::fs::read_dir(&parent)
        .expect("parent")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        beside,
        BTreeSet::from(["root".to_owned(), "sibling".to_owned()]),
        "the parent was written"
    );
    assert_eq!(
        std::fs::read_dir(&sibling).expect("sibling").count(),
        0,
        "the sibling was written"
    );
    let applied = run.resource.applied.expect("applied confinement");
    assert_eq!(
        applied.writable_roots,
        vec![AppliedWritableRoot::Host {
            name: NAME.to_owned(),
            host_path: mount.clone(),
            mount,
        }]
    );
}

/// `workdir` at and beneath a writable root mount runs there (ADR 0038 decision 7).
#[tokio::test(flavor = "multi_thread")]
async fn workdir_beneath_a_writable_root_runs_there() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let (_fixture, host) = fixture("rw-root-workdir-");
    let root = host.join("root");
    std::fs::create_dir_all(root.join("sub")).expect("declared root and subdirectory");
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let driver = delegated_driver(
        cgroup_root,
        workspaces.path(),
        vec![declared(NAME, &root)],
        None,
    );
    let machine = driver.machine();
    for (id, workdir) in [
        ("ex_rw_workdir_at", MOUNT.to_owned()),
        ("ex_rw_workdir_beneath", format!("{MOUNT}/sub")),
    ] {
        let mut input = exec_input(
            &machine.snapshot,
            &["/bin/sh", "-c", "pwd -P && echo here >here"],
            vec![host_root(NAME, MOUNT)],
        );
        input.workdir = Some(workdir.clone());
        let run = observed(&driver, id, &input).await;
        assert_eq!(exit_code(&run), Some(0), "{id}: {}", stderr(&run));
        assert_eq!(
            String::from_utf8_lossy(&run.stdout),
            format!("{workdir}\n"),
            "{id}"
        );
    }
    assert!(root.join("here").is_file());
    assert!(root.join("sub/here").is_file());
}

/// A declared root that is no longer what was declared is refused by name at the start, before
/// anything runs: gone, a symlink, a file, or a directory the daemon does not own. Nothing is
/// widened and nothing is silently dropped.
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_root_that_changed_is_refused_by_name_at_the_start() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let (_fixture, host) = fixture("rw-root-changed-");
    let elsewhere = host.join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("symlink target");
    let mut roots = Vec::new();
    let mut cases = Vec::new();
    for (name, code) in [
        ("gone", "exec.writable-root-absent"),
        ("linked", "exec.writable-root-not-a-directory"),
        ("file", "exec.writable-root-not-a-directory"),
    ] {
        let path = host.join(name);
        std::fs::create_dir(&path).expect("declared root");
        roots.push(declared(name, &path));
        cases.push((name, path, code));
    }
    // A directory this process does not own, where the host has one: declared as it is, because
    // the driver's own check is what this case asks about, not the daemon's startup one.
    let foreign = Path::new("/usr");
    let foreign_case = std::fs::metadata(foreign)
        .is_ok_and(|metadata| std::os::unix::fs::MetadataExt::uid(&metadata) != nix_euid());
    if foreign_case {
        roots.push(declared("foreign", foreign));
    }
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let driver = delegated_driver(cgroup_root, workspaces.path(), roots, None);
    let machine = driver.machine();
    for (name, path, _) in &cases {
        match *name {
            "gone" => std::fs::remove_dir(path).expect("remove the declared root"),
            "linked" => {
                std::fs::remove_dir(path).expect("remove the declared root");
                std::os::unix::fs::symlink(&elsewhere, path).expect("symlink in its place");
            }
            _ => {
                std::fs::remove_dir(path).expect("remove the declared root");
                std::fs::write(path, "not a directory\n").expect("file in its place");
            }
        }
    }
    let mut wrong = Vec::new();
    let all = cases
        .iter()
        .map(|(name, _, code)| (*name, *code))
        .chain(foreign_case.then_some(("foreign", "exec.writable-root-not-owned")));
    for (name, code) in all {
        let input = exec_input(
            &machine.snapshot,
            &["/bin/sh", "-c", "echo ran >/srv/substrate-writable/ran"],
            vec![host_root(name, MOUNT)],
        );
        let (got, message) = refusal(&driver, &format!("ex_rw_changed_{name}"), &input).await;
        if got != code || !message.contains(name) {
            wrong.push(format!(
                "{name}: expected {code} naming it, got {got}: {message}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert_eq!(
        std::fs::read_dir(&elsewhere)
            .expect("symlink target")
            .count(),
        0,
        "a refused start wrote through the symlink"
    );
}

fn nix_euid() -> u32 {
    std::fs::metadata("/proc/self")
        .map(|metadata| std::os::unix::fs::MetadataExt::uid(&metadata))
        .expect("own process directory")
}

/// A bubblewrap that, only when handed the exec that mounts [`SWAP_MOUNT`], records its argv and
/// then renames the admitted root away and puts a symlink to `decoy` at its path, before it runs
/// the real bubblewrap with the same arguments: the window between admission and the mount.
fn swapping_backend(directory: &Path, admitted: &Path, decoy: &Path) -> (PathBuf, PathBuf) {
    let backend = directory.join("bwrap");
    let log = directory.join("argv.log");
    let admitted = text(admitted);
    std::fs::write(
        &backend,
        format!(
            "#!/bin/sh\n\
             for argument in \"$@\"; do\n\
             \x20 if [ \"$argument\" = '{SWAP_MOUNT}' ]; then\n\
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

/// A root swapped for a symlink to another directory after admission and before the mount still
/// mounts the admitted directory: the write lands in it, never in the decoy, and the argv
/// bubblewrap was handed binds it by descriptor with `--bind-fd` and carries no host path.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_swapped_for_a_symlink_after_admission_still_mounts_the_admitted_directory() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let (_fixture, host) = fixture("rw-root-swap-");
    let admitted = host.join("admitted");
    let decoy = host.join("decoy");
    std::fs::create_dir(&admitted).expect("admitted directory");
    std::fs::create_dir(&decoy).expect("decoy directory");
    let backend_directory = host.join("backend");
    std::fs::create_dir(&backend_directory).expect("backend directory");
    let (backend, log) = swapping_backend(&backend_directory, &admitted, &decoy);
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let driver = delegated_driver(
        cgroup_root,
        workspaces.path(),
        vec![declared(NAME, &admitted)],
        Some(backend),
    );
    let machine = driver.machine();
    assert_eq!(
        machine.facts.exec_writable_roots_host,
        Some(vec![NAME.to_owned()]),
        "the probe proved the mechanism through the swapping backend"
    );
    let input = exec_input(
        &machine.snapshot,
        &[
            "/bin/sh",
            "-c",
            &format!("echo written >{SWAP_MOUNT}/written"),
        ],
        vec![host_root(NAME, SWAP_MOUNT)],
    );
    let run = observed(&driver, "ex_rw_swapped", &input).await;
    assert_eq!(exit_code(&run), Some(0), "{}", stderr(&run));

    // The swap happened: without it a green is also what a backend that never swapped looks like.
    assert!(
        std::fs::symlink_metadata(&admitted)
            .expect("the declared path after the run")
            .file_type()
            .is_symlink(),
        "the backend did not swap the root; the case observed nothing"
    );
    assert_eq!(
        std::fs::read_dir(&decoy).expect("decoy").count(),
        0,
        "the write was redirected into the decoy"
    );
    let renamed = host.join("admitted.renamed");
    assert_eq!(
        std::fs::read_to_string(renamed.join("written")).expect("the write in the admitted root"),
        "written\n"
    );

    let recorded = std::fs::read_to_string(&log).expect("the backend recorded the exec argv");
    let argv: Vec<&str> = recorded.lines().take_while(|line| *line != ".").collect();
    let bind = argv
        .windows(3)
        .find(|window| window[2] == SWAP_MOUNT)
        .expect("the exec argv mounts the declared root");
    assert_eq!(bind[0], "--bind-fd", "{argv:?}");
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
