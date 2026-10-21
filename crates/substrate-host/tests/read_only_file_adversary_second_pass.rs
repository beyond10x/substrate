//! Second adversarial pass against ADR 0033 (`story:read-only-host-file-bind`) after the
//! descriptor-bind fix.
//!
//! Delegated lane only: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, or without bubblewrap, the delegated cases return early and are **absent,
//! never passed**. `bash scripts/delegated-lane.sh` runs them.

// Every case drives the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use substrate_host::{
    DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver, SecretSlot,
};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode,
    PipeSessionStartInput, PtyWindow, ReadOnlyFile, ReadOnlyRoot, SandboxProfile,
    SecretSlotRequest, SessionMode, WorkspaceAccess,
};

const WORKSPACE: &str = "ws_test";
const ORIGINAL: &str = "#!/bin/sh\necho original\n";
/// A classic-BPF program of one instruction, `RET SECCOMP_RET_ALLOW`, as the slot's bytes: if a
/// slot placement replaces the driver's seccomp descriptor, bubblewrap installs this instead.
const ALLOW_ALL_BPF: [u8; 8] = [0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x7f];

fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    fixture: PathBuf,
    _workspaces: tempfile::TempDir,
    _fixture: tempfile::TempDir,
}

fn lane_with(slot: bool) -> Option<Lane> {
    lane_configured(slot, None)
}

/// `aperture`, when given, is the pinned destination of one declared aperture named `model`.
fn lane_configured(slot: bool, aperture: Option<std::net::SocketAddr>) -> Option<Lane> {
    let delegated = delegated_cgroup_root()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() {
        return None;
    }
    let fixture_dir = tempfile::Builder::new()
        .prefix("ro-file-adv2-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("fixture directory");
    let fixture = std::fs::canonicalize(fixture_dir.path()).expect("canonical fixture");
    std::fs::write(fixture.join("gate"), ORIGINAL).expect("host program");
    std::fs::set_permissions(fixture.join("gate"), std::fs::Permissions::from_mode(0o755))
        .expect("program mode");
    std::fs::create_dir(fixture.join("root")).expect("root directory");
    std::fs::write(fixture.join("root/tool"), "tool\n").expect("root file");
    std::fs::write(fixture.join("secret"), ALLOW_ALL_BPF).expect("secret");
    std::fs::set_permissions(
        fixture.join("secret"),
        std::fs::Permissions::from_mode(0o600),
    )
    .expect("secret mode");
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(workspaces.path().join("workspaces"));
    config.cgroup_root = Some(delegated);
    if slot {
        config.secret_slots = vec![SecretSlot {
            name: "probe_slot".to_owned(),
            path: fixture.join("secret"),
        }];
    }
    if let Some(pinned) = aperture {
        config.egress_apertures = vec![substrate_host::EgressAperture {
            name: "model".to_owned(),
            host: "app.example.invalid".to_owned(),
            port: pinned.port(),
            pinned,
            max_bytes: None,
        }];
        config.aperture_root = workspaces.path().join("apertures");
    }
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    if machine.facts.exec_argv_only != Some(true) {
        eprintln!(
            "lane absent: the floor is not proven on this host: {:?}",
            machine.facts
        );
        return None;
    }
    std::fs::create_dir_all(driver.root().join(WORKSPACE).join("out")).expect("workspace");
    std::fs::write(driver.root().join(WORKSPACE).join("kept"), "kept\n").expect("kept");
    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        fixture,
        _workspaces: workspaces,
        _fixture: fixture_dir,
    })
}

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8 fixture path").to_owned()
}

fn exec_input(snapshot: &str, argv: &[&str]) -> ExecStartInput {
    ExecStartInput {
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

async fn waited(lane: &Lane, id: &str, input: &ExecStartInput) -> ExecObservation {
    match lane.driver.start_exec(id, WORKSPACE, input).await {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("{id} must dispatch: {} {}", error.code, error.message)
        }
    }
}

async fn session_output(lane: &Lane, id: &str, start: &PipeSessionStartInput) -> String {
    match lane.driver.start_pipe_session(id, WORKSPACE, start).await {
        DispatchOutcome::Observed(_) => {}
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("{id} must dispatch: {} {}", error.code, error.message)
        }
    }
    let mut transcript = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        match lane
            .driver
            .read_pipe_session(id, Duration::from_secs(5))
            .await
        {
            Ok(Some(frame)) => transcript.extend_from_slice(&frame.bytes),
            Ok(None) => break,
            Err(_deadline) => {}
        }
    }
    String::from_utf8_lossy(&transcript).into_owned()
}

/// `ls -l /proc/self/fd/` run as the program itself: descriptor 3 is `ls`'s own directory handle.
const LIST: [&str; 3] = ["/usr/bin/ls", "-l", "/proc/self/fd/"];

/// Every descriptor above stdio that the listing shows, except `ls`'s own directory handle.
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

/// Item 1: in every exec mode that carries pinned descriptors (read-only workspace, scoped
/// subtrees, a declared file, beside a declared root), and through the pipe and pty session paths,
/// the program holds no descriptor beyond its stdio.
#[tokio::test(flavor = "multi_thread")]
async fn no_exec_mode_hands_the_program_a_descriptor_beyond_stdio() {
    let Some(lane) = lane_with(false) else {
        return;
    };
    let file = ReadOnlyFile {
        host_path: text(&lane.fixture.join("gate")),
        mount: "/opt/gates/bin/gate".to_owned(),
    };
    let root = ReadOnlyRoot {
        host_path: text(&lane.fixture.join("root")),
        mount: "/opt/root".to_owned(),
    };
    let modes: Vec<(&str, ExecStartInput)> = vec![
        ("plain", exec_input(&lane.snapshot, &LIST)),
        ("read-only workspace", {
            let mut input = exec_input(&lane.snapshot, &LIST);
            input.workspace_access = WorkspaceAccess::ReadOnly;
            input
        }),
        ("scoped workspace", {
            let mut input = exec_input(&lane.snapshot, &LIST);
            input.workspace_access = WorkspaceAccess::Scoped {
                writable_subtrees: vec!["out".to_owned()],
            };
            input
        }),
        ("file, root and scoped workspace", {
            let mut input = exec_input(&lane.snapshot, &LIST);
            input.read_only_files = vec![file.clone()];
            input.read_only_roots = vec![root.clone()];
            input.workspace_access = WorkspaceAccess::Scoped {
                writable_subtrees: vec!["out".to_owned()],
            };
            input
        }),
    ];
    let mut leaks = Vec::new();
    for (index, (mode, input)) in modes.iter().enumerate() {
        let observed = waited(&lane, &format!("ex_adv2_fds_{index}"), input).await;
        let listing = String::from_utf8_lossy(&observed.stdout).into_owned();
        assert!(listing.contains(" 0 -> "), "{mode}: no listing:\n{listing}");
        let extra = extra_descriptors(&listing);
        if !extra.is_empty() {
            leaks.push(format!("{mode}: {extra:?}"));
        }
    }
    for mode in [SessionMode::Pipes, SessionMode::Pty] {
        let mut exec = exec_input(&lane.snapshot, &LIST);
        exec.wait = false;
        exec.read_only_files = vec![file.clone()];
        exec.workspace_access = WorkspaceAccess::Scoped {
            writable_subtrees: vec!["out".to_owned()],
        };
        let start = PipeSessionStartInput {
            capture: substrate_wire::CaptureMode::Recorded,
            exec,
            input_limit_bytes: 65_536,
            frame_limit_bytes: 4_096,
            queued_frames: 8,
            mode,
            window: (mode == SessionMode::Pty).then_some(PtyWindow {
                columns: 200,
                rows: 24,
            }),
        };
        let id = format!("ex_adv2_session_{mode:?}").to_lowercase();
        let listing = session_output(&lane, &id, &start).await;
        assert!(
            listing.contains(" 0 -> "),
            "{mode:?}: no listing:\n{listing}"
        );
        let extra = extra_descriptors(&listing);
        if !extra.is_empty() {
            leaks.push(format!("{mode:?} session: {extra:?}"));
        }
    }
    assert!(
        leaks.is_empty(),
        "descriptors reached the program:\n{}",
        leaks.join("\n")
    );
}

/// Item 1: a writable subtree's descriptor bind gives write access to that subtree and nothing
/// else — not through `..`, a rename out of it, or a hard link into it.
#[tokio::test(flavor = "multi_thread")]
async fn a_writable_subtree_does_not_reach_the_rest_of_the_workspace() {
    let Some(lane) = lane_with(false) else {
        return;
    };
    let workspace = lane.driver.root().join(WORKSPACE);
    let script = "cd /workspace/out && printf in >inside && \
         ! (printf x >../escaped) 2>/dev/null && \
         ! (printf x >/workspace/out/../escaped2) 2>/dev/null && \
         ! mv inside ../moved 2>/dev/null && \
         ! ln ../kept kept-link 2>/dev/null && \
         ! (printf x >>../kept) 2>/dev/null && echo contained";
    let mut input = exec_input(&lane.snapshot, &["/bin/sh", "-c", script]);
    input.workspace_access = WorkspaceAccess::Scoped {
        writable_subtrees: vec!["out".to_owned()],
    };
    let observed = waited(&lane, "ex_adv2_subtree", &input).await;
    let stdout = String::from_utf8_lossy(&observed.stdout);
    assert_eq!(
        stdout.trim(),
        "contained",
        "stderr: {}",
        String::from_utf8_lossy(&observed.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("out/inside")).unwrap(),
        "in"
    );
    assert!(!workspace.join("escaped").exists() && !workspace.join("escaped2").exists());
    assert!(!workspace.join("moved").exists() && !workspace.join("out/kept-link").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.join("kept")).unwrap(),
        "kept\n"
    );
}

/// Item 2, the implementor's open question: a secret slot may be placed at any descriptor in
/// `3..=MAX_SECRET_SLOT_FD`, while the launch barrier, the info pipe and the seccomp program are
/// created at the lowest free descriptor numbers of the daemon. A slot placed on one of them
/// replaces it. Every dispatched start must hand the program its slot at the declared number
/// **and** keep the `AF_UNIX` `socket(2)` refusal of the seccomp profile; a start that cannot do
/// both must be refused before dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn no_secret_slot_placement_replaces_a_driver_descriptor() {
    let Some(lane) = lane_with(true) else {
        return;
    };
    assert!(
        lane.driver.machine().facts.secrets_slots.is_some(),
        "the delegated host publishes the declared slot"
    );
    every_slot_placement_reaches_the_program(&lane, false).await;
}

/// The same with an egress aperture, so bubblewrap is also handed its `--info-fd` write end: a
/// slot placed on that number would take the sandbox pid report and leave the aperture uninstalled.
#[tokio::test(flavor = "multi_thread")]
async fn no_secret_slot_placement_replaces_the_info_pipe_of_an_aperture_exec() {
    let destination = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .expect("a pinned loopback destination");
    let pinned = destination.local_addr().expect("pinned address");
    let Some(lane) = lane_configured(true, Some(pinned)) else {
        return;
    };
    let facts = lane.driver.machine().facts;
    assert!(
        facts.secrets_slots.is_some(),
        "the delegated host publishes the declared slot"
    );
    assert!(
        facts.exec_egress_apertures.is_some(),
        "the delegated host publishes the declared aperture"
    );
    every_slot_placement_reaches_the_program(&lane, true).await;
    drop(destination);
}

async fn every_slot_placement_reaches_the_program(lane: &Lane, aperture: bool) {
    let mut failures = Vec::new();
    let mut refused = Vec::new();
    for fd in 3..=63_u32 {
        let script = format!(
            "od -An -tx1 /proc/self/fd/{fd} | tr -d ' \\n'; echo; \
             socat -u /dev/null UNIX-CONNECT:/workspace/absent-socket 2>&1 \
               | grep -o 'Permission denied\\|No such file or directory' | head -n1; \
             cut -d: -f3 /proc/self/cgroup"
        );
        let mut input = exec_input(&lane.snapshot, &["/bin/sh", "-c", &script]);
        input.secret_slots = vec![SecretSlotRequest {
            slot: "probe_slot".to_owned(),
            fd,
        }];
        if aperture {
            input.sandbox.network = NetworkMode::Aperture;
            input.sandbox.aperture = Some("model".to_owned());
        }
        let id = format!("ex_adv2_slot_{}{fd}", if aperture { "ap_" } else { "" });
        match lane.driver.start_exec(&id, WORKSPACE, &input).await {
            DispatchOutcome::NotDispatched(error) => {
                refused.push(format!("fd {fd}: {}", error.code));
            }
            DispatchOutcome::Observed(observed) => {
                let stdout = String::from_utf8_lossy(&observed.stdout).into_owned();
                let mut lines = stdout.lines();
                let slot = lines.next().unwrap_or("");
                let socket = lines.next().unwrap_or("");
                if slot != "060000000000ff7f" || socket != "Permission denied" {
                    failures.push(format!(
                        "fd {fd}: slot bytes {slot:?}, AF_UNIX socket: {socket:?}, cgroup {:?}, \
                         stderr {:?}",
                        lines.next().unwrap_or(""),
                        String::from_utf8_lossy(&observed.stderr).trim()
                    ));
                }
            }
            DispatchOutcome::ContainedAbsent(error) | DispatchOutcome::OutcomeUnknown(error) => {
                failures.push(format!(
                    "fd {fd}: dispatched and failed: {} {}",
                    error.code, error.message
                ));
            }
        }
    }
    // A start refused for every descriptor would make the loop above vacuous.
    assert!(
        refused.len() < 4,
        "the slot starts were refused, so nothing was attacked: {refused:?}"
    );
    eprintln!("refused before dispatch: {refused:?}");
    assert!(
        failures.is_empty(),
        "a slot placement replaced a driver descriptor:\n{}",
        failures.join("\n")
    );
}

/// Item 3: a bubblewrap without descriptor binds (before 0.8.0) proves the confinement floor but
/// neither the read-only file nor the scoped-workspace mechanism; both facts are withheld rather
/// than published over a path-bind fallback.
#[test]
fn a_bubblewrap_without_descriptor_binds_withholds_both_facts() {
    if delegated_cgroup_root().is_none() || !PathBuf::from("/usr/bin/bwrap").is_file() {
        return;
    }
    let directory = tempfile::tempdir().expect("scratch");
    let wrapper = directory.path().join("bwrap");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in --ro-bind-fd|--bind-fd) \
         echo \"bwrap: Unknown option $a\" >&2; exit 1;; esac; done\nexec /usr/bin/bwrap \"$@\"\n",
    )
    .expect("wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.cgroup_root = delegated_cgroup_root();
    config.bubblewrap = wrapper;
    let driver = HostDriver::open(config).expect("host driver");
    let facts = driver.machine().facts;
    assert_eq!(
        facts.exec_argv_only,
        Some(true),
        "the floor is still proven"
    );
    assert!(facts.exec_read_only_files.is_none());
    assert_ne!(facts.exec_workspace_scoped_write, Some(true));
}
