use std::path::PathBuf;
use std::process::{Command, Stdio};

use chrono::Utc;
use sha2::{Digest as _, Sha256};
use substrate_wire::{
    CapabilityFacts, CapabilitySnapshot, CgroupLimitFacts, HostDriverKind, NamespaceFacts,
    OPERATION_LEDGER_GLOBAL_MAX_BYTES, OPERATION_LEDGER_GLOBAL_MAX_ROWS,
    OPERATION_LEDGER_SUBJECT_MAX_BYTES, OPERATION_LEDGER_SUBJECT_MAX_ROWS, Signal,
};

use crate::HostConfig;

pub fn probe(config: &HostConfig, openat2: bool) -> CapabilitySnapshot {
    let workspace = openat2;
    let namespaces = probe_bubblewrap(config);
    let cgroup = probe_cgroup(config);
    let lease_clock = probe_lease_clock();
    let unprivileged = effective_uid() != 0;
    let close_range = probe_close_range();
    let exec = namespaces && cgroup && unprivileged && close_range;
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
        snapshot_provenance_events: Some(config.snapshot_provenance_events),
    };
    let serialized = serde_json::to_vec(&facts).expect("capability facts serialize");
    CapabilitySnapshot {
        snapshot: format!("sha256:{}", hex::encode(Sha256::digest(serialized))),
        driver: HostDriverKind::Host,
        driver_version: env!("CARGO_PKG_VERSION").to_owned(),
        config_generation: config.config_generation,
        probed_at: Utc::now(),
        valid_until: None,
        facts,
    }
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
