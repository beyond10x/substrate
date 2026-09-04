//! Adversary (wave B u3, pass 2): `AF_ALG` is recorded allowed on a claim measurement refutes.
//!
//! `seccomp::FAMILY_POLICY` records `AF_ALG` (family 38) **allowed**, with this reason:
//!
//! > reachable and namespace-free … but it is not a channel to another domain: nothing sent
//! > through it reaches any other process, and two sandboxes each got their own independent
//! > transform. It is kernel attack surface rather than an escape …
//!
//! That claim is false on the axis that matters to a confinement floor. A `bind(2)` on an
//! `AF_ALG` socket names an algorithm, and the kernel's crypto API `request_module()`s the
//! backing implementation **into the host's single, global module table** — with the kernel's own
//! privilege, not the caller's, so `--disable-userns` and the empty user namespace do not stop
//! it. The confined process therefore:
//!
//!   * **influences** state outside its own sandbox — the loaded module persists on the host after
//!     the sandbox exits, and the *choice of which* module (which kernel code, of the ~80 crypto
//!     modules on disk here) is attacker-controlled, so this is host kernel attack-surface
//!     expansion driven from inside the sandbox; and
//!   * is **observable** across the boundary — a mutually-isolated sibling sandbox reads the same
//!     host-global `/proc/modules` (procfs `modules` is not namespaced) and sees exactly what a
//!     prior `AF_ALG` user loaded. That is a cross-sandbox covert channel, the precise thing the
//!     "two sandboxes each got their own independent transform" clause denies exists.
//!
//! Measured at the bwrap layer while writing this case (see the adversary report), on this host
//! with the crate's own confinement argv and its own seccomp profile: a confined process bound
//! `AF_ALG` `hash`/`skcipher` algorithms whose modules were **absent** from the host beforehand
//! (`md4`, `rmd160`, `wp512`, `cast5_generic`, `serpent_generic`, `blowfish_generic`, …) and each
//! one appeared in the host's `/proc/modules` afterwards and stayed. A second sandbox, freshly
//! isolated, then observed those modules in its own `/proc/modules`.
//!
//! By invariant 3 (a missing isolation guarantee is a *named refusal*, never silent degradation)
//! and by the unit's own stated principle — deny a family the sandbox does not confine — a correct
//! profile refuses `socket(AF_ALG, …)` with `EACCES`, exactly as it refuses `AF_VSOCK` and
//! `AF_QIPCRTR`, which closes this influence/observation channel. This case fails today because
//! the profile permits `AF_ALG`, so a confined exec loads host-global kernel modules of its
//! choosing.
//!
//! Delegated lane only, and gated on the host actually offering `AF_ALG` (the `algif_hash`
//! interface present) and on at least one candidate algorithm's module being absent so the load
//! is observable rather than a no-op. Without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2
//! subtree this process is inside, the case is **absent, never reported as passed** (invariant 3).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use substrate_host::{DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode, SandboxProfile,
    WorkspaceAccess,
};

const AF_ALG: libc::c_int = 38;

/// The delegated cgroup v2 subtree this process is inside, or nothing.
fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

/// Whether this host's kernel offers `AF_ALG` at all. When it does not — no `algif_*` interface —
/// a *correct* profile would answer `EAFNOSUPPORT` rather than open, and the module-autoload
/// mechanism this case measures is unreachable, so the case is absent rather than red for the
/// wrong reason.
fn host_offers_alg() -> bool {
    // SAFETY: socket takes three integers; -1 or an owned descriptor closed immediately below.
    let fd = unsafe { libc::socket(AF_ALG, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd >= 0 {
        // SAFETY: fd was returned by the successful call above.
        unsafe { libc::close(fd) };
        return true;
    }
    false
}

/// The host's currently-loaded kernel module names, read from the global (non-namespaced)
/// `/proc/modules`. The set the confined exec must not be able to grow.
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
    if !PathBuf::from("/usr/bin/bwrap").is_file() || !PathBuf::from("/usr/bin/python3").is_file() {
        return None;
    }
    if !host_offers_alg() {
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

/// A confined `AF_ALG` `bind(2)` must not load a host-global kernel module of the confined
/// process's choosing.
///
/// Two execs, for the reason the shipped vsock and qrtr cases give: "the command failed" is also
/// what an absent interpreter and a broken harness look like. The first proves `/usr/bin/python3`
/// runs in this sandbox; the second is the measurement.
#[tokio::test(flavor = "multi_thread")]
async fn a_confined_process_cannot_autoload_a_host_kernel_module_via_af_alg() {
    let Some(lane) = lane() else {
        return;
    };

    let present = waited(
        &lane,
        "ex_alg_present",
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

    let before = host_modules();

    // The confined child binds a broad spread of `AF_ALG` `hash` and `skcipher` algorithms whose
    // backing modules are separate `.ko` files (not built-in), so at least one is absent on any
    // ordinary host and its bind triggers a host-global `request_module`. Each `bind` opens a
    // fresh socket; failures (an algorithm this kernel lacks) are ignored — the point is that
    // *any* of them reaches the host module table. Indentation-free `-c` body so no whitespace
    // rides a Rust line escape.
    // `concat!` with explicit `\n` and one-space indentation, never a `\<newline>` continuation:
    // a Rust line-continuation strips the leading whitespace of the next line, and this body needs
    // its indentation to survive the crossing into argv (the shipped survey case documents the
    // same hazard).
    let probe = concat!(
        "import socket\n",
        "hashes=('md4','rmd160','wp512','tgr192','michael_mic','sm3','streebog256',",
        "'blake2b-256','xxhash64','xcbc(aes)','cmac(aes)','nhpoly1305')\n",
        "skciphers=('ecb(cast5)','cbc(serpent)','ecb(blowfish)','cbc(camellia)','ecb(cast6)',",
        "'cbc(twofish)','ecb(des)','cbc(sm4)','ecb(aria)','ecb(tea)','ecb(khazad)',",
        "'ecb(anubis)','ecb(seed)','cts(cbc(aes))','lrw(aes)','ofb(aes)','cfb(aes)','xts(aes)',",
        "'adiantum(xchacha12,aes)','essiv(cbc(aes),sha256)','xctr(aes)')\n",
        "aeads=('gcm(aes)','ccm(aes)','rfc4106(gcm(aes))','authenc(hmac(sha256),cbc(aes))')\n",
        "def bind(t,n):\n",
        " try:\n",
        "  s=socket.socket(socket.AF_ALG,socket.SOCK_SEQPACKET,0)\n",
        "  s.bind((t,n)); s.close(); return n\n",
        " except OSError: return None\n",
        "bound=[n for n in hashes if bind('hash',n)]",
        "+[n for n in skciphers if bind('skcipher',n)]",
        "+[n for n in aeads if bind('aead',n)]\n",
        "print('ALG-BOUND',' '.join(bound))\n",
    );
    let bound = waited(&lane, "ex_alg_bind", &["/usr/bin/python3", "-c", probe]).await;
    let stdout = String::from_utf8_lossy(&bound.stdout).into_owned();
    // The child must actually have bound at least one algorithm, or the measurement did not run
    // and a clean module diff would be a false pass, not a confinement.
    let bound_line = stdout
        .lines()
        .find(|line| line.starts_with("ALG-BOUND "))
        .map_or("", |line| line.trim_start_matches("ALG-BOUND ").trim());
    if bound_line.is_empty() {
        // No algorithm was bindable inside the sandbox at all — this host cannot exercise the
        // mechanism, so the case is absent rather than a false green (invariant 3).
        return;
    }

    let after = host_modules();
    let newly_loaded: Vec<&String> = after.difference(&before).collect();

    assert!(
        newly_loaded.is_empty(),
        "a confined exec loaded host-global kernel modules {newly_loaded:?} of its own choosing by \
         binding AF_ALG algorithms {bound_line:?}; the modules persist on the host after the \
         sandbox exits and every sibling sandbox observes them in the shared /proc/modules. \
         seccomp::FAMILY_POLICY records AF_ALG allowed because 'nothing sent through it reaches \
         any other process' and 'two sandboxes each got their own independent transform', but this \
         is a cross-boundary influence and observation channel. Invariant 3 and the unit's own \
         denial principle require a named refusal of the family, as for AF_VSOCK and AF_QIPCRTR."
    );
}
