//! ADR 0039: the three module-autoload triggers the socket and ioctl allowlist is proven against.
//!
//! Each case runs one trigger inside a real admitted exec and asserts two things. First, the call
//! is refused with `EACCES`, the profile's named refusal. Second, the host's `/proc/modules`, read
//! on the host before and after the exec, gained no module. The refusal is the standing
//! assertion: a module diff alone turns green once the module is resident, because the kernel
//! loads it at most once per boot.
//!
//! The probe is Rust: this test executable, copied into the workspace and run inside the sandbox
//! with `probe_entry` selected and `SUBSTRATE_KMOD_PROBE` naming the trigger. Outside a sandbox
//! that variable is unset and `probe_entry` does nothing. In the same run the probe also makes one
//! allowed call (a TCP socket, a `NETLINK_ROUTE` socket, opening the pty), so an exec that ran
//! nothing cannot pass.
//!
//! Delegated lane only. Without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, every case is **absent, never reported as passed** (invariant 3).

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;

use substrate_host::{DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode, SandboxProfile,
    WorkspaceAccess,
};

/// The variable that switches `probe_entry` from a no-op into the in-sandbox probe.
const PROBE_VARIABLE: &str = "SUBSTRATE_KMOD_PROBE";
/// The name the test executable is copied to inside the workspace.
const PROBE_FILE: &str = "kmod-probe";
const IPPROTO_SCTP: libc::c_int = 132;
/// `NETLINK_ISCSI`: a netlink protocol other than `NETLINK_ROUTE`, served by a module.
const NETLINK_ISCSI: libc::c_int = 8;
/// `N_HDLC`: a line discipline served by a module.
const N_HDLC: libc::c_int = 13;

/// One outcome line, `<label> OK` or `<label> ERRNO <n>`.
fn outcome(label: &str, result: libc::c_int) -> String {
    if result >= 0 {
        format!("{label} OK")
    } else {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
        format!("{label} ERRNO {errno}")
    }
}

/// Opens a socket and closes it again, returning the raw result for `outcome`.
fn try_socket(family: libc::c_int, socket_type: libc::c_int, protocol: libc::c_int) -> String {
    // SAFETY: socket takes three integers and returns an owned descriptor or -1.
    let fd = unsafe { libc::socket(family, socket_type | libc::SOCK_CLOEXEC, protocol) };
    let line = outcome("", fd);
    if fd >= 0 {
        // SAFETY: fd was returned by the successful call above.
        unsafe { libc::close(fd) };
    }
    line.trim_start().to_owned()
}

/// The in-sandbox half. A no-op wherever `SUBSTRATE_KMOD_PROBE` is unset, which is everywhere
/// except inside the execs the cases below start.
#[test]
fn probe_entry() {
    let Ok(trigger) = std::env::var(PROBE_VARIABLE) else {
        return;
    };
    let (allowed, refused) = match trigger.as_str() {
        "sctp" => (
            try_socket(libc::AF_INET, libc::SOCK_STREAM, libc::IPPROTO_TCP),
            try_socket(libc::AF_INET, libc::SOCK_STREAM, IPPROTO_SCTP),
        ),
        "netlink" => (
            try_socket(libc::AF_NETLINK, libc::SOCK_RAW, libc::NETLINK_ROUTE),
            try_socket(libc::AF_NETLINK, libc::SOCK_RAW, NETLINK_ISCSI),
        ),
        "tiocsetd" => {
            // SAFETY: posix_openpt takes flags and returns an owned descriptor or -1.
            let pty =
                unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
            let allowed = outcome("", pty).trim_start().to_owned();
            let refused = if pty >= 0 {
                let discipline: libc::c_int = N_HDLC;
                // SAFETY: TIOCSETD reads one int through the pointer, which outlives the call.
                let result = unsafe { libc::ioctl(pty, libc::TIOCSETD, &raw const discipline) };
                let line = outcome("", result).trim_start().to_owned();
                // SAFETY: pty was returned by the successful posix_openpt above.
                unsafe { libc::close(pty) };
                line
            } else {
                "NOT-RUN".to_owned()
            };
            (allowed, refused)
        }
        other => panic!("unknown probe trigger {other}"),
    };
    println!("KMOD-ALLOWED {allowed}");
    println!("KMOD-TRIGGER {refused}");
}

/// The delegated cgroup v2 subtree this process is inside, or nothing.
fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

/// The host's loaded kernel module names, from the global, un-namespaced `/proc/modules`. Read on
/// the host side: the claim is about what the sandbox left behind.
fn host_modules() -> BTreeSet<String> {
    std::fs::read_to_string("/proc/modules")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    _directory: tempfile::TempDir,
}

fn lane() -> Option<Lane> {
    let delegated = delegated_cgroup_root()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        return None;
    }
    let directory = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.cgroup_root = Some(delegated);
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    if machine.facts.exec_argv_only != Some(true) {
        eprintln!("kernel_module_triggers: absent, the machine does not confine an exec");
        // The machine cannot confine an exec at all; nothing here is this profile's to prove.
        return None;
    }
    let workspace = driver.root().join("ws_test");
    std::fs::create_dir_all(&workspace).expect("workspace directory");
    let probe = workspace.join(PROBE_FILE);
    std::fs::copy(std::env::current_exe().expect("test executable"), &probe)
        .expect("copy the probe into the workspace");
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755))
        .expect("probe is executable");
    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        _directory: directory,
    })
}

fn exec_input(snapshot: &str, trigger: &str) -> ExecStartInput {
    ExecStartInput {
        writable_roots: Vec::new(),
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workspace: "ws_test".to_owned(),
        argv: [
            &format!("/workspace/{PROBE_FILE}"),
            "probe_entry",
            "--exact",
            "--nocapture",
            "--test-threads=1",
            "--quiet",
        ]
        .iter()
        .map(|part| (*part).to_owned())
        .collect(),
        env: ExecEnvironment {
            allow: vec![],
            set: BTreeMap::from([(PROBE_VARIABLE.to_owned(), trigger.to_owned())]),
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
            memory_bytes: 268_435_456,
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

/// Runs one trigger inside an admitted exec and asserts the refusal and the unchanged module set.
async fn assert_trigger_refused(trigger: &str, description: &str) {
    let Some(lane) = lane() else {
        return;
    };
    let before = host_modules();
    let input = exec_input(&lane.snapshot, trigger);
    let id = format!("ex_kmod_{trigger}");
    let observed: ExecObservation = match lane.driver.start_exec(&id, "ws_test", &input).await {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "the delegated lane must dispatch {id}: {} {}",
            error.code, error.message
        ),
    };
    let after = host_modules();
    let stdout = String::from_utf8_lossy(&observed.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&observed.stderr).into_owned();

    assert!(
        stdout.lines().any(|line| line == "KMOD-ALLOWED OK"),
        "the probe's allowed call did not succeed inside the sandbox, so the {trigger} probe did \
         not run as intended: stdout {stdout:?} stderr {stderr:?}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line == format!("KMOD-TRIGGER ERRNO {}", libc::EACCES)),
        "{description} must be refused with EACCES inside a confined exec (ADR 0039): stdout \
         {stdout:?} stderr {stderr:?}"
    );
    let appeared: Vec<&String> = after.difference(&before).collect();
    assert!(
        appeared.is_empty(),
        "the {trigger} exec left new host-global kernel modules {appeared:?} behind"
    );
    eprintln!(
        "kernel_module_triggers: {trigger} ran inside an admitted exec: {}; no module appeared",
        stdout.trim().replace('\n', ", ")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_sctp_socket_is_refused_and_loads_no_module() {
    assert_trigger_refused("sctp", "socket(AF_INET, SOCK_STREAM, IPPROTO_SCTP)").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_non_route_netlink_socket_is_refused_and_loads_no_module() {
    assert_trigger_refused("netlink", "socket(AF_NETLINK, SOCK_RAW, NETLINK_ISCSI)").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_line_discipline_change_is_refused_and_loads_no_module() {
    assert_trigger_refused("tiocsetd", "ioctl(TIOCSETD, N_HDLC) on a pty").await;
}
