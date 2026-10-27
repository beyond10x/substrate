//! Security review of `story:egress-third-install-case-holds-under-load`: an aperture's listening
//! socket is now held by the installing process for the life of the run, so that its teardown can
//! shut the socket down. Holding it longer is safe only because of one fact, which these cases
//! measure: the descriptor is close-on-exec, so no program the host starts in the meantime receives
//! it, and it is released when the run ends.
//!
//! Delegated lane only: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, or without bubblewrap, the case returns early and is **absent, never
//! passed**. `bash scripts/delegated-lane.sh` runs it.

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use substrate_host::{DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, ExecState, NetworkMode,
    SandboxProfile, WorkspaceAccess,
};

const WORKSPACE: &str = "ws_test";

fn delegated_cgroup_root() -> Option<PathBuf> {
    let Some(root) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT").map(PathBuf::from) else {
        println!("egress listener review: ABSENT (SUBSTRATE_VECTORS_CGROUP_ROOT is unset)");
        return None;
    };
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    _workspaces: tempfile::TempDir,
}

/// A delegated host with one declared aperture, `model`, pinned to `pinned`.
fn lane(pinned: SocketAddr) -> Option<Lane> {
    let delegated = delegated_cgroup_root()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        println!("egress listener review: ABSENT (no bubblewrap)");
        return None;
    }
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(workspaces.path().join("workspaces"));
    config.cgroup_root = Some(delegated);
    config.egress_apertures = vec![substrate_host::EgressAperture {
        name: "model".to_owned(),
        host: "app.example.invalid".to_owned(),
        port: pinned.port(),
        pinned,
        max_bytes: None,
    }];
    config.aperture_root = workspaces.path().join("apertures");
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    if machine.facts.exec_argv_only != Some(true) || machine.facts.exec_egress_apertures.is_none() {
        println!(
            "egress listener review: ABSENT (the floor or the aperture fact is not proven here)"
        );
        return None;
    }
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace");
    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        _workspaces: workspaces,
    })
}

fn exec_input(snapshot: &str, argv: &[&str], aperture: bool, wait: bool) -> ExecStartInput {
    ExecStartInput {
        writable_roots: Vec::new(),
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workspace: WORKSPACE.to_owned(),
        argv: argv.iter().map(|part| (*part).to_owned()).collect(),
        env: ExecEnvironment {
            allow: vec![],
            set: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        },
        sandbox: ConfinementRequest {
            capability_snapshot: snapshot.to_owned(),
            network: if aperture {
                NetworkMode::Aperture
            } else {
                NetworkMode::None
            },
            aperture: aperture.then(|| "model".to_owned()),
            apertures: Vec::new(),
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
        wait,
        workspace_access: WorkspaceAccess::ReadWrite,
        scratch: None,
        measurements: BTreeSet::new(),
        capsule: None,
        lease_ttl_ms: Some(60_000),
        workdir: None,
    }
}

async fn started(lane: &Lane, id: &str, input: &ExecStartInput) -> ExecObservation {
    match lane.driver.start_exec(id, WORKSPACE, input).await {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("{id} must dispatch: {} {}", error.code, error.message)
        }
    }
}

/// A listening socket this process holds: its descriptor number and whether it is close-on-exec.
#[derive(Debug, PartialEq, Eq)]
struct Held {
    fd: i32,
    cloexec: bool,
}

/// Every listening socket in this process's descriptor table.
fn listening_sockets() -> Vec<Held> {
    let mut held = Vec::new();
    for entry in std::fs::read_dir("/proc/self/fd")
        .expect("this process's fd table")
        .flatten()
    {
        let Some(fd) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        let is_socket = std::fs::read_link(entry.path())
            .is_ok_and(|target| target.to_string_lossy().starts_with("socket:"));
        if !is_socket {
            continue;
        }
        let mut listening: libc::c_int = 0;
        let mut length =
            libc::socklen_t::try_from(std::mem::size_of::<libc::c_int>()).expect("an int's size");
        // SAFETY: `getsockopt` writes at most `length` bytes into a local int; a descriptor closed
        // since the listing makes it fail, and that entry is skipped.
        let answered = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ACCEPTCONN,
                std::ptr::from_mut(&mut listening).cast(),
                &raw mut length,
            )
        } == 0;
        if !answered || listening == 0 {
            continue;
        }
        // SAFETY: `F_GETFD` reads the descriptor flags and touches no memory.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            continue;
        }
        held.push(Held {
            fd,
            cloexec: flags & libc::FD_CLOEXEC != 0,
        });
    }
    held
}

/// Every descriptor above stdio an `ls -l /proc/self/fd/` listing shows, except `ls`'s own handle.
fn extra_descriptors(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter(|line| line.contains(" -> "))
        .filter(|line| {
            let name = line
                .split(" -> ")
                .next()
                .and_then(|left| left.split_whitespace().last())
                .unwrap_or("");
            !matches!(name, "0" | "1" | "2")
        })
        .filter(|line| !line.trim_end().ends_with("/fd"))
        .map(str::to_owned)
        .collect()
}

/// While a run with an aperture is running, the listening socket the host holds for it is
/// close-on-exec, and a second run started meanwhile hands its program no descriptor beyond
/// stdio. Once the run has ended, the host holds no listening socket it did not hold before.
#[tokio::test(flavor = "multi_thread")]
async fn an_apertures_held_listener_is_close_on_exec_and_released_when_the_run_ends() {
    let destination = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a pinned destination");
    let pinned = destination.local_addr().expect("its address");
    let Some(lane) = lane(pinned) else {
        return;
    };
    let before = listening_sockets();

    let first = started(
        &lane,
        "ex_review_listener_held",
        &exec_input(&lane.snapshot, &["/bin/sh", "-c", "sleep 3"], true, false),
    )
    .await;
    assert_eq!(
        first.resource.state,
        ExecState::Running,
        "the aperture run did not start"
    );
    let during = listening_sockets();
    let added: Vec<&Held> = during
        .iter()
        .filter(|held| !before.iter().any(|old| old.fd == held.fd))
        .collect();
    assert_eq!(
        added.len(),
        1,
        "a run with one aperture holds one listening socket here; before {before:?}, during \
         {during:?}"
    );
    assert!(
        during.iter().all(|held| held.cloexec),
        "a listening socket the host holds during a run is inheritable across exec: {during:?}"
    );

    let second = started(
        &lane,
        "ex_review_listener_neighbour",
        &exec_input(
            &lane.snapshot,
            &["/usr/bin/ls", "-l", "/proc/self/fd/"],
            false,
            true,
        ),
    )
    .await;
    let listing = String::from_utf8_lossy(&second.stdout).into_owned();
    assert!(listing.contains(" 0 -> "), "no listing:\n{listing}");
    let extra = extra_descriptors(&listing);
    assert!(
        extra.is_empty(),
        "a run started while an aperture's listener was held received descriptors: {extra:?}"
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let ended = loop {
        let observed = lane
            .driver
            .observe_exec("ex_review_listener_held")
            .await
            .expect("observe the aperture run");
        if observed.resource.state != ExecState::Running {
            break observed;
        }
        assert!(Instant::now() < deadline, "the aperture run never ended");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        ended.resource.state,
        ExecState::Exited,
        "the aperture run did not exit"
    );
    let after = listening_sockets();
    assert_eq!(
        after, before,
        "the host still holds an aperture's listening socket after its run ended"
    );
    drop(destination);
    println!("egress listener review: executed, held listener close-on-exec and released");
}
