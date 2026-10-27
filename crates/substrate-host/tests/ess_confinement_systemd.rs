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
use std::sync::Arc;
use std::time::{Duration, Instant};

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, Command, ConfinementAdapter, ConfinementError,
    ConfinementProfile, ContractFamily, Fact, InnerAdapter, ProcessBounds, RunState,
    linux::LinuxRequest,
    local::LocalRequest,
    systemd::{InnerRequest, SystemdRequest},
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
