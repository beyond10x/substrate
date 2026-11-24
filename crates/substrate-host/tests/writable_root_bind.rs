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
//!
//! The last three cases are about the start itself, through a backend script as the swapped-root
//! case uses one: the forked child joins the run's cgroup before it executes the backend
//! (`story:delegated-start-names-a-child-that-ended-before-its-cgroup-attach`). The two named
//! `review_…` are its security reviewer's.

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use substrate_host::{
    DeclaredWritableRoot, DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver,
};
use substrate_wire::{
    AppliedWritableRoot, CaptureMode, ConfinementRequest, ExecEnvironment, ExecLimits,
    ExecStartInput, NetworkMode, PipeSessionStartInput, PtyWindow, SandboxProfile, SessionMode,
    WorkspaceAccess, WritableHostRoot, WritableRoot,
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

/// The program argument that tells [`forking_backend`] which exec to act on.
const FORKING_MARK: &str = "substrate-forking-leader";

/// A bubblewrap that, only when handed the exec whose program carries [`FORKING_MARK`], records
/// the cgroup it is in at its first instruction and then forks a thousand short-lived children,
/// four at a time, before it runs the real bubblewrap with the same arguments: a leader whose
/// process tree changes for the whole of the driver's start.
fn forking_backend(directory: &Path) -> (PathBuf, PathBuf) {
    let backend = directory.join("bwrap");
    let log = directory.join("leader.log");
    std::fs::write(
        &backend,
        format!(
            "#!/bin/sh\n\
             for argument in \"$@\"; do\n\
             \x20 if [ \"$argument\" = '{FORKING_MARK}' ]; then\n\
             \x20   read -r own </proc/self/cgroup\n\
             \x20   printf '%s\\n' \"$own\" >>'{log}'\n\
             \x20   turn() {{ n=0; while [ \"$n\" -lt 250 ]; do ( : ); n=$((n + 1)); done; }}\n\
             \x20   turn & turn & turn & turn &\n\
             \x20   wait\n\
             \x20   printf 'forked\\n' >>'{log}'\n\
             \x20   break\n\
             \x20 fi\n\
             done\n\
             exec /usr/bin/bwrap \"$@\"\n",
            log = text(&log),
        ),
    )
    .expect("write the forking backend");
    std::fs::set_permissions(&backend, std::fs::Permissions::from_mode(0o755))
        .expect("make the forking backend executable");
    (backend, log)
}

/// A backend that forks short-lived children before it becomes bubblewrap is started like any
/// other (`story:delegated-start-names-a-child-that-ended-before-its-cgroup-attach`): the start is
/// dispatched and the program runs. The backend is inside the exec's cgroup at its first
/// instruction, so every child it forks is born there and the driver moves none of them, and a
/// child that ended during the start is not a process the driver names to the kernel.
#[tokio::test(flavor = "multi_thread")]
async fn a_backend_that_forks_short_lived_children_is_started_inside_its_cgroup() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let (_fixture, host) = fixture("rw-root-forking-");
    let backend_directory = host.join("backend");
    std::fs::create_dir(&backend_directory).expect("backend directory");
    let (backend, log) = forking_backend(&backend_directory);
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let driver = delegated_driver(cgroup_root, workspaces.path(), Vec::new(), Some(backend));
    let machine = driver.machine();
    let id = "ex_forking_leader";
    let input = exec_input(
        &machine.snapshot,
        &["/bin/sh", "-c", "echo ran", FORKING_MARK],
        Vec::new(),
    );
    let run = observed(&driver, id, &input).await;
    assert_eq!(exit_code(&run), Some(0), "{}", stderr(&run));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "ran\n");

    // The backend forked: without it a green is also what a backend that never forked looks like.
    let recorded = std::fs::read_to_string(&log).expect("the backend recorded its start");
    let lines: Vec<&str> = recorded.lines().collect();
    let [own, "forked"] = lines.as_slice() else {
        panic!("the backend did not fork its children once; the case observed nothing: {lines:?}");
    };
    assert!(
        own.ends_with(&format!("/substrate-{id}")),
        "the backend ran its first instruction outside the exec's cgroup: {own}"
    );
}

/// Every start the two `review_…` cases mark carries one argument with this prefix;
/// [`RECORDING_BACKEND`] acts on no other.
const JOIN_MARK: &str = "review-join-mark-";

/// What the confined program of a marked start does: it records its own cgroup and the target of
/// each of its descriptors in the workspace, under the name of its mark, and says when it is done.
const JOIN_INSIDE: &str = "read -r own </proc/self/cgroup\n\
    printf '%s\\n' \"$own\" >\"$0.cgroup\"\n\
    for descriptor in /proc/$$/fd/*; do\n\
    \x20 printf '%s\\n' \"$(readlink \"$descriptor\")\"\n\
    done >\"$0.fds\"\n\
    : >\"$0.done\"\n";

/// A backend that, for a marked start, records before anything else the cgroup it is in, then a
/// wall-clock stamp and the target of each of its descriptors; then it is bubblewrap.
const RECORDING_BACKEND: &str = "#!/bin/sh\n\
    for argument in \"$@\"; do\n\
    \x20 case \"$argument\" in\n\
    \x20   review-join-mark-*)\n\
    \x20     read -r own </proc/self/cgroup\n\
    \x20     stamp=$(date +%s%N)\n\
    \x20     {\n\
    \x20       printf '%s stamp %s\\n' \"$argument\" \"$stamp\"\n\
    \x20       printf '%s cgroup %s\\n' \"$argument\" \"$own\"\n\
    \x20       for descriptor in /proc/$$/fd/*; do\n\
    \x20         printf '%s fd %s\\n' \"$argument\" \"$(readlink \"$descriptor\")\"\n\
    \x20       done\n\
    \x20     } >>'@LOG@'\n\
    \x20     break\n\
    \x20     ;;\n\
    \x20 esac\n\
    done\n\
    exec /usr/bin/bwrap \"$@\"\n";

/// Writes [`RECORDING_BACKEND`] into `directory` and returns it with its log.
fn recording_backend(directory: &Path) -> (PathBuf, PathBuf) {
    let backend = directory.join("bwrap");
    let log = directory.join("backend.log");
    std::fs::write(&backend, RECORDING_BACKEND.replace("@LOG@", &text(&log)))
        .expect("write the recording backend");
    std::fs::set_permissions(&backend, std::fs::Permissions::from_mode(0o755))
        .expect("make the recording backend executable");
    (backend, log)
}

/// A session whose program is [`JOIN_INSIDE`] under `mark`.
fn session_input(snapshot: &str, mark: &str, mode: SessionMode) -> PipeSessionStartInput {
    PipeSessionStartInput {
        capture: CaptureMode::Recorded,
        exec: ExecStartInput {
            wait: false,
            ..exec_input(snapshot, &["/bin/sh", "-c", JOIN_INSIDE, mark], Vec::new())
        },
        input_limit_bytes: 65_536,
        frame_limit_bytes: 4_096,
        queued_frames: 8,
        mode,
        window: (mode == SessionMode::Pty).then_some(PtyWindow {
            columns: 80,
            rows: 24,
        }),
    }
}

/// The lines the recording backend wrote for `mark`, each without the mark.
fn recorded(log: &Path, mark: &str) -> Vec<String> {
    let prefix = format!("{mark} ");
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
        .collect()
}

/// Whether a descriptor's link target is a file of the cgroup filesystem. On the host the link
/// is the path under `/sys/fs/cgroup`; inside the sandbox, where that mount is not in the view,
/// it is the path within the cgroup filesystem, which still ends in the control file's name.
fn is_cgroup_file(target: &str) -> bool {
    target.contains("/sys/fs/cgroup/")
        || target
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("cgroup."))
}

async fn wait_for(what: &str, limit: Duration, mut reached: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !reached() {
        assert!(Instant::now() < deadline, "{what} within {limit:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// What one marked run must show, whatever started it.
async fn check_run(cgroup_root: &Path, workspace: &Path, log: &Path, mark: &str, id: &str) {
    let name = format!("/substrate-{id}");
    let inside = |suffix: &str| workspace.join(format!("{mark}.{suffix}"));
    wait_for(
        "the confined program finished its record",
        Duration::from_secs(20),
        || inside("done").exists(),
    )
    .await;

    let lines = recorded(log, mark);
    let backend_cgroups: Vec<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("cgroup "))
        .collect();
    let [backend_cgroup] = backend_cgroups.as_slice() else {
        panic!("{id}: the backend did not record its start exactly once: {lines:?}");
    };
    assert!(
        backend_cgroup.ends_with(&name),
        "{id}: the backend ran its first instruction outside the run's cgroup: {backend_cgroup}"
    );
    let backend_fds: Vec<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("fd "))
        .collect();
    // Without this a green is also what an empty record looks like: the backend holds at least
    // its three standard descriptors and the launch barrier.
    assert!(
        backend_fds.len() >= 4 && backend_fds.iter().any(|fd| fd.starts_with("pipe:")),
        "{id}: the backend's descriptor record observed nothing: {backend_fds:?}"
    );
    assert!(
        !backend_fds.iter().any(|fd| is_cgroup_file(fd)),
        "{id}: the backend holds a descriptor on a cgroup file: {backend_fds:?}"
    );

    let program_cgroup = std::fs::read_to_string(inside("cgroup")).expect("the program's cgroup");
    assert!(
        program_cgroup.trim_end().ends_with(&name),
        "{id}: the confined program is outside the run's cgroup: {program_cgroup}"
    );
    let program_fds = std::fs::read_to_string(inside("fds")).expect("the program's descriptors");
    let program_fds: Vec<&str> = program_fds.lines().collect();
    assert!(
        program_fds.len() >= 3,
        "{id}: the program's descriptor record observed nothing: {program_fds:?}"
    );
    assert!(
        !program_fds.iter().any(|fd| is_cgroup_file(fd)),
        "{id}: the confined program holds a descriptor on a cgroup file: {program_fds:?}"
    );

    let cgroup = cgroup_root.join(format!("substrate-{id}"));
    wait_for(
        "the ended run's cgroup is removed",
        Duration::from_secs(20),
        || !cgroup.exists(),
    )
    .await;
}

fn dispatched(id: &str, outcome: DispatchOutcome<ExecObservation>) -> ExecObservation {
    match outcome {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "the delegated lane must dispatch {id}: {} {}",
            error.code, error.message
        ),
    }
}

/// SECURITY REVIEW CASE 1. Claim under review: every process of a run is inside the run's cgroup
/// before the backend executes its first instruction, on every start path, and the descriptor
/// the leader joins through reaches neither the backend nor the confined program.
///
/// One driver, three starts through the public API: an exec, a raw-pipe session, a pty session.
#[tokio::test(flavor = "multi_thread")]
async fn review_every_start_path_runs_its_backend_inside_the_cgroup_without_a_cgroup_descriptor() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let (_fixture, host) = fixture("review-join-paths-");
    let (backend, log) = recording_backend(&host);
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let driver = delegated_driver(
        cgroup_root.clone(),
        workspaces.path(),
        Vec::new(),
        Some(backend),
    );
    let machine = driver.machine();
    assert_eq!(
        machine.facts.sessions_pty,
        Some(true),
        "the delegated lane publishes sessions.pty, or the pty start below observes nothing"
    );
    let workspace = driver.root().join(WORKSPACE);

    let (id, mark) = ("ex_review_join_exec", format!("{JOIN_MARK}exec"));
    let input = exec_input(
        &machine.snapshot,
        &["/bin/sh", "-c", JOIN_INSIDE, &mark],
        Vec::new(),
    );
    let run = dispatched(id, driver.start_exec(id, WORKSPACE, &input).await);
    assert_eq!(exit_code(&run), Some(0), "{}", stderr(&run));
    check_run(&cgroup_root, &workspace, &log, &mark, id).await;

    for (id, kind, mode) in [
        ("ex_review_join_pipes", "pipes", SessionMode::Pipes),
        ("ex_review_join_pty", "pty", SessionMode::Pty),
    ] {
        let mark = format!("{JOIN_MARK}{kind}");
        let input = session_input(&machine.snapshot, &mark, mode);
        dispatched(id, driver.start_pipe_session(id, WORKSPACE, &input).await);
        check_run(&cgroup_root, &workspace, &log, &mark, id).await;
    }
}

/// Makes the kernel refuse every process the cgroup `substrate-<id>` is offered, from the moment
/// that cgroup exists: a cgroup that delegates a domain controller to its children takes no
/// process of its own, and one that holds a process refuses to delegate it (`memory` is such a
/// controller; `pids` and `cpu` are not). Answers the wall-clock instant the cgroup began to
/// refuse, in nanoseconds, or the error when it could not be made to (it already held a process,
/// or never appeared).
fn refuse_members(
    cgroup_root: &Path,
    id: &str,
    stop: &Arc<AtomicBool>,
    spinning: &Arc<AtomicBool>,
) -> std::thread::JoinHandle<std::io::Result<u128>> {
    let control = cgroup_root
        .join(format!("substrate-{id}"))
        .join("cgroup.subtree_control");
    let (stop, spinning) = (Arc::clone(stop), Arc::clone(spinning));
    std::thread::spawn(move || {
        loop {
            match std::fs::write(&control, "+memory") {
                Ok(()) => {
                    return Ok(SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("a clock after the epoch")
                        .as_nanos());
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && !stop.load(Ordering::Acquire) =>
                {
                    spinning.store(true, Ordering::Release);
                    std::hint::spin_loop();
                }
                Err(error) => return Err(error),
            }
        }
    })
}

/// SECURITY REVIEW CASE 2. Claim under review: a child that cannot join its cgroup executes
/// nothing, the start is refused with a named code, and the cgroup and every other resource of
/// the start is removed.
///
/// The refusal is the kernel's own: the cgroup is made to refuse members between its creation
/// and the leader's join, by a thread of this process, which stands for a process outside the
/// sandbox with write access to the delegated subtree. An attempt in which the cgroup could not
/// be made to refuse before the leader joined built no such condition and is tried again under
/// another id, at most eight times; the case fails when no attempt builds it, and never passes on
/// one that did not.
#[tokio::test(flavor = "multi_thread")]
async fn review_a_leader_its_cgroup_refuses_executes_nothing_and_the_start_leaves_nothing() {
    let Some(cgroup_root) = delegated_cgroup_root() else {
        return;
    };
    let (_fixture, host) = fixture("review-join-refused-");
    let (backend, log) = recording_backend(&host);
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let driver = delegated_driver(
        cgroup_root.clone(),
        workspaces.path(),
        Vec::new(),
        Some(backend),
    );
    let machine = driver.machine();

    for attempt in 0..8 {
        let id = format!("ex_review_join_refused_{attempt}");
        let mark = format!("{JOIN_MARK}refused-{attempt}");
        let (stop, spinning) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let refusing = refuse_members(&cgroup_root, &id, &stop, &spinning);
        wait_for("the refusing thread runs", Duration::from_secs(10), || {
            spinning.load(Ordering::Acquire)
        })
        .await;
        let input = exec_input(
            &machine.snapshot,
            &["/bin/sh", "-c", ":", &mark],
            Vec::new(),
        );
        let outcome = driver.start_exec(&id, WORKSPACE, &input).await;
        stop.store(true, Ordering::Release);
        let refused_at = refusing.join().expect("the refusing thread");
        let backend_ran_at = recorded(&log, &mark)
            .iter()
            .find_map(|line| line.strip_prefix("stamp ")?.parse::<u128>().ok());
        match (refused_at, backend_ran_at) {
            // The cgroup refused members and no backend executed: the condition, and the claim.
            (Ok(_), None) => {}
            (Ok(refused_at), Some(ran_at)) if ran_at > refused_at => panic!(
                "{id}: the backend executed after its cgroup refused every member, so it ran \
                 outside the cgroup"
            ),
            // The leader had joined before the cgroup could be made to refuse: nothing built.
            _ => continue,
        }
        match outcome {
            DispatchOutcome::ContainedAbsent(error) => {
                assert_eq!(error.code, "exec.spawn-failed", "{}", error.message);
                assert!(
                    error.message.contains("os error 16"),
                    "{id}: the refusal is not the join's own EBUSY: {}",
                    error.message
                );
                println!("review: {id} refused {}: {}", error.code, error.message);
            }
            DispatchOutcome::Observed(observed) => panic!(
                "{id}: a start whose leader could not join was observed as a run: {:?} {:?}",
                observed.resource.state, observed.resource.exit
            ),
            DispatchOutcome::NotDispatched(error) | DispatchOutcome::OutcomeUnknown(error) => {
                panic!(
                    "{id}: must end contained and absent: {} {}",
                    error.code, error.message
                )
            }
        }
        assert!(
            !cgroup_root.join(format!("substrate-{id}")).exists(),
            "{id}: the cgroup of a start that executed nothing is still there"
        );
        // Every other resource of the refused start went with it: the driver starts the next.
        let next = "ex_review_join_after_refusal";
        let input = exec_input(&machine.snapshot, &["/bin/true"], Vec::new());
        let run = dispatched(next, driver.start_exec(next, WORKSPACE, &input).await);
        assert_eq!(exit_code(&run), Some(0));
        return;
    }
    panic!(
        "no attempt made the cgroup refuse its leader before the join; the case observed nothing"
    );
}
