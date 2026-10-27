//! Security review of design 29 (`docs/design/29-scoped-workspace-write-and-output-tail.md`):
//! conformance checks of the `.git` protection a scoped workspace write claims, and of the
//! consumer-facing statement of its limits.
//!
//! The delegated cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree
//! this process is inside, bubblewrap and Git. Without them each prints `<name>: ABSENT (<reason>)`
//! and is **absent, never reported as passed** (invariant 3). `bash scripts/delegated-lane.sh`
//! provides the environment.
//!
//! Every host-side Git command here runs with a cleared environment, a private `HOME` and no
//! system configuration, inside a temporary directory, so nothing outside the case is read or
//! written.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementProfile, RunState,
    linux::LinuxRequest,
};

const CHANGELOG: &str = include_str!("../../../../CHANGELOG.md");
const GIT: &str = "/usr/bin/git";

/// The newest CHANGELOG section, `[Unreleased]` or a release, that adds `workspace_access`: what
/// the release carrying scoped workspace write tells its consumers.
fn scoped_write_section() -> &'static str {
    CHANGELOG
        .split("\n## [")
        .skip(1)
        .find(|section| section.contains("gains `workspace_access`"))
        .expect(
            "story:linux-adapter-serves-scoped-workspace-write: a CHANGELOG section adds \
             `workspace_access`",
        )
}

/// Commit b7d677ef says "The CHANGELOG states both limits": a `.git` nested inside a writable
/// subtree stays writable, and a hard link made on the host before the run into a read-only part
/// stays writable through the subtree. Design 29 § 2 names both as limits the note states rather
/// than closes. A consumer that relies on the `.git` protection learns of them only there.
#[test]
fn the_changelog_states_the_two_limits_of_a_scoped_workspace_write() {
    let section = scoped_write_section().to_ascii_lowercase();
    let missing: Vec<&str> = [
        ("hard link", "a pre-existing hard link stays writable"),
        (
            "nested",
            "a nested .git inside a writable subtree stays writable",
        ),
    ]
    .iter()
    .filter(|(needle, _)| !section.contains(needle))
    .map(|(_, limit)| *limit)
    .collect();
    assert!(
        missing.is_empty(),
        "story:linux-adapter-serves-scoped-workspace-write: the CHANGELOG section that adds \
         scoped workspace write does not state these limits of it: {missing:?}"
    );
}

/// The delegated cgroup v2 subtree this process is inside, or the reason the case is absent.
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
    if !Path::new(GIT).is_file() {
        return Err("no git at /usr/bin/git");
    }
    Ok(root)
}

/// A `linux` adapter over a fresh host root, and the workspace path beneath its workspace root.
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
    let config = HostConfig::minimum(host.path().join("workspaces"));
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .expect("linux opens");
    let workspace = host.path().join("workspaces").join("ws_review");
    std::fs::create_dir_all(&workspace).expect("workspace");
    Some((host, adapter, workspace))
}

/// Runs Git on the host the way an operator would, isolated from this machine's configuration:
/// a cleared environment, `HOME` in the case's own directory, no system configuration.
fn host_git(home: &Path, args: &[&str]) -> std::process::Output {
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

fn host_git_ok(home: &Path, args: &[&str]) {
    let output = host_git(home, args);
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The operator's Git identity and the one setting a local submodule clone needs.
fn git_home(host: &Path) -> PathBuf {
    let home = host.join("home");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(
        home.join("gitconfig"),
        "[user]\n\tname = review\n\temail = review@example.invalid\n\
         [init]\n\tdefaultBranch = main\n[protocol \"file\"]\n\tallow = always\n",
    )
    .expect("gitconfig");
    home
}

/// Every entry beneath `root` with its bytes; directories as `None`, symlinks as their target.
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, directory: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let mut names: Vec<_> = std::fs::read_dir(directory)
            .expect("read_dir")
            .map(|entry| entry.expect("entry").path())
            .collect();
        names.sort();
        for path in names {
            let relative = path.strip_prefix(root).expect("beneath").to_path_buf();
            let kind = std::fs::symlink_metadata(&path)
                .expect("metadata")
                .file_type();
            if kind.is_dir() {
                entries.insert(relative, None);
                walk(root, &path, entries);
            } else if kind.is_symlink() {
                let target = std::fs::read_link(&path).expect("link");
                entries.insert(relative, Some(target.into_os_string().into_encoded_bytes()));
            } else {
                entries.insert(relative, Some(std::fs::read(&path).expect("file")));
            }
        }
    }
    let mut entries = BTreeMap::new();
    walk(root, root, &mut entries);
    entries
}

/// Runs `script` under `/bin/sh -c` with `Scoped(["out"])` and returns its stdout once it ended.
async fn run_scoped_out(
    adapter: &Arc<dyn ConfinementAdapter>,
    workspace: &Path,
    script: &str,
    env: &[(&str, String)],
) -> String {
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.to_path_buf()),
            workspace_access: confinement::WorkspaceAccess::Scoped {
                writable_subtrees: vec!["out".to_owned()],
            },
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let mut variables = BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
    for (key, value) in env {
        variables.insert((*key).to_owned(), value.clone());
    }
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sh".into(), "-c".into(), script.to_owned()],
                env: variables,
            },
        )
        .await
        .expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// What a run needs, with `sh` alone, to make `<dir>/.git` a repository Git accepts whose
/// configuration it chose. `$VALUE` is the `core.fsmonitor` it sets.
const PLANT_REPOSITORY: &str = "rm -f .git && mkdir -p .git/objects .git/refs/heads && \
     printf 'ref: refs/heads/main\\n' > .git/HEAD && \
     printf '[core]\\n\\trepositoryformatversion = 0\\n\\tfsmonitor = %s\\n' \"$VALUE\" > .git/config && \
     echo planted";

/// Design 29 § 2 names a `.git` nested in a writable subtree as a limit, by example of one that
/// already exists ("a vendored repository, a submodule checkout"). The confined run can make one
/// itself: a repository in `out/` whose configuration it chose, which host Git run in `out/`
/// discovers before the workspace's own read-only `.git`. Its `core.fsmonitor` is a command Git
/// runs on the host. This reads the value back with `git config`, which runs nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_run_cannot_plant_a_repository_host_git_discovers_in_its_subtree() {
    const NAME: &str = "security design 29 planted repository";
    let Some((host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let home = git_home(host.path());
    let workspace_text = workspace.to_str().expect("utf-8");
    host_git_ok(&home, &["init", "-q", workspace_text]);
    std::fs::create_dir_all(workspace.join("out")).expect("out");
    let before = tree(&workspace.join(".git"));
    let stdout = run_scoped_out(
        &adapter,
        &workspace,
        &format!("cd /workspace/out && {PLANT_REPOSITORY}"),
        &[("VALUE", "run-chosen-command".to_owned())],
    )
    .await;
    assert_eq!(
        tree(&workspace.join(".git")),
        before,
        "the workspace's own .git is unchanged"
    );
    let discovered = host_git(
        &home,
        &[
            "-C",
            workspace.join("out").to_str().expect("utf-8"),
            "config",
            "--get",
            "core.fsmonitor",
        ],
    );
    let value = String::from_utf8_lossy(&discovered.stdout);
    println!("{NAME}: executed");
    // story:linux-adapter-serves-scoped-workspace-write — decided as a documented limit (design 29
    // § 2, CHANGELOG): Scoped protects the root `.git`, not host Git run inside a writable subtree
    // from a repository the run creates there. This pins the limit so a change to it is seen.
    assert_eq!(
        value.trim(),
        "run-chosen-command",
        "the documented limit changed: host git in out/ no longer reads a repository the run \
         planted there (run output: {stdout})"
    );
}

/// The workspace has a submodule at `out/vendor`; its Git directory is in the workspace's own
/// read-only `.git/modules/`, and `out/vendor/.git` is a `gitdir:` file. Under `Scoped(["out"])`
/// the run replaces that file with a repository whose `core.fsmonitor` it chose. A `git status` in
/// the workspace root, the operator's ordinary command, checks each submodule by running Git in
/// it, so it runs the command the run chose. The command here creates a marker file in the case's
/// own directory.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_git_status_does_not_run_a_command_a_scoped_run_chose() {
    const NAME: &str = "security design 29 submodule redirect";
    let Some((host, adapter, workspace)) = linux_workspace(NAME) else {
        return;
    };
    let home = git_home(host.path());
    let upstream = host.path().join("upstream");
    let upstream_text = upstream.to_str().expect("utf-8");
    let workspace_text = workspace.to_str().expect("utf-8");
    host_git_ok(&home, &["init", "-q", upstream_text]);
    host_git_ok(
        &home,
        &[
            "-C",
            upstream_text,
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    host_git_ok(&home, &["init", "-q", workspace_text]);
    host_git_ok(
        &home,
        &[
            "-C",
            workspace_text,
            "submodule",
            "add",
            "-q",
            upstream_text,
            "out/vendor",
        ],
    );
    host_git_ok(
        &home,
        &["-C", workspace_text, "commit", "-q", "-m", "submodule"],
    );
    assert!(
        workspace.join("out/vendor/.git").is_file(),
        "the submodule's .git is a gitdir file into the workspace's own .git"
    );
    let marker = host.path().join("marker");
    host_git_ok(&home, &["-C", workspace_text, "status", "--short"]);
    assert!(!marker.exists(), "nothing runs before the confined run");
    let before = tree(&workspace.join(".git"));
    // story:linux-adapter-serves-scoped-workspace-write — the review's fix: a writable subtree
    // that holds a submodule `.gitmodules` registers is refused at start as `.git` itself is, so
    // the run that would replace `out/vendor/.git` never starts.
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: confinement::WorkspaceAccess::Scoped {
                writable_subtrees: vec!["out".to_owned()],
            },
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let script = format!(
        "VALUE='touch {}'; cd /workspace/out/vendor && {PLANT_REPOSITORY}",
        marker.to_str().expect("utf-8")
    );
    let error = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sh".into(), "-c".into(), script],
                env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            },
        )
        .await
        .expect_err("a subtree holding a registered submodule is refused");
    assert_eq!(error.code(), confinement::WRITABLE_SUBTREE_GIT, "{error}");
    assert_eq!(error.status(), 422);
    assert_eq!(
        tree(&workspace.join(".git")),
        before,
        "the workspace's own .git is unchanged"
    );
    let _ = host_git(&home, &["-C", workspace_text, "status", "--short"]);
    println!("{NAME}: executed");
    assert!(
        !marker.exists(),
        "a git status in the workspace root ran a command chosen for a refused run"
    );
}
