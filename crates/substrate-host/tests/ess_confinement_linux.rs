//! The confinement contract's delegated lane for `linux`
//! (`story:confinement-contract-on-linux-and-local`).
//!
//! Executes every authored `-on-delegated` scenario of `spec/confinement-suite.json` that opens
//! only `linux`, against the real adapter over `HostDriver` with a delegated cgroup v2 root, and
//! starts one confined run through the adapter. The count is pinned at 9 and must equal
//! what ran. Beside them, design 29's runs: a scoped workspace, the limits it states rather than
//! closes, and each output retention mode.
//!
//! Without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this process is inside, or
//! without bubblewrap, these cases are **absent, never reported as passed** (invariant 3):
//! each prints `ABSENT` and its reason. `bash scripts/delegated-lane.sh` runs them and requires
//! the executed count this file prints.

#![cfg(target_os = "linux")]

mod support;

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, Command, ConfinementAdapter, ConfinementError,
    ConfinementProfile, ContractFamily, Fact, ProcessBounds, RunState, linux::LinuxRequest,
};
use substrate_host::{EgressAperture, HostConfig};

/// The delegated cgroup v2 subtree this process is inside, or the reason the lane is absent.
fn delegated() -> Result<PathBuf, &'static str> {
    let root = PathBuf::from(
        std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")
            .ok_or("SUBSTRATE_VECTORS_CGROUP_ROOT is unset")?,
    );
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids")
        .map_err(|_| "the delegated cgroup root does not accept controllers")?;
    if !Path::new("/usr/bin/bwrap").is_file() {
        return Err("no bubblewrap at /usr/bin/bwrap");
    }
    Ok(root)
}

fn open_linux(
    host: &Path,
    cgroup_root: &Path,
    aperture: bool,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    let mut config = HostConfig::minimum(host.join("workspaces"));
    if aperture {
        // The setup the egress scenario names: an operator-declared aperture. The probe proves
        // the mechanism in a throwaway sandbox and never dials the destination.
        config.egress_apertures = vec![EgressAperture {
            name: "registry".to_owned(),
            host: "localhost".to_owned(),
            port: 9,
            pinned: SocketAddr::from((Ipv4Addr::LOCALHOST, 9)),
            max_bytes: None,
        }];
    }
    confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root.to_path_buf())),
    )))
}

/// The authored `-on-delegated` scenarios of `spec/confinement-suite.json` that open only
/// `linux`. Pinned, so a scenario lost or renamed out of the lane fails here instead of shrinking
/// what runs.
const LINUX_DELEGATED: usize = 9;

#[test]
fn ess_confinement_delegated_lane_executes_every_authored_linux_scenario() {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("ESS confinement linux delegated lane: ABSENT ({reason}); 0 executed");
            return;
        }
    };
    let scenarios = support::scenarios("delegated", |adapter| adapter == "linux");
    assert_eq!(
        scenarios.len(),
        LINUX_DELEGATED,
        "spec/confinement-suite.json holds another number of authored -on-delegated scenarios \
         opening only linux than this lane pins"
    );
    let declared = LINUX_DELEGATED;
    let mut hosts = Vec::new();
    let (executed, audited, open_checks) = support::run(&scenarios, |step| {
        assert_eq!(step.adapter, "linux");
        let host = tempfile::tempdir().expect("host root");
        let aperture = step
            .served_families
            .iter()
            .any(|family| family == "network_egress");
        let adapter = open_linux(host.path(), &cgroup_root, aperture);
        hosts.push(host);
        adapter
    });
    assert_eq!(
        executed, declared,
        "a delegated linux scenario of spec/confinement-suite.json did not run"
    );
    let others = support::lane_size("delegated") - declared;
    println!(
        "ESS confinement linux delegated lane: {executed} scenarios executed of {declared} \
         declared for linux, {audited} refusal audit records asserted, {open_checks} open checks refused, 0 \
         failed; {others} delegated scenarios on other adapters not executed by this case"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linux_run_is_confined_bounded_and_not_declared_insecure() {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("ESS confinement linux run: ABSENT ({reason})");
            return;
        }
    };
    let host = tempfile::tempdir().expect("host root");
    let adapter = open_linux(host.path(), &cgroup_root, false).expect("linux opens");
    for family in [ContractFamily::ProcessRun, ContractFamily::Limits] {
        assert!(adapter.facts().serves(&family), "{:?}", adapter.facts());
    }
    let workspace = host.path().join("workspaces").join("ws_run");
    std::fs::create_dir_all(workspace.join("sub")).expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.clone()),
            workdir: Some("sub".into()),
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
            &Command {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "pwd; printf done > /workspace/marker; exit 4".into(),
                ],
                env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            },
        )
        .await
        .expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(4));
    assert_eq!(ended.adapter, AdapterName::Linux);
    assert_eq!(ended.declared_insecure, None);
    let output = adapter.output(&run).await.expect("output");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "/workspace/sub\n");
    assert_eq!(
        std::fs::read_to_string(workspace.join("marker")).expect("marker"),
        "done"
    );
    println!("ESS confinement linux run: executed");
}

/// A `linux` adapter on the delegated root with one workspace beneath its workspace root, or
/// `None` after printing `<name>: ABSENT (<reason>)`.
fn linux_workspace(
    name: &str,
) -> Option<(tempfile::TempDir, Arc<dyn ConfinementAdapter>, PathBuf)> {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("{name}: ABSENT ({reason})");
            return None;
        }
    };
    let host = tempfile::tempdir().expect("host root");
    let adapter = open_linux(host.path(), &cgroup_root, false).expect("linux opens");
    let workspace = host.path().join("workspaces").join("ws_scoped");
    std::fs::create_dir_all(&workspace).expect("workspace");
    Some((host, adapter, workspace))
}

/// Admits `profile`, runs `script` under `/bin/sh -c` and returns its stdout once it ended.
async fn run_script(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    script: &str,
) -> confinement::RunOutput {
    let admitted = adapter.admit(profile).expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sh".into(), "-c".into(), script.to_owned()],
                env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            },
        )
        .await
        .expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    adapter.output(&run).await.expect("output")
}

/// A host `.git` with a config, a HEAD and a hooks directory.
fn host_git(workspace: &Path) {
    std::fs::create_dir_all(workspace.join(".git/hooks")).expect(".git/hooks");
    std::fs::write(workspace.join(".git/config"), "[core]\n").expect("config");
    std::fs::write(workspace.join(".git/HEAD"), "ref: refs/heads/main\n").expect("HEAD");
}

// A1: only the named subtree is writable.
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_linux_run_writes_only_its_subtree() {
    const NAME: &str = "ESS confinement linux scoped write";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    support::scoped_write_workspace(&workspace);
    let output = run_script(
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: support::scoped_out(),
            ..ConfinementProfile::default()
        },
        support::SCOPED_WRITE_SCRIPT,
    )
    .await;
    support::assert_scoped_write(NAME, &workspace, &output.stdout);
    println!("{NAME}: executed");
}

// A2: the workspace's own `.git` lies in the read-only root, and nothing the run does changes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_linux_run_cannot_change_the_workspace_git() {
    const NAME: &str = "ESS confinement linux scoped git";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    support::scoped_write_workspace(&workspace);
    host_git(&workspace);
    let before = support::tree(&workspace.join(".git"));
    let output = run_script(
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: support::scoped_out(),
            ..ConfinementProfile::default()
        },
        "cd /workspace; \
         t() { if \"$@\" 2>/dev/null; then echo \"$*: done\"; else echo \"$*: refused\"; fi; }; \
         t sh -c 'printf x >> .git/config'; t mkdir .git/hooks/x; t mv .git g; t rm -rf .git; \
         ln .git/config out/link 2>&1 || true",
    )
    .await;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for attempt in [
        "sh -c printf x >> .git/config: refused",
        "mkdir .git/hooks/x: refused",
        "mv .git g: refused",
        "rm -rf .git: refused",
    ] {
        assert!(stdout.contains(attempt), "{attempt} in {stdout}");
    }
    assert!(!stdout.contains(": done"), "{stdout}");
    assert!(
        stdout.contains("Invalid cross-device link"),
        "a hard link out of the read-only root is EXDEV: {stdout}"
    );
    assert_eq!(
        support::tree(&workspace.join(".git")),
        before,
        "the host's .git"
    );
    assert!(!workspace.join("g").exists() && !workspace.join("out/link").exists());
    println!("{NAME}: executed");
}

// A6: a subtree that is a symlink is refused at start, by the host's name, and audited.
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_linux_subtree_that_is_a_symlink_is_refused_at_start() {
    const NAME: &str = "ESS confinement linux subtree symlink";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    std::fs::create_dir_all(workspace.join("src")).expect("src");
    std::os::unix::fs::symlink("src", workspace.join("out")).expect("out -> src");
    let records = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: support::scoped_out(),
            audit: Some(confinement::AuditSink::new(
                move |record: &confinement::AuditRecord| {
                    sink_records
                        .lock()
                        .push(serde_json::to_value(record).expect("record"));
                },
            )),
            ..ConfinementProfile::default()
        })
        .expect("admission opens no workspace");
    let error = adapter
        .start(
            &admitted,
            &Command {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "echo x > /workspace/out/f".into(),
                ],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect_err("refused");
    assert_eq!(
        error.code(),
        substrate_wire::EXEC_WORKSPACE_SUBTREE_SYMLINK,
        "{error}"
    );
    assert_eq!(error.status(), 422);
    let records = records.lock();
    let events: Vec<_> = records.iter().map(|record| &record["event"]).collect();
    assert_eq!(
        events,
        ["profile_admitted", "profile_refused"],
        "{records:?}"
    );
    assert_eq!(
        records[1]["code"],
        substrate_wire::EXEC_WORKSPACE_SUBTREE_SYMLINK
    );
    assert!(!workspace.join("src/f").exists());
    println!("{NAME}: executed");
}

// Design 29 § 2, first limit: a `.git` nested inside a writable subtree stays writable.
#[tokio::test(flavor = "multi_thread")]
async fn open_point_a_git_nested_in_a_writable_subtree_stays_writable() {
    const NAME: &str = "ESS confinement linux nested git";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    support::scoped_write_workspace(&workspace);
    std::fs::create_dir_all(workspace.join("out/vendor/.git")).expect("nested .git");
    std::fs::write(workspace.join("out/vendor/.git/config"), "[core]\n").expect("config");
    let output = run_script(
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: support::scoped_out(),
            ..ConfinementProfile::default()
        },
        "if (printf 'nested\\n' >> /workspace/out/vendor/.git/config) 2>/dev/null; \
         then echo written; else echo refused; fi",
    )
    .await;
    assert_eq!(String::from_utf8_lossy(&output.stdout), "written\n");
    assert_eq!(
        std::fs::read_to_string(workspace.join("out/vendor/.git/config")).expect("config"),
        "[core]\nnested\n"
    );
    println!("{NAME}: executed, a nested .git in a writable subtree is writable");
}

// Design 29 § 2, second limit: a hard link made on the host before the run, from the writable
// subtree to a file in the read-only root, is writable through the subtree.
#[tokio::test(flavor = "multi_thread")]
async fn open_point_a_hard_link_made_before_the_run_is_writable_through_the_subtree() {
    const NAME: &str = "ESS confinement linux hard link";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    support::scoped_write_workspace(&workspace);
    host_git(&workspace);
    std::fs::hard_link(workspace.join(".git/config"), workspace.join("out/alias"))
        .expect("hard link");
    let output = run_script(
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: support::scoped_out(),
            ..ConfinementProfile::default()
        },
        "a() { if (printf \"$2\" >> \"$1\") 2>/dev/null; \
         then echo \"$1 written\"; else echo \"$1 refused\"; fi; }; \
         a /workspace/.git/config 'direct\\n'; a /workspace/out/alias 'alias\\n'",
    )
    .await;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "/workspace/.git/config refused\n/workspace/out/alias written\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join(".git/config")).expect("config"),
        "[core]\nalias\n",
        "the read-only path's inode changed through the link"
    );
    println!("{NAME}: executed, a pre-existing hard link is writable through the subtree");
}

/// The bound the `linux` retention cases set, so each moves little data.
const LINUX_BOUND: usize = 4096;
/// `seq 1 3000` is 13,893 bytes: over the bound.
const LINUX_SEQ_LAST: u32 = 3000;
/// The host driver's truncation marker, which only `Head` over the bound carries.
const MARKER: &[u8] = b"\n[substrate: output truncated]\n";

/// Runs `seq 1 3000` to stdout, and to stderr when `both`, under `retention` with the bound at
/// 4096, and checks each stream against the bytes `seq` writes.
async fn linux_retains(name: &str, retention: confinement::OutputRetention, both: bool) {
    let Some((_host, adapter, workspace)) = linux_workspace(name) else {
        return;
    };
    let script = if both {
        format!("seq 1 {LINUX_SEQ_LAST}; seq 1 {LINUX_SEQ_LAST} >&2")
    } else {
        format!("seq 1 {LINUX_SEQ_LAST}")
    };
    let output = run_script(
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace),
            io_bounds: Some(confinement::IoBounds {
                output_bytes: LINUX_BOUND as u64,
            }),
            output_retention: retention,
            ..ConfinementProfile::default()
        },
        &script,
    )
    .await;
    let stream = support::seq_bytes(LINUX_SEQ_LAST);
    let marker = |mode| (mode == confinement::Retention::Head).then_some(MARKER);
    support::assert_retained(
        &format!("{name} stdout"),
        &stream,
        &output.stdout,
        &output.stdout_kept,
        retention.stdout,
        LINUX_BOUND,
        marker(retention.stdout),
    );
    assert!(output.stdout_truncated);
    if both {
        support::assert_retained(
            &format!("{name} stderr"),
            &stream,
            &output.stderr,
            &output.stderr_kept,
            retention.stderr,
            LINUX_BOUND,
            marker(retention.stderr),
        );
        assert!(output.stderr_truncated);
    }
    println!("{name}: executed");
}

// B5: the default still ends in the marker.
#[tokio::test(flavor = "multi_thread")]
async fn a_linux_run_under_head_keeps_the_first_bytes_and_the_marker() {
    linux_retains(
        "ESS confinement linux retention head",
        confinement::OutputRetention::default(),
        false,
    )
    .await;
}

// B6
#[tokio::test(flavor = "multi_thread")]
async fn a_linux_run_under_tail_keeps_the_last_bytes() {
    linux_retains(
        "ESS confinement linux retention tail",
        confinement::OutputRetention {
            stdout: confinement::Retention::Tail,
            stderr: confinement::Retention::Head,
        },
        false,
    )
    .await;
}

// B7: each stream takes its own mode in one run.
#[tokio::test(flavor = "multi_thread")]
async fn a_linux_run_keeps_each_stream_in_its_own_mode() {
    linux_retains(
        "ESS confinement linux retention per stream",
        confinement::OutputRetention {
            stdout: confinement::Retention::HeadAndTail,
            stderr: confinement::Retention::Tail,
        },
        true,
    )
    .await;
}
