//! Design 35 D30, *Through the view's mounts*, on the checks of
//! `story:linux-executable-allow-list` that ask whether something lies where a run can write:
//! **what is mounted beneath a writable location lies in it**, although its place is on another
//! file system.
//!
//! A host mount beneath `workspace_root`, a declared writable root or a workspace is carried
//! into a run's view by the recursive bind of that location, and the run can write it. Each
//! case here has such a mount, made in a user and mount namespace of its own, and opens the
//! real `linux` adapter there with the lane's statically linked helper:
//!
//! - a read-only root declared on the mount holds a symbolic link a run planted, and the
//!   profile lists the link (K10's attack across a mount);
//! - a read-only root holds a mount on which a declared writable root lies, and the profile
//!   lists a link planted through that root;
//! - a read-only root holds a mount of a file-system type outside the list, which no rule
//!   compares, and the profile lists a link on it: the link has no place;
//! - the helper's own file is installed on the mount.
//!
//! The cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this
//! process is inside, bubblewrap, `SUBSTRATE_EXEC_GATE` naming the static helper, and a host
//! whose user namespaces may mount a tmpfs. Without one of them each prints
//! `<name>: ABSENT (<reason>)` and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile,
    DeclaredWritableRoot, Fact, ReadOnlyRoot, RunState, linux::LinuxRequest,
};

const NAME: &str = "ESS confinement linux exec view mounts";

/// The one case of this file, as the harness names it.
const CASE: &str = "what_is_mounted_beneath_a_writable_location_lies_in_it";

/// Names the host root, in the process this file starts inside a mount namespace of its own.
const INNER: &str = "SUBSTRATE_VIEW_MOUNTS_HOST";

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

/// The helper the lane built. A lane with the delegated root and no helper fails here.
fn helper() -> PathBuf {
    PathBuf::from(std::env::var_os("SUBSTRATE_EXEC_GATE").expect(
        "SUBSTRATE_EXEC_GATE is unset: `bash scripts/delegated-lane.sh` builds \
         substrate-exec-gate and names it",
    ))
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
    host: &Path,
    cgroup_root: &Path,
    gate: PathBuf,
    writable_roots: Vec<DeclaredWritableRoot>,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    let mut config = HostConfig::minimum(host.join("workspaces"));
    config.exec_gate = Some(gate);
    config.writable_roots = writable_roots;
    confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root.to_path_buf())),
    )))
}

/// Starts the shell with `script` under a profile that declares `root` at `/opt/k` and lists
/// `entry`: what the run printed, or the refusal of the admission or the start.
async fn run(
    adapter: &Arc<dyn ConfinementAdapter>,
    workspace: &Path,
    root: &Path,
    entry: &str,
    script: &str,
) -> String {
    let profile = ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        read_only_roots: vec![ReadOnlyRoot {
            host_path: text(root),
            mount: "/opt/k".to_owned(),
        }],
        exec_subpaths: vec![entry.to_owned()],
        ..ConfinementProfile::default()
    };
    let command = Command {
        argv: vec![
            text(&find("sh").expect("a shell")),
            "-c".to_owned(),
            script.to_owned(),
        ],
        env: BTreeMap::new(),
    };
    let refused = |error: ConfinementError| format!("refused {}: {error}", error.code());
    let admitted = match adapter.admit(&profile) {
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

/// The case inside its namespace, where `<host>/workspaces/ws_other/media` and
/// `<host>/tools/media` are mounts. Prints one line per observation.
async fn inside(host: &Path) {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("{NAME} inside: ABSENT ({reason})");
            return;
        }
    };
    let Some(program) = probe_program() else {
        println!("{NAME} inside: ABSENT (/usr/bin holds none of the probed programs as a file)");
        return;
    };
    let script = format!("{program} --version >/dev/null 2>&1; echo $?");
    let workspace = host.join("workspaces/ws_exec");
    std::fs::create_dir_all(workspace.join("bin")).expect("bin");

    // What a run with the workspace `ws_other` left on the mount beneath it: a link out.
    let planted = host.join("workspaces/ws_other/media/tools");
    std::fs::create_dir_all(&planted).expect("a directory a run wrote");
    std::os::unix::fs::symlink("/usr/bin", planted.join("link")).expect("a planted link");
    let adapter = match open(host, &cgroup_root, helper(), Vec::new()) {
        Ok(adapter) if adapter.facts().proves(&Fact::ExecSubpaths) => adapter,
        Ok(adapter) => {
            println!(
                "{NAME} inside: ABSENT (the adapter did not prove exec_subpaths: {:?})",
                adapter.facts()
            );
            return;
        }
        Err(error) => {
            println!("{NAME} inside: ABSENT (linux did not open: {error})");
            return;
        }
    };
    println!(
        "{NAME} inside: control {}",
        run(&adapter, &workspace, &planted, "bin", &script).await
    );
    println!(
        "{NAME} inside: planted {}",
        run(&adapter, &workspace, &planted, "/opt/k/link", &script).await
    );
    drop(adapter);

    // The other direction: a read-only root that holds a mount on which a declared writable
    // root lies. A run that was given that root left a link in it.
    let tools = host.join("tools");
    let out = tools.join("media/out");
    std::fs::create_dir_all(&out).expect("the writable root");
    std::os::unix::fs::symlink("/usr/bin", out.join("link")).expect("a planted link");
    let declared = vec![DeclaredWritableRoot {
        name: "out".to_owned(),
        path: out,
    }];
    match open(host, &cgroup_root, helper(), declared) {
        Ok(adapter) => println!(
            "{NAME} inside: holding {}",
            run(
                &adapter,
                &workspace,
                &tools,
                "/opt/k/media/out/link",
                &script
            )
            .await
        ),
        Err(error) => println!("{NAME} inside: holding did not open: {error}"),
    }

    // A link on a mount of a type outside the list, which nothing writable shows and no
    // rule compares: where the link lies cannot be said.
    let apart = host.join("apart");
    std::os::unix::fs::symlink("/usr/bin", apart.join("media/link")).expect("a link on the ramfs");
    match open(host, &cgroup_root, helper(), Vec::new()) {
        Ok(adapter) => println!(
            "{NAME} inside: unlisted {}",
            run(&adapter, &workspace, &apart, "/opt/k/media/link", &script).await
        ),
        Err(error) => println!("{NAME} inside: unlisted did not open: {error}"),
    }

    // The helper's own file on the mount beneath the workspace root.
    let gate = host.join("workspaces/ws_other/media/substrate-exec-gate");
    std::fs::copy(helper(), &gate).expect("install the helper on the mount");
    match open(host, &cgroup_root, gate, Vec::new()) {
        Ok(adapter) => println!(
            "{NAME} inside: gate opened, exec_subpaths proved: {}",
            adapter.facts().proves(&Fact::ExecSubpaths)
        ),
        Err(error) => println!("{NAME} inside: gate refused {}: {error}", error.code()),
    }
}

/// **D30 on D21 and D24, staged.** Inside the namespace a tmpfs is mounted at
/// `<workspace root>/ws_other/media` and another at `<host>/tools/media`:
///
/// - `planted`: a read-only root declared on the first mount holds `link`, a symbolic link to
///   `/usr/bin` a run could have planted, and the profile lists it. The root is classed
///   `beneath`, so the start is refused `exec.sandbox-path-unrepresentable` before dispatch
///   and nothing of `/usr/bin` runs through it. The control lists `bin` instead, and a program
///   of `/usr/bin` ends with status 126;
/// - `holding`: a read-only root holds the second mount, on which a declared writable root
///   lies with such a link. The same refusal;
/// - `unlisted`: a third mount, a `ramfs` at `<host>/apart/media`, is of a type outside the
///   list. A read-only root holds it and the profile lists a link on it. The link has no
///   place, so the start is refused `exec.sandbox-place-unserved` with the cause;
/// - `gate`: the helper's file on the first mount is refused at the open,
///   `config.exec-gate-invalid`.
#[tokio::test(flavor = "multi_thread")]
async fn what_is_mounted_beneath_a_writable_location_lies_in_it() {
    if let Some(host) = std::env::var_os(INNER) {
        inside(Path::new(&host)).await;
        return;
    }
    let stdout = match staged() {
        Ok(stdout) => stdout,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let inside = format!("{NAME} inside: ");
    let line = |what: &str| {
        stdout
            .lines()
            // The harness writes `test <name> ... ` before the first line of a case.
            .filter_map(|line| line.split_once(inside.as_str()).map(|(_, rest)| rest))
            .find_map(|line| line.strip_prefix(what))
            .map(|rest| rest.trim().to_owned())
    };
    if let Some(reason) = line("ABSENT") {
        println!("{NAME}: ABSENT (inside the namespace: {reason})");
        return;
    }
    let observed = |what: &str| {
        line(what).unwrap_or_else(|| {
            panic!("the run inside the namespace printed no `{what}` line: {stdout}")
        })
    };
    assert_eq!(
        observed("control"),
        "ran \"126\\n\"",
        "the control: with `bin` listed a program of /usr/bin does not run"
    );
    let mut failures = Vec::new();
    for (what, shape) in [
        (
            "planted",
            "a read-only root declared on a mount beneath the workspace root",
        ),
        (
            "holding",
            "a read-only root that holds a mount on which a declared writable root lies",
        ),
    ] {
        let seen = observed(what);
        println!("{NAME}: {what}: {seen}");
        if !seen.starts_with("refused exec.sandbox-path-unrepresentable") {
            failures.push(format!(
                "{shape} was classed `follow` and the planted link was followed: {seen}"
            ));
        }
    }
    let unlisted = observed("unlisted");
    println!("{NAME}: unlisted: {unlisted}");
    if !(unlisted.starts_with("refused exec.sandbox-place-unserved")
        && unlisted.contains("/opt/k/media/link: file-system-unlisted ramfs"))
    {
        failures.push(format!(
            "a link on a mount of a type outside the list was followed, or refused for another \
             reason: {unlisted}"
        ));
    }
    let gate = observed("gate");
    println!("{NAME}: gate: {gate}");
    if !gate.starts_with("refused config.exec-gate-invalid") {
        failures.push(format!(
            "the helper's file on a mount beneath the workspace root was taken: {gate}"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME}: executed");
}

/// Runs this case again inside a user and mount namespace in which the two tmpfs and the
/// ramfs are mounted:
/// what that run printed, with its diagnostics after it, or why the case is absent.
fn staged() -> Result<String, String> {
    delegated()?;
    let (Some(unshare), Some(mount), Some(setpriv), Some(shell)) =
        (find("unshare"), find("mount"), find("setpriv"), find("sh"))
    else {
        return Err("no unshare, mount, setpriv or sh on this host".to_owned());
    };
    let directory = tempfile::tempdir().expect("host root");
    let host = directory
        .path()
        .canonicalize()
        .expect("canonical host root");
    let mounts = [
        host.join("workspaces/ws_other/media"),
        host.join("tools/media"),
        host.join("apart/media"),
    ];
    for mount_point in &mounts {
        std::fs::create_dir_all(mount_point).expect("a mount point");
    }
    // The namespace maps this user to itself and keeps its capabilities for the two mounts;
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
            "{{ {mount} -t tmpfs tmpfs \"$1\" && {mount} -t tmpfs tmpfs \"$2\" \
             && {mount} -t ramfs ramfs \"$3\"; }} >&2 || exit {STAGE_FAILED}; shift 3; \
             exec \"$@\"",
            mount = mount.display()
        ))
        .arg("sh")
        .args(&mounts)
        .arg(setpriv)
        .args(["--inh-caps=-all", "--ambient-caps=-all", "--"])
        .arg(std::env::current_exe().expect("this test binary"))
        .args(["--exact", CASE, "--nocapture", "--test-threads=1"])
        .env(INNER, &host)
        .output()
        .expect("unshare starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(STAGE_FAILED) {
        return Err(format!("no mount in a user namespace: {}", stderr.trim()));
    }
    // `unshare` itself ended: this host gives this user no user or mount namespace.
    if output.status.code() == Some(1) && stderr.trim_start().starts_with("unshare:") {
        return Err(format!("no user and mount namespace: {}", stderr.trim()));
    }
    Ok(format!("{stdout}\n{stderr}"))
}
