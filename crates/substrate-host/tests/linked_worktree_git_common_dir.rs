//! A linked Git worktree reaches its common directory (`story:linked-worktree-git-common-dir`).
//!
//! A linked worktree's `.git` is a file, `gitdir: <common dir>/worktrees/<name>`, naming an
//! absolute host path outside the workspace. Inside a confined exec that path does not exist, so
//! Git cannot find the repository. ADR 0010 already admits a host directory read-only at a declared
//! mount point; declaring the common directory as one more read-only root, mounted at its own host
//! path, makes the `gitdir:` line resolve. No Git-specific field is involved: the request is the
//! general `read_only_roots` list.
//!
//! The workspace is a linked worktree created beneath the host's workspace root. Its repository —
//! and so its common directory — lives outside that root, in a directory under
//! `CARGO_TARGET_TMPDIR` (this checkout's `target/tmp`), never under `/tmp`: ADR 0010 reserves the
//! `/tmp` mount point, so a root mounted at its own path beneath it would be refused. Git inside the
//! sandbox runs with a cleared environment and no `HOME`; it needs no configuration to read.
//!
//! Delegated lane only: without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this
//! process is inside, or without bubblewrap and `/usr/bin/git`, these cases are **absent, never
//! reported as passed** (invariant 3). `bash scripts/delegated-lane.sh` runs them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use substrate_host::{DispatchOutcome, Driver as _, ExecObservation, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementRequest, ExecEnvironment, ExecLimits, ExecStartInput, NetworkMode, ReadOnlyRoot,
    SandboxProfile, WorkspaceAccess,
};

const GIT: &str = "/usr/bin/git";
const WORKSPACE: &str = "ws_test";

/// The delegated cgroup v2 subtree this process is inside, or nothing.
fn delegated_cgroup_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").ok()?;
    Some(root)
}

/// Host-side Git, isolated from the operator's own configuration so a global hook, signing key or
/// template cannot change what the fixture is.
fn host_git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new(GIT)
        .arg("-C")
        .arg(directory)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "substrate test")
        .env("GIT_AUTHOR_EMAIL", "test@substrate.invalid")
        .env("GIT_COMMITTER_NAME", "substrate test")
        .env("GIT_COMMITTER_EMAIL", "test@substrate.invalid")
        .output()
        .expect("host git runs");
    assert!(
        output.status.success(),
        "host git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("host git prints UTF-8")
        .trim()
        .to_owned()
}

struct Lane {
    driver: Arc<HostDriver>,
    snapshot: String,
    /// The linked worktree, which is the workspace `ws_test`.
    worktree: PathBuf,
    /// `git rev-parse --git-common-dir` of the worktree, absolute and canonical.
    common: PathBuf,
    /// The commit the worktree has checked out.
    head: String,
    _workspaces: tempfile::TempDir,
    _repository: tempfile::TempDir,
}

impl Lane {
    fn common_root(&self) -> ReadOnlyRoot {
        let path = self.common.to_str().expect("UTF-8 common dir").to_owned();
        ReadOnlyRoot {
            host_path: path.clone(),
            mount: path,
        }
    }
}

fn lane() -> Option<Lane> {
    let delegated = delegated_cgroup_root()?;
    if !PathBuf::from("/usr/bin/bwrap").is_file() || !PathBuf::from(GIT).is_file() {
        return None;
    }
    let workspaces = tempfile::tempdir().expect("temporary host root");
    let mut config = HostConfig::minimum(workspaces.path().join("workspaces"));
    config.cgroup_root = Some(delegated);
    let driver = HostDriver::open(config).expect("host driver");
    let machine = driver.machine();
    if machine.facts.exec_argv_only != Some(true) {
        return None;
    }

    // Outside the workspace root and outside `/tmp`, so the common dir can be mounted at its own
    // path (see the module documentation).
    let repository = tempfile::Builder::new()
        .prefix("linked-worktree-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("repository directory under CARGO_TARGET_TMPDIR");
    let repo = std::fs::canonicalize(repository.path())
        .expect("canonical repository directory")
        .join("repo");
    assert!(
        !repo.starts_with("/tmp"),
        "the case needs a repository outside /tmp, which ADR 0010 reserves; \
         CARGO_TARGET_TMPDIR is {}",
        env!("CARGO_TARGET_TMPDIR")
    );
    std::fs::create_dir(&repo).expect("repository directory");
    host_git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README"), "linked worktree fixture\n").expect("fixture file");
    host_git(&repo, &["add", "README"]);
    host_git(&repo, &["commit", "-q", "-m", "fixture"]);
    let head = host_git(&repo, &["rev-parse", "HEAD"]);

    let worktree = driver.root().join(WORKSPACE);
    host_git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            worktree.to_str().expect("UTF-8 worktree path"),
        ],
    );
    assert!(
        worktree.join(".git").is_file(),
        "a linked worktree's .git is a file naming its gitdir"
    );
    let common = std::fs::canonicalize(host_git(
        &worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ))
    .expect("canonical common dir");
    assert!(
        !common.starts_with(driver.root()),
        "the common dir must lie outside the workspace root"
    );

    Some(Lane {
        driver,
        snapshot: machine.snapshot,
        worktree,
        common,
        head,
        _workspaces: workspaces,
        _repository: repository,
    })
}

fn exec_input(snapshot: &str, argv: &[&str], read_only_roots: Vec<ReadOnlyRoot>) -> ExecStartInput {
    ExecStartInput {
        read_only_roots,
        secret_slots: Vec::new(),
        workspace: WORKSPACE.to_owned(),
        argv: argv.iter().map(|part| (*part).to_owned()).collect(),
        env: ExecEnvironment {
            allow: vec![],
            set: BTreeMap::new(),
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

async fn waited(
    lane: &Lane,
    id: &str,
    argv: &[&str],
    read_only_roots: Vec<ReadOnlyRoot>,
) -> ExecObservation {
    let input = exec_input(&lane.snapshot, argv, read_only_roots);
    match lane.driver.start_exec(id, WORKSPACE, &input).await {
        DispatchOutcome::Observed(observed) => observed,
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "the delegated lane must dispatch {id}: {} {}",
            error.code, error.message
        ),
    }
}

fn exit_code(observed: &ExecObservation) -> Option<u8> {
    observed
        .resource
        .exit
        .as_ref()
        .expect("a waited exec reports its exit")
        .code
}

/// Today's behaviour, and the reason for the story: the `gitdir:` line names a host path the
/// sandbox does not contain, so Git finds no repository.
#[tokio::test(flavor = "multi_thread")]
async fn git_status_in_a_linked_worktree_fails_without_its_common_dir() {
    let Some(lane) = lane() else {
        return;
    };
    let status = waited(
        &lane,
        "ex_git_status_bare",
        &[GIT, "status", "--porcelain"],
        Vec::new(),
    )
    .await;
    assert_ne!(
        exit_code(&status),
        Some(0),
        "without its common dir a linked worktree is not a repository inside the exec: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// The common dir declared read-only at its own path: `git status` and `git log -1` read the
/// repository, and the applied observation names the root that was mounted.
#[tokio::test(flavor = "multi_thread")]
async fn git_status_and_log_run_in_a_linked_worktree_with_its_common_dir_read_only() {
    let Some(lane) = lane() else {
        return;
    };
    let root = lane.common_root();

    let status = waited(
        &lane,
        "ex_git_status",
        &[GIT, "status", "--porcelain"],
        vec![root.clone()],
    )
    .await;
    assert_eq!(
        exit_code(&status),
        Some(0),
        "git status must read a linked worktree whose common dir is declared: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(
        status
            .resource
            .applied
            .as_ref()
            .expect("an admitted exec records what was applied")
            .read_only_roots,
        vec![root.clone()],
        "the observation reports the declared root as mounted (ADR 0010)"
    );

    let log = waited(
        &lane,
        "ex_git_log",
        &[GIT, "log", "-1", "--format=%H"],
        vec![root],
    )
    .await;
    assert_eq!(
        exit_code(&log),
        Some(0),
        "git log must read the worktree's commit: {}",
        String::from_utf8_lossy(&log.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).trim(),
        lane.head,
        "git log prints the commit the worktree has checked out"
    );
}

/// The root stays read-only: the exec cannot create a file in the common dir, directly or through
/// a Git command that writes the index, and nothing appears on the host.
#[tokio::test(flavor = "multi_thread")]
async fn an_exec_cannot_write_into_the_declared_common_dir() {
    let Some(lane) = lane() else {
        return;
    };
    let root = lane.common_root();
    let probe = lane.common.join("probe");
    let probe_arg = probe.to_str().expect("UTF-8 probe path");

    let touch = waited(
        &lane,
        "ex_common_touch",
        &["/usr/bin/touch", probe_arg],
        vec![root.clone()],
    )
    .await;
    assert_ne!(
        exit_code(&touch),
        Some(0),
        "a declared root is never writable: touch {probe_arg} succeeded"
    );
    assert!(
        !probe.exists(),
        "no file may appear in the common dir on the host"
    );

    std::fs::write(lane.worktree.join("added"), "new\n").expect("worktree file");
    let index_before = std::fs::read(lane.common.join("worktrees").join(WORKSPACE).join("index"))
        .expect("the worktree's index");
    let add = waited(&lane, "ex_git_add", &[GIT, "add", "added"], vec![root]).await;
    assert_ne!(
        exit_code(&add),
        Some(0),
        "git add writes the index under the common dir and must be refused"
    );
    let index_after = std::fs::read(lane.common.join("worktrees").join(WORKSPACE).join("index"))
        .expect("the worktree's index");
    assert_eq!(
        index_before, index_after,
        "the worktree's index in the common dir is unchanged"
    );
}
