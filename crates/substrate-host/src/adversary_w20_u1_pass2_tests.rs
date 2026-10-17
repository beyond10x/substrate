//! Adversary pass 2 on the adopted-workspace exec pin (`pin_workspace_directory`, `--bind-fd`).
//!
//! Each case drives exec admission against an entry, or the workspace root, changed after it was
//! adopted or created, and asserts the answer the pin's own documentation and the commit that
//! introduced it give: absent is `resource.not-found`; a link, a file or a mount in the entry's
//! place is `workspace.path-escape`; and the object bubblewrap mounts is the one admission pinned.
//!
//! The admission cases use a stand-in backend and a fake delegation root, as
//! `backend_recheck_conformance_tests` does: a `NotDispatched` answer is decided before any spawn,
//! and the fake root stops an admitted start before the backend runs. The bind case runs the real
//! bubblewrap the host carries, as `probe_workspace_scoped_write` does, and returns without
//! asserting when there is none.

use std::collections::BTreeMap;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use substrate_wire::{
    CapabilityFacts, CapabilitySnapshot, CgroupLimitFacts, ExecEnvironment, ExecLimits,
    ExecStartInput, HostDriverKind, NamespaceFacts, NetworkMode, SandboxProfile, WorkspaceAccess,
};

use crate::process::ProcessRuntime;
use crate::{DispatchOutcome, Driver as _, HostConfig, HostDriver};

const SNAPSHOT: &str = "sha256:7777777777777777777777777777777777777777777777777777777777777777";

struct Host {
    _directory: tempfile::TempDir,
    root: PathBuf,
    workspaces: PathBuf,
    outside: PathBuf,
}

fn host() -> Host {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let backend = bin.join("bwrap");
    std::fs::write(
        &backend,
        format!(
            "#!/bin/sh\nprintf ran >> '{}'\n",
            root.join("ran").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&backend, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cgroup = root.join("cgroup");
    std::fs::create_dir(&cgroup).unwrap();
    std::fs::write(cgroup.join("cgroup.controllers"), "cpu memory pids\n").unwrap();
    let workspaces = root.join("workspaces");
    std::fs::create_dir(&workspaces).unwrap();
    let outside = root.join("outside-the-workspace-root");
    std::fs::create_dir(&outside).unwrap();
    Host {
        _directory: directory,
        root,
        workspaces,
        outside,
    }
}

/// A runtime that has proved scoped workspace mounts, so a narrower access reaches the pin.
fn runtime(host: &Host) -> ProcessRuntime {
    let mut config = HostConfig::minimum(host.workspaces.clone());
    config.bubblewrap = host.root.join("bin").join("bwrap");
    config.cgroup_root = Some(host.root.join("cgroup"));
    let capability = CapabilitySnapshot {
        snapshot: SNAPSHOT.to_owned(),
        driver: HostDriverKind::Host,
        driver_version: "test".to_owned(),
        config_generation: config.config_generation,
        probed_at: chrono::Utc::now(),
        valid_until: None,
        facts: CapabilityFacts {
            exec_argv_only: Some(true),
            exec_namespaces: Some(NamespaceFacts {
                user: true,
                mount: true,
                pid: true,
                ipc: true,
                uts: true,
                network: true,
            }),
            exec_no_egress: Some(true),
            exec_cgroup_limits: Some(CgroupLimitFacts {
                processes: true,
                memory: true,
                cpu: true,
            }),
            exec_cgroup_kill: Some(true),
            exec_output_limit_bytes: Some(65_536),
            exec_workspace_scoped_write: Some(true),
            ..CapabilityFacts::default()
        },
    };
    ProcessRuntime::new(config, capability).expect("runtime")
}

fn input(workspace: &str, access: WorkspaceAccess) -> ExecStartInput {
    ExecStartInput {
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workspace: workspace.to_owned(),
        argv: vec!["/usr/bin/true".to_owned()],
        env: ExecEnvironment {
            allow: vec![],
            set: BTreeMap::new(),
        },
        sandbox: substrate_wire::ConfinementRequest {
            capability_snapshot: SNAPSHOT.to_owned(),
            network: NetworkMode::None,
            aperture: None,
            profile: SandboxProfile::Workspace,
            required: true,
        },
        limits: ExecLimits {
            timeout_ms: 5_000,
            output_bytes: 65_536,
            processes: 16,
            memory_bytes: 67_108_864,
            cpu_millis: 1_000,
        },
        wait: false,
        workspace_access: access,
        scratch: None,
        measurements: std::collections::BTreeSet::new(),
        capsule: None,
        lease_ttl_ms: None,
    }
}

fn scoped() -> WorkspaceAccess {
    WorkspaceAccess::Scoped {
        writable_subtrees: vec!["allowed".to_owned()],
    }
}

/// Creates `name` under the root with an `allowed` subtree, then moves it aside: the state an
/// operator leaves by replacing the adopted directory after adopt.
fn adopted_then_moved_aside(host: &Host, name: &str) -> PathBuf {
    let entry = host.workspaces.join(name);
    std::fs::create_dir_all(entry.join("allowed")).unwrap();
    std::fs::rename(&entry, host.workspaces.join(format!("{name}-before"))).unwrap();
    entry
}

async fn refusal(runtime: &ProcessRuntime, workspace: &Path, input: &ExecStartInput) -> String {
    match runtime.start("ex_pass2", workspace, input).await {
        DispatchOutcome::NotDispatched(error) => error.code.to_owned(),
        _ => "<admitted: not refused before dispatch>".to_owned(),
    }
}

/// The pin's documentation and the commit that added it: "absent is `resource.not-found`", and
/// "Scoped access gives the same two codes". A removed adopted entry under a scoped access is
/// answered by the path walk in `admit_workspace_access`, which runs before the pin.
#[tokio::test]
async fn pass2_scoped_access_in_a_removed_adopted_entry_is_resource_not_found() {
    let host = host();
    let runtime = runtime(&host);
    let entry = adopted_then_moved_aside(&host, "adopted-gone");
    let code = refusal(&runtime, &entry, &input("ws_adopted", scoped())).await;
    assert_eq!(code, "resource.not-found");
}

/// A link in the adopted entry's place is `workspace.path-escape` under every access. Under a
/// scoped access whose subtree the link target lacks, the path walk follows the link and answers
/// for the link target (`exec.workspace-subtree-absent`) before the pin refuses the link.
#[tokio::test]
async fn pass2_scoped_access_through_a_swapped_link_is_path_escape_when_the_target_lacks_the_subtree()
 {
    let host = host();
    let runtime = runtime(&host);
    let entry = adopted_then_moved_aside(&host, "adopted-link");
    symlink(&host.outside, &entry).unwrap();
    let code = refusal(&runtime, &entry, &input("ws_adopted", scoped())).await;
    assert_eq!(code, "workspace.path-escape");
}

/// Control for the case above: with the subtree present in the link target, the walk passes and
/// the pin refuses the link.
#[tokio::test]
async fn pass2_scoped_access_through_a_swapped_link_is_path_escape_when_the_target_has_the_subtree()
{
    let host = host();
    let runtime = runtime(&host);
    let entry = adopted_then_moved_aside(&host, "adopted-link2");
    std::fs::create_dir(host.outside.join("allowed")).unwrap();
    symlink(&host.outside, &entry).unwrap();
    let code = refusal(&runtime, &entry, &input("ws_adopted", scoped())).await;
    assert_eq!(code, "workspace.path-escape");
}

/// Read-only access reaches the pin with no subtree walk.
#[tokio::test]
async fn pass2_read_only_access_through_a_swapped_link_is_path_escape() {
    let host = host();
    let runtime = runtime(&host);
    let entry = adopted_then_moved_aside(&host, "adopted-ro");
    symlink(&host.outside, &entry).unwrap();
    let code = refusal(
        &runtime,
        &entry,
        &input("ws_adopted", WorkspaceAccess::ReadOnly),
    )
    .await;
    assert_eq!(code, "workspace.path-escape");
}

/// A created workspace (`ws_` root name) swapped for a link behaves as an adopted one does.
#[tokio::test]
async fn pass2_a_created_workspace_swapped_for_a_link_is_path_escape() {
    let host = host();
    let runtime = runtime(&host);
    let entry = adopted_then_moved_aside(&host, "ws_created");
    symlink(&host.outside, &entry).unwrap();
    let code = refusal(
        &runtime,
        &entry,
        &input("ws_created", WorkspaceAccess::ReadWrite),
    )
    .await;
    assert_eq!(code, "workspace.path-escape");
}

/// A component above the entry: the configured workspace root itself replaced by a link after
/// the driver opened it. The file routes resolve every root name beneath the root descriptor the
/// driver pinned at open (`GuardedFilesystem::workspace_fd`), so they still serve the adopted
/// directory; exec re-resolves the root by path (`pin_workspace_directory` opens the parent with
/// a following `open`), so it resolves the root name beneath the link target instead. The two
/// must name one object: here the link target has no such entry, so exec answering
/// `resource.not-found` while the file route serves the directory shows exec bound by path.
#[tokio::test]
async fn pass2_exec_and_the_file_routes_resolve_one_workspace_root_after_the_root_is_swapped() {
    let directory = tempfile::tempdir().expect("tempdir");
    let workspaces = directory.path().join("workspaces");
    std::fs::create_dir(&workspaces).unwrap();
    let driver = HostDriver::open(HostConfig::minimum(&workspaces)).expect("host driver");
    if !driver.filesystem.openat2_available() {
        return;
    }
    std::fs::create_dir(workspaces.join("project")).unwrap();
    driver
        .filesystem
        .observe_workspace("project")
        .expect("the directory is adoptable");

    let elsewhere = directory.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::rename(&workspaces, directory.path().join("workspaces-before")).unwrap();
    symlink(&elsewhere, &workspaces).unwrap();

    driver
        .filesystem
        .observe_workspace("project")
        .expect("the file routes still resolve the adopted directory beneath the pinned root");
    let snapshot = driver.machine().snapshot;
    let mut start = input("ws_adopted", WorkspaceAccess::ReadWrite);
    start.sandbox.capability_snapshot = snapshot;
    let code = match driver.start_exec("ex_rootswap", "project", &start).await {
        DispatchOutcome::NotDispatched(error) => error.code.to_owned(),
        _ => "<admitted>".to_owned(),
    };
    assert_ne!(
        code, "resource.not-found",
        "exec resolved the root name beneath the swapped workspace root, not the pinned one"
    );
}

/// The bind itself, with the host's real bubblewrap: pin the entry, then swap it for a link (the
/// window between admission and the mount), then mount the prepared read-write access exactly as
/// an exec does. The write must land in the pinned directory and nowhere in the link target, and
/// the program must hold no descriptor that resolves to the workspace.
#[test]
fn pass2_a_link_swapped_in_after_the_pin_never_reaches_the_mount_and_no_descriptor_leaks() {
    let bubblewrap = Path::new("/usr/bin/bwrap");
    if !bubblewrap.is_file() {
        return;
    }
    let host = host();
    let entry = host.workspaces.join("adopted-window");
    std::fs::create_dir(&entry).unwrap();
    let pinned = crate::fs::pin_workspace_directory(&entry).expect("pinned");
    let access = ProcessRuntime::prepare_workspace_access(&pinned, &WorkspaceAccess::ReadWrite)
        .expect("read-write access");
    drop(pinned);
    let before = host.workspaces.join("adopted-window-before");
    std::fs::rename(&entry, &before).unwrap();
    symlink(&host.outside, &entry).unwrap();

    let descriptors = access.raw_fds();
    let script = format!(
        "printf pinned > /workspace/probe && test \"$(pwd -P)\" = /workspace && {}",
        crate::process::no_descriptor_resolves_to(&["/workspace"], "/descriptor-probe")
    );
    let mut command = Command::new(bubblewrap);
    // SAFETY: `pre_exec` calls only async-signal-safe `fcntl`; the vector is built before the fork.
    unsafe {
        command.pre_exec(move || crate::process::inherit(&descriptors));
    }
    let output = command
        .env_clear()
        .args(crate::process::USER_NAMESPACE_ARGV)
        .args([
            "--unshare-ipc",
            "--unshare-pid",
            "--unshare-net",
            "--unshare-uts",
            "--new-session",
            "--die-with-parent",
            "--clearenv",
            "--ro-bind",
            "/usr",
            "/usr",
            "--ro-bind-try",
            "/bin",
            "/bin",
            "--ro-bind-try",
            "/lib",
            "/lib",
            "--ro-bind-try",
            "/lib64",
            "/lib64",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
        ])
        .args(access.bind_args())
        .args(["--chdir", "/workspace", "--", "/bin/sh", "-c"])
        .arg(script)
        .output()
        .expect("bubblewrap runs");
    drop(access);
    assert!(
        output.status.success(),
        "{:?}\nstdout {}\nstderr {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(before.join("probe")).expect("the write landed in the pinned directory"),
        b"pinned"
    );
    assert_eq!(
        std::fs::read_dir(&host.outside).unwrap().count(),
        0,
        "the link target received the write"
    );
}
