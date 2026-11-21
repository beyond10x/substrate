//! Security review of design 35 D30 as amended (§ 18, *Two decisions after the first review of
//! that branch*), through the real `linux` adapter and the lane's statically linked helper.
//!
//! `security5_view_mounts.rs` stages one file system beneath a workspace and beneath a
//! read-only root's source. Since the amendment that shape is answered by the comparison of
//! two table rows of an unlisted type, which classes the root `beneath`. These cases ask what
//! is left when that comparison has nothing to compare, and what the read-only side's count
//! must not break:
//!
//! - **the link rule alone**: a mount beneath a read-only root's source *only*, with a link on
//!   it. No writable location reaches it and no second row names its device, which is all the
//!   driver sees of a file system that has two device numbers. On a `ramfs`, a type outside
//!   the list, the link has no place and the start is refused `exec.sandbox-place-unserved`;
//!   on a tmpfs, the same shape on a listed type, the link is placed and followed;
//! - **a root whose mounts this user cannot ask starts**: `/var/lib` as a read-only root, on a
//!   host that has mounts beneath it this user cannot open, with a list and without one.
//!
//! The first case makes its mounts in a user and mount namespace of its own and runs itself
//! again there. The cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2
//! subtree this process is inside, bubblewrap and `SUBSTRATE_EXEC_GATE` naming the static
//! helper. Without one of them, or without what a case names, it prints
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
const INNER: &str = "SECURITY6_HOST";

/// What the stage of that namespace exits with when one of its commands failed.
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

/// The adapter over `workspace_root`, with `exec_subpaths` proved, or why the case is absent.
fn open(workspace_root: &Path) -> Result<Arc<dyn ConfinementAdapter>, String> {
    let cgroup_root = delegated()?;
    let mut config = HostConfig::minimum(workspace_root.to_path_buf());
    config.exec_gate = Some(helper());
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .map_err(|error| format!("linux did not open: {error}"))?;
    if !adapter.facts().proves(&Fact::ExecSubpaths) {
        return Err(format!(
            "the adapter did not prove exec_subpaths: {:?}",
            adapter.facts()
        ));
    }
    Ok(adapter)
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

/// A profile over `workspace` with `root` declared read-only at `mount` and `entries` listed.
fn profile(workspace: &Path, root: &str, mount: &str, entries: &[&str]) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        exec_subpaths: entries.iter().map(|entry| (*entry).to_owned()).collect(),
        read_only_roots: vec![ReadOnlyRoot {
            host_path: root.to_owned(),
            mount: mount.to_owned(),
        }],
        ..ConfinementProfile::default()
    }
}

/// Runs `case` again inside a user and mount namespace after `stage`, a shell fragment whose
/// first positional parameter is `mount_point`: what that run printed, with its diagnostics
/// after it, or why the case is absent.
fn staged(case: &str, host: &Path, stage: &str, mount_point: &Path) -> Result<String, String> {
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
            "{{ {stage}; }} >&2 || exit {STAGE_FAILED}; shift 1; exec \"$@\""
        ))
        .arg("sh")
        .arg(mount_point)
        .arg(setpriv)
        .args(["--inh-caps=-all", "--ambient-caps=-all", "--"])
        .arg(std::env::current_exe().expect("this test binary"))
        .args(["--exact", case, "--nocapture", "--test-threads=1"])
        .env(INNER, host)
        .output()
        .expect("unshare starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(STAGE_FAILED) {
        return Err(format!(
            "no such mount in a user namespace: {}",
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

const ROOT_ONLY: &str = "security6 view mounts link on a mount beneath a root only";

/// The case inside its namespace: one file system is mounted at `<host>/tools/media`, with
/// `link`, a link to `/usr/bin`, and the directory `dir` on it; `<host>/tools/plain` is a
/// directory of the host's own file system.
async fn root_only_inside(host: &Path) {
    let absent = |reason: &str| println!("{ROOT_ONLY} inside: ABSENT ({reason})");
    let adapter = match open(&host.join("workspaces")) {
        Ok(adapter) => adapter,
        Err(reason) => return absent(&reason),
    };
    let Some(program) = probe_program() else {
        return absent("/usr/bin holds none of the probed programs as a file");
    };
    let script = format!("{program} --version >/dev/null 2>&1; echo $?");
    let workspace = host.join("workspaces/ws_exec");
    std::fs::create_dir_all(workspace.join("bin")).expect("bin");
    let tools = text(&host.join("tools"));
    for (what, entry, script) in [
        ("control", "bin", script.as_str()),
        ("listed", "/opt/k/media/link", script.as_str()),
        ("end", "/opt/k/media/dir", "echo ran"),
        ("apart", "/opt/k/plain", "/opt/k/plain/true; echo $?"),
    ] {
        println!(
            "{ROOT_ONLY} inside: {what} {}",
            run(
                &adapter,
                &profile(&workspace, &tools, "/opt/k", &[entry]),
                script
            )
            .await
        );
    }
}

/// **A link on a mount no writable location reaches is placed all the same** (design 35 D30
/// as amended: "Every symbolic link a followed entry's walk reads is placed, in the driver and
/// in the helper").
///
/// A file system is mounted beneath the host directory of a declared read-only root and
/// nowhere else, and holds `link`, a symbolic link to `/usr/bin`. A profile declares the root
/// at `/opt/k` and lists `/opt/k/media/link`. No workspace holds the mount, so the comparison
/// of two rows of an unlisted type has no second row: this is all the driver sees of a file
/// system that is mounted under two device numbers, the case that comparison misses by
/// construction.
///
/// - On a **ramfs**, a type outside the list, the link has no place: the start is refused
///   `exec.sandbox-place-unserved` with `file-system-unlisted ramfs`, and so is an entry that
///   ends on the mount.
/// - On a **tmpfs**, a type of the list, the link is placed, nothing writable reaches it, and
///   it is followed: a program of `/usr/bin` runs.
///
/// On both, an entry of the same root that stays apart from the mount runs, and the control,
/// which lists `bin` only, ends a program of `/usr/bin` with status 126.
#[tokio::test(flavor = "multi_thread")]
async fn security6_a_link_on_an_unlisted_mount_beneath_a_root_only_refuses_for_its_place() {
    const CASE: &str =
        "security6_a_link_on_an_unlisted_mount_beneath_a_root_only_refuses_for_its_place";
    if let Some(host) = std::env::var_os(INNER) {
        root_only_inside(Path::new(&host)).await;
        return;
    }
    let (Some(mount), Some(ln), Some(mkdir), Some(tool)) =
        (find("mount"), find("ln"), find("mkdir"), find("true"))
    else {
        println!("{ROOT_ONLY}: ABSENT (no mount, ln, mkdir or true on this host)");
        return;
    };
    let mut seen = Vec::new();
    for kind in ["tmpfs", "ramfs"] {
        let directory = tempfile::tempdir().expect("host root");
        let host = directory
            .path()
            .canonicalize()
            .expect("canonical host root");
        let mount_point = host.join("tools/media");
        for path in [&mount_point, &host.join("tools/plain")] {
            std::fs::create_dir_all(path).expect("a directory");
        }
        std::fs::copy(&tool, host.join("tools/plain/true")).expect("copy true");
        let stage = format!(
            "{mount} -t {kind} {kind} \"$1\" && {ln} -s /usr/bin \"$1/link\" && {mkdir} \"$1/dir\"",
            mount = mount.display(),
            ln = ln.display(),
            mkdir = mkdir.display()
        );
        let stdout = match staged(CASE, &host, &stage, &mount_point) {
            Ok(stdout) => stdout,
            Err(reason) => {
                println!("{ROOT_ONLY}: ABSENT ({kind}: {reason})");
                return;
            }
        };
        if let Some(reason) = observed(ROOT_ONLY, &stdout, "ABSENT") {
            println!("{ROOT_ONLY}: ABSENT (inside the namespace: {reason})");
            return;
        }
        let line = |what: &str| {
            observed(ROOT_ONLY, &stdout, what)
                .unwrap_or_else(|| panic!("{kind}: no `{what}` line: {stdout}"))
        };
        assert_eq!(
            line("control"),
            "ran \"126\\n\"",
            "{kind}: the control: with `bin` listed a program of /usr/bin does not run"
        );
        assert_eq!(
            line("apart"),
            "ran \"0\\n\"",
            "{kind}: an entry of the root that stays apart from the mount beneath it"
        );
        seen.push((kind, line("listed"), line("end")));
    }
    println!("{ROOT_ONLY}: {seen:?}");
    let (_, listed, end) = &seen[0];
    assert_eq!(
        (listed.as_str(), end.as_str()),
        ("ran \"0\\n\"", "ran \"ran\\n\""),
        "tmpfs: a link on a listed type that nothing writable reaches is placed and followed"
    );
    for (what, answer) in [("the link", &seen[1].1), ("the end", &seen[1].2)] {
        assert!(
            answer.starts_with("refused exec.sandbox-place-unserved")
                && answer.contains("file-system-unlisted ramfs"),
            "ramfs: {what} on a mount of a type outside the list has no place, and the start \
             was not refused for that: {answer}"
        );
    }
    println!("{ROOT_ONLY}: executed");
}

const UNASKED: &str = "security6 view mounts root with mounts this user cannot open";

/// The mount points beneath `directory` in this process's mount table, each with whether this
/// user can open it.
fn mounts_beneath(directory: &str) -> Vec<(String, bool)> {
    let prefix = format!("{directory}/");
    std::fs::read_to_string("/proc/self/mountinfo")
        .expect("the mount table")
        .lines()
        .filter_map(|line| line.split(' ').nth(4))
        .filter(|mount_point| mount_point.starts_with(&prefix))
        .map(|mount_point| {
            (
                mount_point.to_owned(),
                std::fs::File::open(mount_point).is_ok(),
            )
        })
        .collect()
}

/// **A root whose mounts this user cannot ask starts** (design 35 D30 as amended: "On the
/// read-only side a mount that cannot be asked, or is covered, never refuses").
///
/// `/var/lib` is declared a read-only root on a host that has mounts beneath it this user
/// cannot open, a container engine's layers for one. Measured before the amendment: such a
/// start was refused `path-unopenable`. A listed start runs its listed program, an unlisted
/// program of `/usr/bin` ends with status 126, the root itself listed as an entry starts too,
/// and a start without a list runs as before.
#[tokio::test(flavor = "multi_thread")]
async fn security6_a_root_whose_mounts_this_user_cannot_open_starts_with_a_list() {
    const ROOT: &str = "/var/lib";
    let beneath = mounts_beneath(ROOT);
    let unopenable = beneath.iter().filter(|(_, opens)| !opens).count();
    if unopenable == 0 {
        println!(
            "{UNASKED}: ABSENT (this host has no mount beneath {ROOT} this user cannot open, \
             of {} beneath it)",
            beneath.len()
        );
        return;
    }
    let directory = tempfile::tempdir().expect("host root");
    let host = directory
        .path()
        .canonicalize()
        .expect("canonical host root");
    let workspace = host.join("workspaces/ws_exec");
    std::fs::create_dir_all(workspace.join("bin")).expect("bin");
    std::fs::copy(find("true").expect("true"), workspace.join("bin/tool")).expect("copy true");
    let adapter = match open(&host.join("workspaces")) {
        Ok(adapter) => adapter,
        Err(reason) => {
            println!("{UNASKED}: ABSENT ({reason})");
            return;
        }
    };
    let Some(program) = probe_program() else {
        println!("{UNASKED}: ABSENT (/usr/bin holds none of the probed programs as a file)");
        return;
    };
    let script = format!("bin/tool; echo $?; {program} --version >/dev/null 2>&1; echo $?");
    for (what, entries, expected) in [
        ("a listed start", &["bin"][..], "ran \"0\\n126\\n\""),
        (
            "the root itself listed",
            &["bin", "/opt/state"][..],
            "ran \"0\\n126\\n\"",
        ),
        ("a start without a list", &[][..], "ran \"0\\n0\\n\""),
    ] {
        assert_eq!(
            run(
                &adapter,
                &profile(&workspace, ROOT, "/opt/state", entries),
                &script
            )
            .await,
            expected,
            "{what}, with {ROOT} a read-only root and {unopenable} of the {} mounts beneath it \
             not openable by this user",
            beneath.len()
        );
    }
    println!(
        "{UNASKED}: executed, {unopenable} of {} mounts beneath {ROOT} not openable",
        beneath.len()
    );
}

const THROUGH: &str = "security6 view mounts trusted link through the workspace";

/// **A trusted link whose target passes through a directory of the workspace names what its
/// author named, or is refused** (design 35 D24, D30 as amended).
///
/// A declared read-only root holds `through`, a link to
/// `/workspace/a/d/../../../opt/k/plain`: the target enters the workspace, a mount the run can
/// write, and leaves it again with `..`. No link lies in the workspace, and the end,
/// `/opt/k/plain`, is in the root. The profile lists `/opt/k/through`.
///
/// This is the shape `security6_gate.rs` moves a directory under. Held: the start is refused
/// before dispatch.
///
/// **Pinned by the implementor in the third round of the fix, on the coordinator's
/// decision.** The case accepted either answer: the refusal, or a run in which the program of
/// `plain` starts and the one of `other`, a second directory of the root, does not. The
/// decision: a followed path whose walk visits any directory in a place a writable location
/// reaches is refused, by the driver before dispatch with
/// `exec.sandbox-path-unrepresentable`. Only the refusal is taken now.
#[tokio::test(flavor = "multi_thread")]
async fn security6_a_trusted_link_through_a_workspace_directory_names_what_its_author_named() {
    let directory = tempfile::tempdir().expect("host root");
    let host = directory
        .path()
        .canonicalize()
        .expect("canonical host root");
    let workspace = host.join("workspaces/ws_exec");
    std::fs::create_dir_all(workspace.join("a/d")).expect("a/d");
    let tool = find("true").expect("true");
    for name in ["plain", "other"] {
        std::fs::create_dir_all(host.join("tools").join(name)).expect("a directory of the root");
        std::fs::copy(&tool, host.join("tools").join(name).join("true")).expect("copy true");
    }
    std::os::unix::fs::symlink(
        "/workspace/a/d/../../../opt/k/plain",
        host.join("tools/through"),
    )
    .expect("the trusted link");
    let adapter = match open(&host.join("workspaces")) {
        Ok(adapter) => adapter,
        Err(reason) => {
            println!("{THROUGH}: ABSENT ({reason})");
            return;
        }
    };
    let answer = run(
        &adapter,
        &profile(
            &workspace,
            &text(&host.join("tools")),
            "/opt/k",
            &["/opt/k/through"],
        ),
        "/opt/k/plain/true; echo $?; /opt/k/other/true; echo $?",
    )
    .await;
    println!("{THROUGH}: {answer}");
    assert!(
        answer.starts_with("refused exec.sandbox-path-unrepresentable"),
        "a trusted link through a directory of the workspace: {answer}"
    );
    println!("{THROUGH}: executed");
}
