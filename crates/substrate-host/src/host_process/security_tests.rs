//! Security review cases for the host-process driver (ADR 0036, design 26 § S2, S3, S7).
//!
//! Each case checks the driver against a sentence of the decision it implements. The delegated
//! cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` (`scripts/delegated-lane.sh`); without it they
//! return before asserting anything: absent, never reported as passed (invariant 3).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::{
    EndedBy, HostProcessSpec, HostProcessSupervisor, LogStreamSelection, ProcessExit, StopCause,
};

fn spec(argv: &[&str], cwd: &Path) -> HostProcessSpec {
    HostProcessSpec {
        argv: argv.iter().map(|item| (*item).to_owned()).collect(),
        cwd: cwd.to_path_buf(),
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        deadline: None,
    }
}

fn delegated(name: &str) -> Option<(Arc<HostProcessSupervisor>, PathBuf, tempfile::TempDir)> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    let path = root.join(format!("host-proc-sec-{name}"));
    if path.is_dir() {
        let _replaced = std::fs::remove_dir(&path);
    }
    std::fs::create_dir(&path).ok()?;
    let controllers = "+cpu +memory +pids";
    std::fs::write(root.join("cgroup.subtree_control"), controllers).ok()?;
    std::fs::write(path.join("cgroup.subtree_control"), controllers).ok()?;
    let state = tempfile::tempdir().ok()?;
    let supervisor = HostProcessSupervisor::open(Some(&path), &state.path().join("processes"))
        .expect("a delegated root serves host processes");
    Some((supervisor, path, state))
}

fn workdir() -> PathBuf {
    std::env::temp_dir().canonicalize().expect("temp dir")
}

fn cgroup_dir(root: &Path, id: &str) -> PathBuf {
    root.join(format!("substrate-hp_{id}"))
}

/// Best-effort removal of what a red run leaves behind, so the next run starts clean. Only
/// directories this case named; a populated one is left in place (rmdir refuses it).
fn remove_leftovers(root: &Path, id: &str) {
    let outer = cgroup_dir(root, id);
    let _inner = std::fs::remove_dir(outer.join("inner"));
    let _outer = std::fs::remove_dir(&outer);
    let _root = std::fs::remove_dir(root);
}

async fn wait_for_line(supervisor: &HostProcessSupervisor, id: &str, wanted: &str) -> bool {
    for _ in 0..100 {
        let seen = supervisor
            .log_tail(id, LogStreamSelection::Stdout, 4_096)
            .is_ok_and(|tail| {
                tail.lines
                    .iter()
                    .any(|line| line.bytes == wanted.as_bytes())
            });
        if seen {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// S3: "a record ends with an empty cgroup", and the driver removes it. A member runs as the
/// daemon's uid, so it can create a cgroup beneath its own (the directory is the daemon's), as a
/// nested Substrate daemon or a cgroupfs-managed rootless container runtime does. The record's
/// cgroup is then empty but has a child, `rmdir` answers `EBUSY`, and a clean exit must still end
/// the record: the driver removes the descendants it can prove empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_a_nested_cgroup_a_member_creates_does_not_keep_a_clean_exit_from_ending() {
    let Some((supervisor, root, _state)) = delegated("nested-exit") else {
        return;
    };
    let id = "hp_sec_nested_exit";
    let inner = cgroup_dir(&root, id).join("inner");
    let mut request = spec(
        &["/bin/sh", "-c", "mkdir \"$INNER\" && echo made"],
        &workdir(),
    );
    request
        .env
        .insert("INNER".to_owned(), inner.display().to_string());
    supervisor.spawn(id, &request).expect("spawn");
    let end = tokio::time::timeout(Duration::from_secs(30), supervisor.wait(id))
        .await
        .expect("the record ends within its bounds");
    let made = wait_for_line(&supervisor, id, "made").await;
    let outer_left = cgroup_dir(&root, id).exists();
    remove_leftovers(&root, id);

    assert!(made, "the member created its nested cgroup");
    let end = end.expect(
        "a nested cgroup left empty by a member must not turn a clean exit into \
         process.cgroup-failed",
    );
    assert_eq!(end.ended_by, EndedBy::Exit);
    assert_eq!(end.exit, Some(ProcessExit::Code(0)));
    assert!(
        !outer_left,
        "the record's cgroup, descendants included, is removed"
    );
}

/// S3: "Stopping a host process signals the cgroup, `SIGTERM` to every member, waits a bounded
/// grace period, then writes `cgroup.kill`." A member in a cgroup beneath the record's is still a
/// member (`cgroup.kill` reaches it), so it must receive the `SIGTERM` and its grace too, not
/// only the `cgroup.kill`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_stop_sends_sigterm_to_members_of_nested_cgroups_too() {
    let Some((supervisor, root, state)) = delegated("nested-term") else {
        return;
    };
    let id = "hp_sec_nested_term";
    let inner = cgroup_dir(&root, id).join("inner");
    let mark = state.path().join("term-mark");
    let script = r#"mkdir "$INNER" || exit 1
sh -c 'echo $$ > "$INNER/cgroup.procs" || exit 1
trap "echo term > \"$MARK\"; exit 0" TERM
echo moved
while :; do sleep 0.1; done' &
wait"#;
    let mut request = spec(&["/bin/sh", "-c", script], &workdir());
    request
        .env
        .insert("INNER".to_owned(), inner.display().to_string());
    request
        .env
        .insert("MARK".to_owned(), mark.display().to_string());
    supervisor.spawn(id, &request).expect("spawn");
    let moved = wait_for_line(&supervisor, id, "moved").await;
    let stopped = tokio::time::timeout(
        Duration::from_secs(30),
        supervisor.stop(id, StopCause::Stop, Duration::from_secs(3)),
    )
    .await;
    let termed = mark.exists();
    remove_leftovers(&root, id);

    assert!(moved, "the member moved itself into the nested cgroup");
    assert!(stopped.is_ok(), "the stop returns within its bounds");
    assert!(
        termed,
        "the member of the nested cgroup received SIGTERM before cgroup.kill"
    );
}

/// S1: "Environment values are secrets in practice ... never a value." The spec's `Debug` is the
/// form any `{:?}` in a log or error takes; it must not carry the values.
#[test]
fn a_spec_debug_form_carries_no_environment_value() {
    let mut request = spec(&["/bin/true"], Path::new("/"));
    request
        .env
        .insert("API_TOKEN".to_owned(), "s3cr3t-value-for-review".to_owned());
    let printed = format!("{request:?}");
    assert!(
        !printed.contains("s3cr3t-value-for-review"),
        "Debug printed an environment value: {printed}"
    );
}

/// S3/S5: a stop "kills the whole tree" and returns. The main process is the driver's own
/// unreaped child, so the driver can always end it, cgroup or not. A main process that leaves
/// its cgroup — as `exec systemd-run --user --scope …` does while keeping its pid, and as
/// `scripts/delegated-lane.sh` itself does — leaves the cgroup empty; the stop must still end it
/// and return rather than wait forever for an exit that never comes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_stop_ends_a_main_process_that_left_its_cgroup_and_returns() {
    let Some((supervisor, root, _state)) = delegated("left") else {
        return;
    };
    let id = "hp_sec_left";
    let elsewhere = root.join("sec-elsewhere");
    let script = r#"mkdir "$ELSEWHERE" && echo $$ > "$ELSEWHERE/cgroup.procs" && echo left && exec sleep 300"#;
    let mut request = spec(&["/bin/sh", "-c", script], &workdir());
    request
        .env
        .insert("ELSEWHERE".to_owned(), elsewhere.display().to_string());
    let started = supervisor.spawn(id, &request).expect("spawn");
    let left = wait_for_line(&supervisor, id, "left").await;
    let stopped = tokio::time::timeout(
        Duration::from_secs(20),
        supervisor.stop(id, StopCause::Stop, Duration::from_secs(1)),
    )
    .await;
    let main_alive = Path::new(&format!("/proc/{}", started.pid)).exists();
    // Clean up only the one cgroup this case created, by its exact path.
    let _killed = std::fs::write(elsewhere.join("cgroup.kill"), "1");
    for _ in 0..100 {
        if std::fs::remove_dir(&elsewhere).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    remove_leftovers(&root, id);

    assert!(left, "the main process left its cgroup");
    let end = stopped
        .expect("the stop returns within its bounds")
        .expect("the stop ends the record");
    assert_eq!(end.ended_by, EndedBy::Stop);
    assert!(!main_alive, "the main process did not survive its stop");
}
