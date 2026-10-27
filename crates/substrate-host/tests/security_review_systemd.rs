//! Security review of `story:systemd-scope-bounds-provider` (unit commit b0d11fa).
//!
//! Defensive conformance checks of the `systemd` adapter's own claims: that the host's `local`
//! policy and the declared-insecure posture survive the wrapper, that its refusals are audited,
//! that the `linux` isolation floor is unchanged inside the scope (no user bus, no cgroupfs, no
//! nested user namespace, no network), that `whole_tree_kill` holds for a tree that does nothing
//! hostile, that opening the adapter leaves the embedder inside the bounds it was started under,
//! and that the bus address is only ever an address.
//!
//! Every case needs a reachable user manager; without one it prints `ABSENT` and executes nothing.
//! Cases that must observe a fresh process re-run this binary as a child, selected by `--exact`.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementError,
    ConfinementProfile, Fact, InnerAdapter, ProcessBounds, RunState,
    linux::LinuxRequest,
    local::{LocalPolicy, LocalRequest},
    systemd::{InnerRequest, SystemdRequest},
};
use substrate_wire::Signal;

const REASON: &str = "the security review bounds a host process on purpose";
const CHILD: &str = "SUBSTRATE_SECURITY_REVIEW_SYSTEMD_CHILD";
const BOUNDED_TASKS: &str = "4242";

/// Cases that open `systemd` share this process's scope; one at a time.
static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn user_manager() -> Result<OsString, String> {
    let Some(bus) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").filter(|bus| !bus.is_empty())
    else {
        return Err("DBUS_SESSION_BUS_ADDRESS is unset".to_owned());
    };
    let answered = std::process::Command::new("/usr/bin/systemctl")
        .args(["--user", "show", "--property=Version", "--value"])
        .env_clear()
        .env("DBUS_SESSION_BUS_ADDRESS", &bus)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("systemctl: {error}"))?;
    if !answered.status.success() {
        return Err("systemctl --user does not answer".to_owned());
    }
    Ok(bus)
}

fn this_cgroup() -> PathBuf {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").expect("own cgroup");
    let path = cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .expect("a cgroup v2 membership");
    PathBuf::from("/sys/fs/cgroup").join(path.trim_start_matches('/'))
}

fn records() -> (AuditSink, Arc<Mutex<Vec<AuditRecord>>>) {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&kept);
    (
        AuditSink::new(move |record| into.lock().expect("records").push(record.clone())),
        kept,
    )
}

fn shell(script: &str, env: &[(&str, &str)]) -> Command {
    let mut set = BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
    for (name, value) in env {
        set.insert((*name).to_owned(), (*value).to_owned());
    }
    Command {
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env: set,
    }
}

/// Runs one case of this binary in a fresh process, optionally under `wrapper`, and returns its
/// output after checking the case was selected.
fn rerun(case: &str, env: &[(&str, &str)], wrapper: &[&str]) -> std::process::Output {
    let exe = std::env::current_exe().expect("this test binary");
    let mut argv: Vec<OsString> = wrapper.iter().map(OsString::from).collect();
    argv.push("/usr/bin/env".into());
    argv.push(format!("{CHILD}=1").into());
    for (name, value) in env {
        argv.push(format!("{name}={value}").into());
    }
    argv.push(exe.into_os_string());
    for arg in ["--exact", case, "--nocapture", "--test-threads=1"] {
        argv.push(arg.into());
    }
    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("re-run this binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("running 1 test"),
        "the child selected no case {case}: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn assert_child_passed(case: &str, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{case} failed in its child process:\n--- stdout\n{}\n--- stderr\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---- invariant 3: the local policy, the reason and the audit records survive the wrapper ----

#[test]
fn security_review_systemd_local_keeps_the_local_policy_reason_and_audit() {
    let _held = OPENING.blocking_lock();
    let bus = match user_manager() {
        Ok(bus) => bus,
        Err(reason) => {
            println!("security review systemd policy: ABSENT ({reason})");
            return;
        }
    };
    let workspace = tempfile::tempdir().expect("workspace");
    let profile = |reason: Option<&str>, audit: AuditSink| ConfinementProfile {
        // story:systemd-scope-bounds-provider, adversary F1: the contract refuses `systemd`
        // requested without its `inner`.
        inner: Some(InnerAdapter::Local),
        local_reason: reason.map(str::to_owned),
        workspace: Some(workspace.path().to_path_buf()),
        audit: Some(audit),
        ..ConfinementProfile::default()
    };

    // An embedder that refuses local refuses it inside systemd too.
    let refused = confinement::open(AdapterRequest::Systemd(SystemdRequest {
        inner: InnerRequest::Local(LocalRequest {
            policy: LocalPolicy::Refuse,
        }),
        session_bus: Some(bus.clone()),
    }))
    .expect("systemd around a refused local opens; admission refuses");
    assert!(refused.facts().proves(&Fact::DeclaredInsecure));
    assert!(!refused.facts().proves(&Fact::LocalPermitted));
    let (sink, kept) = records();
    let error = refused
        .admit(&profile(Some(REASON), sink))
        .expect_err("local-refused");
    assert_eq!(error.code(), "confinement.local-refused", "{error}");
    assert_eq!(error.status(), 403);
    assert_eq!(
        *kept.lock().expect("records"),
        [AuditRecord::ProfileRefused {
            adapter: AdapterName::Systemd,
            inner: Some(InnerAdapter::Local),
            code: "confinement.local-refused",
            family: None,
            fact: None,
            field: None,
        }]
    );

    // A permitted one still needs a reason, and a refused bound is audited by name.
    let permitted = confinement::open(AdapterRequest::Systemd(SystemdRequest {
        inner: InnerRequest::Local(LocalRequest::default()),
        session_bus: Some(bus),
    }))
    .expect("systemd around local opens");
    let (sink, kept) = records();
    let error = permitted
        .admit(&profile(None, sink))
        .expect_err("reason missing");
    assert_eq!(error.code(), "confinement.local-reason-missing", "{error}");
    assert_eq!(kept.lock().expect("records").len(), 1);
    let (sink, kept) = records();
    let mut unbounded = profile(Some(REASON), sink);
    unbounded.process_bounds = Some(ProcessBounds {
        timeout_ms: 1_000,
        processes: 0,
        memory_bytes: 64 << 20,
        cpu_millis: 1_000,
    });
    let error = permitted.admit(&unbounded).expect_err("limit-unserved");
    assert_eq!(error.code(), "exec.limit-unserved", "{error}");
    assert!(
        matches!(
            kept.lock().expect("records").as_slice(),
            [AuditRecord::ProfileRefused {
                code: "exec.limit-unserved",
                ..
            }]
        ),
        "{:?}",
        kept.lock().expect("records")
    );
    println!("security review systemd policy: executed");
}

/// The host's own policy variable, read at open, forbids `local` inside `systemd` as well.
#[test]
fn security_review_systemd_local_honours_the_host_policy_variable() {
    if std::env::var_os(CHILD).is_some() {
        let bus = user_manager().expect("the parent checked the manager");
        let adapter = confinement::open(AdapterRequest::Systemd(SystemdRequest {
            inner: InnerRequest::Local(LocalRequest::default()),
            session_bus: Some(bus),
        }))
        .expect("opens");
        assert!(!adapter.facts().proves(&Fact::LocalPermitted));
        let workspace = tempfile::tempdir().expect("workspace");
        let error = adapter
            .admit(&ConfinementProfile {
                // story:systemd-scope-bounds-provider, adversary F1: `inner` is named.
                inner: Some(InnerAdapter::Local),
                local_reason: Some(REASON.into()),
                workspace: Some(workspace.path().to_path_buf()),
                ..ConfinementProfile::default()
            })
            .expect_err("local-refused");
        assert_eq!(error.code(), "confinement.local-refused", "{error}");
        return;
    }
    if let Err(reason) = user_manager() {
        println!("security review systemd host policy: ABSENT ({reason})");
        return;
    }
    let case = "security_review_systemd_local_honours_the_host_policy_variable";
    let output = rerun(case, &[("SUBSTRATE_CONFINEMENT_LOCAL", "deny")], &[]);
    assert_child_passed(case, &output);
    println!("security review systemd host policy: executed");
}

// ---- the linux floor inside the scope ----

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One probe, run twice, then each line it reported.
async fn security_review_systemd_linux_run_reaches_no_user_manager_and_keeps_the_floor() {
    let _held = OPENING.lock().await;
    let bus = match user_manager() {
        Ok(bus) => bus,
        Err(reason) => {
            println!("security review systemd linux floor: ABSENT ({reason})");
            return;
        }
    };
    if !Path::new("/usr/bin/bwrap").is_file() {
        println!("security review systemd linux floor: ABSENT (no bubblewrap)");
        return;
    }
    let host = tempfile::tempdir().expect("host root");
    let adapter = confinement::open(AdapterRequest::Systemd(SystemdRequest {
        inner: InnerRequest::Linux(Box::new(LinuxRequest::new(HostConfig::minimum(
            host.path().join("workspaces"),
        )))),
        session_bus: Some(bus.clone()),
    }))
    .expect("systemd around linux opens");
    let workspace = host.path().join("workspaces").join("ws_review");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            inner: Some(InnerAdapter::Linux),
            workspace: Some(workspace.clone()),
            process_bounds: Some(ProcessBounds {
                timeout_ms: 30_000,
                processes: 16,
                memory_bytes: 128 << 20,
                cpu_millis: 10_000,
            }),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let uid = unsafe { libc::getuid() }.to_string();
    let runtime_dir = format!("/run/user/{uid}");
    let probe = "\
        echo ENV-BEGIN; /usr/bin/env; echo ENV-END; \
        if [ -e /run/user ]; then echo RUN_USER_VISIBLE; else echo run-user-absent; fi; \
        if [ -e /sys/fs/cgroup/cgroup.procs ]; then echo CGROUPFS_VISIBLE; else echo cgroupfs-absent; fi; \
        if /usr/bin/busctl --user call org.freedesktop.systemd1 /org/freedesktop/systemd1 \
             org.freedesktop.DBus.Peer Ping >/dev/null 2>&1; then echo USER_BUS_REACHED; \
           else echo user-bus-unreached; fi; \
        if /usr/bin/systemd-run --user --scope --quiet /bin/true >/dev/null 2>&1; \
           then echo SCOPE_CREATED; else echo scope-refused; fi; \
        if /usr/bin/unshare -U /bin/true 2>/dev/null; then echo NESTED_USERNS; \
           else echo nested-userns-refused; fi; \
        echo NET $(tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' ' | tr '\\n' ' ')";
    let bus_text = bus.to_string_lossy().into_owned();
    // A client that passes its own session's bus and runtime directory, and one that passes none.
    for env in [
        vec![
            ("DBUS_SESSION_BUS_ADDRESS", bus_text.as_str()),
            ("XDG_RUNTIME_DIR", runtime_dir.as_str()),
        ],
        Vec::new(),
    ] {
        let run = adapter
            .start(&admitted, &shell(probe, &env))
            .await
            .expect("started");
        let ended = tokio::time::timeout(Duration::from_secs(30), adapter.wait(&run))
            .await
            .expect("the probe ends")
            .expect("waited");
        assert_eq!(ended.state, RunState::Ended);
        let stdout = String::from_utf8_lossy(&adapter.output(&run).await.expect("output").stdout)
            .into_owned();
        println!(
            "probe with client env {:?}:\n{stdout}",
            env.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        for line in [
            "run-user-absent",
            "cgroupfs-absent",
            "user-bus-unreached",
            "scope-refused",
            "nested-userns-refused",
            "NET lo",
        ] {
            assert!(
                stdout.lines().any(|have| have == line),
                "a confined linux run inside the systemd scope did not report {line:?}: {stdout}"
            );
        }
        let environment = stdout
            .split("ENV-BEGIN")
            .nth(1)
            .and_then(|rest| rest.split("ENV-END").next())
            .expect("env block");
        for name in [
            "INVOCATION_ID",
            "JOURNAL_STREAM",
            "SUBSTRATE_CONFINEMENT_LOCAL",
        ] {
            assert!(
                !environment
                    .lines()
                    .any(|line| line.starts_with(&format!("{name}="))),
                "{name} reached a confined run: {environment}"
            );
        }
        if env.is_empty() {
            for name in ["DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR"] {
                assert!(
                    !environment
                        .lines()
                        .any(|line| line.starts_with(&format!("{name}="))),
                    "{name} reached a confined run its client did not give it: {environment}"
                );
            }
        }
    }
    println!("security review systemd linux floor: executed");
}

// ---- whole_tree_kill around local, for a tree that works against nothing ----

struct StopUnit {
    unit: String,
    bus: OsString,
}

impl Drop for StopUnit {
    fn drop(&mut self) {
        let _ = std::process::Command::new("/usr/bin/systemctl")
            .args(["--user", "kill", "--signal=KILL", &self.unit])
            .env_clear()
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus)
            .output();
        let _ = std::process::Command::new("/usr/bin/systemctl")
            .args(["--user", "stop", &self.unit])
            .env_clear()
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus)
            .output();
    }
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z'))
    })
}

/// The adapter proves `whole_tree_kill` around `local`. A run whose client passed it its session
/// (as a local developer's tool does) and that starts a helper the ordinary way a user's tools do,
/// `systemd-run --user --scope`, has a descendant the user manager moved out of the run's cgroup:
/// `cgroup.kill` no longer reaches it, and it outlives `KILL` and `wait`.
///
/// story:systemd-scope-bounds-provider, security F1: a start whose environment names the session
/// bus or the runtime directory is refused `exec.environment-refused`, which closes the path the
/// case used; the case asserts that, then lets the run name the bus itself, the path that remains,
/// and still requires the helper, which stayed in the leader's process group, dead after `KILL`
/// and `wait`.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // The two refusals, then one run and each thing it left.
async fn security_review_systemd_local_whole_tree_kill_reaches_a_descendant_started_through_the_user_manager()
 {
    let _held = OPENING.lock().await;
    let bus = match user_manager() {
        Ok(bus) => bus,
        Err(reason) => {
            println!("security review systemd local tree kill: ABSENT ({reason})");
            return;
        }
    };
    let adapter = confinement::open(AdapterRequest::Systemd(SystemdRequest {
        inner: InnerRequest::Local(LocalRequest::default()),
        session_bus: Some(bus.clone()),
    }))
    .expect("systemd around local opens");
    assert!(adapter.facts().proves(&Fact::WholeTreeKill));
    let workspace = tempfile::tempdir().expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            // story:systemd-scope-bounds-provider, adversary F1: `inner` is named.
            inner: Some(InnerAdapter::Local),
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            facts: vec![Fact::WholeTreeKill],
            process_bounds: Some(ProcessBounds {
                timeout_ms: 60_000,
                processes: 16,
                memory_bytes: 64 << 20,
                cpu_millis: 30_000,
            }),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let unit = format!(
        "security-review-{}.scope",
        ulid::Ulid::generate().to_string().to_ascii_lowercase()
    );
    let _stop = StopUnit {
        unit: unit.clone(),
        bus: bus.clone(),
    };
    let bus_text = bus.to_string_lossy().into_owned();
    let script = format!(
        "/usr/bin/systemd-run --user --scope --quiet --unit={unit} /bin/sleep 300 \
         </dev/null >/dev/null 2>&1 & echo helper=$!; wait"
    );
    // story:systemd-scope-bounds-provider, security F1: the client's session is refused.
    let runtime_dir = format!("/run/user/{}", unsafe { libc::getuid() });
    for env in [
        ("DBUS_SESSION_BUS_ADDRESS", bus_text.as_str()),
        ("XDG_RUNTIME_DIR", runtime_dir.as_str()),
    ] {
        let refused = adapter
            .start(&admitted, &shell(&script, &[env]))
            .await
            .err()
            .unwrap_or_else(|| panic!("a start naming {} was not refused", env.0));
        assert_eq!(
            refused.code(),
            "exec.environment-refused",
            "{}: {refused}",
            env.0
        );
    }
    let script =
        format!("DBUS_SESSION_BUS_ADDRESS='{bus_text}'; export DBUS_SESSION_BUS_ADDRESS; {script}");
    let run = adapter
        .start(&admitted, &shell(&script, &[]))
        .await
        .expect("started");
    let started = Instant::now();
    let helper: u32 = loop {
        let stdout = adapter.output(&run).await.expect("output").stdout;
        if let Some(pid) = String::from_utf8_lossy(&stdout)
            .lines()
            .find_map(|line| line.strip_prefix("helper="))
            .and_then(|pid| pid.trim().parse().ok())
        {
            break pid;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "no helper pid");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    // Precondition: the helper is a descendant of the run and now sits in its own unit.
    let moved = {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let cgroup =
                std::fs::read_to_string(format!("/proc/{helper}/cgroup")).unwrap_or_default();
            if cgroup.contains(&unit) {
                break cgroup;
            }
            assert!(
                Instant::now() < deadline,
                "precondition: the helper {helper} never reached {unit}: {cgroup}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    adapter.signal(&run, Signal::Kill).await.expect("killed");
    let ended = tokio::time::timeout(Duration::from_secs(20), adapter.wait(&run))
        .await
        .expect("the run ends after a kill")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !alive(helper),
        "whole_tree_kill: process {helper}, started by the run, is alive after KILL and wait, in \
         {} (the run's tree left its cgroup through the user manager the scope is made by)",
        moved.trim()
    );
    println!("security review systemd local tree kill: executed");
}

// ---- the embedder's own posture ----

fn ancestor_tasks_bounds(cgroup: &Path) -> Vec<(PathBuf, String)> {
    cgroup
        .ancestors()
        .take_while(|path| {
            path.starts_with("/sys/fs/cgroup") && *path != Path::new("/sys/fs/cgroup")
        })
        .filter_map(|path| {
            std::fs::read_to_string(path.join("pids.max"))
                .ok()
                .map(|value| (path.to_path_buf(), value.trim().to_owned()))
        })
        .collect()
}

/// An embedder started under a bounded unit (a user service or scope with `TasksMax=`) is still
/// under that bound after it opens `systemd`: the adapter must not move its embedder out of the
/// bounds an operator placed it in.
#[test]
fn security_review_opening_systemd_keeps_the_embedder_inside_the_bounds_it_was_started_under() {
    let case =
        "security_review_opening_systemd_keeps_the_embedder_inside_the_bounds_it_was_started_under";
    if std::env::var_os(CHILD).is_some() {
        let before = this_cgroup();
        let bounds_before = ancestor_tasks_bounds(&before);
        assert!(
            bounds_before
                .iter()
                .any(|(_, value)| value == BOUNDED_TASKS),
            "precondition: started under TasksMax={BOUNDED_TASKS}: {bounds_before:?}"
        );
        let bus = user_manager().expect("the parent checked the manager");
        let adapter = confinement::open(AdapterRequest::Systemd(SystemdRequest {
            inner: InnerRequest::Local(LocalRequest::default()),
            session_bus: Some(bus),
        }))
        .expect("systemd around local opens");
        let after = this_cgroup();
        let bounds_after = ancestor_tasks_bounds(&after);
        drop(adapter);
        assert!(
            bounds_after.iter().any(|(_, value)| value == BOUNDED_TASKS),
            "opening systemd moved the embedder out of the TasksMax={BOUNDED_TASKS} unit it was \
             started under: before {} {bounds_before:?}, after {} {bounds_after:?}",
            before.display(),
            after.display()
        );
        return;
    }
    if let Err(reason) = user_manager() {
        println!("security review systemd embedder bounds: ABSENT ({reason})");
        return;
    }
    let unit = format!(
        "--unit=security-review-embedder-{}.scope",
        ulid::Ulid::generate().to_string().to_ascii_lowercase()
    );
    let property = format!("TasksMax={BOUNDED_TASKS}");
    let output = rerun(
        case,
        &[],
        &[
            "/usr/bin/systemd-run",
            "--user",
            "--scope",
            "--quiet",
            &unit,
            "-p",
            &property,
            "--",
        ],
    );
    assert_child_passed(case, &output);
    println!("security review systemd embedder bounds: executed");
}

// ---- the bus address is an address ----

fn percent(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_/.\\*".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02x}")
            }
        })
        .collect()
}

/// A session bus value is the address of the user manager's bus. D-Bus also has an address form
/// that names a program to run (`unixexec:`); `systemd` passes the value to `systemctl` and
/// `busctl`, so an address of that form would make opening the adapter execute that program. The
/// adapter refuses it before anything runs.
#[test]
fn security_review_systemd_session_bus_names_a_bus_and_never_a_program() {
    let marker_dir = tempfile::tempdir().expect("marker directory");
    let marker = marker_dir.path().join("executed");
    let address = format!(
        "unixexec:path=/usr/bin/touch,argv0=touch,argv1={}",
        percent(&marker.to_string_lossy())
    );
    let error = confinement::open(AdapterRequest::Systemd(SystemdRequest {
        inner: InnerRequest::Local(LocalRequest::default()),
        session_bus: Some(address.clone().into()),
    }))
    .err()
    .expect("no user manager answers at a program");
    assert!(
        matches!(error, ConfinementError::AdapterUnserved { .. }),
        "{error}"
    );
    assert!(
        !marker.exists(),
        "opening systemd with session bus {address:?} executed /usr/bin/touch, which created {}",
        marker.display()
    );
}
