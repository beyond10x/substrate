//! Security review of design 35 D30, *Through the view's mounts*, at the four sites that ask
//! whether something lies where a run can write, through the real `linux` adapter and the
//! lane's statically linked helper.
//!
//! The committed case (`tests/exec_lists_view_mounts.rs`) stages a tmpfs beneath the workspace
//! root. These cases ask what it does not:
//!
//! - **the one rule that skips**: a mount of a file-system type outside the closed list is
//!   compared at none of the four sites. The design's reason is that a path on such a type has
//!   no place and is refused where it is placed. A followed entry is placed where it *ends*; a
//!   symbolic link it passes *through* is read and never placed. So the same shape the
//!   committed case refuses on a tmpfs is staged twice here, once on a tmpfs and once on a
//!   `ramfs`, which a user namespace may mount and which is outside the list;
//! - **nothing ordinary breaks**: a mount beneath `/usr`, a stack of two mounts that no
//!   location reaches, and a listed directory with a mount beneath it start and run as they
//!   did; and a stack beneath the workspace root fails closed, `mount-covered`, at the open.
//!
//! Each case makes its mounts in a user and mount namespace of its own and runs itself again
//! there. The cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree
//! this process is inside, bubblewrap, `SUBSTRATE_EXEC_GATE` naming the static helper, and a
//! host whose user namespaces may mount. Without one of them each prints
//! `<name>: ABSENT (<reason>)` and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    ReadOnlyRoot, RunState, linux::LinuxRequest,
};

/// Names the host root, in the process a case starts inside a mount namespace of its own.
const INNER: &str = "SECURITY5_HOST";

/// Names the file-system type the stage mounted, in that process.
const KIND: &str = "SECURITY5_KIND";

/// What the stage of that namespace exits with when a mount failed.
const STAGE_FAILED: i32 = 97;

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

/// The helper the lane built.
fn helper() -> PathBuf {
    PathBuf::from(
        std::env::var_os("SUBSTRATE_EXEC_GATE")
            .expect("SUBSTRATE_EXEC_GATE is unset: the lane names the helper it built"),
    )
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

/// A program that is a file of `/usr/bin` itself, exits 0 given `--version` and is not the
/// shell: what a list that admits `/usr/bin` runs and a list without it does not.
fn probe_program() -> Option<String> {
    let shell = std::fs::canonicalize(find("sh")?).ok()?;
    ["sed", "grep", "gzip", "tar"]
        .iter()
        .map(|name| Path::new("/usr/bin").join(name))
        .find(|path| {
            std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
                && *path != shell
        })
        .map(|path| text(&path))
}

fn open(
    workspace_root: &Path,
    cgroup_root: &Path,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    open_with(workspace_root, cgroup_root, helper())
}

/// [`open`] with the helper's file at `gate`.
fn open_with(
    workspace_root: &Path,
    cgroup_root: &Path,
    gate: PathBuf,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    let mut config = HostConfig::minimum(workspace_root.to_path_buf());
    config.exec_gate = Some(gate);
    confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root.to_path_buf())),
    )))
}

/// Starts the shell with `script` under `profile`: what the run printed, or the refusal of the
/// admission or the start.
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
    format!("ran {:?}", String::from_utf8_lossy(&output.stdout))
}

fn profile(workspace: &Path, entries: &[&str]) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        exec_subpaths: entries.iter().map(|entry| (*entry).to_owned()).collect(),
        ..ConfinementProfile::default()
    }
}

/// The adapter of a case inside its namespace, with `exec_subpaths` proved, or `None` after
/// printing why the case is absent there.
fn opened_inside(
    name: &str,
    workspace_root: &Path,
) -> Option<(Arc<dyn ConfinementAdapter>, PathBuf)> {
    let absent = |reason: &str| {
        println!("{name} inside: ABSENT ({reason})");
        None
    };
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => return absent(&reason),
    };
    match open(workspace_root, &cgroup_root) {
        Ok(adapter) if adapter.facts().proves(&Fact::ExecSubpaths) => Some((adapter, cgroup_root)),
        Ok(adapter) => absent(&format!(
            "the adapter did not prove exec_subpaths: {:?}",
            adapter.facts()
        )),
        Err(error) => absent(&format!("linux did not open: {error}")),
    }
}

/// Runs `case` again inside a user and mount namespace after `stage`, a shell fragment whose
/// positional parameters are `arguments`: what that run printed, with its diagnostics after
/// it, or why the case is absent.
fn staged(
    case: &str,
    host: &Path,
    kind: &str,
    stage: &str,
    arguments: &[PathBuf],
) -> Result<String, String> {
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
            "{{ {stage}; }} >&2 || exit {STAGE_FAILED}; shift {}; exec \"$@\"",
            arguments.len()
        ))
        .arg("sh")
        .args(arguments)
        .arg(setpriv)
        .args(["--inh-caps=-all", "--ambient-caps=-all", "--"])
        .arg(std::env::current_exe().expect("this test binary"))
        .args(["--exact", case, "--nocapture", "--test-threads=1"])
        .env(INNER, host)
        .env(KIND, kind)
        .output()
        .expect("unshare starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(STAGE_FAILED) {
        return Err(format!(
            "no {kind} mount in a user namespace: {}",
            stderr.trim()
        ));
    }
    if output.status.code() == Some(1) && stderr.trim_start().starts_with("unshare:") {
        return Err(format!("no user and mount namespace: {}", stderr.trim()));
    }
    Ok(format!("{stdout}\n{stderr}"))
}

/// The observation `what` of the run inside the namespace, from what it printed.
fn observed(name: &str, stdout: &str, what: &str) -> Option<String> {
    let inside = format!("{name} inside: ");
    stdout
        .lines()
        // The harness writes `test <name> ... ` before the first line of a case.
        .filter_map(|line| line.split_once(inside.as_str()).map(|(_, rest)| rest))
        .find_map(|line| line.strip_prefix(what))
        .map(|rest| rest.trim().to_owned())
}

const SKIP: &str = "security5 view mounts link through a skipped mount";

/// The link case inside its namespace: one file system is mounted at
/// `<host>/workspaces/ws_other/media` and, the same one, at `<host>/tools/media`.
async fn link_case_inside(host: &Path) {
    let Some((adapter, cgroup_root)) = opened_inside(SKIP, &host.join("workspaces")) else {
        return;
    };
    let Some(program) = probe_program() else {
        println!("{SKIP} inside: ABSENT (/usr/bin holds none of the probed programs as a file)");
        return;
    };
    let script = format!("{program} --version >/dev/null 2>&1; echo $?");
    let workspace = host.join("workspaces/ws_exec");
    std::fs::create_dir_all(workspace.join("bin")).expect("bin");
    // A run whose workspace holds the mount plants the link: no list, nothing staged for it.
    println!(
        "{SKIP} inside: planted {}",
        run(
            &adapter,
            &profile(&host.join("workspaces/ws_other"), &[]),
            "ln -s /usr/bin media/link 2>&1; echo $?"
        )
        .await
    );
    let with_root = |entry: &str| ConfinementProfile {
        read_only_roots: vec![ReadOnlyRoot {
            host_path: text(&host.join("tools")),
            mount: "/opt/k".to_owned(),
        }],
        ..profile(&workspace, &[entry])
    };
    println!(
        "{SKIP} inside: control {}",
        run(&adapter, &with_root("bin"), &script).await
    );
    println!(
        "{SKIP} inside: listed {}",
        run(&adapter, &with_root("/opt/k/media/link"), &script).await
    );
    drop(adapter);
    // The helper's own file on the mount: a run with that workspace could replace it.
    let gate = host.join("workspaces/ws_other/media/substrate-exec-gate");
    std::fs::copy(helper(), &gate).expect("install the helper on the mount");
    match open_with(&host.join("workspaces"), &cgroup_root, gate) {
        Ok(adapter) => println!(
            "{SKIP} inside: gate opened, exec_subpaths proved: {}",
            adapter.facts().proves(&Fact::ExecSubpaths)
        ),
        Err(error) => println!("{SKIP} inside: gate refused {}: {error}", error.code()),
    }
}

/// **A link on a mount the rule skips is not followed out of a read-only root** (design 35
/// D24: a path that "has a symbolic link in a mount that shows something a run can write" is
/// refused; D30: "No writable place is called read-only").
///
/// One file system is mounted beneath a workspace and, a second time, beneath the host
/// directory of a declared read-only root. A run with that workspace plants `link`, a symbolic
/// link to `/usr/bin`, on it. A later profile declares the root at `/opt/k` and lists
/// `/opt/k/media/link`. The control lists `bin` instead, and a program of `/usr/bin` ends with
/// status 126.
///
/// - On a **tmpfs**, a type of the list, the root reaches the mount and so does the workspace
///   root: the root is classed `beneath` and the start is refused before dispatch.
/// - On a **ramfs**, a type outside the list, the mount is skipped on both sides. The same
///   start must not run a program of `/usr/bin` either: it is refused, or the program ends
///   with status 126, or the helper refuses.
///
/// On both, the helper's own file installed on the mount is not taken with the lists served.
#[tokio::test(flavor = "multi_thread")]
async fn security5_a_link_on_a_mount_the_rule_skips_is_not_followed_out_of_a_read_only_root() {
    const CASE: &str =
        "security5_a_link_on_a_mount_the_rule_skips_is_not_followed_out_of_a_read_only_root";
    if let Some(host) = std::env::var_os(INNER) {
        link_case_inside(Path::new(&host)).await;
        return;
    }
    let Some(mount) = find("mount") else {
        println!("{SKIP}: ABSENT (no mount on this host)");
        return;
    };
    let mut seen = Vec::new();
    for kind in ["tmpfs", "ramfs"] {
        let directory = tempfile::tempdir().expect("host root");
        let host = directory
            .path()
            .canonicalize()
            .expect("canonical host root");
        let mounts = [
            host.join("workspaces/ws_other/media"),
            host.join("tools/media"),
        ];
        for mount_point in &mounts {
            std::fs::create_dir_all(mount_point).expect("a mount point");
        }
        let stage = format!(
            "{mount} -t {kind} {kind} \"$1\" && {mount} --bind \"$1\" \"$2\"",
            mount = mount.display()
        );
        let stdout = match staged(CASE, &host, kind, &stage, &mounts) {
            Ok(stdout) => stdout,
            Err(reason) => {
                println!("{SKIP}: ABSENT ({reason})");
                return;
            }
        };
        if let Some(reason) = observed(SKIP, &stdout, "ABSENT") {
            println!("{SKIP}: ABSENT (inside the namespace: {reason})");
            return;
        }
        let line = |what: &str| {
            observed(SKIP, &stdout, what)
                .unwrap_or_else(|| panic!("{kind}: no `{what}` line: {stdout}"))
        };
        assert_eq!(
            line("planted"),
            "ran \"0\\n\"",
            "{kind}: a run whose workspace holds the mount writes a link on it"
        );
        assert_eq!(
            line("control"),
            "ran \"126\\n\"",
            "{kind}: the control: with `bin` listed a program of /usr/bin does not run"
        );
        let gate = line("gate");
        assert!(
            !gate.ends_with("proved: true"),
            "{kind}: the helper's file on a mount beneath a workspace was taken, and the lists \
             are served with it: {gate}"
        );
        println!("{SKIP}: {kind}: the helper's file on the mount: {gate}");
        seen.push((kind, line("listed")));
    }
    println!("{SKIP}: {seen:?}");
    assert!(
        seen[0]
            .1
            .starts_with("refused exec.sandbox-path-unrepresentable"),
        "tmpfs: the root that holds a mount a run can write was classed `follow`: {seen:?}"
    );
    assert_ne!(
        seen[1].1, "ran \"0\\n\"",
        "ramfs: the same file system is mounted beneath a workspace and beneath a read-only \
         root, a run planted a link to /usr/bin on it, and a profile that lists the link ran a \
         program of /usr/bin: the mount is of a type outside the list, so it was compared on \
         neither side, the root was classed `follow`, and the link was followed (tmpfs, the \
         same shape on a listed type: {})",
        seen[0].1
    );
    println!("{SKIP}: executed");
}

const ORDINARY: &str = "security5 view mounts ordinary";

/// The ordinary case inside its namespace: a tmpfs at `/usr/local/src`, two stacked at
/// `<host>/elsewhere`, and two stacked at `<host>/b/workspaces/ws_exec/stack`.
async fn ordinary_case_inside(host: &Path) {
    let Some((adapter, cgroup_root)) = opened_inside(ORDINARY, &host.join("a/workspaces")) else {
        return;
    };
    let Some(program) = probe_program() else {
        println!("{ORDINARY} inside: ABSENT (/usr/bin holds none of the probed programs)");
        return;
    };
    let workspace = host.join("a/workspaces/ws_exec");
    std::fs::create_dir_all(workspace.join("bin")).expect("bin");
    let tool = find("true").expect("true");
    std::fs::copy(&tool, workspace.join("bin/tool")).expect("copy true");
    std::fs::copy(&tool, "/usr/local/src/tool").expect("copy true onto the mount beneath /usr");
    println!(
        "{ORDINARY} inside: listed {}",
        run(
            &adapter,
            &profile(&workspace, &["bin"]),
            &format!(
                "bin/tool; echo listed:$?; {program} --version >/dev/null 2>&1; echo unlisted:$?"
            )
        )
        .await
    );
    println!(
        "{ORDINARY} inside: beneath {}",
        run(
            &adapter,
            &profile(&workspace, &["bin", "/usr/local"]),
            "/usr/local/src/tool; echo mount:$?"
        )
        .await
    );
    println!(
        "{ORDINARY} inside: unlisted {}",
        run(
            &adapter,
            &profile(&workspace, &[]),
            "/usr/local/src/tool; echo plain:$?"
        )
        .await
    );
    drop(adapter);
    match open(&host.join("b/workspaces"), &cgroup_root) {
        Ok(adapter) => println!(
            "{ORDINARY} inside: stack opened, exec_subpaths proved: {}",
            adapter.facts().proves(&Fact::ExecSubpaths)
        ),
        Err(error) => println!("{ORDINARY} inside: stack refused {}: {error}", error.code()),
    }
}

/// **Nothing ordinary breaks, and a stack beneath a writable location fails closed** (design
/// 35 D30, rule 5 and the fail-closed table).
///
/// The host has a mount beneath `/usr` and, somewhere no location reaches, two mounts stacked
/// on one mount point. A listed start runs its listed program and refuses an unlisted one; a
/// listed directory admits what is mounted beneath it; a start without a list runs as before.
/// A second adapter, whose workspace root holds a stack, is refused at the open
/// `config.exec-gate-unserved` with the cause `mount-covered`.
#[tokio::test(flavor = "multi_thread")]
async fn security5_ordinary_mounts_do_not_refuse_and_a_stack_beneath_a_writable_location_does() {
    const CASE: &str =
        "security5_ordinary_mounts_do_not_refuse_and_a_stack_beneath_a_writable_location_does";
    const COVERED: &str = "/usr/local/src";
    if let Some(host) = std::env::var_os(INNER) {
        ordinary_case_inside(Path::new(&host)).await;
        return;
    }
    let Some(mount) = find("mount") else {
        println!("{ORDINARY}: ABSENT (no mount on this host)");
        return;
    };
    if !Path::new(COVERED).is_dir() {
        println!("{ORDINARY}: ABSENT (this host has no {COVERED} to mount over)");
        return;
    }
    let directory = tempfile::tempdir().expect("host root");
    let host = directory
        .path()
        .canonicalize()
        .expect("canonical host root");
    let mounts = [
        host.join("elsewhere"),
        host.join("b/workspaces/ws_exec/stack"),
    ];
    for path in mounts.iter().chain([&host.join("a/workspaces")]) {
        std::fs::create_dir_all(path).expect("a directory");
    }
    let stage = format!(
        "{mount} -t tmpfs tmpfs {COVERED} && {mount} -t tmpfs tmpfs \"$1\" \
         && {mount} -t tmpfs tmpfs \"$1\" && {mount} -t tmpfs tmpfs \"$2\" \
         && {mount} -t tmpfs tmpfs \"$2\"",
        mount = mount.display()
    );
    let stdout = match staged(CASE, &host, "tmpfs", &stage, &mounts) {
        Ok(stdout) => stdout,
        Err(reason) => {
            println!("{ORDINARY}: ABSENT ({reason})");
            return;
        }
    };
    if let Some(reason) = observed(ORDINARY, &stdout, "ABSENT") {
        println!("{ORDINARY}: ABSENT (inside the namespace: {reason})");
        return;
    }
    let line = |what: &str| {
        observed(ORDINARY, &stdout, what).unwrap_or_else(|| panic!("no `{what}` line: {stdout}"))
    };
    assert_eq!(
        line("listed"),
        "ran \"listed:0\\nunlisted:126\\n\"",
        "a listed start on a host with a mount beneath /usr and a stack elsewhere"
    );
    assert_eq!(
        line("beneath"),
        "ran \"mount:0\\n\"",
        "a listed directory admits what is mounted beneath it"
    );
    assert_eq!(
        line("unlisted"),
        "ran \"plain:0\\n\"",
        "a start without a list"
    );
    let stack = line("stack");
    assert!(
        stack.starts_with("refused config.exec-gate-unserved") && stack.contains("mount-covered"),
        "a stack of two mounts beneath the workspace root did not refuse the open by name: \
         {stack}"
    );
    println!("{ORDINARY}: executed, {stack}");
}
