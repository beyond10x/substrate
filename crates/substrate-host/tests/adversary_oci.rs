//! Adversary cases for `story:oci-adapter`.
//!
//! The portable case needs no runtime: a stand-in program answers `info`. The lane cases run real
//! containers with `SUBSTRATE_OCI_LANE=1` (runtime `SUBSTRATE_OCI_RUNTIME`, default `docker`) and
//! print `ABSENT` without it. Every lane case removes the container it started, whatever it found.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
// story:oci-adapter, adversary 1: the socket case is re-pointed at the `ipc_socket_allowlist`
// refusal, so it needs `Fact` and no longer a socket client image or a listener thread.
use substrate_host::confinement::{
    self, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementAdapter, ConfinementProfile,
    Fact, Network, ProcessBounds, RunExit, RunId, RunObservation, RunState,
    local::LocalRequest,
    oci::{self, OciRequest},
};
use substrate_wire::{ReadOnlyRoot, Signal};

const LANE_ENV: &str = "SUBSTRATE_OCI_LANE";
const RUNTIME_ENV: &str = "SUBSTRATE_OCI_RUNTIME";
/// The suite's own test image: Alpine 3.20 by index digest.
const IMAGE: &str = "docker.io/library/alpine@sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc";
/// Pinned, well-formed, and in no registry anybody can reach.
const ABSENT_IMAGE: &str =
    "registry.invalid/run@sha256:0000000000000000000000000000000000000000000000000000000000000000";

const BOUNDS: ProcessBounds = ProcessBounds {
    timeout_ms: 120_000,
    processes: 32,
    memory_bytes: 128 << 20,
    cpu_millis: 120_000,
};

fn recording() -> (AuditSink, Arc<Mutex<Vec<AuditRecord>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    (
        AuditSink::new(move |record: &AuditRecord| sink_records.lock().push(record.clone())),
        records,
    )
}

fn events(records: &Mutex<Vec<AuditRecord>>) -> Vec<serde_json::Value> {
    records
        .lock()
        .iter()
        .map(|record| serde_json::to_value(record).expect("record"))
        .collect()
}

fn canonical_tempdir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = std::fs::canonicalize(dir.path()).expect("canonical");
    (dir, path)
}

/// story:oci-adapter, security F1: the operator's workspace root; every [`canonical_tempdir`] sits
/// directly beneath it.
fn workspace_root() -> PathBuf {
    std::fs::canonicalize(std::env::temp_dir()).expect("canonical temporary directory")
}

// ---- portable ----

fn fake_docker() -> &'static PathBuf {
    static FAKE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &FAKE
        .get_or_init(|| {
            let (dir, root) = canonical_tempdir();
            let info = "{\"ServerVersion\":\"29.0.0\",\"SecurityOptions\":[\"name=seccomp,profile=builtin\"],\
                 \"MemoryLimit\":true,\"SwapLimit\":true,\"PidsLimit\":true,\"CpuCfsQuota\":true,\"NCPU\":4}";
            let path = root.join("docker-ok");
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\nif [ \"$1\" = info ]; then printf '%s' '{info}'; exit 0; fi\nexit 1\n"
                ),
            )
            .expect("fake runtime");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("executable");
            for _ in 0..100 {
                match std::process::Command::new(&path)
                    .arg("version")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .status()
                {
                    Ok(_) => break,
                    Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
            (dir, path)
        })
        .1
}

/// ADR 0109 decision 2 / `ConfinementProfile::audit`: "Where admission, refusal and start records
/// go". A start the adapter refuses by name after admission (here `exec.environment-refused`, a
/// refusal this unit added) must leave a refusal record; it leaves none.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_oci_a_start_refusal_leaves_an_audit_record() {
    let (_ws, workspace) = canonical_tempdir();
    let adapter = confinement::open(AdapterRequest::Oci(OciRequest {
        runtime: fake_docker().clone(),
        // story:oci-adapter, security F1: the request names the operator's workspace root.
        workspace_root: workspace_root(),
        writable_roots: Vec::new(),
    }))
    .expect("oci opens over the stand-in");
    let (sink, records) = recording();
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(ABSENT_IMAGE.into()),
            workspace: Some(workspace),
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let error = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/true".into()],
                env: BTreeMap::from([("A=B".to_owned(), "x".to_owned())]),
            },
        )
        .await
        .expect_err("refused before anything runs");
    assert_eq!(error.code(), "exec.environment-refused");
    let records = events(&records);
    assert!(
        records
            .iter()
            .any(|record| record["code"] == "exec.environment-refused"),
        "the start was refused exec.environment-refused and no audit record says so: {records:?}"
    );
}

// ---- lane: a real container runtime ----

fn lane() -> Option<PathBuf> {
    (std::env::var_os(LANE_ENV).as_deref() == Some("1".as_ref())).then(|| {
        std::env::var_os(RUNTIME_ENV).map_or_else(|| PathBuf::from("docker"), PathBuf::from)
    })
}

fn open_lane(runtime: &Path) -> Arc<dyn ConfinementAdapter> {
    let adapter = confinement::open(AdapterRequest::Oci(OciRequest {
        runtime: runtime.to_path_buf(),
        // story:oci-adapter, security F1: the request names the operator's workspace root.
        workspace_root: workspace_root(),
        writable_roots: Vec::new(),
    }))
    .unwrap_or_else(|error| panic!("{LANE_ENV}=1 and oci does not open: {error}"));
    pull(runtime);
    adapter
}

/// story:oci-adapter, adversary 7: a run pulls nothing (`--pull never`), so the lane pulls its
/// pinned images once before any run.
fn pull(runtime: &Path) {
    static PULLED: OnceLock<()> = OnceLock::new();
    PULLED.get_or_init(|| {
        let status = std::process::Command::new(runtime)
            .args(["pull", IMAGE])
            .stdin(std::process::Stdio::null())
            .status()
            .expect("the runtime runs");
        assert!(status.success(), "pulling {IMAGE} failed");
    });
}

fn runtime_ok(runtime: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new(runtime)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the runtime runs");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn containers(runtime: &Path, run: &RunId) -> String {
    runtime_ok(
        runtime,
        &[
            "ps",
            "--all",
            "--quiet",
            "--filter",
            &format!("label={}={}", oci::RUN_LABEL, run.as_str()),
        ],
    )
}

/// Ends and removes a run whatever state the case left it in.
async fn reap(adapter: &Arc<dyn ConfinementAdapter>, runtime: &Path, run: &RunId) {
    let _ = adapter.signal(run, Signal::Kill).await;
    let _ = tokio::time::timeout(Duration::from_secs(90), adapter.wait(run)).await;
    let _ = runtime_ok(runtime, &["rm", "--force", run.as_str()]);
    assert_eq!(containers(runtime, run), "", "the case left a container");
}

async fn run_to_end(
    adapter: &Arc<dyn ConfinementAdapter>,
    runtime: &Path,
    profile: ConfinementProfile,
    argv: &[&str],
) -> (RunId, Option<RunObservation>, String, String) {
    let admitted = adapter.admit(&profile).expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("started");
    let ended = tokio::time::timeout(Duration::from_secs(150), adapter.wait(&run))
        .await
        .ok()
        .map(|observed| observed.expect("waited"));
    let output = adapter.output(&run).await.expect("output");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    reap(adapter, runtime, &run).await;
    (run, ended, stdout, stderr)
}

/// `signal(Term)` ends `sleep 60` under `local` (signal 15). Under `oci` the command is the
/// container's PID 1, which the kernel shields from every signal it has no handler for, so the same
/// call returns `Ok` and the run keeps going until its timeout. No `--init` is passed.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_oci_a_term_signal_ends_the_run_as_it_does_under_local() {
    let Some(runtime) = lane() else {
        println!("adversary oci term: ABSENT ({LANE_ENV} is not 1)");
        return;
    };
    // The same request under `local`: the reference behaviour.
    let (_lws, local_ws) = canonical_tempdir();
    let local = confinement::open(AdapterRequest::Local(LocalRequest::default())).expect("local");
    let admitted = local
        .admit(&ConfinementProfile {
            workspace: Some(local_ws),
            local_reason: Some("adversary reference run".into()),
            ..ConfinementProfile::default()
        })
        .expect("local admits");
    let run = local
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sleep".into(), "60".into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("local starts");
    tokio::time::sleep(Duration::from_millis(300)).await;
    local.signal(&run, Signal::Term).await.expect("signalled");
    let local_ended = tokio::time::timeout(Duration::from_secs(10), local.wait(&run))
        .await
        .expect("local ends on TERM")
        .expect("waited");
    assert_eq!(
        local_ended.exit,
        Some(RunExit {
            code: None,
            signal: Some(15)
        })
    );

    let adapter = open_lane(&runtime);
    let (_ws, workspace) = canonical_tempdir();
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(IMAGE.into()),
            workspace: Some(workspace),
            process_bounds: Some(BOUNDS),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sleep".into(), "60".into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("started");
    let deadline = Instant::now() + Duration::from_secs(90);
    while runtime_ok(
        &runtime,
        &[
            "ps",
            "--quiet",
            "--filter",
            &format!("label={}={}", oci::RUN_LABEL, run.as_str()),
            "--filter",
            "status=running",
        ],
    )
    .is_empty()
    {
        assert!(Instant::now() < deadline, "the container never ran");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    adapter
        .signal(&run, Signal::Term)
        .await
        .expect("signal returns Ok");
    let ended = tokio::time::timeout(Duration::from_secs(20), adapter.wait(&run))
        .await
        .ok()
        .map(|observed| observed.expect("waited"));
    reap(&adapter, &runtime, &run).await;
    let ended = ended.expect(
        "signal(Term) returned Ok and the oci run was still running 20 s later; \
         under local the same call ended it with signal 15",
    );
    assert_eq!(
        ended.exit,
        Some(RunExit {
            code: None,
            signal: Some(15)
        }),
        "{ended:?}"
    );
}

/// A command killed by a signal the adapter did not send (here the kernel's OOM killer, SIGKILL,
/// at the profile's own memory bound) is reported `code: Some(137)`, `signal: None`: the runtime
/// client's `128 + n` is decoded only for a signal the adapter itself sent. `local` and `linux`
/// report the same death as `signal: Some(9)`, so a client reading `exit.signal` sees a different
/// answer per adapter (invariant 4).
#[tokio::test(flavor = "multi_thread")]
async fn adversary_oci_a_run_killed_by_its_memory_bound_reports_signal_9() {
    let Some(runtime) = lane() else {
        println!("adversary oci oom: ABSENT ({LANE_ENV} is not 1)");
        return;
    };
    let adapter = open_lane(&runtime);
    let (_ws, workspace) = canonical_tempdir();
    let (_run, ended, stdout, stderr) = run_to_end(
        &adapter,
        &runtime,
        ConfinementProfile {
            image: Some(IMAGE.into()),
            workspace: Some(workspace),
            process_bounds: Some(ProcessBounds {
                memory_bytes: 16 << 20,
                ..BOUNDS
            }),
            ..ConfinementProfile::default()
        },
        &[
            "/bin/sh",
            "-c",
            "x=aaaaaaaaaaaaaaaa; while :; do x=$x$x; done",
        ],
    )
    .await;
    let ended = ended.expect("the run ends");
    assert_eq!(ended.state, RunState::Ended, "{ended:?} {stdout} {stderr}");
    assert_eq!(
        ended.exit,
        Some(RunExit {
            code: None,
            signal: Some(9)
        }),
        "a SIGKILLed command is reported as an exit code: {ended:?} stderr: {stderr}"
    );
}

/// A pinned image the runtime cannot obtain: the container never exists and no command runs. The
/// runtime client exits 125 and the adapter reports the run `Ended` with `code: Some(125)` after an
/// audit `run_started` - indistinguishable from a command that ran and exited 125 (invariant 3: a
/// capability that could not be applied is a named refusal, never an ordinary result).
#[tokio::test(flavor = "multi_thread")]
async fn adversary_oci_a_container_that_never_started_is_not_a_command_exit() {
    let Some(runtime) = lane() else {
        println!("adversary oci never-started: ABSENT ({LANE_ENV} is not 1)");
        return;
    };
    let adapter = open_lane(&runtime);
    let (_ws, workspace) = canonical_tempdir();
    let (sink, records) = recording();
    // story:oci-adapter, adversary 4 (coordinator's decision): a container the runtime did not
    // create is a named refusal of the start itself, so no run exists to end, with 125 or
    // otherwise. The start is refused `confinement.image-absent`, audited as a refusal, and no
    // `run_started` record is written.
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(ABSENT_IMAGE.into()),
            workspace: Some(workspace),
            process_bounds: Some(BOUNDS),
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let refused = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/true".into()],
                env: BTreeMap::new(),
            },
        )
        .await;
    let records = events(&records);
    let events: Vec<_> = records
        .iter()
        .map(|record| record["event"].as_str().unwrap_or_default().to_owned())
        .collect();
    let error = match refused {
        Ok(run) => {
            let ended = tokio::time::timeout(Duration::from_secs(150), adapter.wait(&run))
                .await
                .ok()
                .map(|observed| observed.expect("waited"));
            reap(&adapter, &runtime, &run).await;
            panic!(
                "no container was ever created, and a run was started: {ended:?}; audit {events:?}"
            );
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), oci::IMAGE_ABSENT, "{error}");
    assert_eq!(
        events,
        ["profile_admitted", "profile_refused"],
        "{records:?}"
    );
    assert_eq!(records[1]["code"], oci::IMAGE_ABSENT, "{records:?}");
    // The runtime's own error text is the adapter's to keep, not the client's.
    assert!(
        !error.to_string().contains("daemon") && !error.to_string().contains("No such image"),
        "{error}"
    );
}

/// A read-only root is "mounted read-only" (ADR 0010). `:ro` does not stop `connect(2)` on a Unix
/// socket beneath it, so a confined command with `--network none` reaches any user-owned socket
/// under a read-only root (an ssh agent, a session bus, a process manager's RPC socket, a rootless
/// runtime's API socket) and talks to a host process outside the container.
///
/// story:oci-adapter, adversary 1 (coordinator's decision): socket reach is what the
/// `ipc_socket_allowlist` fact states, and `oci` does not prove it. A profile that needs sockets
/// beneath its roots unreachable requires that fact and is refused `confinement.fact-unserved`
/// before anything runs; design 04 records that a root's sockets are reachable under `oci`, and
/// the runtime's own socket stays refused by name (security F4). The case no longer needs a
/// runtime, so it runs on every lane.
#[test]
fn adversary_oci_a_socket_under_a_read_only_root_is_not_reachable() {
    let (_ro, read_only) = canonical_tempdir();
    let _listener = UnixListener::bind(read_only.join("s")).expect("listener");
    let (_ws, workspace) = canonical_tempdir();
    let adapter = confinement::open(AdapterRequest::Oci(OciRequest {
        runtime: fake_docker().clone(),
        workspace_root: workspace_root(),
        writable_roots: Vec::new(),
    }))
    .expect("oci opens over the stand-in");
    assert!(!adapter.facts().proves(&Fact::IpcSocketAllowlist));
    let (sink, records) = recording();
    let error = adapter
        .admit(&ConfinementProfile {
            image: Some(IMAGE.into()),
            workspace: Some(workspace),
            read_only_roots: vec![ReadOnlyRoot {
                host_path: read_only.display().to_string(),
                mount: "/opt/ro".into(),
            }],
            network: Some(Network::Isolated),
            process_bounds: Some(BOUNDS),
            facts: vec![Fact::IpcSocketAllowlist],
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect_err("a profile that needs sockets unreachable is refused on oci");
    assert!(
        matches!(
            &error,
            confinement::ConfinementError::FactUnserved {
                fact: Fact::IpcSocketAllowlist,
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(error.code(), "confinement.fact-unserved");
    let records = events(&records);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["event"], "profile_refused");
    assert_eq!(records[0]["fact"], "ipc_socket_allowlist");
}

/// The story's acceptance: "a write outside the declared mounts fails". The runtime's own
/// `/dev/shm` (64 MiB tmpfs) is no declared mount, and a write there succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_oci_a_write_to_dev_shm_fails_like_any_undeclared_path() {
    let Some(runtime) = lane() else {
        println!("adversary oci shm: ABSENT ({LANE_ENV} is not 1)");
        return;
    };
    let adapter = open_lane(&runtime);
    let (_ws, workspace) = canonical_tempdir();
    let (_run, ended, stdout, stderr) = run_to_end(
        &adapter,
        &runtime,
        ConfinementProfile {
            image: Some(IMAGE.into()),
            workspace: Some(workspace),
            process_bounds: Some(BOUNDS),
            ..ConfinementProfile::default()
        },
        &[
            "/bin/sh",
            "-c",
            "touch /dev/shm/x 2>/dev/null && echo wrote-shm || echo shm-refused",
        ],
    )
    .await;
    assert_eq!(
        stdout.trim(),
        "shm-refused",
        "an undeclared path is writable: {ended:?} {stderr}"
    );
}
