//! Security review pass 1 of ADR 0039: conformance checks of the confinement seccomp program
//! against the contract the ADR states, driven inside a real admitted exec.
//!
//! The probe is this test executable, copied into the workspace and run inside the sandbox with
//! `review_probe_entry` selected and `SUBSTRATE_REVIEW_PROBE` naming the check. Each probe also
//! makes one call the profile is known to answer (a native refusal or an allowed call), so an
//! exec that ran nothing cannot pass.
//!
//! Delegated lane only. Without `SUBSTRATE_VECTORS_CGROUP_ROOT` every case is absent.

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

const PROBE_VARIABLE: &str = "SUBSTRATE_REVIEW_PROBE";
const PROBE_FILE: &str = "review-probe";

fn errno_line(result: libc::c_long) -> String {
    if result >= 0 {
        "OK".to_owned()
    } else {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
        format!("ERRNO {errno}")
    }
}

#[test]
fn review_probe_entry() {
    let Ok(check) = std::env::var(PROBE_VARIABLE) else {
        return;
    };
    let (control, subject) = match check.as_str() {
        // ADR 0039 decision 3 and 4: TIOCSETD is refused EACCES, and the syscall number is
        // normalised for x32. On x86_64 the x32 ABI's ioctl is not 16 | X32 bit but its own
        // compat entry, 514 | X32 bit. Issued on descriptor -1 so no terminal is touched: a
        // refusal answers EACCES, a call the profile admits answers EBADF from the kernel.
        #[cfg(target_arch = "x86_64")]
        "x32-tiocsetd" => {
            let discipline: libc::c_int = 0;
            // SAFETY: ioctl on descriptor -1 with a pointer that outlives the call.
            let native = unsafe {
                libc::syscall(libc::SYS_ioctl, -1, libc::TIOCSETD, &raw const discipline)
            };
            let control = errno_line(native);
            // SAFETY: the x32 compat ioctl number with the same arguments.
            let x32 = unsafe {
                libc::syscall(
                    0x4000_0000 | 0x202,
                    -1,
                    libc::TIOCSETD,
                    &raw const discipline,
                )
            };
            (control, errno_line(x32))
        }
        // ADR 0039 decision 2: socketpair(2) stays AF_UNIX with a non-datagram type only. The
        // kernel's unix_create turns SOCK_RAW into SOCK_DGRAM, so SOCK_RAW is a datagram pair.
        "unix-raw-socketpair" => {
            let mut sockets = [-1; 2];
            // SAFETY: `sockets` has room for exactly two returned descriptors.
            let dgram = unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
                    0,
                    sockets.as_mut_ptr(),
                )
            };
            let control = errno_line(libc::c_long::from(dgram));
            // SAFETY: as above.
            let raw = unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                    0,
                    sockets.as_mut_ptr(),
                )
            };
            let mut subject = errno_line(libc::c_long::from(raw));
            if raw == 0 {
                let mut kind: libc::c_int = -1;
                let mut length =
                    libc::socklen_t::try_from(std::mem::size_of::<libc::c_int>()).expect("socklen");
                // SAFETY: SO_TYPE writes one int into `kind`.
                unsafe {
                    libc::getsockopt(
                        sockets[0],
                        libc::SOL_SOCKET,
                        libc::SO_TYPE,
                        (&raw mut kind).cast(),
                        &raw mut length,
                    );
                    libc::close(sockets[0]);
                    libc::close(sockets[1]);
                }
                subject = format!("{subject} SO_TYPE {kind}");
            }
            (control, subject)
        }
        other => panic!("unknown review probe {other}"),
    };
    println!("REVIEW-CONTROL {control}");
    println!("REVIEW-SUBJECT {subject}");
}

fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
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
        return None;
    }
    let workspace = driver.root().join("ws_review");
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

fn exec_input(snapshot: &str, check: &str) -> ExecStartInput {
    ExecStartInput {
        writable_roots: Vec::new(),
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workspace: "ws_review".to_owned(),
        argv: [
            &format!("/workspace/{PROBE_FILE}"),
            "review_probe_entry",
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
            set: BTreeMap::from([(PROBE_VARIABLE.to_owned(), check.to_owned())]),
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

async fn assert_refused(check: &str, description: &str) {
    let Some(lane) = lane() else {
        eprintln!("seccomp_review_pass1: {check} absent, no delegated lane");
        return;
    };
    let id = format!("ex_review_{}", check.replace('-', "_"));
    let observed: ExecObservation = match lane
        .driver
        .start_exec(&id, "ws_review", &input_for(&lane, check))
        .await
    {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "the delegated lane must dispatch {id}: {} {}",
            error.code, error.message
        ),
    };
    let stdout = String::from_utf8_lossy(&observed.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&observed.stderr).into_owned();
    assert!(
        stdout
            .lines()
            .any(|line| line == format!("REVIEW-CONTROL ERRNO {}", libc::EACCES)),
        "the control call was not refused EACCES, so the {check} probe did not run under the \
         profile: stdout {stdout:?} stderr {stderr:?}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line == format!("REVIEW-SUBJECT ERRNO {}", libc::EACCES)),
        "{description} must be refused with EACCES inside a confined exec (ADR 0039): stdout \
         {stdout:?} stderr {stderr:?}"
    );
}

fn input_for(lane: &Lane, check: &str) -> ExecStartInput {
    exec_input(&lane.snapshot, check)
}

#[cfg(target_arch = "x86_64")]
#[tokio::test(flavor = "multi_thread")]
async fn review_x32_ioctl_cannot_bypass_the_tiocsetd_refusal() {
    assert_refused(
        "x32-tiocsetd",
        "ioctl(TIOCSETD) through the x32 compat number 514",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn review_unix_sock_raw_socketpair_is_refused_as_a_datagram_pair() {
    assert_refused(
        "unix-raw-socketpair",
        "socketpair(AF_UNIX, SOCK_RAW), which the kernel creates as SOCK_DGRAM",
    )
    .await;
}
