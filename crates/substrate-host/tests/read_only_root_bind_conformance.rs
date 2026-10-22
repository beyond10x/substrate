//! Independent conformance cases for `story:read-only-roots-bound-by-descriptor` (design 26 § 3).
//!
//! What `read_only_root_bind.rs` does not already state: the `EACCES` mapping, the symlink forms
//! a declared root can take beyond a plain parent or trailing link, the root descriptors at the
//! maximum count beside the maximum number of read-only files and secret slots, and how long the
//! driver itself holds a root descriptor once the exec is spawned.
//!
//! The refusal cases are portable. The rest are delegated: without `SUBSTRATE_VECTORS_CGROUP_ROOT`
//! naming a cgroup v2 subtree this process is inside, or without bubblewrap, they return early and
//! are **absent, never passed**. `bash scripts/delegated-lane.sh` runs them.

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use substrate_host::{DispatchOutcome, Driver as _, HostConfig, HostDriver, SecretSlot};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode, ReadOnlyFile,
    ReadOnlyRoot, SandboxProfile, SecretSlotRequest, WorkspaceAccess,
};

const WORKSPACE: &str = "ws_test";

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8 fixture path").to_owned()
}

/// A canonical directory under `CARGO_TARGET_TMPDIR`, so a declared host path has no symlink
/// component unless a case puts one there on purpose.
fn fixture(prefix: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("fixture directory under CARGO_TARGET_TMPDIR");
    let canonical = std::fs::canonicalize(directory.path()).expect("canonical fixture directory");
    (directory, canonical)
}

fn root_at(host_path: &str, mount: &str) -> ReadOnlyRoot {
    ReadOnlyRoot {
        host_path: host_path.to_owned(),
        mount: mount.to_owned(),
    }
}

fn exec_input(snapshot: &str, argv: &[&str]) -> ExecStartInput {
    ExecStartInput {
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
    }
}

/// The code a start declaring `roots` is answered with before dispatch.
async fn refusal(
    driver: &HostDriver,
    snapshot: &str,
    id: &str,
    roots: Vec<ReadOnlyRoot>,
) -> String {
    let mut input = exec_input(snapshot, &["/bin/true"]);
    input.read_only_roots = roots;
    match driver.start_exec(id, WORKSPACE, &input).await {
        DispatchOutcome::NotDispatched(error) => error.code.to_owned(),
        DispatchOutcome::Observed(_) => panic!("{id} was dispatched; it must be refused first"),
        DispatchOutcome::ContainedAbsent(error) | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("{id} reached dispatch before its refusal: {}", error.code)
        }
    }
}

fn portable_driver() -> (tempfile::TempDir, Arc<HostDriver>, String) {
    let directory = tempfile::tempdir().expect("temporary host root");
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    let snapshot = driver.machine().snapshot;
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace directory");
    (directory, driver, snapshot)
}

/// Restores search permission on drop, so the fixture can be removed whatever the case asserted.
struct Unlock(PathBuf);

impl Drop for Unlock {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
    }
}

/// Invariant 4: a root beneath a directory the driver may not search is answered
/// `exec.read-only-root-absent`, as a root that is not there is, and never as `not-a-directory`
/// or a later code.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_beneath_an_unsearchable_directory_is_absent() {
    // SAFETY: `geteuid` reads the caller's effective uid and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("absent: uid 0 searches a mode-000 directory, so EACCES cannot be produced");
        return;
    }
    let (_host_root, driver, snapshot) = portable_driver();
    let (_fixture, host) = fixture("ro-root-eacces-");
    let locked = host.join("locked");
    std::fs::create_dir_all(locked.join("inner")).expect("locked/inner");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("mode 000");
    let _unlock = Unlock(locked.clone());
    assert_eq!(
        std::fs::symlink_metadata(locked.join("inner"))
            .expect_err("the fixture is unsearchable")
            .raw_os_error(),
        Some(libc::EACCES),
        "the fixture does not produce EACCES on this host"
    );
    let got = refusal(
        &driver,
        &snapshot,
        "ex_root_eacces",
        vec![root_at(&text(&locked.join("inner")), "/opt/locked")],
    )
    .await;
    assert_eq!(got, "exec.read-only-root-absent");
}

/// Invariant 3, the symlink forms the existing case does not state: a dangling link, a link to a
/// regular file, a link loop and a `/proc/<pid>/root` magic link in a parent component. Each is a
/// symlink at some component, so each is `exec.read-only-root-not-a-directory`.
#[tokio::test(flavor = "multi_thread")]
async fn every_symlink_form_of_a_root_is_not_a_directory() {
    let (_host_root, driver, snapshot) = portable_driver();
    let (_fixture, host) = fixture("ro-root-links-");
    let real = host.join("real");
    std::fs::create_dir(&real).expect("real directory");
    std::fs::write(host.join("file"), "file\n").expect("regular file");
    std::os::unix::fs::symlink(host.join("nowhere"), host.join("dangling")).expect("dangling");
    std::os::unix::fs::symlink(host.join("file"), host.join("to-file")).expect("to-file");
    std::os::unix::fs::symlink(host.join("loop-b"), host.join("loop-a")).expect("loop-a");
    std::os::unix::fs::symlink(host.join("loop-a"), host.join("loop-b")).expect("loop-b");
    let magic = format!("/proc/{}/root{}", std::process::id(), text(&real));
    assert!(
        Path::new(&magic).is_dir(),
        "the magic-link path {magic} must reach a real directory, or the case proves nothing"
    );

    let mut wrong = Vec::new();
    for (id, declared) in [
        ("ex_root_dangling", text(&host.join("dangling"))),
        ("ex_root_to_file", text(&host.join("to-file"))),
        ("ex_root_loop", text(&host.join("loop-a"))),
        (
            "ex_root_loop_parent",
            text(&host.join("loop-a").join("inner")),
        ),
        ("ex_root_magic_parent", magic.clone()),
    ] {
        let got = refusal(
            &driver,
            &snapshot,
            id,
            vec![root_at(&declared, "/opt/link")],
        )
        .await;
        if got != "exec.read-only-root-not-a-directory" {
            wrong.push(format!(
                "{id} ({declared}): expected exec.read-only-root-not-a-directory, got {got}"
            ));
        }
    }
    let control = refusal(
        &driver,
        &snapshot,
        "ex_root_real",
        vec![root_at(&text(&real), "/opt/link")],
    )
    .await;
    assert!(
        !control.starts_with("exec.read-only-root-"),
        "an admissible root was refused by root admission: {control}"
    );
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The delegated cgroup v2 subtree this process is inside, or nothing.
fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

const SLOT_NAMES: [&str; 8] = [
    "slot_a", "slot_b", "slot_c", "slot_d", "slot_e", "slot_f", "slot_g", "slot_h",
];
/// The top of the secret-slot range, next to where every pinned descriptor is raised.
const SLOT_FDS: [u32; 8] = [56, 57, 58, 59, 60, 61, 62, 63];

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    fixture: PathBuf,
    _workspaces: tempfile::TempDir,
    _fixture: tempfile::TempDir,
}

/// A delegated driver; with `slots`, eight secret slots declared, `slot_x` holding `slot-<fd>`.
fn lane(prefix: &str, slots: bool) -> Option<Lane> {
    let delegated = delegated_cgroup_root()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        return None;
    }
    let (fixture_dir, fixture) = fixture(prefix);
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(workspaces.path().join("workspaces"));
    config.cgroup_root = Some(delegated);
    if slots {
        std::fs::create_dir(fixture.join("slots")).expect("slot directory");
        config.secret_slots = SLOT_NAMES
            .iter()
            .zip(SLOT_FDS)
            .map(|(name, fd)| {
                let path = fixture.join("slots").join(name);
                std::fs::write(&path, format!("slot-{fd}")).expect("slot bytes");
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    .expect("slot mode");
                SecretSlot {
                    name: (*name).to_owned(),
                    path,
                }
            })
            .collect();
    }
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    if machine.facts.exec_argv_only != Some(true) {
        eprintln!(
            "lane absent: the floor is not proven on this host: {:?}",
            machine.facts
        );
        return None;
    }
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");
    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        fixture,
        _workspaces: workspaces,
        _fixture: fixture_dir,
    })
}

/// Invariants 5 and 6: four roots, sixteen read-only files and eight secret slots at the top of
/// the slot range, in one start. It dispatches; the program reads every root, every file and every
/// slot at its declared descriptor; and it holds no descriptor beyond its stdio, its slots and
/// `ls`'s own directory handle, so no root, file or workspace descriptor reached it.
#[tokio::test(flavor = "multi_thread")]
async fn four_roots_sixteen_files_and_eight_slots_start_together() {
    let Some(lane) = lane("ro-root-max-", true) else {
        return;
    };
    assert!(
        lane.driver.machine().facts.secrets_slots.is_some(),
        "the delegated host publishes the declared slots"
    );
    let max_roots = usize::try_from(substrate_wire::MAX_READ_ONLY_ROOTS).expect("bound");
    let max_files = usize::try_from(substrate_wire::MAX_READ_ONLY_FILES).expect("bound");
    let mut roots = Vec::new();
    let mut expected = String::new();
    let mut script = String::new();
    for index in 0..max_roots {
        let host = lane.fixture.join(format!("root{index}"));
        std::fs::create_dir(&host).expect("root directory");
        std::fs::write(host.join("marker"), format!("root {index}\n")).expect("root marker");
        roots.push(root_at(&text(&host), &format!("/opt/roots/r{index}")));
        writeln!(expected, "root {index}").expect("writing to a String");
        write!(script, "cat /opt/roots/r{index}/marker; ").expect("writing to a String");
    }
    let mut files = Vec::new();
    std::fs::create_dir(lane.fixture.join("files")).expect("file directory");
    for index in 0..max_files {
        let host = lane.fixture.join("files").join(format!("f{index:02}"));
        std::fs::write(&host, format!("file {index}\n")).expect("file bytes");
        files.push(ReadOnlyFile {
            host_path: text(&host),
            mount: format!("/opt/files/f{index:02}"),
        });
        writeln!(expected, "file {index}").expect("writing to a String");
        write!(script, "cat /opt/files/f{index:02}; ").expect("writing to a String");
    }
    for fd in SLOT_FDS {
        writeln!(expected, "slot-{fd}").expect("writing to a String");
        write!(script, "cat /proc/self/fd/{fd}; echo; ").expect("writing to a String");
    }
    script.push_str("echo --; ls /proc/self/fd/");
    let mut input = exec_input(&lane.snapshot, &["/bin/sh", "-c", &script]);
    input.read_only_roots = roots;
    input.read_only_files = files;
    input.secret_slots = SLOT_NAMES
        .iter()
        .zip(SLOT_FDS)
        .map(|(name, fd)| SecretSlotRequest {
            slot: (*name).to_owned(),
            fd,
        })
        .collect();

    let observed = match lane
        .driver
        .start_exec("ex_root_max", WORKSPACE, &input)
        .await
    {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "four roots, sixteen files and eight slots must dispatch: {} {}",
            error.code, error.message
        ),
    };
    let stdout = String::from_utf8_lossy(&observed.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&observed.stderr).into_owned();
    assert_eq!(
        observed.resource.exit.as_ref().and_then(|exit| exit.code),
        Some(0),
        "the exec did not run cleanly: {stderr}"
    );
    let (reads, listing) = stdout
        .split_once("--\n")
        .unwrap_or_else(|| panic!("no descriptor listing: {stdout}\n{stderr}"));
    assert_eq!(reads, expected, "a root, file or slot read wrong: {stderr}");
    let mut allowed: BTreeSet<String> = ["0", "1", "2", "3"].map(str::to_owned).into();
    allowed.extend(SLOT_FDS.iter().map(u32::to_string));
    let extra: Vec<&str> = listing
        .split_whitespace()
        .filter(|name| !allowed.contains(*name))
        .collect();
    assert!(
        extra.is_empty(),
        "descriptors beyond stdio and the slots reached the program: {extra:?}\n{listing}"
    );
}

/// Every descriptor of this process that resolves to `target`.
fn descriptors_resolving_to(target: &Path) -> Vec<String> {
    std::fs::read_dir("/proc/self/fd")
        .expect("this process's descriptor table")
        .filter_map(Result::ok)
        .filter(|entry| std::fs::read_link(entry.path()).is_ok_and(|link| link == target))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Invariant 5, the driver's side: "closed after spawn". Once the program is running, the driver
/// holds no descriptor on the root it pinned; bubblewrap had the only copy that mattered and has
/// already mounted it. A start with `wait: true` is the case that tells: the start call itself is
/// still open while the program runs.
#[tokio::test(flavor = "multi_thread")]
async fn the_driver_holds_no_root_descriptor_once_the_program_runs() {
    let Some(lane) = lane("ro-root-held-", false) else {
        return;
    };
    let admitted = lane.fixture.join("admitted");
    std::fs::create_dir(&admitted).expect("admitted directory");

    // The scan is not vacuous: a descriptor this case opens on the root is found by it.
    let probe = std::fs::File::open(&admitted).expect("a probe descriptor on the root");
    assert!(
        !descriptors_resolving_to(&admitted).is_empty(),
        "the descriptor scan does not see an open descriptor on the root"
    );
    drop(probe);
    assert!(
        descriptors_resolving_to(&admitted).is_empty(),
        "a descriptor on the root is open before the start"
    );

    let started = lane.driver.root().join(WORKSPACE).join("started");
    let mut input = exec_input(
        &lane.snapshot,
        &["/bin/sh", "-c", "touch /workspace/started; sleep 3"],
    );
    input.read_only_roots = vec![root_at(&text(&admitted), "/opt/held")];
    let driver = Arc::clone(&lane.driver);
    let start =
        tokio::spawn(async move { driver.start_exec("ex_root_held", WORKSPACE, &input).await });

    let deadline = Instant::now() + Duration::from_secs(20);
    while !started.exists() {
        assert!(
            Instant::now() < deadline && !start.is_finished(),
            "the program never ran"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let while_running = descriptors_resolving_to(&admitted);
    let outcome = start.await.expect("the start task");
    let after_return = descriptors_resolving_to(&admitted);
    match outcome {
        DispatchOutcome::Observed(observed) => assert_eq!(
            observed.resource.exit.as_ref().and_then(|exit| exit.code),
            Some(0),
            "{}",
            String::from_utf8_lossy(&observed.stderr)
        ),
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("the exec must dispatch: {} {}", error.code, error.message)
        }
    }
    assert!(
        while_running.is_empty() && after_return.is_empty(),
        "the driver holds the pinned root after spawn: descriptors {while_running:?} while the \
         program runs, {after_return:?} after the start returned"
    );
}
