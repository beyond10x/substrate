//! The confinement contract's `systemd` lane (`story:systemd-scope-bounds-provider`).
//!
//! Executes every authored `systemd-*-on-delegated` scenario of `spec/confinement-suite.json`,
//! both `inner` variants, against the real adapter opened through `confinement::open`, and starts
//! real runs through it: around `local`, a scope bounds an unconfined tree, refuses its forks past
//! `pids.max` and leaves no process after a kill; around `linux`, the scope is the host driver's
//! delegated cgroup root and a confined run ends in it. The count is read from the suite and must
//! equal what ran.
//!
//! The lane's prerequisite is a reachable user systemd manager (`DBUS_SESSION_BUS_ADDRESS` set and
//! `systemctl --user` answering), plus bubblewrap for the `linux` variant. Without it every case
//! here prints `ABSENT` and its reason and executes nothing: absent, never reported as passed
//! (invariant 3). `bash scripts/delegated-lane.sh` runs this file and requires the executed count
//! it prints.
//!
//! Opening `systemd` moves this test process into a transient scope, so this file is its own test
//! binary and its cases that open the adapter hold one lock.

#![cfg(target_os = "linux")]

mod support;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementAdapter,
    ConfinementError, ConfinementProfile, ContractFamily, Fact, InnerAdapter, ProcessBounds,
    RunState,
    linux::LinuxRequest,
    local::LocalRequest,
    systemd::{self, InnerRequest, SystemdRequest},
};
use substrate_wire::Signal;

const REASON: &str = "the test bounds a host process on purpose";

/// The cases that open `systemd` share one process scope; one at a time.
static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn local_inner() -> InnerRequest {
    InnerRequest::Local(LocalRequest::default())
}

fn linux_inner(host: &Path) -> InnerRequest {
    InnerRequest::Linux(Box::new(LinuxRequest::new(HostConfig::minimum(
        host.join("workspaces"),
    ))))
}

fn open(inner: InnerRequest) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    confinement::open(AdapterRequest::Systemd(SystemdRequest::new(inner)))
}

/// Whether the user manager is reachable here, or the reason the lane is absent. Decided by this
/// file and not by the adapter, so an adapter that refuses a reachable manager fails the lane
/// rather than making it look absent.
fn user_manager() -> Result<(), String> {
    let Some(bus) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").filter(|bus| !bus.is_empty())
    else {
        return Err("DBUS_SESSION_BUS_ADDRESS is unset".to_owned());
    };
    let answered = std::process::Command::new("systemctl")
        .args(["--user", "show", "--property=Version", "--value"])
        .env_clear()
        .env("DBUS_SESSION_BUS_ADDRESS", bus)
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("systemctl: {error}"))?;
    if !answered.status.success() {
        return Err(format!(
            "systemctl --user does not answer: {}",
            String::from_utf8_lossy(&answered.stderr).trim()
        ));
    }
    Ok(())
}

/// The `linux` variant's further prerequisite.
fn bubblewrap() -> Result<(), String> {
    if Path::new("/usr/bin/bwrap").is_file() {
        Ok(())
    } else {
        Err("no bubblewrap at /usr/bin/bwrap".to_owned())
    }
}

fn this_cgroup() -> PathBuf {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").expect("own cgroup");
    let path = cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .expect("a cgroup v2 membership");
    PathBuf::from("/sys/fs/cgroup").join(path.trim_start_matches('/'))
}

#[test]
fn ess_confinement_systemd_lane_executes_every_systemd_scenario() {
    let _held = OPENING.blocking_lock();
    if let Err(reason) = user_manager().and_then(|()| bubblewrap()) {
        println!("ESS confinement systemd delegated lane: ABSENT ({reason}); 0 executed");
        return;
    }
    let scenarios = support::scenarios("delegated", |adapter| adapter == "systemd");
    assert!(
        !scenarios.is_empty(),
        "the suite has no delegated systemd scenario"
    );
    let declared = scenarios.len();
    let mut inners = BTreeMap::<String, usize>::new();
    let mut hosts = Vec::new();
    let (executed, audited, open_checks) = support::run(&scenarios, |step| {
        assert_eq!(step.adapter, "systemd");
        let inner = step
            .inner
            .clone()
            .expect("a systemd scenario names its inner");
        *inners.entry(inner.clone()).or_default() += 1;
        let host = tempfile::tempdir().expect("host root");
        let request = match inner.as_str() {
            "local" => local_inner(),
            "linux" => linux_inner(host.path()),
            other => panic!("systemd wraps no {other}"),
        };
        hosts.push(host);
        open(request)
    });
    assert_eq!(
        executed, declared,
        "a delegated systemd scenario did not run"
    );
    for inner in ["local", "linux"] {
        assert!(
            inners.get(inner).copied().unwrap_or_default() > 0,
            "no systemd scenario around {inner} ran: {inners:?}"
        );
    }
    println!(
        "ESS confinement systemd delegated lane: {executed} scenarios executed of {declared} \
         declared for systemd ({inners:?}), {audited} refusal audit records asserted, \
         {open_checks} open checks refused, 0 failed"
    );
}

#[test]
fn without_a_reachable_user_manager_systemd_is_unserved() {
    let host = tempfile::tempdir().expect("host root");
    for session_bus in [
        None,
        Some(OsString::new()),
        Some(OsString::from("unix:path=/nonexistent/substrate-user-bus")),
    ] {
        for inner in [local_inner(), linux_inner(host.path())] {
            let error = confinement::open(AdapterRequest::Systemd(SystemdRequest {
                inner,
                session_bus: session_bus.clone(),
            }))
            .err()
            .unwrap_or_else(|| panic!("systemd opened with session bus {session_bus:?}"));
            assert!(
                matches!(
                    &error,
                    ConfinementError::AdapterUnserved {
                        adapter: AdapterName::Systemd,
                        ..
                    }
                ),
                "{session_bus:?}: {error}"
            );
            assert_eq!(error.code(), "confinement.adapter-unserved");
            assert_eq!(error.status(), 501);
        }
    }
}

/// Reads `name` beneath `cgroup` as one decimal, or the `key` line of a flat-keyed file.
fn counter(cgroup: &Path, name: &str, key: Option<&str>) -> u64 {
    let text = std::fs::read_to_string(cgroup.join(name)).expect(name);
    let value = match key {
        None => text.trim(),
        Some(key) => text
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
            .unwrap_or_else(|| panic!("{name} has no {key}: {text}")),
    };
    value
        .parse()
        .unwrap_or_else(|_| panic!("{name}: {value:?}"))
}

/// Polls `until` every 20 ms for up to `limit`.
async fn eventually(limit: Duration, mut until: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < limit {
        if until() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    until()
}

fn shell(script: &str) -> Command {
    Command {
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_systemd_local_run_is_bounded_and_a_kill_leaves_no_process_of_its_tree() {
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager() {
        println!("ESS confinement systemd local run: ABSENT ({reason})");
        return;
    }
    let adapter = open(local_inner()).expect("systemd around local opens");
    let facts = adapter.facts();
    assert_eq!(facts.adapter, AdapterName::Systemd);
    assert_eq!(facts.inner, Some(InnerAdapter::Local));
    assert!(facts.serves(&ContractFamily::Limits), "{facts:?}");
    assert!(facts.proves(&Fact::WholeTreeKill), "{facts:?}");
    assert!(facts.declared_insecure(), "{facts:?}");
    let workspace = tempfile::tempdir().expect("workspace");
    let memory_bytes = 64 << 20;
    let admitted = adapter
        .admit(&ConfinementProfile {
            inner: Some(InnerAdapter::Local),
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            facts: vec![Fact::WholeTreeKill],
            process_bounds: Some(ProcessBounds {
                timeout_ms: 60_000,
                processes: 8,
                memory_bytes,
                cpu_millis: 30_000,
            }),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    assert!(admitted.declared_insecure());
    let run = adapter
        .start(
            &admitted,
            &shell(
                "cat /proc/self/cgroup; i=0; while [ $i -lt 32 ]; do sleep 60 & i=$((i+1)); done; \
                 wait",
            ),
        )
        .await
        .expect("started");
    let started = Instant::now();
    let cgroup = loop {
        let output = adapter.output(&run).await.expect("output").stdout;
        if let Some(path) = String::from_utf8_lossy(&output)
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
        {
            break PathBuf::from("/sys/fs/cgroup").join(path.trim_start_matches('/'));
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the run never reported its cgroup"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        cgroup.starts_with(this_cgroup().parent().expect("scope")),
        "the run's cgroup {} is not in this process's scope {}",
        cgroup.display(),
        this_cgroup().display()
    );
    assert_eq!(counter(&cgroup, "pids.max", None), 8);
    assert_eq!(counter(&cgroup, "memory.max", None), memory_bytes);
    assert_eq!(counter(&cgroup, "memory.swap.max", None), 0);
    assert!(
        eventually(Duration::from_secs(20), || counter(
            &cgroup,
            "pids.events",
            Some("max")
        ) > 0)
        .await,
        "no fork was refused at pids.max"
    );
    assert!(counter(&cgroup, "pids.current", None) <= 8);
    let observed = adapter.observe(&run).await.expect("observed");
    assert_eq!(observed.state, RunState::Running);
    assert_eq!(observed.adapter, AdapterName::Systemd);
    assert_eq!(observed.inner, Some(InnerAdapter::Local));
    assert_eq!(observed.declared_insecure.as_deref(), Some(REASON));
    let tree = std::fs::read_to_string(cgroup.join("cgroup.procs")).expect("cgroup.procs");
    assert!(tree.lines().count() >= 2, "the tree forked nothing: {tree}");
    adapter.signal(&run, Signal::Kill).await.expect("killed");
    let ended = tokio::time::timeout(Duration::from_secs(20), adapter.wait(&run))
        .await
        .expect("the run ends after a kill")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    assert_eq!(ended.exit.and_then(|exit| exit.signal), Some(9));
    assert_eq!(ended.declared_insecure.as_deref(), Some(REASON));
    let left = std::fs::read_to_string(cgroup.join("cgroup.procs")).unwrap_or_default();
    assert!(
        left.trim().is_empty(),
        "processes of the tree are left after wait: {left}"
    );
    println!(
        "ESS confinement systemd local run: executed ({} processes killed)",
        tree.lines().count()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_systemd_local_run_ends_at_its_timeout_and_its_cpu_budget() {
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager() {
        println!("ESS confinement systemd local bounds: ABSENT ({reason})");
        return;
    }
    let adapter = open(local_inner()).expect("systemd around local opens");
    let workspace = tempfile::tempdir().expect("workspace");
    for (bounds, script) in [
        (
            ProcessBounds {
                timeout_ms: 300,
                processes: 8,
                memory_bytes: 64 << 20,
                cpu_millis: 30_000,
            },
            "sleep 60",
        ),
        (
            ProcessBounds {
                timeout_ms: 60_000,
                processes: 8,
                memory_bytes: 64 << 20,
                cpu_millis: 200,
            },
            "while :; do :; done",
        ),
    ] {
        let admitted = adapter
            .admit(&ConfinementProfile {
                inner: Some(InnerAdapter::Local),
                local_reason: Some(REASON.into()),
                workspace: Some(workspace.path().to_path_buf()),
                process_bounds: Some(bounds),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let started = Instant::now();
        let run = adapter
            .start(&admitted, &shell(script))
            .await
            .expect("started");
        let ended = tokio::time::timeout(Duration::from_secs(20), adapter.wait(&run))
            .await
            .unwrap_or_else(|_| panic!("{script} outlived its bounds {bounds:?}"))
            .expect("waited");
        assert_eq!(ended.exit.and_then(|exit| exit.signal), Some(9), "{script}");
        assert!(started.elapsed() < Duration::from_secs(20));
    }
    println!("ESS confinement systemd local bounds: executed");
}

#[tokio::test(flavor = "multi_thread")]
async fn systemd_around_linux_gives_linux_the_scope_as_its_delegated_root() {
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager().and_then(|()| bubblewrap()) {
        println!("ESS confinement systemd linux run: ABSENT ({reason})");
        return;
    }
    let host = tempfile::tempdir().expect("host root");
    let adapter = open(linux_inner(host.path())).expect("systemd around linux opens");
    let facts = adapter.facts();
    assert_eq!(facts.adapter, AdapterName::Systemd);
    assert_eq!(facts.inner, Some(InnerAdapter::Linux));
    assert!(!facts.declared_insecure(), "{facts:?}");
    for family in [ContractFamily::ProcessRun, ContractFamily::Limits] {
        assert!(facts.serves(&family), "{facts:?}");
    }
    assert!(facts.proves(&Fact::WholeTreeKill), "{facts:?}");
    // This process sits in a leaf of a transient user scope, which is the driver's root.
    let scope = this_cgroup().parent().expect("scope").to_path_buf();
    assert!(
        scope
            .extension()
            .is_some_and(|extension| extension == "scope"),
        "{}",
        scope.display()
    );
    let second = open(linux_inner(host.path()))
        .err()
        .expect("one process scope is the root of one linux driver");
    assert_eq!(second.code(), "confinement.adapter-unserved", "{second}");

    let workspace = host.path().join("workspaces").join("ws_systemd");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            inner: Some(InnerAdapter::Linux),
            workspace: Some(workspace.clone()),
            facts: vec![Fact::WholeTreeKill, Fact::IpcSignalsContained],
            process_bounds: Some(ProcessBounds {
                timeout_ms: 30_000,
                processes: 16,
                memory_bytes: 128 << 20,
                cpu_millis: 10_000,
            }),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    assert!(!admitted.declared_insecure());
    let run = adapter
        .start(
            &admitted,
            &shell("printf done > /workspace/marker; sleep 60"),
        )
        .await
        .expect("started");
    let mut exec_cgroup = None;
    assert!(
        eventually(Duration::from_secs(10), || {
            exec_cgroup = std::fs::read_dir(&scope)
                .expect("scope")
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("substrate-ex_"))
                        && std::fs::read_to_string(path.join("cgroup.procs"))
                            .is_ok_and(|procs| !procs.trim().is_empty())
                });
            exec_cgroup.is_some()
        })
        .await,
        "no exec cgroup holding the run appeared beneath the scope {}",
        scope.display()
    );
    let observed = adapter.observe(&run).await.expect("observed");
    assert_eq!(observed.adapter, AdapterName::Systemd);
    assert_eq!(observed.inner, Some(InnerAdapter::Linux));
    assert_eq!(observed.declared_insecure, None);
    // The confined child wrote into the workspace, which the host sees.
    assert!(
        eventually(Duration::from_secs(10), || std::fs::read_to_string(
            workspace.join("marker")
        )
        .is_ok_and(|marker| marker == "done"))
        .await,
        "the run never wrote its marker"
    );
    adapter.signal(&run, Signal::Kill).await.expect("killed");
    let ended = tokio::time::timeout(Duration::from_secs(20), adapter.wait(&run))
        .await
        .expect("the run ends after a kill")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    assert_eq!(ended.adapter, AdapterName::Systemd);
    let exec_cgroup = exec_cgroup.expect("exec cgroup");
    let left = std::fs::read_to_string(exec_cgroup.join("cgroup.procs")).unwrap_or_default();
    assert!(left.trim().is_empty(), "left after wait: {left}");
    drop(adapter);
    let reopened = open(linux_inner(host.path())).expect("the scope is free again once dropped");
    drop(reopened);
    println!("ESS confinement systemd linux run: executed");
}

/// Whether `pid` is a live process, not a zombie.
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z'))
    })
}

/// Around `local`, a start or render whose environment names a variable through which a client
/// finds the user manager is refused `exec.environment-refused`, audited, before anything runs:
/// the manager would move a process of the run out of its bounds (security F1, adversary F3).
#[tokio::test(flavor = "multi_thread")]
async fn a_systemd_local_run_naming_the_user_manager_is_refused_before_it_runs() {
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager() {
        println!("ESS confinement systemd user-manager environment: ABSENT ({reason})");
        return;
    }
    let adapter = open(local_inner()).expect("systemd around local opens");
    let workspace = tempfile::tempdir().expect("workspace");
    let records = Arc::new(Mutex::new(Vec::<AuditRecord>::new()));
    let sink = Arc::clone(&records);
    let admitted = adapter
        .admit(&ConfinementProfile {
            inner: Some(InnerAdapter::Local),
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            audit: Some(AuditSink::new(move |record: &AuditRecord| {
                sink.lock().expect("records").push(record.clone());
            })),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let marker = workspace.path().join("ran");
    for name in systemd::USER_MANAGER_ENV {
        let mut command = shell(&format!("touch {}", marker.display()));
        command
            .env
            .insert(name.to_owned(), "unix:path=/nonexistent".to_owned());
        let rendered = adapter
            .render(&admitted, &command)
            .expect_err("render refused");
        assert_eq!(
            rendered.code(),
            "exec.environment-refused",
            "{name}: {rendered}"
        );
        let started = adapter
            .start(&admitted, &command)
            .await
            .expect_err("start refused");
        assert_eq!(
            started.code(),
            "exec.environment-refused",
            "{name}: {started}"
        );
    }
    assert!(!marker.exists(), "a refused run ran");
    let refused = records
        .lock()
        .expect("records")
        .iter()
        .filter(|record| {
            matches!(
                record,
                AuditRecord::ProfileRefused {
                    code: "exec.environment-refused",
                    ..
                }
            )
        })
        .count();
    assert_eq!(refused, systemd::USER_MANAGER_ENV.len());
    println!("ESS confinement systemd user-manager environment: executed");
}

const EMBEDDER_CHILD: &str = "ESS_SYSTEMD_EMBEDDER_CHILD";

/// An embedder that dies leaves nothing of a run past the next open (adversary F4): the run's
/// leader dies with it (`PR_SET_PDEATHSIG`), and a descendant that left the leader's process group
/// is killed, and its scope stopped and counted, by the next process that opens `systemd`.
#[test]
#[allow(clippy::too_many_lines)] // The embedder's half, then each thing its death left.
fn a_dead_embedders_run_is_ended_and_its_scope_reclaimed_by_the_next_open() {
    const NAME: &str = "a_dead_embedders_run_is_ended_and_its_scope_reclaimed_by_the_next_open";
    if std::env::var_os(EMBEDDER_CHILD).is_some() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let adapter = open(local_inner()).expect("systemd around local opens");
            let workspace = tempfile::tempdir().expect("workspace");
            let admitted = adapter
                .admit(&ConfinementProfile {
                    inner: Some(InnerAdapter::Local),
                    local_reason: Some(REASON.into()),
                    workspace: Some(workspace.path().to_path_buf()),
                    process_bounds: Some(ProcessBounds {
                        timeout_ms: 500,
                        processes: 8,
                        memory_bytes: 64 << 20,
                        cpu_millis: 30_000,
                    }),
                    ..ConfinementProfile::default()
                })
                .expect("admitted");
            let run = adapter
                .start(
                    &admitted,
                    &shell(
                        "cat /proc/self/cgroup; echo leader=$$; \
                         setsid sleep 600 </dev/null >/dev/null 2>&1 & echo orphan=$!; \
                         exec sleep 600",
                    ),
                )
                .await
                .expect("started");
            let started = Instant::now();
            loop {
                let stdout = adapter.output(&run).await.expect("output").stdout;
                let stdout = String::from_utf8_lossy(&stdout).into_owned();
                if stdout.contains("orphan=") {
                    print!("{stdout}");
                    break;
                }
                assert!(started.elapsed() < Duration::from_secs(10), "{stdout}");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        // The embedder ends, as a crashed daemon does, before the run's 500 ms timeout.
        std::process::exit(0);
    }
    let _held = OPENING.blocking_lock();
    if let Err(reason) = user_manager() {
        println!("ESS confinement systemd dead embedder: ABSENT ({reason})");
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
        .env(EMBEDDER_CHILD, "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the embedder");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "the embedder failed: {stdout}");
    let value = |key: &str| {
        stdout
            .lines()
            // libtest prints a case's first line after its own `test ... `.
            .find_map(|line| {
                line.split_once(key)
                    .map(|(_, value)| value.trim().to_owned())
            })
            .unwrap_or_else(|| panic!("the embedder printed no {key}: {stdout}"))
    };
    let cgroup = PathBuf::from("/sys/fs/cgroup").join(value("0::").trim_start_matches('/'));
    let scope = cgroup.parent().expect("scope").to_path_buf();
    let leader: u32 = value("leader=").parse().expect("leader pid");
    let orphan: u32 = value("orphan=").parse().expect("orphan pid");
    let ended = |pid: u32| {
        let started = Instant::now();
        while alive(pid) && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        !alive(pid)
    };
    let leader_ended = ended(leader);
    let orphan_left = alive(orphan);
    let before = systemd::reclaimed_scopes();
    let adapter = open(local_inner());
    let reclaimed = systemd::reclaimed_scopes() - before;
    let orphan_ended = ended(orphan);
    // Clean up whatever the adapter left, before asserting.
    let _ = std::fs::write(scope.join("cgroup.kill"), "1");
    let adapter = adapter.expect("systemd around local opens");
    drop(adapter);
    assert!(leader_ended, "the leader {leader} outlived its embedder");
    assert!(
        orphan_left,
        "precondition: the descendant {orphan} outside the leader's group outlived the embedder"
    );
    assert!(
        orphan_ended,
        "the next open left {orphan} alive in {}",
        cgroup.display()
    );
    assert!(reclaimed >= 1, "the next open reclaimed {reclaimed} scopes");
    let stopped = {
        let started = Instant::now();
        while scope.exists() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        !scope.exists()
    };
    assert!(
        stopped,
        "the leftover scope {} is still there",
        scope.display()
    );
    println!("ESS confinement systemd dead embedder: executed ({reclaimed} scopes reclaimed)");
}

// A10, around `local`: the inner adapter's refusal, by the host's name.
#[tokio::test(flavor = "multi_thread")]
async fn systemd_around_local_refuses_a_narrower_workspace() {
    const NAME: &str = "ESS confinement systemd local scoped write";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager() {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let adapter = open(local_inner()).expect("systemd around local opens");
    let workspace = tempfile::tempdir().expect("workspace");
    let error = adapter
        .admit(&ConfinementProfile {
            inner: Some(InnerAdapter::Local),
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            workspace_access: support::scoped_out(),
            ..ConfinementProfile::default()
        })
        .expect_err("refused");
    assert_eq!(
        error.code(),
        substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED,
        "{error}"
    );
    assert_eq!(error.status(), 501);
    println!("{NAME}: executed");
}

// A10, around `linux`: served, with A1's assertions.
#[tokio::test(flavor = "multi_thread")]
async fn systemd_around_linux_serves_a_scoped_workspace() {
    const NAME: &str = "ESS confinement systemd linux scoped write";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager().and_then(|()| bubblewrap()) {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let host = tempfile::tempdir().expect("host root");
    let adapter = open(linux_inner(host.path())).expect("systemd around linux opens");
    let workspace = host.path().join("workspaces").join("ws_scoped");
    support::scoped_write_workspace(&workspace);
    let admitted = adapter
        .admit(&ConfinementProfile {
            inner: Some(InnerAdapter::Linux),
            workspace: Some(workspace.clone()),
            workspace_access: support::scoped_out(),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run = adapter
        .start(&admitted, &shell(support::SCOPED_WRITE_SCRIPT))
        .await
        .expect("started");
    let ended = tokio::time::timeout(Duration::from_secs(30), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    support::assert_scoped_write(NAME, &workspace, &output.stdout);
    drop(adapter);
    println!("{NAME}: executed");
}

// B8: `systemd` around `local` keeps the last bytes under `Tail`.
#[tokio::test(flavor = "multi_thread")]
async fn a_systemd_local_run_under_tail_keeps_the_last_bytes() {
    const NAME: &str = "ESS confinement systemd local retention tail";
    const BOUND: usize = 4096;
    const LAST: u32 = 3000;
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager() {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let adapter = open(local_inner()).expect("systemd around local opens");
    let workspace = tempfile::tempdir().expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            inner: Some(InnerAdapter::Local),
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            io_bounds: Some(confinement::IoBounds {
                output_bytes: BOUND as u64,
            }),
            output_retention: confinement::OutputRetention {
                stdout: confinement::Retention::Tail,
                stderr: confinement::Retention::Head,
            },
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run = adapter
        .start(&admitted, &shell(&format!("seq 1 {LAST}")))
        .await
        .expect("started");
    let ended = tokio::time::timeout(Duration::from_secs(30), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(0));
    let output = adapter.output(&run).await.expect("output");
    support::assert_retained(
        NAME,
        &support::seq_bytes(LAST),
        &output.stdout,
        &output.stdout_kept,
        confinement::Retention::Tail,
        BOUND,
        None,
    );
    assert!(output.stdout_truncated);
    println!("{NAME}: executed");
}

// T13 of design 32, around `local`: the scope's run reads a piped standard input through the
// adapter's own writer.
#[tokio::test(flavor = "multi_thread")]
async fn a_systemd_local_run_reads_its_piped_stdin() {
    const NAME: &str = "ESS confinement systemd local stdin";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager() {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let adapter = open(local_inner()).expect("systemd around local opens");
    let workspace = tempfile::tempdir().expect("workspace");
    support::stdin_echoes_and_ends_on_close(
        &adapter,
        &ConfinementProfile {
            inner: Some(InnerAdapter::Local),
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            stdin: confinement::StdinMode::Piped { limit_bytes: 1_024 },
            ..ConfinementProfile::default()
        },
        &[b"one\n", b"two\n", b"three\n"],
    )
    .await;
    println!("{NAME}: executed");
}

// T13 of design 32, around `linux`: `systemd` forwards the writes to the inner adapter's writer.
#[tokio::test(flavor = "multi_thread")]
async fn a_systemd_linux_run_reads_its_piped_stdin() {
    const NAME: &str = "ESS confinement systemd linux stdin";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager().and_then(|()| bubblewrap()) {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let host = tempfile::tempdir().expect("host root");
    let adapter = open(linux_inner(host.path())).expect("systemd around linux opens");
    let workspace = host.path().join("workspaces").join("ws_scoped");
    std::fs::create_dir_all(&workspace).expect("workspace");
    support::stdin_echoes_and_ends_on_close(
        &adapter,
        &ConfinementProfile {
            inner: Some(InnerAdapter::Linux),
            workspace: Some(workspace),
            stdin: confinement::StdinMode::Piped { limit_bytes: 1_024 },
            ..ConfinementProfile::default()
        },
        &[b"one\n", b"two\n", b"three\n"],
    )
    .await;
    drop(adapter);
    println!("{NAME}: executed");
}

/// What T13's natural end writes: two lines on each stream, interleaved.
const BOTH_STREAMS: &str =
    "printf 'out-one\\n'; printf 'err-one\\n' >&2; printf 'out-two\\n'; printf 'err-two\\n' >&2";

/// The `systemd` profile around `local` in `workspace`, output recorded only.
fn local_recorded(workspace: &Path) -> ConfinementProfile {
    ConfinementProfile {
        inner: Some(InnerAdapter::Local),
        local_reason: Some(REASON.into()),
        workspace: Some(workspace.to_path_buf()),
        ..ConfinementProfile::default()
    }
}

/// The `systemd` profile around `linux` in `workspace`, output recorded only.
fn linux_recorded(workspace: &Path) -> ConfinementProfile {
    ConfinementProfile {
        inner: Some(InnerAdapter::Linux),
        workspace: Some(workspace.to_path_buf()),
        ..ConfinementProfile::default()
    }
}

// T13 of design 32, around `local`: T4.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_systemd_local_run_delivers_its_record() {
    const NAME: &str = "ESS confinement systemd local live natural end";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager() {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let adapter = open(local_inner()).expect("systemd around local opens");
    let workspace = tempfile::tempdir().expect("workspace");
    support::live_natural_end(
        NAME,
        &adapter,
        &local_recorded(workspace.path()),
        &support::sh(BOTH_STREAMS),
        b"out-one\nout-two\n",
        b"err-one\nerr-two\n",
    )
    .await;
    println!("{NAME}: executed");
}

// T13 of design 32, around `local`: T5.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_systemd_local_run_never_read_stalls_and_ends() {
    const NAME: &str = "ESS confinement systemd local live stall";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager() {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let adapter = open(local_inner()).expect("systemd around local opens");
    let workspace = tempfile::tempdir().expect("workspace");
    support::live_stall(
        NAME,
        &adapter,
        &local_recorded(workspace.path()),
        &support::sh("exec yes"),
        1,
    )
    .await;
    println!("{NAME}: executed");
}

// T13 of design 32, around `linux`: T4, read through the wrapper.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_systemd_linux_run_delivers_its_record() {
    const NAME: &str = "ESS confinement systemd linux live natural end";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager().and_then(|()| bubblewrap()) {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let host = tempfile::tempdir().expect("host root");
    let adapter = open(linux_inner(host.path())).expect("systemd around linux opens");
    let workspace = host.path().join("workspaces").join("ws_scoped");
    std::fs::create_dir_all(&workspace).expect("workspace");
    support::live_natural_end(
        NAME,
        &adapter,
        &linux_recorded(&workspace),
        &support::sh(BOTH_STREAMS),
        b"out-one\nout-two\n",
        b"err-one\nerr-two\n",
    )
    .await;
    drop(adapter);
    println!("{NAME}: executed");
}

// T13 of design 32, around `linux`: T5, `output_stalled` read through the wrapper.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_systemd_linux_run_never_read_stalls_and_ends() {
    const NAME: &str = "ESS confinement systemd linux live stall";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager().and_then(|()| bubblewrap()) {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let host = tempfile::tempdir().expect("host root");
    let adapter = open(linux_inner(host.path())).expect("systemd around linux opens");
    let workspace = host.path().join("workspaces").join("ws_scoped");
    std::fs::create_dir_all(&workspace).expect("workspace");
    support::live_stall(
        NAME,
        &adapter,
        &linux_recorded(&workspace),
        &support::sh("exec yes"),
        1,
    )
    .await;
    drop(adapter);
    println!("{NAME}: executed");
}

// ---- adversary pass on `story:adapter-live-output` ----

// § 3: around `local`, a timeout is an imposed end that closes the queue first, not a stall
// (`SystemdLocal::watch` closes it before `kill_tree`).
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_live_systemd_local_run_ended_by_its_timeout_did_not_stall() {
    const NAME: &str = "ESS confinement systemd local adv live timeout";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager() {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let adapter = open(local_inner()).expect("systemd around local opens");
    let workspace = tempfile::tempdir().expect("workspace");
    support::adv_live_timeout_is_not_a_stall(
        NAME,
        &adapter,
        &ConfinementProfile {
            process_bounds: Some(ProcessBounds {
                timeout_ms: 400,
                processes: 8,
                memory_bytes: 64 << 20,
                cpu_millis: 30_000,
            }),
            ..local_recorded(workspace.path())
        },
    )
    .await;
    println!("{NAME}: executed");
}

// § 3: around `local`, `Kill` stops delivery at once; only the queued chunk is delivered.
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_killed_live_systemd_local_run_delivers_only_the_queued_chunk() {
    const NAME: &str = "ESS confinement systemd local adv live kill";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager() {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let adapter = open(local_inner()).expect("systemd around local opens");
    let workspace = tempfile::tempdir().expect("workspace");
    let marker = workspace.path().join("written");
    support::adv_live_kill_delivers_only_the_queued_chunk(
        NAME,
        &adapter,
        &local_recorded(workspace.path()),
        &marker,
        &marker,
    )
    .await;
    println!("{NAME}: executed");
}

// § 3: around `linux`, `Kill` through the wrapper stops delivery at once.
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_killed_live_systemd_linux_run_delivers_only_the_queued_chunk() {
    const NAME: &str = "ESS confinement systemd linux adv live kill";
    let _held = OPENING.lock().await;
    if let Err(reason) = user_manager().and_then(|()| bubblewrap()) {
        println!("{NAME}: ABSENT ({reason})");
        return;
    }
    let host = tempfile::tempdir().expect("host root");
    let adapter = open(linux_inner(host.path())).expect("systemd around linux opens");
    let workspace = host.path().join("workspaces").join("ws_scoped");
    std::fs::create_dir_all(&workspace).expect("workspace");
    support::adv_live_kill_delivers_only_the_queued_chunk(
        NAME,
        &adapter,
        &linux_recorded(&workspace),
        &workspace.join("written"),
        &Path::new(substrate_wire::EXEC_WORKSPACE_MOUNT).join("written"),
    )
    .await;
    drop(adapter);
    println!("{NAME}: executed");
}
