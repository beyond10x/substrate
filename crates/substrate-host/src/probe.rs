use std::os::fd::AsRawFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::process::CommandExt as _;
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
        exec_egress_apertures: egress_apertures,
        secrets_slots,
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
}
