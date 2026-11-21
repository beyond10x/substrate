//! Security review of design 35 § 18, *Three decisions after the second review of that branch*,
//! through the real `linux` adapter and the lane's statically linked helper: **a base
//! directory that overlaps a location a run can write.**
//!
//! "A source whose reach as written overlaps a writable location is a writable location for
//! the driver too. Its reach is asked strictly, and where that fails the start is refused
//! before dispatch." The committed cases stage that for a declared read-only root. A base
//! directory, `/usr`, goes through the same function with another address, and every listed
//! start has it in its view, so the case here stages it there: the workspace root on a file
//! system mounted beneath `/usr`, once alone and once with a stack of two mounts beneath
//! `/usr` beside it, which the strict question cannot answer.
//!
//! Held: with a list, such a host answers with a named refusal before dispatch, never with a
//! run the helper ends; without a list, the same start runs as it did.
//!
//! The case makes its mounts in a user and mount namespace of its own and runs itself again
//! there. It needs `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this
//! process is inside, bubblewrap, `SUBSTRATE_EXEC_GATE` naming the static helper, and a host
//! whose user namespaces may mount. Without one of them it prints `<name>: ABSENT (<reason>)`
//! and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    RunState, linux::LinuxRequest,
};

/// Names the staged shape, in the process the case starts inside a mount namespace of its own.
const INNER: &str = "SECURITY7_SHAPE";

/// What the stage of that namespace exits with when one of its commands failed.
const STAGE_FAILED: i32 = 97;

/// The directory beneath `/usr` the workspace root's file system is mounted at.
const HELD: &str = "/usr/local/src";

/// The directory beneath `/usr` two mounts are stacked at.
const STACKED: &str = "/usr/local/include";

const NAME: &str = "security7 view mounts base directory that holds the workspace root";

/// The delegated cgroup v2 subtree this process is inside, or why the case is absent.
fn delegated() -> Result<PathBuf, String> {
    let root = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")
        .map(PathBuf::from)
        .ok_or("SUBSTRATE_VECTORS_CGROUP_ROOT is unset")?;
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids")
        .map_err(|_| "the delegated cgroup root does not accept controllers")?;
    if !Path::new("/usr/bin/bwrap").is_file() {
        return Err("no bubblewrap at /usr/bin/bwrap".to_owned());
    }
    Ok(root)
}

/// A tool of the host, where this distribution keeps it.
fn find(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
}

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8").to_owned()
}

/// Starts the shell with `script` under `profile`: what the run printed and how it ended, or
/// the refusal of the admission or the start.
async fn run(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    script: &str,
) -> String {
    let command = Command {
        argv: vec![
            text(&find("sh").expect("a shell")),
            "-c".to_owned(),
            script.to_owned(),
        ],
        env: BTreeMap::new(),
    };
    let refused = |error: ConfinementError| format!("refused {}: {error}", error.code());
    let admitted = match adapter.admit(profile) {
        Ok(admitted) => admitted,
        Err(error) => return refused(error),
    };
    let run = match adapter.start(&admitted, &command).await {
        Ok(run) => run,
        Err(error) => return refused(error),
    };
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    format!(
        "ran {:?} {:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The case inside its namespace: a tmpfs is mounted at [`HELD`], and holds the workspace
/// root.
async fn inside() {
    let say = |what: &str, answer: &str| println!("{NAME} inside: {what} {answer}");
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => return say("ABSENT", &format!("({reason})")),
    };
    let workspace_root = Path::new(HELD).join("workspaces");
    let workspace = workspace_root.join("ws_exec");
    std::fs::create_dir_all(workspace.join("bin")).expect("the workspace on the mount");
    std::fs::copy(find("true").expect("true"), workspace.join("bin/tool")).expect("copy true");
    let mut config = HostConfig::minimum(workspace_root);
    config.exec_gate = Some(PathBuf::from(
        std::env::var_os("SUBSTRATE_EXEC_GATE")
            .expect("SUBSTRATE_EXEC_GATE is unset: the lane names the helper it built"),
    ));
    let adapter = match confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    ))) {
        Ok(adapter) => adapter,
        Err(error) => return say("open", &format!("refused {}: {error}", error.code())),
    };
    say(
        "open",
        &format!(
            "opened, exec_subpaths proved: {}; {:?}",
            adapter.facts().proves(&Fact::ExecSubpaths),
            adapter.facts()
        ),
    );
    for (what, entries) in [("listed", &["bin"][..]), ("unlisted", &[][..])] {
        let profile = ConfinementProfile {
            workspace: Some(workspace.clone()),
            exec_subpaths: entries.iter().map(|entry| (*entry).to_owned()).collect(),
            ..ConfinementProfile::default()
        };
        say(what, &run(&adapter, &profile, "bin/tool; echo $?").await);
    }
}

/// Runs `case` again inside a user and mount namespace after `stage`: what that run printed,
/// with its diagnostics after it, or why the case is absent.
fn staged(case: &str, shape: &str, stage: &str) -> Result<String, String> {
    delegated()?;
    let (Some(unshare), Some(setpriv), Some(shell)) =
        (find("unshare"), find("setpriv"), find("sh"))
    else {
        return Err("no unshare, setpriv or sh on this host".to_owned());
    };
    // The namespace maps this user to itself and keeps its capabilities for the mounts;
    // `setpriv` then drops them all, because bubblewrap refuses to start with any, so the
    // driver inside is what it is outside: this user, with no capability.
    let output = std::process::Command::new(unshare)
        .args([
            "--user",
            "--map-current-user",
            "--keep-caps",
            "--mount",
            "--",
        ])
        .arg(shell)
        .arg("-c")
        .arg(format!(
            "{{ {stage}; }} >&2 || exit {STAGE_FAILED}; exec \"$@\""
        ))
        .arg("sh")
        .arg(setpriv)
        .args(["--inh-caps=-all", "--ambient-caps=-all", "--"])
        .arg(std::env::current_exe().expect("this test binary"))
        .args(["--exact", case, "--nocapture", "--test-threads=1"])
        .env(INNER, shape)
        .output()
        .expect("unshare starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(STAGE_FAILED) {
        return Err(format!(
            "no tmpfs mount in a user namespace: {}",
            stderr.trim()
        ));
    }
    if output.status.code() == Some(1) && stderr.trim_start().starts_with("unshare:") {
        return Err(format!("no user and mount namespace: {}", stderr.trim()));
    }
    Ok(format!("{stdout}\n{stderr}"))
}

/// The observation `what` of the run inside the namespace, from what it printed.
fn observed(stdout: &str, what: &str) -> Option<String> {
    let inside = format!("{NAME} inside: ");
    stdout
        .lines()
        // The harness writes `test <name> ... ` before the first line of a case.
        .filter_map(|line| line.split_once(inside.as_str()).map(|(_, rest)| rest))
        .find_map(|line| line.strip_prefix(what))
        .map(|rest| rest.trim().to_owned())
}

/// **A base directory that holds the workspace root answers before dispatch** (design 35
/// § 18: "A source whose reach as written overlaps a writable location is a writable location
/// for the driver too. Its reach is asked strictly, and where that fails the start is refused
/// before dispatch"; § 6: on a host whose `/usr` holds the workspace root a link in it is
/// refused).
///
/// The workspace root lies on a tmpfs mounted at `/usr/local/src`, so `/usr` reaches a place a
/// run can write. Two shapes:
///
/// - **alone**: every mount beneath `/usr` can be asked;
/// - **stacked**: two more mounts are stacked at `/usr/local/include`, and the strict question
///   has no answer for the covered one.
///
/// In both, whatever the open decided, a start with a list is refused by name, by the
/// admission or by the start, and no run is dispatched for the helper to end with status 125;
/// a start without a list runs its program. In the stacked shape the refusal names what
/// cannot be asked: the place is unserved, or the open already withheld the fact.
#[tokio::test(flavor = "multi_thread")]
async fn security7_a_base_directory_that_holds_the_workspace_root_refuses_a_list_before_dispatch() {
    const CASE: &str =
        "security7_a_base_directory_that_holds_the_workspace_root_refuses_a_list_before_dispatch";
    if std::env::var_os(INNER).is_some() {
        inside().await;
        return;
    }
    let Some(mount) = find("mount") else {
        return println!("{NAME}: ABSENT (no mount on this host)");
    };
    if !Path::new(HELD).is_dir() || !Path::new(STACKED).is_dir() {
        return println!("{NAME}: ABSENT (this host has no {HELD} or no {STACKED})");
    }
    let mount = mount.display();
    let alone = format!("{mount} -t tmpfs tmpfs {HELD}");
    let stacked =
        format!("{alone} && {mount} -t tmpfs tmpfs {STACKED} && {mount} -t tmpfs tmpfs {STACKED}");
    for (shape, stage) in [("alone", alone.as_str()), ("stacked", stacked.as_str())] {
        let stdout = match staged(CASE, shape, stage) {
            Ok(stdout) => stdout,
            Err(reason) => return println!("{NAME}: ABSENT ({shape}: {reason})"),
        };
        if let Some(reason) = observed(&stdout, "ABSENT") {
            return println!("{NAME}: ABSENT (inside the namespace: {reason})");
        }
        let line = |what: &str| observed(&stdout, what);
        let open = line("open").unwrap_or_else(|| panic!("{shape}: no `open` line: {stdout}"));
        println!("{NAME}: {shape}: open: {open}");
        if open.starts_with("refused ") {
            // The open refused by name: no start exists on this host.
            continue;
        }
        let listed = line("listed").unwrap_or_else(|| panic!("{shape}: no `listed`: {stdout}"));
        let unlisted =
            line("unlisted").unwrap_or_else(|| panic!("{shape}: no `unlisted`: {stdout}"));
        println!("{NAME}: {shape}: listed: {listed}");
        println!("{NAME}: {shape}: unlisted: {unlisted}");
        assert!(
            listed.starts_with("refused "),
            "{shape}: /usr reaches the workspace root, and a start with a list was dispatched: \
             {listed}"
        );
        assert!(
            unlisted.starts_with("ran \"0\\n\""),
            "{shape}: a start without a list on the same host: {unlisted}"
        );
    }
    println!("{NAME}: executed");
}
