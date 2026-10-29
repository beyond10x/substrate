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
//!
//! Design 29's adversary and security review passes are modules of this binary, not binaries of
//! their own, so the gate links one test binary fewer for each.

#![cfg(target_os = "linux")]

#[path = "design29/adversary.rs"]
mod adversary_design29;
#[path = "design29/security.rs"]
mod security_design29;
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
const LINUX_DELEGATED: usize = 11;

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

const GIT: &str = "/usr/bin/git";

/// Runs Git on the host as an operator would, isolated from this machine's configuration: a
/// cleared environment, `HOME` in the case's own directory and no system configuration.
fn run_git(home: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(GIT)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .args(args)
        .output()
        .expect("git runs")
}

fn run_git_ok(home: &Path, args: &[&str]) {
    let output = run_git(home, args);
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A private Git home beside the workspace: an identity, and the one setting a local submodule
/// clone needs. `None` after printing `<name>: ABSENT (no git)` on a host without Git.
fn git_home(name: &str, host: &Path) -> Option<PathBuf> {
    if !Path::new(GIT).is_file() {
        println!("{name}: ABSENT (no git at {GIT})");
        return None;
    }
    let home = host.join("home");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(
        home.join("gitconfig"),
        "[user]\n\tname = lane\n\temail = lane@example.invalid\n\
         [init]\n\tdefaultBranch = main\n[protocol \"file\"]\n\tallow = always\n",
    )
    .expect("gitconfig");
    Some(home)
}

/// What a run needs, with `sh` alone, to make `.git` in its directory a repository Git accepts
/// whose `core.fsmonitor` is `$VALUE`, a command Git runs.
const PLANT_REPOSITORY: &str = "rm -f .git && mkdir -p .git/objects .git/refs/heads && \
     printf 'ref: refs/heads/main\\n' > .git/HEAD && \
     printf '[core]\\n\\trepositoryformatversion = 0\\n\\tfsmonitor = %s\\n' \"$VALUE\" > .git/config && \
     echo planted";

/// Makes `workspace` a repository with one committed submodule at `path`, cloned from a local
/// upstream beside it, as `git submodule add` registers one: an entry in `.gitmodules` and a
/// `gitdir:` file at `<path>/.git` into the workspace's own `.git/modules/`.
fn add_submodule(home: &Path, host: &Path, workspace: &Path, path: &str) {
    let upstream = host.join("upstream");
    let upstream = upstream.to_str().expect("utf-8");
    let workspace = workspace.to_str().expect("utf-8");
    run_git_ok(home, &["init", "-q", upstream]);
    run_git_ok(
        home,
        &[
            "-C",
            upstream,
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    run_git_ok(home, &["init", "-q", workspace]);
    run_git_ok(
        home,
        &["-C", workspace, "submodule", "add", "-q", upstream, path],
    );
    run_git_ok(home, &["-C", workspace, "commit", "-q", "-m", "submodule"]);
}

// Design 29 § 2: a writable subtree that holds a submodule the workspace registers is refused
// `confinement.writable-subtree-git` at start, as `.git` itself is, because the run could replace
// the submodule's `gitdir:` file and a host `git status` in the root runs Git in it. A subtree
// beside the submodule is served.
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_subtree_holding_a_registered_submodule_is_refused_at_start() {
    const NAME: &str = "ESS confinement linux submodule subtree";
    let Some((host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let Some(home) = git_home(NAME, host.path()) else {
        return;
    };
    add_submodule(&home, host.path(), &workspace, "out/vendor");
    let workspace_text = workspace.to_str().expect("utf-8");
    std::fs::create_dir_all(workspace.join("build")).expect("build");
    let gitdir_file = std::fs::read(workspace.join("out/vendor/.git")).expect("gitdir file");
    let before = support::tree(&workspace.join(".git"));
    let marker = host.path().join("marker");

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
    let script = format!(
        "VALUE='touch {}'; cd /workspace/out/vendor && {PLANT_REPOSITORY}",
        marker.display()
    );
    let command = Command {
        argv: vec!["/bin/sh".into(), "-c".into(), script],
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
    };
    let error = adapter
        .start(&admitted, &command)
        .await
        .expect_err("refused");
    assert_eq!(error.code(), confinement::WRITABLE_SUBTREE_GIT, "{error}");
    assert_eq!(error.status(), 422);
    assert!(error.to_string().contains("out/vendor"), "{error}");
    let rendered = adapter
        .render(&admitted, &command)
        .expect_err("a render refuses as the start does");
    assert_eq!(
        rendered.code(),
        confinement::WRITABLE_SUBTREE_GIT,
        "{rendered}"
    );
    assert_eq!(rendered.status(), 422);
    {
        let records = records.lock();
        let events: Vec<_> = records.iter().map(|record| &record["event"]).collect();
        assert_eq!(
            events,
            ["profile_admitted", "profile_refused"],
            "{records:?}"
        );
        assert_eq!(records[1]["code"], confinement::WRITABLE_SUBTREE_GIT);
    }
    assert_eq!(
        std::fs::read(workspace.join("out/vendor/.git")).expect("gitdir file"),
        gitdir_file,
        "nothing ran"
    );
    let _ = run_git(&home, &["-C", workspace_text, "status", "--short"]);
    assert!(
        !marker.exists(),
        "a host git status ran nothing the run chose"
    );

    let output = run_script(
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: confinement::WorkspaceAccess::Scoped {
                writable_subtrees: vec!["build".to_owned()],
            },
            ..ConfinementProfile::default()
        },
        "a() { if (printf x > \"$1\") 2>/dev/null; \
         then echo \"$1 written\"; else echo \"$1 refused\"; fi; }; \
         a /workspace/build/f; a /workspace/out/vendor/.git",
    )
    .await;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "/workspace/build/f written\n/workspace/out/vendor/.git refused\n"
    );
    assert_eq!(
        support::tree(&workspace.join(".git")),
        before,
        "the host's .git"
    );
    println!("{NAME}: executed");
}

// Design 29 § 2, third limit: `Scoped` does not protect host Git run inside a writable subtree
// from a repository the run creates there. Host Git in `out/` finds `out/.git` before the
// workspace's own, so it reads a configuration the run chose; its `core.fsmonitor` is a command
// Git runs. A `ReadWrite` workspace has the same exposure. This reads the value back with
// `git config`, which runs nothing, and leaves the root's `.git` unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn limit_a_repository_the_run_creates_in_its_subtree_is_read_by_host_git_there() {
    const NAME: &str = "ESS confinement linux planted repository";
    let Some((host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let Some(home) = git_home(NAME, host.path()) else {
        return;
    };
    run_git_ok(&home, &["init", "-q", workspace.to_str().expect("utf-8")]);
    std::fs::create_dir_all(workspace.join("out")).expect("out");
    let before = support::tree(&workspace.join(".git"));
    let output = run_script(
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: support::scoped_out(),
            ..ConfinementProfile::default()
        },
        &format!("VALUE=run-chosen-command; cd /workspace/out && {PLANT_REPOSITORY}"),
    )
    .await;
    assert_eq!(String::from_utf8_lossy(&output.stdout), "planted\n");
    assert_eq!(
        support::tree(&workspace.join(".git")),
        before,
        "the root's .git"
    );
    let discovered = run_git(
        &home,
        &[
            "-C",
            workspace.join("out").to_str().expect("utf-8"),
            "config",
            "--get",
            "core.fsmonitor",
        ],
    );
    assert_eq!(
        String::from_utf8_lossy(&discovered.stdout).trim(),
        "run-chosen-command",
        "host git in out/ reads the repository the run created"
    );
    println!("{NAME}: executed, host git in a writable subtree reads a repository the run created");
}

// Design 29 § 6: `linux` reports output when the run ends. While the run is running, `output()`
// holds none of its bytes, whatever the retention; once it ended, all of them. The run prints,
// says so through a file, then waits for the go-ahead, so the read is during the run and after
// the bytes were written, with no timing guess.
#[tokio::test(flavor = "multi_thread")]
async fn a_linux_run_reports_its_output_when_it_ends() {
    const NAME: &str = "ESS confinement linux output while running";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.clone()),
            output_retention: confinement::OutputRetention {
                stdout: confinement::Retention::Tail,
                stderr: confinement::Retention::Head,
            },
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf abc; : > /workspace/printed; i=0; \
                     while [ ! -e /workspace/go ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done"
                        .into(),
                ],
                env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            },
        )
        .await
        .expect("started");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !workspace.join("printed").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "{NAME}: never printed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let during = adapter.output(&run).await.expect("output while running");
    std::fs::write(workspace.join("go"), "").expect("go");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let after = adapter.output(&run).await.expect("output");
    assert_eq!(during.stdout, b"", "{NAME}: while running");
    assert_eq!(after.stdout, b"abc", "{NAME}: once it ended");
    println!("{NAME}: executed");
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

// ---- design 32, T12's stdin rows: a piped standard input under `linux` ----

/// A `linux` profile in `workspace` with a piped standard input of `limit_bytes`, and the output
/// bound the design's linux cases lower to 4096.
fn linux_piped(workspace: &Path, limit_bytes: u64) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        stdin: confinement::StdinMode::Piped { limit_bytes },
        io_bounds: Some(confinement::IoBounds {
            output_bytes: LINUX_BOUND as u64,
        }),
        ..ConfinementProfile::default()
    }
}

/// The gate as the confined child sees it: `go` beneath its `/workspace`.
fn linux_gate(workspace: &Path) -> support::Gate {
    support::Gate::seen_as(workspace, Path::new(substrate_wire::EXEC_WORKSPACE_MOUNT))
}

// T12: T2.
#[tokio::test(flavor = "multi_thread")]
async fn a_linux_run_reads_three_writes_and_close_ends_it() {
    const NAME: &str = "ESS confinement linux stdin echo";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    support::stdin_echoes_and_ends_on_close(
        &adapter,
        &linux_piped(&workspace, 1_024),
        &[b"one\n", b"two \x10\x11\n", b"three\n"],
    )
    .await;
    println!("{NAME}: executed");
}

/// A gated child that then counts what it reads: T8's and T9's run under `linux`.
async fn linux_counting_behind(
    adapter: &Arc<dyn ConfinementAdapter>,
    workspace: &Path,
    gate: &support::Gate,
) -> confinement::RunId {
    let admitted = adapter
        .admit(&linux_piped(workspace, confinement::MAX_STDIN_BYTES))
        .expect("admitted");
    adapter
        .start(&admitted, &support::sh(&format!("{}; wc -c", gate.line())))
        .await
        .expect("started")
}

// T12: T8.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_linux_write_closes_stdin_and_the_child_reads_a_prefix_then_eof() {
    const NAME: &str = "ESS confinement linux stdin cancel";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let gate = linux_gate(&workspace);
    let run = linux_counting_behind(&adapter, &workspace, &gate).await;
    let (completed, pending) = support::fill(&adapter, &run).await;
    pending.abort();
    assert!(pending.await.is_err(), "the pending write was cancelled");
    support::assert_refused(
        &adapter
            .write_stdin(&run, b"x")
            .await
            .expect_err("after a cancellation"),
        substrate_wire::SESSION_INPUT_CLOSED,
        422,
    );
    adapter.close_stdin(&run).await.expect("close is Ok");
    gate.open();
    let ended = tokio::time::timeout(std::time::Duration::from_secs(30), adapter.wait(&run))
        .await
        .expect("the child read end of file")
        .expect("waited");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(0));
    let output = adapter.output(&run).await.expect("output");
    let read: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("wc's count");
    assert!(
        (completed..=completed + 65_536).contains(&read),
        "{NAME}: the child read {read} bytes; the completed writes held {completed}"
    );
    println!("{NAME}: executed");
}

// T12: T9.
#[tokio::test(flavor = "multi_thread")]
async fn a_linux_close_waits_for_a_pending_write_until_it_is_cancelled() {
    const NAME: &str = "ESS confinement linux stdin close waits";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let gate = linux_gate(&workspace);
    let run = linux_counting_behind(&adapter, &workspace, &gate).await;
    let (_, pending) = support::fill(&adapter, &run).await;
    let mut closing = tokio::spawn({
        let adapter = Arc::clone(&adapter);
        let run = run.clone();
        async move { adapter.close_stdin(&run).await }
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut closing)
            .await
            .is_err(),
        "{NAME}: close returned while a write was pending"
    );
    pending.abort();
    let _ = pending.await;
    tokio::time::timeout(std::time::Duration::from_secs(3), closing)
        .await
        .expect("close returned once the write was cancelled")
        .expect("joined")
        .expect("close is Ok");
    gate.open();
    adapter.wait(&run).await.expect("waited");
    println!("{NAME}: executed");
}

// T12: T10 (a).
#[tokio::test(flavor = "multi_thread")]
async fn after_a_linux_run_ended_a_write_is_input_closed_and_close_is_ok() {
    const NAME: &str = "ESS confinement linux stdin after the end";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let admitted = adapter
        .admit(&linux_piped(&workspace, 1_024))
        .expect("admitted");
    let run = adapter
        .start(&admitted, &support::sh("exit 0"))
        .await
        .expect("started");
    adapter.wait(&run).await.expect("waited");
    support::assert_refused(
        &adapter
            .write_stdin(&run, b"x")
            .await
            .expect_err("after the end"),
        substrate_wire::SESSION_INPUT_CLOSED,
        422,
    );
    adapter.close_stdin(&run).await.expect("close is Ok");
    println!("{NAME}: executed");
}

// T12: T10 (b).
#[tokio::test(flavor = "multi_thread")]
async fn a_linux_write_pending_when_the_child_exits_fails() {
    const NAME: &str = "ESS confinement linux stdin pending at the end";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let gate = linux_gate(&workspace);
    let admitted = adapter
        .admit(&linux_piped(&workspace, confinement::MAX_STDIN_BYTES))
        .expect("admitted");
    let run = adapter
        .start(&admitted, &support::sh(&gate.line()))
        .await
        .expect("started");
    let (_, pending) = support::fill(&adapter, &run).await;
    gate.open();
    let written = tokio::time::timeout(std::time::Duration::from_secs(10), pending)
        .await
        .expect("the pending write returned")
        .expect("joined");
    support::assert_refused(
        &written.expect_err("the child exited"),
        substrate_wire::SESSION_WRITE_FAILED,
        500,
    );
    adapter.wait(&run).await.expect("waited");
    println!("{NAME}: executed");
}

// T12: `Piped` with `Recorded` output never stalls: 64 KiB the embedder never reads is recorded
// to the bound, and the run ends on its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_piped_linux_run_with_unread_recorded_output_does_not_stall() {
    const NAME: &str = "ESS confinement linux stdin recorded output";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let admitted = adapter
        .admit(&linux_piped(&workspace, 1_024))
        .expect("admitted");
    let run = adapter
        .start(&admitted, &support::sh("head -c 65536 /dev/zero"))
        .await
        .expect("started");
    let ended = tokio::time::timeout(std::time::Duration::from_secs(30), adapter.wait(&run))
        .await
        .expect("the run ended without anyone reading its output")
        .expect("waited");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(0));
    let output = adapter.output(&run).await.expect("output");
    assert!(output.stdout_truncated, "{NAME}: 64 KiB is past the bound");
    println!("{NAME}: executed");
}

// ---- design 32 § 3, T12's output rows: live output under `linux` ----

/// A `linux` profile in `workspace` whose output is recorded only, with the output bound
/// lowered to `bound` when one is named.
fn linux_recorded(workspace: &Path, bound: Option<usize>) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        io_bounds: bound.map(|bound| confinement::IoBounds {
            output_bytes: bound as u64,
        }),
        ..ConfinementProfile::default()
    }
}

// T12: T4.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_linux_run_delivers_its_record_on_a_natural_end() {
    const NAME: &str = "ESS confinement linux live natural end";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    support::live_natural_end(
        NAME,
        &adapter,
        &linux_recorded(&workspace, None),
        &support::sh(
            "printf 'out-one\\n'; printf 'err-one\\n' >&2; printf 'out-two\\n'; \
             printf 'err-two\\n' >&2",
        ),
        b"out-one\nout-two\n",
        b"err-one\nerr-two\n",
    )
    .await;
    println!("{NAME}: executed");
}

// T12: T5, at the host's default output bound: a lower one would end the forwarding before the
// queue filled, and nothing would stall.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_linux_run_never_read_stalls_and_ends() {
    const NAME: &str = "ESS confinement linux live stall";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    support::live_stall(
        NAME,
        &adapter,
        &linux_recorded(&workspace, None),
        &support::sh("exec yes"),
        1,
    )
    .await;
    println!("{NAME}: executed");
}

// T12: T6.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_linux_run_past_its_bound_names_it_after_the_last_chunk() {
    const NAME: &str = "ESS confinement linux live bounded";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    support::live_bounded(
        NAME,
        &adapter,
        &linux_recorded(&workspace, Some(LINUX_BOUND)),
        &support::sh(&format!("yes | head -c {}", LINUX_BOUND + 64)),
        LINUX_BOUND,
    )
    .await;
    println!("{NAME}: executed");
}

// T12: T11.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_linux_run_writing_both_streams_stalls_once() {
    const NAME: &str = "ESS confinement linux live both streams";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    support::live_stall(
        NAME,
        &adapter,
        &linux_recorded(&workspace, None),
        &support::sh("yes >&2 & exec yes"),
        2,
    )
    .await;
    println!("{NAME}: executed");
}

// T12: T19.
#[tokio::test(flavor = "multi_thread")]
async fn a_killed_live_linux_run_delivers_a_prefix_without_a_stall() {
    const NAME: &str = "ESS confinement linux live kill";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let gate = linux_gate(&workspace);
    support::live_kill_is_a_prefix(
        NAME,
        &adapter,
        &linux_recorded(&workspace, None),
        &gate,
        &workspace.join("written"),
        &Path::new(substrate_wire::EXEC_WORKSPACE_MOUNT).join("written"),
    )
    .await;
    println!("{NAME}: executed");
}

// T12: T20. A read pending on a silent run does not hold a `Kill` back, and ends with `None`.
#[tokio::test(flavor = "multi_thread")]
async fn a_kill_beside_a_pending_live_linux_read_ends_both() {
    const NAME: &str = "ESS confinement linux live pending read";
    let Some((_host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let admitted = adapter
        .admit(&support::live(&linux_recorded(&workspace, None), 16))
        .expect("admitted");
    let run = adapter
        .start(&admitted, &support::sh("exec sleep 60"))
        .await
        .expect("started");
    let reading = {
        let adapter = Arc::clone(&adapter);
        let run = run.clone();
        tokio::spawn(async move {
            adapter
                .read_output(&run, std::time::Duration::from_secs(30))
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        !reading.is_finished(),
        "{NAME}: the read waits on a silent run"
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        adapter.signal(&run, substrate_wire::Signal::Kill),
    )
    .await
    .expect("the kill is not held back by the pending read")
    .expect("killed");
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), reading)
        .await
        .expect("the pending read ends")
        .expect("joined")
        .expect("read");
    assert_eq!(read, None, "{NAME}");
    adapter.wait(&run).await.expect("waited");
    assert!(!adapter.output_stalled(&run), "{NAME}");
    println!("{NAME}: executed");
}
