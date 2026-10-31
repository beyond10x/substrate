//! Exec admission re-binds the confinement backend before it executes it.
//!
//! Each case builds a host whose backend is a stand-in script, proves an exec on the unchanged
//! backend passes admission, then replaces the backend one way and asserts the next exec is
//! refused with `exec.capability-stale` before the spawn (the script would append its own letter
//! to a marker file). The replacements keep everything a cheaper identity check could compare equal
//! that a non-privileged user can keep equal: the size always, the modification time where the
//! replacement allows it. A refusal must not depend on the backend changing size or mtime.
//!
//! The unreadable cases hold AGENTS.md invariant 3: a backend whose identity cannot be read is a
//! refusal, never an admission on the strength of an earlier probe.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use substrate_wire::{
    CapabilityFacts, CapabilitySnapshot, CgroupLimitFacts, ExecEnvironment, ExecLimits,
    ExecStartInput, HostDriverKind, NamespaceFacts, NetworkMode, SandboxProfile,
};

use crate::process::ProcessRuntime;
use crate::{DispatchOutcome, HostConfig};

const SNAPSHOT_DIGIT: &str = "6";

struct Host {
    _root: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    backend: PathBuf,
    marker: PathBuf,
    workspace: PathBuf,
}

/// The stand-in backend: it appends `letter` to the marker and exits. Every letter yields a script
/// of the same length, so a replacement never differs in size.
fn script(marker: &Path, letter: char) -> Vec<u8> {
    format!(
        "#!/bin/sh\nprintf {letter} >> '{}'\nexit 0\n",
        marker.display()
    )
    .into_bytes()
}

fn write_executable(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn host() -> Host {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let marker = root.join("marker");
    let backend = bin.join("bwrap");
    write_executable(&backend, &script(&marker, 'A'));
    let cgroup = root.join("cgroup");
    std::fs::create_dir(&cgroup).unwrap();
    std::fs::write(cgroup.join("cgroup.controllers"), "cpu memory pids\n").unwrap();
    let workspaces = root.join("workspaces");
    std::fs::create_dir(&workspaces).unwrap();
    let workspace = workspaces.join("ws_test");
    std::fs::create_dir(&workspace).unwrap();
    // Older than any timestamp tick of the probe, so an implementation that trusts settled
    // metadata trusts it here: each replacement below then meets the cheaper check, not the hash.
    std::thread::sleep(Duration::from_millis(2_100));
    Host {
        _root: directory,
        root,
        bin,
        backend,
        marker,
        workspace,
    }
}

fn runtime(host: &Host, configured_backend: &Path) -> ProcessRuntime {
    let mut config = HostConfig::minimum(host.root.join("workspaces"));
    config.bubblewrap = configured_backend.to_path_buf();
    config.cgroup_root = Some(host.root.join("cgroup"));
    let capability = CapabilitySnapshot {
        snapshot: format!("sha256:{}", SNAPSHOT_DIGIT.repeat(64)),
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
            ..CapabilityFacts::default()
        },
    };
    ProcessRuntime::new(config, capability).expect("runtime")
}

fn input() -> ExecStartInput {
    ExecStartInput {
        writable_roots: Vec::new(),
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workspace: "ws_test".to_owned(),
        argv: vec!["/usr/bin/true".to_owned()],
        env: ExecEnvironment {
            allow: vec![],
            set: BTreeMap::new(),
        },
        sandbox: substrate_wire::ConfinementRequest {
            requires: None,
            capability_snapshot: format!("sha256:{}", SNAPSHOT_DIGIT.repeat(64)),
            network: NetworkMode::None,
            aperture: None,
            apertures: Vec::new(),
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
        workspace_access: substrate_wire::WorkspaceAccess::ReadWrite,
        scratch: None,
        measurements: std::collections::BTreeSet::new(),
        capsule: None,
        lease_ttl_ms: None,
        workdir: None,
    }
}

fn marker(host: &Host) -> String {
    std::fs::read_to_string(&host.marker).unwrap_or_default()
}

/// Admission on the unchanged backend passes, backend recheck included: the control that makes
/// every refusal below attributable to the replacement and not to the fixture.
///
/// `NotDispatched` is returned only before the exec cgroup exists, so it is always before the
/// spawn. The fake delegation root stops a start after admission (`ContainedAbsent` or
/// `OutcomeUnknown`), so the backend itself never runs here; the marker assertion after a refusal
/// is a second guard, not the evidence.
async fn assert_unchanged_backend_is_admitted(host: &Host, runtime: &ProcessRuntime) {
    let outcome = runtime.start("ex_control", &host.workspace, &input()).await;
    if let DispatchOutcome::NotDispatched(error) = &outcome {
        panic!(
            "the unchanged backend must be admitted, got {}: {}",
            error.code, error.message
        );
    }
}

async fn assert_refused_before_backend_runs(host: &Host, runtime: &ProcessRuntime, case: &str) {
    let outcome = runtime.start("ex_after", &host.workspace, &input()).await;
    let DispatchOutcome::NotDispatched(error) = outcome else {
        tokio::time::sleep(Duration::from_millis(500)).await;
        panic!(
            "{case}: the replaced backend was admitted (marker {:?})",
            marker(host)
        );
    };
    assert_eq!(
        error.code, "exec.capability-stale",
        "{case}: {}",
        error.message
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !marker(host).contains('B'),
        "{case}: the replaced backend executed"
    );
}

fn modified(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

fn set_modified(path: &Path, time: std::time::SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

#[tokio::test]
async fn rename_over_of_a_same_size_backend_with_its_mtime_is_stale() {
    let host = host();
    let runtime = runtime(&host, &host.backend);
    assert_unchanged_backend_is_admitted(&host, &runtime).await;
    let staged = host.bin.join("bwrap.new");
    write_executable(&staged, &script(&host.marker, 'B'));
    set_modified(&staged, modified(&host.backend));
    std::fs::rename(&staged, &host.backend).unwrap();
    assert_refused_before_backend_runs(&host, &runtime, "rename-over").await;
}

#[tokio::test]
async fn in_place_rewrite_of_the_same_size_is_stale() {
    let host = host();
    let runtime = runtime(&host, &host.backend);
    assert_unchanged_backend_is_admitted(&host, &runtime).await;
    let mut file = OpenOptions::new().write(true).open(&host.backend).unwrap();
    file.write_all(&script(&host.marker, 'B')).unwrap();
    drop(file);
    assert_refused_before_backend_runs(&host, &runtime, "in-place rewrite").await;
}

#[tokio::test]
async fn in_place_rewrite_with_the_mtime_restored_is_stale() {
    let host = host();
    let runtime = runtime(&host, &host.backend);
    assert_unchanged_backend_is_admitted(&host, &runtime).await;
    let before = modified(&host.backend);
    let mut file = OpenOptions::new().write(true).open(&host.backend).unwrap();
    file.write_all(&script(&host.marker, 'B')).unwrap();
    file.set_modified(before).unwrap();
    drop(file);
    assert_eq!(modified(&host.backend), before, "fixture: mtime restored");
    assert_refused_before_backend_runs(&host, &runtime, "rewrite then mtime restored").await;
}

#[tokio::test]
async fn hard_link_swap_onto_the_configured_path_is_stale() {
    let host = host();
    let runtime = runtime(&host, &host.backend);
    assert_unchanged_backend_is_admitted(&host, &runtime).await;
    let other = host.bin.join("other");
    write_executable(&other, &script(&host.marker, 'B'));
    set_modified(&other, modified(&host.backend));
    let link = host.bin.join("bwrap.link");
    std::fs::hard_link(&other, &link).unwrap();
    std::fs::rename(&link, &host.backend).unwrap();
    assert_refused_before_backend_runs(&host, &runtime, "hard link swap").await;
}

#[tokio::test]
async fn symlink_retarget_of_the_configured_path_is_stale() {
    let host = host();
    let configured = host.bin.join("bwrap-current");
    symlink(&host.backend, &configured).unwrap();
    let runtime = runtime(&host, &configured);
    assert_unchanged_backend_is_admitted(&host, &runtime).await;
    let other = host.bin.join("other");
    write_executable(&other, &script(&host.marker, 'B'));
    set_modified(&other, modified(&host.backend));
    let staged = host.bin.join("bwrap-next");
    symlink(&other, &staged).unwrap();
    std::fs::rename(&staged, &configured).unwrap();
    assert_refused_before_backend_runs(&host, &runtime, "symlink retarget").await;
}

#[tokio::test]
async fn backend_whose_metadata_cannot_be_read_is_refused() {
    let host = host();
    let runtime = runtime(&host, &host.backend);
    assert_unchanged_backend_is_admitted(&host, &runtime).await;
    // Without search permission on its directory, the backend cannot be stat-ed, opened or hashed.
    std::fs::set_permissions(&host.bin, std::fs::Permissions::from_mode(0o000)).unwrap();
    let outcome = runtime.start("ex_after", &host.workspace, &input()).await;
    std::fs::set_permissions(&host.bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let DispatchOutcome::NotDispatched(error) = outcome else {
        panic!("a backend whose metadata is unreadable was admitted");
    };
    assert_eq!(error.code, "exec.capability-stale", "{}", error.message);
}

#[tokio::test]
async fn backend_whose_bytes_cannot_be_read_is_refused() {
    let host = host();
    let runtime = runtime(&host, &host.backend);
    assert_unchanged_backend_is_admitted(&host, &runtime).await;
    // Metadata stays readable; the bytes do not. Identity that cannot be hashed is not proven.
    std::fs::set_permissions(&host.backend, std::fs::Permissions::from_mode(0o000)).unwrap();
    let outcome = runtime.start("ex_after", &host.workspace, &input()).await;
    std::fs::set_permissions(&host.backend, std::fs::Permissions::from_mode(0o755)).unwrap();
    let DispatchOutcome::NotDispatched(error) = outcome else {
        panic!("a backend whose bytes are unreadable was admitted");
    };
    assert_eq!(error.code, "exec.capability-stale", "{}", error.message);
}

#[tokio::test]
async fn removed_backend_is_refused() {
    let host = host();
    let runtime = runtime(&host, &host.backend);
    assert_unchanged_backend_is_admitted(&host, &runtime).await;
    std::fs::remove_file(&host.backend).unwrap();
    let DispatchOutcome::NotDispatched(error) =
        runtime.start("ex_after", &host.workspace, &input()).await
    else {
        panic!("a removed backend was admitted");
    };
    assert_eq!(error.code, "exec.capability-stale", "{}", error.message);
}
