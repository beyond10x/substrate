use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use chrono::Utc;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use substrate_wire::{
    CapabilityFacts, CapabilitySnapshot, CgroupLimitFacts, EXECUTION_CAPSULE_MOUNT,
    ExecutionCapsuleFacts, HostDriverKind, MAX_EXECUTION_CAPSULE_BYTES,
    MAX_EXECUTION_CAPSULE_FILE_BYTES, MAX_EXECUTION_CAPSULE_FILES, NamespaceFacts,
    OPERATION_LEDGER_GLOBAL_MAX_BYTES, OPERATION_LEDGER_GLOBAL_MAX_ROWS,
    OPERATION_LEDGER_SUBJECT_MAX_BYTES, OPERATION_LEDGER_SUBJECT_MAX_ROWS, Signal,
};

use crate::HostConfig;

const MAX_BACKEND_BINARY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BackendBinding {
    bubblewrap_path: String,
    bubblewrap_sha256: String,
    bubblewrap_device: u64,
    bubblewrap_inode: u64,
    cgroup_root: String,
    cgroup_device: u64,
    cgroup_inode: u64,
    controllers: Vec<String>,
}

#[derive(Serialize)]
struct SnapshotMaterial<'a> {
    driver: HostDriverKind,
    driver_version: &'a str,
    config_generation: u64,
    probed_at: chrono::DateTime<Utc>,
    facts: &'a CapabilityFacts,
    backend: &'a Option<BackendBinding>,
}

pub fn probe(config: &HostConfig, openat2: bool) -> CapabilitySnapshot {
    let probed_at = Utc::now();
    let backend = backend_binding(config);
    let workspace = openat2;
    let namespaces = probe_bubblewrap(config);
    let cgroup = probe_cgroup(config);
    let lease_clock = probe_lease_clock();
    let unprivileged = effective_uid() != 0;
    let close_range = probe_close_range();
    let exec = namespaces && cgroup && unprivileged && close_range && backend.is_some();
    let facts = CapabilityFacts {
        events_pull: Some(true),
        events_stream: Some(true),
        events_retention_events: Some(config.event_retention),
        operation_ledger_subject_max_rows: OPERATION_LEDGER_SUBJECT_MAX_ROWS,
        operation_ledger_subject_max_bytes: OPERATION_LEDGER_SUBJECT_MAX_BYTES,
        operation_ledger_global_max_rows: OPERATION_LEDGER_GLOBAL_MAX_ROWS,
        operation_ledger_global_max_bytes: OPERATION_LEDGER_GLOBAL_MAX_BYTES,
        leases_explicit: lease_clock.then_some(true),
        leases_clock_tolerance_ms: lease_clock.then_some(substrate_wire::LEASE_CLOCK_TOLERANCE_MS),
        workspace_guarded_io: workspace.then_some(true),
        workspace_openat2_beneath: workspace.then_some(true),
        workspace_atomic_replace: workspace.then_some(true),
        workspace_max_current: Some(config.max_current_workspaces),
        workspace_max_file_bytes: workspace.then_some(config.max_file_bytes),
        workspace_read_limit_bytes: workspace.then_some(config.read_limit_bytes),
        workspace_list_limit_items: workspace.then_some(config.list_limit_items),
        exec_argv_only: exec.then_some(true),
        exec_namespaces: exec.then_some(NamespaceFacts {
            user: true,
            mount: true,
            pid: true,
            ipc: true,
            uts: true,
            network: true,
        }),
        exec_no_egress: exec.then_some(true),
        exec_cgroup_limits: exec.then_some(CgroupLimitFacts {
            processes: true,
            memory: true,
            cpu: true,
        }),
        exec_cgroup_kill: exec.then_some(true),
        // Persisted bounded output remains observable even when new exec admission is unavailable.
        exec_output_limit_bytes: Some(config.output_limit_bytes),
        exec_max_current: Some(config.max_current_execs),
        exec_signals: exec.then_some(vec![Signal::Int, Signal::Term, Signal::Kill]),
        exec_inline_capsule: exec.then_some(ExecutionCapsuleFacts {
            mount: EXECUTION_CAPSULE_MOUNT.to_owned(),
            max_files: MAX_EXECUTION_CAPSULE_FILES,
            max_file_bytes: MAX_EXECUTION_CAPSULE_FILE_BYTES,
            max_total_bytes: MAX_EXECUTION_CAPSULE_BYTES,
        }),
        snapshot_provenance_events: Some(config.snapshot_provenance_events),
    };
    let driver_version = env!("CARGO_PKG_VERSION");
    let serialized = serde_json::to_vec(&SnapshotMaterial {
        driver: HostDriverKind::Host,
        driver_version,
        config_generation: config.config_generation,
        probed_at,
        facts: &facts,
        backend: &backend,
    })
    .expect("capability snapshot material serializes");
    CapabilitySnapshot {
        snapshot: format!("sha256:{}", hex::encode(Sha256::digest(serialized))),
        driver: HostDriverKind::Host,
        driver_version: driver_version.to_owned(),
        config_generation: config.config_generation,
        probed_at,
        valid_until: None,
        facts,
    }
}

pub(crate) fn backend_binding(config: &HostConfig) -> Option<BackendBinding> {
    let bubblewrap_path = config.bubblewrap.canonicalize().ok()?;
    let bubblewrap_metadata = std::fs::metadata(&bubblewrap_path).ok()?;
    if !bubblewrap_metadata.is_file() || bubblewrap_metadata.len() > MAX_BACKEND_BINARY_BYTES {
        return None;
    }
    let bubblewrap = std::fs::read(&bubblewrap_path).ok()?;
    let cgroup_root = config.cgroup_root.as_ref()?.canonicalize().ok()?;
    let cgroup_metadata = std::fs::metadata(&cgroup_root).ok()?;
    if !cgroup_metadata.is_dir() {
        return None;
    }
    let mut controllers = std::fs::read_to_string(cgroup_root.join("cgroup.controllers"))
        .ok()?
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    controllers.sort();
    controllers.dedup();
    if !["cpu", "memory", "pids"]
        .iter()
        .all(|required| controllers.iter().any(|controller| controller == required))
    {
        return None;
    }
    Some(BackendBinding {
        bubblewrap_path: bubblewrap_path.to_string_lossy().into_owned(),
        bubblewrap_sha256: hex::encode(Sha256::digest(bubblewrap)),
        bubblewrap_device: bubblewrap_metadata.dev(),
        bubblewrap_inode: bubblewrap_metadata.ino(),
        cgroup_root: cgroup_root.to_string_lossy().into_owned(),
        cgroup_device: cgroup_metadata.dev(),
        cgroup_inode: cgroup_metadata.ino(),
        controllers,
    })
}

fn probe_lease_clock() -> bool {
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id");
    let uptime = std::fs::read_to_string("/proc/uptime");
    boot_id.is_ok_and(|value| {
        let value = value.trim();
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    }) && uptime.is_ok_and(|value| {
        value
            .split_whitespace()
            .next()
            .is_some_and(|seconds| seconds.parse::<f64>().is_ok_and(f64::is_finite))
    })
}

fn probe_bubblewrap(config: &HostConfig) -> bool {
    if !config.bubblewrap.is_file() {
        return false;
    }
    let mut command = Command::new(&config.bubblewrap);
    command
        .env_clear()
        .args([
            "--unshare-user",
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
            "--tmpfs",
            "/tmp",
            "--",
            "/usr/bin/env",
            "-u",
            "PWD",
            "--",
            "/usr/bin/true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.status().is_ok_and(|status| status.success())
}

fn probe_cgroup(config: &HostConfig) -> bool {
    let Some(root) = config.cgroup_root.as_ref() else {
        return false;
    };
    let Some(current) = current_cgroup() else {
        return false;
    };
    let configured = root.canonicalize().ok();
    let current = PathBuf::from("/sys/fs/cgroup").join(current.trim_start_matches('/'));
    if configured
        .as_ref()
        .is_none_or(|root| !current.starts_with(root))
    {
        return false;
    }
    if !root.join("cgroup.procs").is_file() || !root.join("cgroup.controllers").is_file() {
        return false;
    }
    if std::fs::OpenOptions::new()
        .write(true)
        .open(root.join("cgroup.procs"))
        .is_err()
    {
        return false;
    }
    let controllers = std::fs::read_to_string(root.join("cgroup.controllers")).unwrap_or_default();
    if !["cpu", "memory", "pids"]
        .iter()
        .all(|required| controllers.split_whitespace().any(|item| item == *required))
    {
        return false;
    }
    if !std::fs::read_to_string(root.join("cgroup.procs"))
        .is_ok_and(|processes| processes.trim().is_empty())
    {
        return false;
    }
    let enabled = std::fs::read_to_string(root.join("cgroup.subtree_control")).unwrap_or_default();
    if !["cpu", "memory", "pids"]
        .iter()
        .all(|required| enabled.split_whitespace().any(|item| item == *required))
        && std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").is_err()
    {
        return false;
    }
    let probe = root.join(format!("probe-{}", ulid::Ulid::new()));
    if std::fs::create_dir(&probe).is_err() {
        return false;
    }
    let usable = probe.join("cgroup.procs").is_file()
        && probe.join("cgroup.kill").is_file()
        && std::fs::write(probe.join("pids.max"), "4").is_ok()
        && std::fs::write(probe.join("memory.max"), "16777216").is_ok()
        && std::fs::write(probe.join("memory.swap.max"), "0").is_ok()
        && std::fs::write(probe.join("cpu.max"), "10000 100000").is_ok();
    let _ = std::fs::remove_dir(&probe);
    usable
}

fn current_cgroup() -> Option<String> {
    std::fs::read_to_string("/proc/self/cgroup")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("0::").map(ToOwned::to_owned))
}

fn effective_uid() -> libc::uid_t {
    // SAFETY: geteuid takes no arguments and has no preconditions.
    unsafe { libc::geteuid() }
}

fn probe_close_range() -> bool {
    let Ok(file) = std::fs::File::open("/dev/null") else {
        return false;
    };
    let descriptor = std::os::fd::AsRawFd::as_raw_fd(&file);
    // SAFETY: the exact probe descriptor is owned here and intentionally closed by close_range.
    let closed = unsafe {
        libc::syscall(
            libc::SYS_close_range,
            u32::try_from(descriptor).expect("descriptor is non-negative"),
            u32::try_from(descriptor).expect("descriptor is non-negative"),
            0_u32,
        ) == 0
    };
    if closed {
        std::mem::forget(file);
    }
    closed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_identity_binds_configuration_generation() {
        let mut first = HostConfig::minimum("/does/not/exist");
        first.config_generation = 7;
        let mut second = first.clone();
        second.config_generation = 8;
        assert_ne!(
            probe(&first, false).snapshot,
            probe(&second, false).snapshot
        );
    }

    #[test]
    fn backend_binding_detects_binary_replacement_with_unchanged_paths() {
        let directory = tempfile::tempdir().unwrap();
        let bubblewrap = directory.path().join("bwrap");
        let cgroup = directory.path().join("cgroup");
        std::fs::create_dir(&cgroup).unwrap();
        std::fs::write(&bubblewrap, b"first backend").unwrap();
        std::fs::write(cgroup.join("cgroup.controllers"), "cpu memory pids\n").unwrap();
        let mut config = HostConfig::minimum(directory.path().join("workspaces"));
        config.bubblewrap = bubblewrap.clone();
        config.cgroup_root = Some(cgroup);
        let first = backend_binding(&config).unwrap();
        std::fs::write(&bubblewrap, b"replacement backend").unwrap();
        let second = backend_binding(&config).unwrap();
        assert_ne!(first, second);
    }
}
