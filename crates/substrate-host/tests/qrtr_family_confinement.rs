//! Adversary (wave B u3): a second unconfined socket family reaches out of the sandbox.
//!
//! `story:seccomp-denies-af-vsock` shipped a `FAMILY_POLICY` that denies `AF_UNIX` and `AF_VSOCK`
//! and records `AF_NETLINK`/`AF_PACKET` as allowed *because `--unshare-net` already confines
//! them*. The unit states its limit plainly — the table covers four families and names `AF_ALG`
//! as "the next it would examine". This case measures a family it does **not** name: `AF_QIPCRTR`
//! (the Qualcomm IPC router, family 42).
//!
//! Measured at the bubblewrap layer while writing this case (see the adversary report), on this
//! host with the `qrtr` module loaded: two mutually-isolated sandboxes, each with its own fresh
//! network namespace (`net:[4026534396]` and `net:[4026534475]`), exchanged a datagram over
//! `AF_QIPCRTR`; and a confined sandbox exchanged datagrams bidirectionally with a process in the
//! host network namespace. Over the same boundary, `AF_INET` to a host loopback service was
//! refused — so the network namespace confines `AF_INET` and does **not** confine `AF_QIPCRTR`.
//! By the unit's own stated principle (deny a family the netns does not confine) and by invariant
//! 3 (a missing isolation guarantee is a *named refusal*, never silent degradation), a correct
//! confinement profile denies `socket(AF_QIPCRTR, …)` with `EACCES`, exactly as it denies
//! `AF_VSOCK`.
//!
//! This case drives a real admitted exec — the same vehicle the shipped vsock case uses — and
//! asserts the profile refuses the family. It fails today because the profile permits it.
//!
//! Delegated lane only, and gated on the host actually offering the family: without
//! `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this process is inside, and without
//! the `qrtr` module loaded so the family is `EACCES`-vs-open rather than `EAFNOSUPPORT`, the case
//! is **absent, never reported as passed** (invariant 3).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use substrate_host::{DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode, SandboxProfile,
    WorkspaceAccess,
};

const AF_QIPCRTR: libc::c_int = 42;

/// The delegated cgroup v2 subtree this process is inside, or nothing.
fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

/// Whether this host's kernel offers `AF_QIPCRTR` at all. When it does not, a *correct* profile
/// would answer `EAFNOSUPPORT` rather than `EACCES`, so the case cannot distinguish the fix from
/// an absent module and must be absent instead of red for the wrong reason.
fn host_offers_qrtr() -> bool {
    // SAFETY: socket takes three integers; -1 or an owned descriptor closed immediately below.
    let fd = unsafe { libc::socket(AF_QIPCRTR, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd >= 0 {
        // SAFETY: fd was returned by the successful call above.
        unsafe { libc::close(fd) };
        return true;
    }
    false
}

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    _directory: tempfile::TempDir,
}

fn lane() -> Option<Lane> {
    let delegated = delegated_cgroup_root()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() || !PathBuf::from("/usr/bin/python3").is_file() {
        return None;
    }
    if !host_offers_qrtr() {
        return None;
    }
    let directory = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.cgroup_root = Some(delegated);
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    if machine.facts.exec_argv_only != Some(true) {
        // The machine cannot confine an exec at all; nothing here is this profile's to prove.
        return None;
    }
    std::fs::create_dir_all(driver.root().join("ws_test")).expect("workspace directory");
    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        _directory: directory,
    })
}

fn exec_input(snapshot: &str, argv: &[&str]) -> ExecStartInput {
    ExecStartInput {
        read_only_roots: Vec::new(),
        secret_slots: Vec::new(),
        workspace: "ws_test".to_owned(),
        argv: argv.iter().map(|part| (*part).to_owned()).collect(),
        env: ExecEnvironment {
            allow: vec![],
            set: BTreeMap::new(),
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

async fn waited(lane: &Lane, id: &str, argv: &[&str]) -> ExecObservation {
    let input = exec_input(&lane.snapshot, argv);
    match lane.driver.start_exec(id, "ws_test", &input).await {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "the delegated lane must dispatch {id}: {} {}",
            error.code, error.message
        ),
    }
}

/// `AF_QIPCRTR` is a live cross-namespace channel out of the sandbox, so the confinement profile
/// must refuse `socket(AF_QIPCRTR, …)` with `EACCES` just as it refuses `AF_VSOCK`.
///
/// Two execs, for the reason the shipped vsock case gives: "the command failed" is also what an
/// absent interpreter and a broken harness look like. The first proves `/usr/bin/python3` runs in
/// this sandbox; the second differs only in what it asks the kernel for, and must be refused in
/// `socket(2)` itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_confined_process_cannot_open_an_af_qipcrtr_socket() {
    let Some(lane) = lane() else {
        return;
    };

    let present = waited(
        &lane,
        "ex_qrtr_present",
        &["/usr/bin/python3", "-c", "print('py-ok')"],
    )
    .await;
    assert_eq!(
        present
            .resource
            .exit
            .expect("a waited exec reports its exit")
            .code,
        Some(0),
        "the case needs /usr/bin/python3 runnable inside the sandbox: {}",
        String::from_utf8_lossy(&present.stderr)
    );

    // Prints exactly one machine-readable line: `QRTR-OPENED <fd>` if the kernel handed the
    // confined process a descriptor, or `QRTR-ERRNO <n>` if it refused. Indentation-free so the
    // `-c` body cannot be broken by whitespace surviving or not surviving a Rust line escape.
    let probe = "import socket\n\
try: s=socket.socket(42,socket.SOCK_DGRAM,0); print('QRTR-OPENED',s.fileno()); s.close()\n\
except OSError as e: print('QRTR-ERRNO',e.errno)\n";
    let qrtr = waited(&lane, "ex_qrtr_denied", &["/usr/bin/python3", "-c", probe]).await;
    let stdout = String::from_utf8_lossy(&qrtr.stdout).into_owned();
    assert!(
        !stdout.contains("QRTR-OPENED"),
        "a confined process opened an AF_QIPCRTR socket; no network namespace confines qrtr, so \
         on a host with the qrtr transport that socket reaches other domains (host and sibling \
         sandboxes, measured at the bwrap layer): {stdout:?}"
    );
    assert!(
        stdout.contains(&format!("QRTR-ERRNO {}", libc::EACCES)),
        "the confinement profile must refuse AF_QIPCRTR with EACCES, the named refusal invariant 3 \
         requires, rather than any other errno: {stdout:?}"
    );
}
