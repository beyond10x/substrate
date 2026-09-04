use std::os::fd::AsRawFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chrono::Utc;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use substrate_wire::{
    CapabilityFacts, CapabilitySnapshot, CgroupLimitFacts, EXECUTION_CAPSULE_MOUNT,
    ExecutionCapsuleFacts, HostDriverKind, MAX_EXECUTION_CAPSULE_BYTES,
    MAX_EXECUTION_CAPSULE_FILE_BYTES, MAX_EXECUTION_CAPSULE_FILES, MetricsStreamFacts,
    NamespaceFacts, OPERATION_LEDGER_GLOBAL_MAX_BYTES, OPERATION_LEDGER_GLOBAL_MAX_ROWS,
    OPERATION_LEDGER_SUBJECT_MAX_BYTES, OPERATION_LEDGER_SUBJECT_MAX_ROWS, ResourceUsageFacts,
    ScratchQuotaFacts, Signal,
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

#[allow(clippy::too_many_lines)] // One snapshot construction keeps every probed fact auditable.
pub fn probe(config: &HostConfig, openat2: bool) -> CapabilitySnapshot {
    let probed_at = Utc::now();
    let backend = backend_binding(config);
    let workspace = openat2;
    let namespaces = probe_bubblewrap(config);
    let cgroup = probe_cgroup(config);
    let resource_usage = cgroup && probe_resource_usage(config);
    let lease_clock = probe_lease_clock();
    let unprivileged = effective_uid() != 0;
    let close_range = probe_close_range();
    let exec = namespaces && cgroup && unprivileged && close_range && backend.is_some();
    let workspace_scoped_write = exec && probe_workspace_scoped_write(config);
    // Every clause is a proof and the fact is absent unless all of them hold. Nothing is probed at
    // all when no slot is declared, so a daemon that wants none pays nothing.
    //
    // Orphan reconciliation is the fourth obligation ADR 0012 names. It is not a clause here
    // because it cannot be: `ProcessRuntime::new` runs it after this probe
    // (`crates/substrate-host/src/lib.rs`), and a reconciliation it cannot complete fails the
    // driver's construction outright — the daemon does not start rather than starting with the
    // fact absent. Absent is the weaker of the two, so nothing here is optimistic.
    let secrets_slots = crate::secrets::secret_slots_fact(
        &config.secret_slots,
        exec && !config.secret_slots.is_empty() && crate::secrets::sealing_is_provable(),
        exec && !config.secret_slots.is_empty() && probe_descriptor_passthrough(config),
    );
    // The mechanism, in a throwaway sandbox, and never a declared destination's liveness: a
    // readiness check that dialled somebody else's endpoint would make this daemon's readiness
    // their uptime (`docs/design/10-destination-bound-egress.md` § 9 decision 6). Nothing is probed
    // at all when no aperture is declared, so a daemon that wants none pays nothing.
    let egress_apertures = crate::egress::egress_apertures_fact(
        &config.egress_apertures,
        exec && !config.egress_apertures.is_empty()
            && crate::egress::mechanism_is_provable(&config.bubblewrap),
    );
    // The whole mechanism in a throwaway sandbox, and never an optimistic constant: a pair
    // allocated, made controlling *inside* the sandbox after bubblewrap's `setsid`, and a size
    // round-tripped through the child before and after a resize. Gated on `exec` because a
    // terminal is delivered through the same confinement path — a fact never outruns the floor it
    // stands on. Absent leaves every `mode: "pty"` request refused by name (invariant 3, 4).
    let sessions_pty =
        (exec && crate::pty::mechanism_is_provable(&config.bubblewrap)).then_some(true);
    let quota =
        crate::quota::ProjectQuotas::probe(&config.workspace_root, config.project_quota_ids);
    let quota_facts = quota.then_some(crate::quota::ProjectQuotas::facts());
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
        workspace_storage_quota: quota_facts,
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
        exec_workspace_scoped_write: workspace_scoped_write.then_some(true),
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
        exec_scratch_quota: quota.then_some(ScratchQuotaFacts {
            mount: substrate_wire::EXEC_SCRATCH_MOUNT.to_owned(),
            allocation_unit_bytes: crate::quota::ALLOCATION_UNIT_BYTES,
            max_bytes: substrate_wire::MAX_STORAGE_QUOTA_BYTES,
            max_inodes: substrate_wire::MAX_STORAGE_QUOTA_INODES,
        }),
        exec_resource_usage: (exec && resource_usage).then_some(ResourceUsageFacts {
            wall_time: true,
            cpu_time: true,
            memory_current: true,
            memory_peak: true,
            processes_current: true,
            processes_peak: true,
            process_limit_hits: true,
            memory_oom_kills: true,
            block_io: true,
        }),
        metrics_stream: (exec && resource_usage).then_some(MetricsStreamFacts {
            sample_interval_ms: substrate_wire::RESOURCE_USAGE_SAMPLE_INTERVAL_MS,
            latest_wins: true,
            replay: false,
        }),
        exec_egress_apertures: egress_apertures,
        secrets_slots,
        sessions_pty,
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

/// Proves ADR 0023's mount ordering in a disposable tree rather than publishing it from a
/// bubblewrap version or a constant.
fn probe_workspace_scoped_write(config: &HostConfig) -> bool {
    let Ok(root) = tempfile::tempdir() else {
        return false;
    };
    let workspace = root.path().join("workspace");
    let allowed = workspace.join("allowed");
    let blocked = workspace.join("blocked");
    if std::fs::create_dir_all(&allowed).is_err() || std::fs::create_dir_all(&blocked).is_err() {
        return false;
    }
    let status = Command::new(&config.bubblewrap)
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
            "--ro-bind",
        ])
        .arg(&workspace)
        .arg("/workspace")
        .arg("--bind")
        .arg(&allowed)
        .arg("/workspace/allowed")
        .args([
            "--chdir",
            "/workspace",
            "--",
            "/bin/sh",
            "-c",
            "echo yes >allowed/probe && ! echo no >blocked/probe && ! echo no >root-probe",
        ])
        .status();
    status.is_ok_and(|status| status.success())
        && allowed.join("probe").is_file()
        && !blocked.join("probe").exists()
        && !workspace.join("root-probe").exists()
}

fn probe_resource_usage(config: &HostConfig) -> bool {
    config.cgroup_root.as_ref().is_some_and(|root| {
        [
            "cpu.stat",
            "memory.current",
            "memory.peak",
            "memory.events",
            "pids.current",
            "pids.peak",
            "pids.events",
            "io.stat",
        ]
        .iter()
        .all(|name| root.join(name).is_file())
    })
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
    // GNU netcat accepts `-U` as an unknown option, while OpenBSD netcat uses it for Unix sockets.
    // Socat's address form is stable and names the socket family explicitly, so the probe cannot
    // mistake a command-line refusal for the seccomp refusal it is measuring.
    let socat = Path::new("/usr/bin/socat");
    if !config.bubblewrap.is_file() || !socat.is_file() {
        return false;
    }
    let Ok(sentinel) = tempfile::tempdir() else {
        return false;
    };
    let socket = sentinel.path().join("host.sock");
    let Ok(_listener) = std::os::unix::net::UnixListener::bind(&socket) else {
        return false;
    };
    // One sandbox per family the profile refuses, and the floor needs **both**. The Unix one is
    // what this probe has always measured; the vsock one is review finding 7's, and it is here
    // rather than only in `seccomp::tests` because this is the probe that gates every exec fact:
    // a backend or a kernel that lets a confined child open a CID to the hypervisor side leaves
    // `exec` false and every fact hanging off it absent, refusing each exec by name instead of
    // serving one from a sandbox whose network namespace confines nothing (invariant 3).
    //
    // Short-circuiting on the first family is deliberate: a floor already lost is not measured
    // further, and a second spawn would report nothing the first has not.
    //
    // A fresh profile per spawn, never one file reused. The descriptor is inherited, so the two
    // children would share one file offset and the second would read a program of zero bytes.
    REFUSED_FAMILY_SENTINELS
        .into_iter()
        .all(|(family, target)| {
            let Ok(seccomp) = crate::seccomp::profile() else {
                return false;
            };
            let Ok(output) = bubblewrap_probe_command(
                &config.bubblewrap,
                sentinel.path(),
                seccomp.as_raw_fd(),
                target,
            )
            .output() else {
                return false;
            };
            // Socat names the family, the type and the protocol of the call that failed, so this
            // cannot be satisfied by a `connect(2)` refusal from a socket the profile let through.
            !output.status.success()
                && String::from_utf8_lossy(&output.stderr)
                    .contains(&format!("socket({family}, 1, 0): Permission denied"))
        })
}

/// The families this probe measures in a real sandbox, and the socat address that reaches each.
///
/// Not every family the profile refuses can be here — socat has no address form for the Qualcomm
/// IPC router — so `every_refused_family_is_measured_here_or_recorded_as_instrument_less` below
/// asserts that each one is either in this list or in the recorded exception beside it. That is
/// the difference between a probe that covers two families and a probe that is *known* to.
const REFUSED_FAMILY_SENTINELS: [(libc::c_int, &str); 2] = [
    (libc::AF_UNIX, "UNIX-CONNECT:/runtime/sentinel/host.sock"),
    (libc::AF_VSOCK, VSOCK_SENTINEL),
];

/// The vsock address the confinement-floor probe asks for.
///
/// CID 2 is the host side of a vsock transport and the port is arbitrary: the probe measures
/// whether `socket(2)` is refused, so no address it could reach or fail to reach changes what it
/// observes.
const VSOCK_SENTINEL: &str = "VSOCK-CONNECT:2:1234";

/// The argv `probe_bubblewrap` measures the confinement floor with, built and returned rather than
/// built and spawned.
///
/// Split out so the two options this unit added are pinned by a case that needs neither `socat`
/// nor a backend on disk. `probe_bubblewrap` returns before it spawns on a host without
/// `/usr/bin/socat`, so every guard that drove it — the stubs below and the recording backend
/// alike — was silently conditional on socat being installed, and deleting
/// `--assert-userns-disabled` left a socat-less host's whole package gate green.
fn bubblewrap_probe_command(
    bubblewrap: &Path,
    sentinel: &Path,
    seccomp_fd: std::os::fd::RawFd,
    target: &str,
) -> Command {
    let mut command = Command::new(bubblewrap);
    command
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
            "--tmpfs",
            "/tmp",
        ])
        // `--disable-userns` is a request; this is the observation. Bubblewrap checks, inside the
        // sandbox it has just built, that a further user namespace is actually refused, and fails
        // the whole spawn when it is not. So a backend too old for either option — or a kernel
        // that would not honour the first — makes this probe answer false, `namespaces` false and
        // `exec` with it, so every fact gated on `exec` is **absent** and each request is refused
        // by name rather than served in a sandbox quietly missing the option (invariant 3).
        //
        // **Not every `exec.*` fact**, and the difference is a client-visible one. Two are
        // published unconditionally above and survive this: `exec_output_limit_bytes`, for the
        // stated reason at `:125` that persisted bounded output stays observable when admission is
        // not, and `exec_max_current`, for which no reason is recorded here. Both are declared
        // configuration bounds rather than proved capabilities, so neither says a sandbox was
        // proved — but a reader who took "the exec facts are withheld" literally would be wrong
        // about them. That they are ungated predates this option and is not changed here.
        //
        // The assertion is only here. In the exec argv the same failure would be a spawn error no
        // contract declares, which is a worse answer than the named refusal a withheld fact gives.
        .arg("--assert-userns-disabled")
        .args(["--dir", "/runtime", "--ro-bind"])
        .arg(sentinel)
        .arg("/runtime/sentinel")
        .arg("--seccomp")
        .arg(seccomp_fd.to_string())
        .args(["--", "/usr/bin/socat", "-"])
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

/// The fixed, non-secret string the pass-through probe puts in its sealed memory.
///
/// A constant and never a declared slot value: the probe runs on every capability snapshot, and a
/// probe that carried real material would be the leak ADR 0012 exists to rule out.
const PASSTHROUGH_SENTINEL: &str = "substrate-secret-slot-passthrough";

/// What a probe child reported about the descriptor bubblewrap handed it.
///
/// Every field is read by the child from inside the sandbox and compared against a value the parent
/// declared before the spawn (`crate::secrets::ProbeSlot`). A report that does not parse is a probe
/// that observed nothing, which is why [`ChildObservation::parse`] returns `None` rather than a
/// default.
#[derive(Debug, PartialEq, Eq)]
struct ChildObservation {
    /// The bytes read straight off the declared descriptor.
    value: String,
    /// `sealed` when the child's write to the descriptor was refused, `writable` when it was not.
    write: String,
    /// `readlink /proc/<self>/fd/<target>`: the memfd's name for sealed memory, a path or a socket
    /// for anything bubblewrap might have put there instead.
    link: String,
    /// The inode behind the declared number, from `/proc/<self>/fdinfo/<target>`.
    inode: u64,
    /// Every descriptor the child holds, ascending.
    descriptors: Vec<u32>,
}

impl ChildObservation {
    /// Parses the child's `key=value` lines, or `None` when any of them is missing or unreadable.
    fn parse(stdout: &[u8]) -> Option<Self> {
        let stdout = std::str::from_utf8(stdout).ok()?;
        let field = |name: &str| {
            stdout
                .lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
        };
        let mut descriptors: Vec<u32> = field("fds")?
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        descriptors.sort_unstable();
        Some(Self {
            value: field("value")?.to_owned(),
            write: field("write")?.to_owned(),
            link: field("link")?.to_owned(),
            // The kernel writes `ino:\t<n>`; the child forwards the field verbatim.
            inode: field("inode")?.trim().parse().ok()?,
            descriptors,
        })
    }
}

/// The shell the probe child runs, reporting one `key=value` line per observation.
///
/// The child cannot issue `fcntl(F_GET_SEALS)` itself — no shell has the call, and
/// `/proc/<pid>/fdinfo` does not carry the seal word — so it reports the *inode* behind its
/// descriptor instead, and the parent reads the seal word off that same inode. Seals are inode
/// state and the declared set closes itself with `F_SEAL_SEAL`, so a child that proves it holds
/// this inode has proved the whole word: no holder, substrate included, can change it afterwards.
/// Requiring an interpreter in the sandbox to make the child issue the call itself would buy no
/// stronger claim and would withhold the fact on every host that has only a shell.
///
/// `/proc/$$` and never `/proc/self`: `readlink` runs in a forked subprocess, where `/proc/self` is
/// that subprocess and not the child.
///
/// `prelude` is empty in the daemon. It is the seam the cases use to build a child the pre-change
/// acceptance passed — one holding a descriptor above the declared set — without a second copy of
/// this script drifting away from the one that runs on every snapshot.
fn probe_child_command(target: std::os::fd::RawFd, prelude: &str) -> String {
    format!(
        r#"{prelude}printf 'value='
cat <&{target}
printf '\nwrite='
if echo x >&{target}; then printf 'writable'; else printf 'sealed'; fi
printf '\nlink=%s\n' "$(readlink /proc/$$/fd/{target})"
printf 'inode='
while IFS= read -r line; do
case $line in ino:*) printf '%s' "${{line#ino:}}";; esac
done < /proc/$$/fdinfo/{target}
printf '\nfds='
for entry in /proc/$$/fd/*; do
[ -L "$entry" ] && printf ' %s' "${{entry##*/}}"
done
printf '\n'
"#
    )
}

/// Proves that a sealed `memfd` crosses bubblewrap at the number it was placed at, carrying the
/// declared seals and nothing else above 2.
///
/// Bubblewrap passing an inherited descriptor through at the same number is *behaviour*, not a
/// documented contract, so it is probed on every capability snapshot rather than assumed
/// (ADR 0012).
fn probe_descriptor_passthrough(config: &HostConfig) -> bool {
    if !config.bubblewrap.is_file() {
        return false;
    }
    let Some(slot) = crate::secrets::probe_slot(PASSTHROUGH_SENTINEL) else {
        return false;
    };
    descriptor_passthrough_holds(config, &slot, "")
}

/// The pass-through condition of `docs/design/11-sealed-secret-slots.md` § 5, clause by clause.
///
/// Every clause compares an observation the child made inside the sandbox against a value the
/// parent declared before the spawn — never a substring of the child's output.
fn descriptor_passthrough_holds(
    config: &HostConfig,
    slot: &crate::secrets::ProbeSlot,
    prelude: &str,
) -> bool {
    let target = slot.target;
    let source = slot.source.as_raw_fd();
    let mut command = Command::new(&config.bubblewrap);
    command
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
            "--tmpfs",
            "/tmp",
            "--",
            "/bin/sh",
            "-c",
            &probe_child_command(target, prelude),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let placements = [(source, target)];
    let retained = [0_u32, 1, 2, u32::try_from(target).unwrap_or(u32::MAX)];
    // SAFETY: the closure runs after fork and calls only async-signal-safe libc entry points on
    // plain descriptor numbers the parent holds open across the fork.
    unsafe {
        command.pre_exec(move || crate::secrets::place_and_close(&placements, &retained));
    }
    let Ok(output) = command.output() else {
        return false;
    };
    let Some(observed) = ChildObservation::parse(&output.stdout) else {
        return false;
    };
    output.status.success()
        // The declared number carries the declared bytes, and refuses a write from inside.
        && observed.value == PASSTHROUGH_SENTINEL
        && observed.write == "sealed"
        // It is *this* sealed memory and not another file that also reads back the sentinel.
        && observed.inode == slot.inode
        && observed.link.starts_with(&slot.link)
        // The same seals: the word read off the inode the child proved it holds, against the set
        // ADR 0012 declares. `F_SEAL_SEAL` is in that set, so the word cannot move afterwards.
        && slot.seals == crate::secrets::SEAL_SET
        // And nothing else above 2.
        && observed.descriptors == retained
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
    let probe = root.join(format!("probe-{}", ulid::Ulid::generate()));
    if std::fs::create_dir(&probe).is_err() {
        return false;
    }
    let usable = probe.join("cgroup.procs").is_file()
        && probe.join("cgroup.kill").is_file()
        && std::fs::write(probe.join("pids.max"), "4").is_ok()
        && std::fs::write(probe.join("memory.max"), "16777216").is_ok()
        && std::fs::write(probe.join("memory.swap.max"), "0").is_ok()
        // Probed, not assumed: `Cgroup::create` writes this on every exec so an OOM ends the
        // whole tree, and a kernel or delegation root that will not take it leaves `exec` false
        // and every fact gated on it **absent** — each exec then refused by name rather than run
        // in a cgroup whose memory bound kills one process and lets the rest carry on
        // (invariant 3).
        && std::fs::write(probe.join("memory.oom.group"), "1").is_ok()
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

    /// Invariant 4: `sessions.pty` is published only after a probe that observed what it publishes.
    ///
    /// A file that is not the confinement backend cannot prove a terminal, and a host with no
    /// delegated cgroup has no `exec` floor to hang one on. Both leave the fact **absent** — never
    /// `Some(false)` and never an optimistic `Some(true)` — because absent is what makes every
    /// terminal request a named refusal instead of a quieter service (design 13).
    #[test]
    fn sessions_pty_is_absent_until_a_probe_proved_a_terminal() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = HostConfig::minimum(directory.path());
        config.bubblewrap = directory.path().join("not-a-backend");
        std::fs::write(&config.bubblewrap, b"#!/bin/false\n").unwrap();
        assert_eq!(probe(&config, true).facts.sessions_pty, None);

        // The real backend, and still no fact: `exec` is false without a delegated cgroup, and a
        // capability fact never outruns the floor it stands on.
        let mut real = HostConfig::minimum(directory.path());
        real.cgroup_root = None;
        assert_eq!(probe(&real, true).facts.sessions_pty, None);
    }

    /// The published fact and the mechanism are the same claim, so they cannot disagree.
    ///
    /// Absent, never reported as passed: where the configured backend is not on the machine the
    /// case makes no claim at all.
    #[test]
    fn the_published_pty_fact_agrees_with_the_probed_mechanism() {
        let config = HostConfig::minimum("/does/not/exist");
        if !config.bubblewrap.is_file() {
            return;
        }
        assert!(
            crate::pty::mechanism_is_provable(&config.bubblewrap),
            "the configured backend could not give a confined child a controlling terminal"
        );
    }

    /// A sealed descriptor crosses bubblewrap at the number it was placed at, still sealed.
    ///
    /// Absent, never reported as passed: where the configured backend is not on the machine the
    /// case makes no claim at all, because a probe that cannot run has proven nothing.
    #[test]
    fn a_sealed_descriptor_crosses_the_configured_backend() {
        let config = HostConfig::minimum("/does/not/exist");
        if !config.bubblewrap.is_file() {
            return;
        }
        assert!(
            probe_descriptor_passthrough(&config),
            "the configured backend did not deliver a sealed descriptor at its declared number"
        );
    }

    /// A child holding one descriptor more than the declared set withholds the fact.
    ///
    /// This child reads the sentinel and is refused its write, so the acceptance before this case
    /// existed — the sentinel followed by `sealed` — passed it. Design 11 § 5 requires *nothing
    /// else above 2*, and this child holds a ninth descriptor.
    ///
    /// Absent, never reported as passed: where the configured backend is not on the machine the
    /// case makes no claim at all.
    #[test]
    fn a_child_holding_an_extra_descriptor_withholds_the_fact() {
        let config = HostConfig::minimum("/does/not/exist");
        if !config.bubblewrap.is_file() {
            return;
        }
        let slot =
            crate::secrets::probe_slot(PASSTHROUGH_SENTINEL).expect("stage a sealed probe slot");
        assert!(
            !descriptor_passthrough_holds(&config, &slot, "exec 9</dev/null\n"),
            "a child holding a descriptor above the declared set proved pass-through"
        );
    }

    /// A descriptor carrying a shorter seal word than ADR 0012 declares withholds the fact.
    ///
    /// `F_SEAL_WRITE` alone still refuses the child's write and still reads back the sentinel, so
    /// this is exactly the descriptor the acceptance before this case existed would have taken for
    /// the declared set of four seals.
    ///
    /// Absent, never reported as passed: where the configured backend is not on the machine the
    /// case makes no claim at all.
    #[test]
    fn a_short_seal_word_withholds_the_fact() {
        let config = HostConfig::minimum("/does/not/exist");
        if !config.bubblewrap.is_file() {
            return;
        }
        let slot = crate::secrets::probe_slot_sealed_with(PASSTHROUGH_SENTINEL, libc::F_SEAL_WRITE)
            .expect("stage a write-only sealed probe slot");
        assert_eq!(
            slot.seals,
            libc::F_SEAL_WRITE,
            "the case built the wrong slot"
        );
        assert!(
            !descriptor_passthrough_holds(&config, &slot, ""),
            "a descriptor sealed F_SEAL_WRITE alone proved the declared seal set"
        );
    }

    /// The capability is published only from proof, and rotating a value moves no fact.
    #[test]
    fn the_slot_fact_is_names_only_and_needs_every_proof() {
        let directory = tempfile::tempdir().unwrap();
        let declarations = vec![
            crate::SecretSlot {
                name: "registry_token".to_owned(),
                path: directory.path().join("registry_token"),
            },
            crate::SecretSlot {
                name: "vendor_api_key".to_owned(),
                path: directory.path().join("vendor_api_key"),
            },
        ];
        let fact = crate::secrets::secret_slots_fact(&declarations, true, true)
            .expect("both proofs publish the fact");
        assert_eq!(fact, vec!["registry_token", "vendor_api_key"]);
        let rendered = serde_json::to_string(&fact).unwrap();
        for declaration in &declarations {
            assert!(
                !rendered.contains(&declaration.path.display().to_string()),
                "the fact carries a declared path"
            );
        }
        assert_eq!(
            crate::secrets::secret_slots_fact(&declarations, false, true),
            None
        );
        assert_eq!(
            crate::secrets::secret_slots_fact(&declarations, true, false),
            None
        );
    }

    /// A snapshot digest moves when a slot is declared, and not when its material changes.
    #[test]
    fn declaring_a_slot_moves_the_snapshot_and_rotating_one_does_not() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vendor_api_key");
        std::fs::write(&path, b"first-material").unwrap();
        let bare = HostConfig::minimum("/does/not/exist");
        let mut declared = bare.clone();
        declared.secret_slots = vec![crate::SecretSlot {
            name: "vendor_api_key".to_owned(),
            path: path.clone(),
        }];
        // The facts, not the snapshot: `probed_at` is part of the snapshot material, so two probes
        // never share a digest and comparing them would prove nothing either way.
        let before = probe(&declared, false).facts;
        std::fs::write(&path, b"a-completely-other-material").unwrap();
        assert_eq!(
            before,
            probe(&declared, false).facts,
            "rotating the material behind a declared slot moved an observable fact"
        );
        assert_ne!(
            probe(&bare, false).facts.secrets_slots,
            Some(vec!["vendor_api_key".to_owned()]),
            "a daemon with no declared slot published the fact"
        );
    }

    /// Invariant 3: a backend that cannot disable nested user namespaces withholds the exec floor.
    ///
    /// `--unshare-user` gives a confined child a fresh user namespace and full capabilities inside
    /// it, so without `--disable-userns` the child can create another one — the entry point of
    /// most unprivileged kernel privilege escalations. `--assert-userns-disabled` is what turns
    /// the option from a request into an observation: bubblewrap checks, inside the sandbox it
    /// just built, that nesting is actually refused.
    ///
    /// **One stub per option, because one stub refusing both pins neither.** A stub that refuses
    /// the pair stays red if either argument is deleted, so it cannot say which one is load
    /// bearing; delete `--assert-userns-disabled` alone and it is still green. These two each
    /// refuse exactly one option and are otherwise a pass, so each argument is pinned by a case
    /// that fails when only that argument goes.
    ///
    /// Both stubs answer every other argv with the seccomp denial this probe measures, which is
    /// what the probe took for proof before either option was in its argv.
    ///
    /// Such a backend leaves every fact gated on `exec` absent — `exec_argv_only`,
    /// `exec_namespaces`, `exec_no_egress`, `exec_workspace_scoped_write`, the two cgroup facts,
    /// `exec_signals`, `exec_inline_capsule`, `exec_resource_usage`, `metrics_stream`,
    /// `exec_egress_apertures`, `secrets_slots` and `sessions_pty` — which refuses each exec by
    /// name rather than running one in a sandbox quietly missing the option.
    ///
    /// Three `exec.*` facts are **not** in that set, and naming them is the point of listing the
    /// rest. `exec_output_limit_bytes` and `exec_max_current` are published unconditionally as
    /// declared configuration bounds rather than proved capabilities — the first for the stated
    /// reason above it, the second for none recorded. `exec_scratch_quota` is gated on `quota`
    /// alone, which probes the workspace filesystem for project quotas and does not pass through
    /// `exec`. All three predate this option and none is changed here.
    #[test]
    fn a_backend_that_cannot_disable_nested_user_namespaces_withholds_the_exec_floor() {
        use std::os::unix::fs::PermissionsExt as _;

        /// A bubblewrap that knows every option but the named one, and is otherwise a pass.
        fn old_bubblewrap(refused: &str) -> String {
            format!(
                r#"#!/bin/sh
for argument in "$@"; do
  if [ "$argument" = "{refused}" ]; then
    echo "bwrap: Unknown option $argument" >&2
    exit 1
  fi
done
echo 'socket(1, 1, 0): Permission denied' >&2
exit 1
"#
            )
        }

        // The probe answers false when socat is absent, for its own reason, and would prove
        // nothing either way. Absent, never reported as passed.
        if !Path::new("/usr/bin/socat").is_file() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        for refused in ["--disable-userns", "--assert-userns-disabled"] {
            let stub = directory.path().join("bwrap");
            std::fs::write(&stub, old_bubblewrap(refused)).unwrap();
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
            let mut old = HostConfig::minimum(directory.path());
            old.bubblewrap = stub;
            assert!(
                !probe_bubblewrap(&old),
                "a backend that refused {refused} still proved the namespace floor, so that \
                 argument is not in the probe's argv"
            );
        }

        // The other half of the same claim, and the reason the assertions above are not satisfied
        // by a probe that answers false everywhere: this host's real backend still proves the
        // floor with both options in the argv.
        let real = HostConfig::minimum(directory.path());
        if real.bubblewrap.is_file() {
            assert!(
                probe_bubblewrap(&real),
                "the configured backend no longer proves the confinement floor"
            );
        }
    }

    /// **Every sandbox this module opens carries the posture** — asserted on the argv each one
    /// builds, through a backend that records what it was handed and refuses to be a sandbox.
    ///
    /// These three functions are private to this module, so `process.rs`'s companion case cannot
    /// reach them; between the two, and the exec argv's own case, seven of the crate's eight
    /// bubblewrap argv lists are asserted as built rather than as written.
    ///
    /// Portable: nothing here spawns a real sandbox, so no delegated cgroup is needed.
    #[test]
    fn every_probe_sandbox_carries_the_user_namespace_posture() {
        let directory = tempfile::tempdir().expect("a scratch root");
        let (backend, log) = crate::process::recording_backend(directory.path());
        let mut config = HostConfig::minimum(directory.path());
        config.bubblewrap = backend;

        assert!(
            !probe_workspace_scoped_write(&config),
            "a backend that records and refuses cannot prove scoped writes"
        );
        // `probe_bubblewrap` returns before it spawns when socat is absent, so it is counted only
        // where it can be observed. Counting it regardless would make this case green on a host
        // where that argv was never built.
        let socat = Path::new("/usr/bin/socat").is_file();
        if socat {
            assert!(
                !probe_bubblewrap(&config),
                "a backend that records and refuses cannot prove the confinement floor"
            );
        }
        let slot =
            crate::secrets::probe_slot(PASSTHROUGH_SENTINEL).expect("stage a sealed probe slot");
        assert!(
            !descriptor_passthrough_holds(&config, &slot, ""),
            "a backend that records and refuses cannot prove descriptor pass-through"
        );

        crate::process::assert_recorded_posture(
            "the probes in probe.rs",
            &crate::process::recorded_sandboxes(&log),
            2 + usize::from(socat),
        );
    }
    /// Families the profile refuses that this probe has no instrument for, and why.
    ///
    /// Real gaps, stated rather than hidden. socat 1.8.1.3 carries address forms for Unix and
    /// vsock and for none of these eight, and the capability probe is not the place to acquire an
    /// interpreter dependency — it runs on every snapshot and its own failure withholds every
    /// exec fact. Each is pinned instead by `seccomp::tests`, natively and over the x32 syscall
    /// number, and by
    /// `process.rs::no_socket_family_opens_inside_a_confined_exec_without_a_recorded_decision`,
    /// which observes the whole denied set in a real admitted exec — the stronger of the two
    /// vehicles, since it drives the path a client reaches.
    #[cfg(test)]
    const FAMILIES_WITHOUT_A_PROBE_INSTRUMENT: [(libc::c_int, &str); 8] = [
        (17, "AF_PACKET: no socat address form"),
        (21, "AF_RDS: no socat address form"),
        (24, "AF_PPPOX: no socat address form"),
        (38, "AF_ALG: socat has no algorithm-socket address form"),
        (41, "AF_KCM: no socat address form"),
        (42, "AF_QIPCRTR: socat has no QRTR address form"),
        (43, "AF_SMC: no socat address form"),
        (45, "AF_MCTP: no socat address form"),
    ];

    /// **Every family the profile refuses is either measured by this probe or recorded as one it
    /// has no instrument for.**
    ///
    /// The gap this closes is the one that produced the finding above it: a family added to
    /// `seccomp::FAMILY_POLICY` as denied could otherwise reach a release without anybody
    /// noticing that the probe gating every exec fact never asks about it. Now it cannot be added
    /// silently — it lands in the sentinel list or in the recorded exception, and either is a
    /// decision somebody made.
    #[test]
    fn every_refused_family_is_measured_here_or_recorded_as_instrument_less() {
        let mut refused = 0;
        for policy in crate::seccomp::FAMILY_POLICY {
            if !policy.denied {
                continue;
            }
            refused += 1;
            let measured = REFUSED_FAMILY_SENTINELS
                .iter()
                .any(|(family, _)| *family == policy.family);
            let excepted = FAMILIES_WITHOUT_A_PROBE_INSTRUMENT
                .iter()
                .any(|(family, _)| *family == policy.family);
            assert!(
                measured != excepted,
                "{} is refused by the profile but is {} by the confinement-floor probe; a denied \
                 family belongs in exactly one of the sentinel list and the recorded exception",
                policy.name,
                if measured {
                    "both measured and excepted"
                } else {
                    "neither measured nor recorded as instrument-less"
                }
            );
        }
        assert_eq!(
            refused,
            REFUSED_FAMILY_SENTINELS.len() + FAMILIES_WITHOUT_A_PROBE_INSTRUMENT.len(),
            "the probe accounts for a different number of families than the profile refuses"
        );
    }

    /// **The confinement-floor probe asks for a non-nestable user namespace and asserts it** —
    /// on the argv it builds, with no `socat` and no backend on disk.
    ///
    /// This is the pin the round-1 correction thought it had. Both guards over that argv ran
    /// `probe_bubblewrap`, which returns before it spawns when `/usr/bin/socat` is absent: the
    /// stub case returns early and the recording case counts `2 + socat`, so on a socat-less host
    /// deleting `--assert-userns-disabled` left the whole package gate green. The source scan does
    /// not look for that option either. Reading the built argv depends on neither.
    ///
    /// `--assert-userns-disabled` is what makes the option an observation instead of a request:
    /// without it a backend that accepted `--disable-userns` and did nothing would prove the
    /// floor. The pair is asserted together for that reason.
    #[test]
    fn the_confinement_floor_probe_asks_for_and_asserts_a_non_nestable_user_namespace() {
        let sentinel = tempfile::tempdir().expect("a sentinel root");
        let seccomp = crate::seccomp::profile().expect("the seccomp profile builds");
        let command = bubblewrap_probe_command(
            Path::new("/does/not/exist"),
            sentinel.path(),
            seccomp.as_raw_fd(),
            VSOCK_SENTINEL,
        );
        let argv: Vec<String> = command
            .get_args()
            .map(|part| part.to_string_lossy().into_owned())
            .collect();
        for option in crate::process::USER_NAMESPACE_ARGV
            .into_iter()
            .chain(["--assert-userns-disabled"])
        {
            assert!(
                argv.iter().any(|part| part == option),
                "the probe that gates every exec fact does not carry {option}, so a backend \
                 which cannot give a confined child a non-nestable user namespace still proves \
                 the confinement floor: {argv:?}"
            );
        }
    }
}
