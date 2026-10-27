//! Adversary cases for `story:systemd-scope-bounds-provider` (unit commit b0d11fa).
//!
//! Each case runs its body in a fresh process of this test binary, so the transient scope that
//! opening `systemd` moves its process into never holds the harness, and a case that needs to start
//! inside the operator's delegated cgroup root still is. Without a reachable user manager each case
//! prints `ABSENT` and its reason and asserts nothing.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementAdapter, ConfinementError,
    ConfinementProfile, ContractFamily, InnerAdapter, ProcessBounds, RunState,
    linux::LinuxRequest,
    local::LocalRequest,
    systemd::{InnerRequest, SystemdRequest},
};

const REASON: &str = "the adversary bounds a host process on purpose";
const CHILD: &str = "ADV_SYSTEMD_CHILD";

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
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("systemctl: {error}"))?;
    if !answered.status.success() {
        return Err("systemctl --user does not answer".to_owned());
    }
    Ok(())
}

/// In the harness: runs case `name` alone in a fresh process of this binary and requires it to
/// pass. Returns true there; false in that fresh process, which then runs the body.
fn isolated(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return false;
    }
    let output = child(name);
    assert!(output.status.success(), "{name} failed in its own process");
    true
}

fn child(name: &str) -> std::process::Output {
    let output = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .stdin(Stdio::null())
        .output()
        .expect("a fresh process of this binary");
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    output
}

fn block<F: Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let output =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(future)));
    // A run's blocking waiter may still be parked on a process the adapter did not end; dropping
    // the runtime would wait for it.
    runtime.shutdown_background();
    output.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

fn open_systemd_local() -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Systemd(SystemdRequest::new(
        InnerRequest::Local(LocalRequest::default()),
    )))
    .expect("systemd around local opens with a reachable user manager")
}

fn shell(script: &str, env: BTreeMap<String, String>) -> Command {
    Command {
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env,
    }
}

fn path_env() -> BTreeMap<String, String> {
    BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())])
}

fn bounded(workspace: &Path, bounds: ProcessBounds) -> ConfinementProfile {
    ConfinementProfile {
        // story:systemd-scope-bounds-provider, adversary F1: the contract refuses `systemd`
        // requested without its `inner`, so every admitted profile here names it.
        inner: Some(InnerAdapter::Local),
        local_reason: Some(REASON.into()),
        workspace: Some(workspace.to_path_buf()),
        process_bounds: Some(bounds),
        ..ConfinementProfile::default()
    }
}

fn cgroup_in(output: &[u8]) -> Option<PathBuf> {
    String::from_utf8_lossy(output)
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(|path| PathBuf::from("/sys/fs/cgroup").join(path.trim().trim_start_matches('/')))
}

async fn run_cgroup(adapter: &Arc<dyn ConfinementAdapter>, run: &confinement::RunId) -> PathBuf {
    let started = Instant::now();
    loop {
        let output = adapter.output(run).await.expect("output").stdout;
        if let Some(cgroup) = cgroup_in(&output) {
            return cgroup;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the run never reported its cgroup"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ---- 1. the contract: `systemd` requested without its `inner` ----

/// `spec/confinement/README.md`, `AdmitProfile`: "a requested `inner` that differs from the
/// opened row's, present or absent: ... `systemd` requested without its `inner`
/// (`adapter-unserved`: `confinement.adapter-unserved`, 501)". `confinement.yaml`'s
/// `adapter-unserved` branch: `all: ["defined(inner)", "not defined(input.inner)"]`.
#[test]
fn adv_systemd_profile_without_its_inner_is_refused_adapter_unserved() {
    if isolated("adv_systemd_profile_without_its_inner_is_refused_adapter_unserved") {
        return;
    }
    if let Err(reason) = user_manager() {
        println!("adversary systemd inner-absent: ABSENT ({reason})");
        return;
    }
    let adapter = open_systemd_local();
    let workspace = tempfile::tempdir().expect("workspace");
    let records = Arc::new(Mutex::new(Vec::<AuditRecord>::new()));
    let sink = Arc::clone(&records);
    let admitted = adapter.admit(&ConfinementProfile {
        inner: None,
        local_reason: Some(REASON.into()),
        workspace: Some(workspace.path().to_path_buf()),
        audit: Some(AuditSink::new(move |record: &AuditRecord| {
            sink.lock().push(record.clone());
        })),
        ..ConfinementProfile::default()
    });
    let records = records.lock().clone();
    let error = match admitted {
        Ok(admitted) => panic!(
            "systemd around local admitted a profile requesting no inner; the contract refuses it \
             confinement.adapter-unserved. Audit: {records:?}; admitted inner {:?}",
            admitted.inner()
        ),
        Err(error) => error,
    };
    assert!(
        matches!(error, ConfinementError::AdapterUnserved { .. }),
        "{error}"
    );
    assert!(
        matches!(
            records.as_slice(),
            [AuditRecord::ProfileRefused {
                code: "confinement.adapter-unserved",
                ..
            }]
        ),
        "{records:?}"
    );
}

// ---- 2. a run that makes a cgroup beneath its own ----

/// A run is free to make a cgroup beneath its own (rootless container tooling, a nested
/// delegated-root user does exactly this), and its bounds still hold there. `wait` should return
/// once the tree is gone, and the run's cgroup should be removed like every other run's.
#[test]
fn adv_systemd_local_run_with_a_nested_cgroup_is_waited_promptly_and_removed() {
    if isolated("adv_systemd_local_run_with_a_nested_cgroup_is_waited_promptly_and_removed") {
        return;
    }
    if let Err(reason) = user_manager() {
        println!("adversary systemd nested cgroup: ABSENT ({reason})");
        return;
    }
    block(async {
        let adapter = open_systemd_local();
        let workspace = tempfile::tempdir().expect("workspace");
        let admitted = adapter
            .admit(&bounded(
                workspace.path(),
                ProcessBounds {
                    timeout_ms: 60_000,
                    processes: 16,
                    memory_bytes: 64 << 20,
                    cpu_millis: 30_000,
                },
            ))
            .expect("admitted");
        let run = adapter
            .start(
                &admitted,
                &shell(
                    "cat /proc/self/cgroup; \
                     mkdir \"/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)/nested\"",
                    path_env(),
                ),
            )
            .await
            .expect("started");
        let started = Instant::now();
        let ended = tokio::time::timeout(Duration::from_secs(30), adapter.wait(&run))
            .await
            .expect("wait returns")
            .expect("waited");
        let waited = started.elapsed();
        let cgroup = cgroup_in(&adapter.output(&run).await.expect("output").stdout)
            .expect("the run printed its cgroup");
        let left = cgroup.exists();
        let _ = std::fs::remove_dir(cgroup.join("nested"));
        let _ = std::fs::remove_dir(&cgroup);
        assert_eq!(ended.state, RunState::Ended, "{ended:?}");
        assert_eq!(ended.exit.and_then(|exit| exit.code), Some(0), "{ended:?}");
        assert!(
            waited < Duration::from_secs(3) && !left,
            "a run that exited 0 at once was waited {waited:?}, and its cgroup {} {}",
            cgroup.display(),
            if left {
                "was left behind"
            } else {
                "was removed"
            }
        );
    });
}

// ---- 3. a run whose leader leaves its cgroup through the user manager ----

/// A run started with `XDG_RUNTIME_DIR` in its environment (ordinary for a desktop tool) can ask
/// the same user manager for a scope of its own, as every tool that today wraps itself in
/// `systemd-run --user --scope` does. The run's timeout is 500 ms.
///
/// story:systemd-scope-bounds-provider, adversary F3: such a start is refused
/// `exec.environment-refused`, which closes the path the case used; the case asserts that, then
/// takes the remaining path (the run sets `XDG_RUNTIME_DIR` itself) and still requires `wait` to
/// return within 8 s.
#[test]
fn adv_systemd_local_run_ends_at_its_timeout_when_it_calls_systemd_run() {
    if isolated("adv_systemd_local_run_ends_at_its_timeout_when_it_calls_systemd_run") {
        return;
    }
    if let Err(reason) = user_manager() {
        println!("adversary systemd systemd-run escape: ABSENT ({reason})");
        return;
    }
    let Some(runtime_dir) = std::env::var("XDG_RUNTIME_DIR").ok() else {
        println!("adversary systemd systemd-run escape: ABSENT (XDG_RUNTIME_DIR is unset)");
        return;
    };
    block(async {
        let adapter = open_systemd_local();
        let workspace = tempfile::tempdir().expect("workspace");
        let admitted = adapter
            .admit(&bounded(
                workspace.path(),
                ProcessBounds {
                    timeout_ms: 500,
                    processes: 16,
                    memory_bytes: 64 << 20,
                    cpu_millis: 30_000,
                },
            ))
            .expect("admitted");
        let mut env = path_env();
        env.insert("XDG_RUNTIME_DIR".into(), runtime_dir.clone());
        // story:systemd-scope-bounds-provider, adversary F3: the variable is refused at start.
        let refused = adapter
            .start(
                &admitted,
                &shell("systemd-run --user --scope --quiet sleep 20", env),
            )
            .await
            .expect_err("a start naming XDG_RUNTIME_DIR is refused");
        assert_eq!(refused.code(), "exec.environment-refused", "{refused}");
        let run = adapter
            .start(
                &admitted,
                &shell(
                    &format!(
                        "XDG_RUNTIME_DIR={runtime_dir} systemd-run --user --scope --quiet sleep 20"
                    ),
                    path_env(),
                ),
            )
            .await
            .expect("started");
        let started = Instant::now();
        let waited = tokio::time::timeout(Duration::from_secs(8), adapter.wait(&run)).await;
        let observed = adapter.observe(&run).await.expect("observed");
        assert!(
            waited.is_ok(),
            "a run with a 500 ms timeout was still {:?} after {:?}: wait did not return",
            observed.state,
            started.elapsed()
        );
    });
}

// ---- 4. a run outlives its embedder ----

/// The `linux` driver ties a run to its embedder (`--die-with-parent`); here the timeout is
/// enforced only by a task inside the embedder. The embedder starts a run with a 500 ms timeout
/// and exits; three seconds later the run should be gone.
#[test]
fn adv_systemd_local_run_does_not_outlive_its_timeout_after_the_embedder_exits() {
    const NAME: &str =
        "adv_systemd_local_run_does_not_outlive_its_timeout_after_the_embedder_exits";
    if std::env::var_os(CHILD).is_some() {
        if user_manager().is_err() {
            return;
        }
        block(async {
            let adapter = open_systemd_local();
            let workspace = tempfile::tempdir().expect("workspace");
            let admitted = adapter
                .admit(&bounded(
                    workspace.path(),
                    ProcessBounds {
                        timeout_ms: 500,
                        processes: 8,
                        memory_bytes: 64 << 20,
                        cpu_millis: 30_000,
                    },
                ))
                .expect("admitted");
            let run = adapter
                .start(
                    &admitted,
                    &shell("cat /proc/self/cgroup; exec sleep 600", path_env()),
                )
                .await
                .expect("started");
            let cgroup = run_cgroup(&adapter, &run).await;
            println!("ADV-CGROUP={}", cgroup.display());
        });
        // The embedder ends, as a crashed or restarted daemon does.
        std::process::exit(0);
    }
    if let Err(reason) = user_manager() {
        println!("adversary systemd embedder exit: ABSENT ({reason})");
        return;
    }
    let output = child(NAME);
    assert!(output.status.success(), "the embedder failed");
    let cgroup = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.split_once("ADV-CGROUP=").map(|(_, path)| path.trim()))
        .map(PathBuf::from)
        .expect("the embedder reported its run's cgroup");
    std::thread::sleep(Duration::from_secs(3));
    let left = std::fs::read_to_string(cgroup.join("cgroup.procs")).unwrap_or_default();
    // Clean up what the adapter left, before asserting.
    let _ = std::fs::write(cgroup.join("cgroup.kill"), "1");
    std::thread::sleep(Duration::from_millis(300));
    let _ = std::fs::remove_dir(&cgroup);
    assert!(
        left.trim().is_empty(),
        "3 s after its embedder exited, a run with a 500 ms timeout still has processes in {}: {}",
        cgroup.display(),
        left.trim()
    );
}

// ---- 5. a `linux` adapter with the operator's delegated root, in the same process ----

fn delegated_root() -> Result<PathBuf, &'static str> {
    let root = PathBuf::from(
        std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")
            .ok_or("SUBSTRATE_VECTORS_CGROUP_ROOT is unset")?,
    );
    if !Path::new("/usr/bin/bwrap").is_file() {
        return Err("no bubblewrap at /usr/bin/bwrap");
    }
    Ok(root)
}

fn open_linux(host: &Path, root: &Path) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(HostConfig::minimum(host.join("workspaces")))
            .with_cgroup_root(Some(root.to_path_buf())),
    )))
}

/// An embedder that runs `linux` with its operator-delegated root (what loom, takt, conductor and
/// `scripts/delegated-lane.sh` do today) and also opens `systemd` around `local`.
///
/// story:systemd-scope-bounds-provider, adversary F5: opening `systemd` moves the embedder out of
/// that root, and the coordinator kept that as a named refusal: the `linux` adapter opened
/// afterwards refuses a profile that needs `limits` `confinement.family-unserved` (the module
/// documents `systemd{inner: linux}` for such an embedder), and never admits it unbounded.
#[test]
fn adv_linux_opened_after_systemd_refuses_a_profile_needing_limits_by_name() {
    if isolated("adv_linux_opened_after_systemd_refuses_a_profile_needing_limits_by_name") {
        return;
    }
    let root = match user_manager()
        .map_err(|_| "no user manager")
        .and_then(|()| delegated_root())
    {
        Ok(root) => root,
        Err(reason) => {
            println!("adversary systemd then linux: ABSENT ({reason})");
            return;
        }
    };
    let before = tempfile::tempdir().expect("host");
    let linux = open_linux(before.path(), &root).expect("linux opens");
    assert!(
        linux.facts().serves(&ContractFamily::Limits),
        "precondition: linux proves limits in {} before systemd opens: {:?}",
        root.display(),
        linux.facts()
    );
    drop(linux);
    let _systemd = open_systemd_local();
    let after = tempfile::tempdir().expect("host");
    // story:systemd-scope-bounds-provider, adversary F5: a named refusal, not an unbounded run.
    let linux = open_linux(after.path(), &root).expect("linux opens");
    let workspace = after.path().join("workspaces").join("ws_adv");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let error = linux
        .admit(&ConfinementProfile {
            families: vec![ContractFamily::Limits],
            workspace: Some(workspace),
            process_bounds: Some(ProcessBounds {
                timeout_ms: 30_000,
                processes: 16,
                memory_bytes: 128 << 20,
                cpu_millis: 10_000,
            }),
            ..ConfinementProfile::default()
        })
        .err()
        .unwrap_or_else(|| {
            panic!(
                "after systemd opened, linux with the delegated root {} admitted a profile \
                 needing limits: {:?}",
                root.display(),
                linux.facts()
            )
        });
    assert!(
        matches!(
            error,
            ConfinementError::FamilyUnserved {
                family: ContractFamily::Limits,
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(error.code(), "confinement.family-unserved");
}

/// The same embedder, with its `linux` adapter already open when `systemd` moves the process.
#[test]
fn adv_an_open_linux_adapter_still_runs_after_systemd_opens() {
    if isolated("adv_an_open_linux_adapter_still_runs_after_systemd_opens") {
        return;
    }
    let root = match user_manager()
        .map_err(|_| "no user manager")
        .and_then(|()| delegated_root())
    {
        Ok(root) => root,
        Err(reason) => {
            println!("adversary linux then systemd: ABSENT ({reason})");
            return;
        }
    };
    block(async {
        let host = tempfile::tempdir().expect("host");
        let linux = open_linux(host.path(), &root).expect("linux opens");
        assert!(
            linux.facts().serves(&ContractFamily::Limits),
            "precondition: {:?}",
            linux.facts()
        );
        let _systemd = open_systemd_local();
        let workspace = host.path().join("workspaces").join("ws_adv");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let admitted = linux
            .admit(&ConfinementProfile {
                workspace: Some(workspace),
                process_bounds: Some(ProcessBounds {
                    timeout_ms: 30_000,
                    processes: 16,
                    memory_bytes: 128 << 20,
                    cpu_millis: 10_000,
                }),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let run = linux
            .start(&admitted, &shell("exit 3", path_env()))
            .await
            .expect("an open linux adapter starts a run after systemd moved its embedder");
        let ended = tokio::time::timeout(Duration::from_secs(20), linux.wait(&run))
            .await
            .expect("wait returns")
            .expect("waited");
        assert_eq!(ended.state, RunState::Ended, "{ended:?}");
        assert_eq!(ended.exit.and_then(|exit| exit.code), Some(3), "{ended:?}");
    });
}

// ---- 7. a backgrounded grandchild holding the run's output when the leader exits ----

/// No case of the unit lets the leader exit on its own while its tree lives: the kill at the
/// leader's exit (`systemd.rs`, "Nothing of the tree outlives its leader") is what ends a
/// background child that holds the output pipes; without it `wait` blocks on that child.
#[test]
fn adv_systemd_local_background_child_ends_with_its_leader() {
    if isolated("adv_systemd_local_background_child_ends_with_its_leader") {
        return;
    }
    if let Err(reason) = user_manager() {
        println!("adversary systemd background child: ABSENT ({reason})");
        return;
    }
    block(async {
        let adapter = open_systemd_local();
        let workspace = tempfile::tempdir().expect("workspace");
        let admitted = adapter
            .admit(&bounded(
                workspace.path(),
                ProcessBounds {
                    timeout_ms: 60_000,
                    processes: 16,
                    memory_bytes: 64 << 20,
                    cpu_millis: 30_000,
                },
            ))
            .expect("admitted");
        let run = adapter
            .start(
                &admitted,
                &shell(
                    "cat /proc/self/cgroup; setsid sleep 600 & exit 4",
                    path_env(),
                ),
            )
            .await
            .expect("started");
        let ended = tokio::time::timeout(Duration::from_secs(8), adapter.wait(&run))
            .await
            .expect("wait returns once the leader exited")
            .expect("waited");
        let cgroup = cgroup_in(&adapter.output(&run).await.expect("output").stdout)
            .expect("the run printed its cgroup");
        assert_eq!(ended.state, RunState::Ended, "{ended:?}");
        assert_eq!(ended.exit.and_then(|exit| exit.code), Some(4), "{ended:?}");
        assert!(!cgroup.exists(), "{} was left", cgroup.display());
    });
}

// ---- 6. the memory bound ends the whole tree (memory.oom.group) ----

/// No case of the unit observes `memory.max` or `memory.oom.group`: dropping the `oom.group`
/// write leaves the suite green. With it, an out-of-memory run ends whole; without it the kernel
/// kills the largest process and the rest (here the shell waiting on a sleeper) carries on. Set
/// `ADV_SIMULATE_NO_OOM_GROUP=1` to express that mutation inside the case and watch it go red.
#[test]
fn adv_systemd_local_memory_bound_ends_the_whole_tree() {
    if isolated("adv_systemd_local_memory_bound_ends_the_whole_tree") {
        return;
    }
    if let Err(reason) = user_manager() {
        println!("adversary systemd memory bound: ABSENT ({reason})");
        return;
    }
    let mutate = std::env::var_os("ADV_SIMULATE_NO_OOM_GROUP").is_some();
    block(async {
        let adapter = open_systemd_local();
        let workspace = tempfile::tempdir().expect("workspace");
        let admitted = adapter
            .admit(&bounded(
                workspace.path(),
                ProcessBounds {
                    timeout_ms: 60_000,
                    processes: 16,
                    memory_bytes: 64 << 20,
                    cpu_millis: 60_000,
                },
            ))
            .expect("admitted");
        let prefix = if mutate {
            "echo 0 > \"/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)/memory.oom.group\"; "
        } else {
            ""
        };
        let run = adapter
            .start(
                &admitted,
                &shell(
                    &format!("{prefix}sleep 600 & tail /dev/zero; wait"),
                    path_env(),
                ),
            )
            .await
            .expect("started");
        let started = Instant::now();
        let ended = tokio::time::timeout(Duration::from_secs(20), adapter.wait(&run))
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "a run past its 64 MiB memory bound was still running after {:?}",
                    started.elapsed()
                )
            })
            .expect("waited");
        assert_eq!(
            ended.exit.and_then(|exit| exit.signal),
            Some(9),
            "{ended:?}"
        );
    });
}
