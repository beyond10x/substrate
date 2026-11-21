//! Adversary pass on the fix `f402165ad` (design 35 D30, *Through the view's mounts*), through
//! the real `linux` adapter: writable places the driver's checks have to reach, and ordinary
//! hosts they must not refuse.
//!
//! As `tests/exec_lists_view_mounts.rs`: a staged case starts this binary again inside a user
//! and mount namespace of its own in which the stage made its mounts, and opens the adapter
//! there with the lane's statically linked helper. Every case needs
//! `SUBSTRATE_VECTORS_CGROUP_ROOT`, bubblewrap and `SUBSTRATE_EXEC_GATE`; a staged case also a
//! host whose user namespaces may mount. Without one of them a case prints
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

const NAME: &str = "adversary5 exec view mounts";

/// Name the scenario and the host root, in the process a case starts inside its namespace.
const SCENARIO: &str = "ADVERSARY5_VIEW_MOUNTS_SCENARIO";
const HOST: &str = "ADVERSARY5_VIEW_MOUNTS_HOST";

const STAGE_FAILED: i32 = 97;

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

fn helper() -> PathBuf {
    PathBuf::from(std::env::var_os("SUBSTRATE_EXEC_GATE").expect(
        "SUBSTRATE_EXEC_GATE is unset: `bash scripts/delegated-lane.sh` builds \
         substrate-exec-gate and names it",
    ))
}

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

/// The adapter over `<host>/workspaces`, proving `exec_subpaths`, or why it is absent.
fn open(host: &Path) -> Result<Arc<dyn ConfinementAdapter>, String> {
    let cgroup_root = delegated()?;
    let mut config = HostConfig::minimum(host.join("workspaces"));
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

/// The host root of the scenario this process was started for, inside its namespace.
fn inside(scenario: &str) -> Option<PathBuf> {
    (std::env::var(SCENARIO).ok().as_deref() == Some(scenario))
        .then(|| PathBuf::from(std::env::var_os(HOST).expect("the host root")))
}

/// Runs the case `case` again inside a user and mount namespace in which `stage` ran: shell
/// text with `$H` naming a fresh host root and `$mount` and `$mkdir` set. Returns what that
/// run printed after `<NAME> <scenario> inside: `, one line per observation, or why the case
/// is absent.
fn staged(case: &str, scenario: &str, stage: &str) -> Result<Vec<String>, String> {
    delegated()?;
    let tool = |name: &str| {
        find(name)
            .map(|path| text(&path))
            .ok_or_else(|| format!("no {name} on this host"))
    };
    let (unshare, mount, mkdir) = (tool("unshare")?, tool("mount")?, tool("mkdir")?);
    let (setpriv, shell) = (tool("setpriv")?, tool("sh")?);
    let directory = tempfile::tempdir().expect("host root");
    let host = directory
        .path()
        .canonicalize()
        .expect("canonical host root");
    // As the shipped case: the namespace maps this user to itself and keeps its capabilities
    // for the stage; `setpriv` then drops them all, so the driver inside is this user with no
    // capability.
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
            "H=\"$1\"; shift; mount={mount} mkdir={mkdir}; {{ {stage}; }} >&2 \
             || exit {STAGE_FAILED}; exec \"$@\""
        ))
        .arg("sh")
        .arg(&host)
        .arg(setpriv)
        .args(["--inh-caps=-all", "--ambient-caps=-all", "--"])
        .arg(std::env::current_exe().expect("this test binary"))
        .args(["--exact", case, "--nocapture", "--test-threads=1"])
        .env(SCENARIO, scenario)
        .env(HOST, &host)
        .output()
        .expect("unshare starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(STAGE_FAILED) {
        return Err(format!("the stage failed: {}", stderr.trim()));
    }
    if output.status.code() == Some(1) && stderr.trim_start().starts_with("unshare:") {
        return Err(format!("no user and mount namespace: {}", stderr.trim()));
    }
    let marker = format!("{NAME} {scenario} inside: ");
    let lines: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.split_once(marker.as_str()).map(|(_, rest)| rest))
        .map(|rest| rest.trim().to_owned())
        .collect();
    if let Some(reason) = lines.iter().find_map(|line| line.strip_prefix("ABSENT")) {
        return Err(format!("inside the namespace:{reason}"));
    }
    assert!(
        output.status.success() && !lines.is_empty(),
        "the run inside the namespace ended {:?} with {lines:?}: {stdout}\n{stderr}",
        output.status
    );
    Ok(lines)
}

/// The observation `what` of `lines`.
fn observed(lines: &[String], what: &str) -> String {
    lines
        .iter()
        .find_map(|line| line.strip_prefix(what))
        .map_or_else(
            || panic!("no `{what}` line among {lines:?}"),
            |rest| rest.trim().to_owned(),
        )
}

/// What every scenario needs inside its namespace: the adapter, a workspace with `bin`, and
/// the script that runs a program of `/usr/bin` and prints its status.
struct Inside {
    adapter: Arc<dyn ConfinementAdapter>,
    workspace: PathBuf,
    script: String,
}

fn enter(scenario: &str, host: &Path) -> Option<Inside> {
    let absent = |reason: String| {
        println!("{NAME} {scenario} inside: ABSENT ({reason})");
        None
    };
    let Some(program) = probe_program() else {
        return absent("/usr/bin holds none of the probed programs as a file".to_owned());
    };
    let workspace = host.join("workspaces/ws_exec");
    std::fs::create_dir_all(workspace.join("bin")).expect("bin");
    match open(host) {
        Ok(adapter) => Some(Inside {
            adapter,
            workspace,
            script: format!("{program} --version >/dev/null 2>&1; echo $?"),
        }),
        Err(reason) => absent(reason),
    }
}

/// A start that listed `entry` did not let a program of `/usr/bin` run: it was refused, or the
/// helper ended the run before the shell.
fn held(seen: &str) -> bool {
    seen.starts_with("refused ") || seen == "ran \"\""
}

const CONTROL_DENIED: &str = "ran \"126\\n\"";
const RAN: &str = "ran \"0\\n\"";

/// **K10's planted link, on a file system of a type outside the list.** A `ramfs` is mounted
/// beneath the workspace root, at `<workspace root>/ws_other/media`, and a second time beneath
/// the directory `<host>/tools`. A run with the workspace `ws_other` left `link` on it, a link
/// to `/usr/bin`. A profile declares `<host>/tools` read-only at `/opt/k` and lists
/// `/opt/k/media/link`.
///
/// D30 does not compare a mount of an unlisted type, because "whatever is compared with a
/// writable place is placed first, and has no place on such a type". The root's source is on
/// ext4 and what the link resolves to is `/usr/bin`: both have a place. The link itself, which
/// a run can write, is never placed. So the root is classed `follow`, the link is followed,
/// and all of `/usr/bin` is admitted.
#[tokio::test(flavor = "multi_thread")]
async fn adversary5_a_planted_link_on_an_unlisted_file_system_is_not_followed() {
    const CASE: &str = "adversary5_a_planted_link_on_an_unlisted_file_system_is_not_followed";
    const SCENARIO_NAME: &str = "unlisted";
    if let Some(host) = inside(SCENARIO_NAME) {
        let Some(inside) = enter(SCENARIO_NAME, &host) else {
            return;
        };
        let tools = host.join("tools");
        std::os::unix::fs::symlink("/usr/bin", host.join("workspaces/ws_other/media/link"))
            .expect("a link a run planted on the ramfs");
        for (what, entry) in [("control", "bin"), ("planted", "/opt/k/media/link")] {
            println!(
                "{NAME} {SCENARIO_NAME} inside: {what} {}",
                run(
                    &inside.adapter,
                    &inside.workspace,
                    &tools,
                    entry,
                    &inside.script
                )
                .await
            );
        }
        return;
    }
    let lines = match staged(
        CASE,
        SCENARIO_NAME,
        "$mkdir -p \"$H/workspaces/ws_other/media\" \"$H/tools/media\" \
         && $mount -t ramfs ramfs \"$H/workspaces/ws_other/media\" \
         && $mount --bind \"$H/workspaces/ws_other/media\" \"$H/tools/media\"",
    ) {
        Ok(lines) => lines,
        Err(reason) => return println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})"),
    };
    assert_eq!(observed(&lines, "control"), CONTROL_DENIED);
    let planted = observed(&lines, "planted");
    println!("{NAME} {SCENARIO_NAME}: planted: {planted}");
    assert!(
        held(&planted),
        "a link on a file system a run writes through the workspace root was followed, and a \
         program of /usr/bin ran: {planted}"
    );
    println!("{NAME} {SCENARIO_NAME}: executed");
}

/// **A root declared on the directory a bind beneath the workspace root shows**, by that
/// directory's own path. `<host>/elsewhere` is bound at `<workspace root>/ws_other/media`. Its
/// place is not in the workspace root's and no mount is beneath it; the workspace root reaches
/// it through the bind. The planted link is refused before dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn adversary5_a_root_on_the_source_of_a_bind_beneath_the_workspace_root_is_beneath() {
    const CASE: &str =
        "adversary5_a_root_on_the_source_of_a_bind_beneath_the_workspace_root_is_beneath";
    const SCENARIO_NAME: &str = "bind";
    if let Some(host) = inside(SCENARIO_NAME) {
        let Some(inside) = enter(SCENARIO_NAME, &host) else {
            return;
        };
        let elsewhere = host.join("elsewhere");
        std::os::unix::fs::symlink("/usr/bin", host.join("workspaces/ws_other/media/link"))
            .expect("a link a run planted through the bind");
        for (what, entry) in [("control", "bin"), ("planted", "/opt/k/link")] {
            println!(
                "{NAME} {SCENARIO_NAME} inside: {what} {}",
                run(
                    &inside.adapter,
                    &inside.workspace,
                    &elsewhere,
                    entry,
                    &inside.script
                )
                .await
            );
        }
        return;
    }
    let lines = match staged(
        CASE,
        SCENARIO_NAME,
        "$mkdir -p \"$H/workspaces/ws_other/media\" \"$H/elsewhere\" \
         && $mount --bind \"$H/elsewhere\" \"$H/workspaces/ws_other/media\"",
    ) {
        Ok(lines) => lines,
        Err(reason) => return println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})"),
    };
    assert_eq!(observed(&lines, "control"), CONTROL_DENIED);
    let planted = observed(&lines, "planted");
    assert!(
        planted.starts_with("refused exec.sandbox-path-unrepresentable"),
        "{planted}"
    );
    println!("{NAME} {SCENARIO_NAME}: executed");
}

/// **The workspace root is a mount point itself.** A tmpfs at `<host>/workspaces` and one at
/// `ws_other/media` beneath it. (A workspace that is a mount point of its own is refused
/// `workspace.path-escape` before any list is read, by the workspace rules.) An
/// ordinary start runs a program of a listed `/usr/bin`; the control does not; the link
/// planted on the mount beneath the workspace root is refused.
#[tokio::test(flavor = "multi_thread")]
async fn adversary5_a_workspace_root_that_is_a_mount_point_holds() {
    const CASE: &str = "adversary5_a_workspace_root_that_is_a_mount_point_holds";
    const SCENARIO_NAME: &str = "workspace root is a mount point";
    if let Some(host) = inside(SCENARIO_NAME) {
        let Some(inside) = enter(SCENARIO_NAME, &host) else {
            return;
        };
        let planted = host.join("workspaces/ws_other/media/tools");
        std::fs::create_dir_all(&planted).expect("a directory a run wrote");
        std::os::unix::fs::symlink("/usr/bin", planted.join("link")).expect("a planted link");
        for (what, entry) in [
            ("control", "bin"),
            ("listed", "/usr/bin"),
            ("planted", "/opt/k/link"),
        ] {
            println!(
                "{NAME} {SCENARIO_NAME} inside: {what} {}",
                run(
                    &inside.adapter,
                    &inside.workspace,
                    &planted,
                    entry,
                    &inside.script
                )
                .await
            );
        }
        return;
    }
    let lines = match staged(
        CASE,
        SCENARIO_NAME,
        "$mkdir -p \"$H/workspaces\" && $mount -t tmpfs tmpfs \"$H/workspaces\" \
         && $mkdir -p \"$H/workspaces/ws_other/media\" \
         && $mount -t tmpfs tmpfs \"$H/workspaces/ws_other/media\"",
    ) {
        Ok(lines) => lines,
        Err(reason) => return println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})"),
    };
    assert_eq!(observed(&lines, "control"), CONTROL_DENIED);
    assert_eq!(observed(&lines, "listed"), RAN);
    let planted = observed(&lines, "planted");
    assert!(
        planted.starts_with("refused exec.sandbox-path-unrepresentable"),
        "{planted}"
    );
    println!("{NAME} {SCENARIO_NAME}: executed");
}

/// **`/usr` with a file system of its own mounted beneath it**, as a host with `/usr/local` on
/// a separate volume has. Nothing a run can write is involved. A start that lists `/usr/bin`
/// runs its program, and the control does not.
#[tokio::test(flavor = "multi_thread")]
async fn adversary5_usr_with_a_mount_beneath_it_starts_as_before() {
    const CASE: &str = "adversary5_usr_with_a_mount_beneath_it_starts_as_before";
    const SCENARIO_NAME: &str = "usr local";
    if let Some(host) = inside(SCENARIO_NAME) {
        let Some(inside) = enter(SCENARIO_NAME, &host) else {
            return;
        };
        let tools = host.join("tools");
        std::fs::create_dir_all(&tools).expect("tools");
        for (what, entry) in [("control", "bin"), ("listed", "/usr/bin")] {
            println!(
                "{NAME} {SCENARIO_NAME} inside: {what} {}",
                run(
                    &inside.adapter,
                    &inside.workspace,
                    &tools,
                    entry,
                    &inside.script
                )
                .await
            );
        }
        return;
    }
    if !Path::new("/usr/local").is_dir() {
        return println!("{NAME} {SCENARIO_NAME}: ABSENT (no /usr/local on this host)");
    }
    let lines = match staged(CASE, SCENARIO_NAME, "$mount -t tmpfs tmpfs /usr/local") {
        Ok(lines) => lines,
        Err(reason) => return println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})"),
    };
    assert_eq!(observed(&lines, "control"), CONTROL_DENIED);
    assert_eq!(observed(&lines, "listed"), RAN);
    println!("{NAME} {SCENARIO_NAME}: executed");
}

/// **A stack of two mounts beneath a declared read-only root, with nothing writable near it.**
/// Design 35 § 6 raises `mount-covered` "only [for] a mount whose root is compared as a place:
/// one of a listed type that is beneath a denied directory, beneath a writable location, or
/// beneath a listed directory of a start with a deny-list". `<host>/tools/media` is none of
/// them: `<host>/tools` is a read-only root, the list names `/usr/bin`, and there is no
/// deny-list. The start runs.
#[tokio::test(flavor = "multi_thread")]
async fn adversary5_a_stack_beneath_a_read_only_root_refuses_no_start() {
    const CASE: &str = "adversary5_a_stack_beneath_a_read_only_root_refuses_no_start";
    const SCENARIO_NAME: &str = "stack beneath a root";
    if let Some(host) = inside(SCENARIO_NAME) {
        let Some(inside) = enter(SCENARIO_NAME, &host) else {
            return;
        };
        let tools = host.join("tools");
        println!(
            "{NAME} {SCENARIO_NAME} inside: listed {}",
            run(
                &inside.adapter,
                &inside.workspace,
                &tools,
                "/usr/bin",
                &inside.script
            )
            .await
        );
        return;
    }
    let lines = match staged(
        CASE,
        SCENARIO_NAME,
        "$mkdir -p \"$H/tools/media\" && $mount -t tmpfs tmpfs \"$H/tools/media\" \
         && $mount -t tmpfs tmpfs \"$H/tools/media\"",
    ) {
        Ok(lines) => lines,
        Err(reason) => return println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})"),
    };
    assert_eq!(
        observed(&lines, "listed"),
        RAN,
        "a start whose read-only root holds a stack of two mounts, with no writable location \
         and no deny entry near it"
    );
    println!("{NAME} {SCENARIO_NAME}: executed");
}

/// A mount of this host's own table, of a listed type, whose mount point this user cannot
/// open, and the deepest directory above it this user can list.
fn unopenable_mount() -> Option<(String, PathBuf)> {
    let table = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    table.lines().find_map(|line| {
        let (mount, rest) = line.split_once(" - ")?;
        let mount_point = mount.split(' ').nth(4)?;
        let listed = ["ext4", "overlay", "tmpfs"].contains(&rest.split(' ').next()?);
        let denied = std::fs::symlink_metadata(mount_point)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied);
        if !listed || !denied || mount_point.contains('\\') {
            return None;
        }
        let above = Path::new(mount_point)
            .ancestors()
            .skip(1)
            .find(|directory| std::fs::read_dir(directory).is_ok())?;
        (above != Path::new("/")).then(|| (mount_point.to_owned(), above.to_path_buf()))
    })
}

/// **An ordinary host: a declared read-only root that holds a mount this user cannot open.**
/// No namespace and no staged mount: the mount is one the host already has, such as a
/// container's root beneath a directory only root may enter, and the root is a directory
/// above it. Nothing a run can write is near it, the list names `/usr/bin`, and the profile
/// lists nothing in the root. Before the fix this start ran.
#[tokio::test(flavor = "multi_thread")]
async fn adversary5_a_read_only_root_that_holds_a_mount_this_user_cannot_open_starts() {
    const SCENARIO_NAME: &str = "unopenable mount beneath a root";
    let absent = |reason: String| println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})");
    let Some((mount_point, root)) = unopenable_mount() else {
        return absent(
            "this host has no mount of a listed type whose mount point this user cannot open"
                .to_owned(),
        );
    };
    let directory = tempfile::tempdir().expect("host root");
    let host = directory.path().canonicalize().expect("canonical");
    let Some(program) = probe_program() else {
        return absent("/usr/bin holds none of the probed programs as a file".to_owned());
    };
    let workspace = host.join("workspaces/ws_exec");
    std::fs::create_dir_all(workspace.join("bin")).expect("bin");
    let adapter = match open(&host) {
        Ok(adapter) => adapter,
        Err(reason) => return absent(reason),
    };
    let script = format!("{program} --version >/dev/null 2>&1; echo $?");
    let seen = run(&adapter, &workspace, &root, "/usr/bin", &script).await;
    println!(
        "{NAME} {SCENARIO_NAME}: the root {} holds the mount {mount_point}: {seen}",
        root.display()
    );
    // The host's root holds every mount of the host: observed, not asserted, because it also
    // holds the workspace root and is classed `beneath` for that.
    println!(
        "{NAME} {SCENARIO_NAME}: the root / : {}",
        run(&adapter, &workspace, Path::new("/"), "/usr/bin", &script).await
    );
    assert_eq!(
        seen,
        RAN,
        "the read-only root {} holds the mount {mount_point}, which this user cannot open",
        root.display()
    );
    println!("{NAME} {SCENARIO_NAME}: executed");
}
