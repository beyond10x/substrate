//! Adversary round on `d36436a0f` (design 35 § 18, *Two decisions after the first review of
//! that branch*), through the real `linux` adapter.
//!
//! **The first target** is the sentence the read-only side rests on: counting a mount the
//! kernel was not asked about as reached "can only make a class stricter". The cases here
//! stage a read-only root with a mount beneath it that is covered, or that this user cannot
//! open, and whose table row names something a run can write; the count then classes the root
//! `beneath`. They ask what that class does to a start and to an entry.
//!
//! **The second** is the rule that every symbolic link a followed entry's walk reads is
//! placed.
//!
//! As `tests/exec_lists_view_mounts.rs`: a case starts this binary again inside a user and
//! mount namespace of its own in which the stage made its mounts, and opens the adapter there
//! with the lane's statically linked helper. Without `SUBSTRATE_VECTORS_CGROUP_ROOT`,
//! bubblewrap, `SUBSTRATE_EXEC_GATE` or a host whose user namespaces may mount, a case prints
//! `<name>: ABSENT (<reason>)` and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    ReadOnlyRoot, RunState, linux::LinuxRequest,
};

const NAME: &str = "adversary6 exec view mounts";

const SCENARIO: &str = "ADVERSARY6_VIEW_MOUNTS_SCENARIO";
const HOST: &str = "ADVERSARY6_VIEW_MOUNTS_HOST";

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
/// shell.
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
/// `entry`: what the run printed, or the refusal of the admission or the start. A run the
/// helper ended before the shell prints nothing.
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
    let refused = |error: ConfinementError| format!("refused {}", error.code());
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
    format!(
        "ran {:?} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .find(|line| line.contains("substrate-exec-gate"))
            .unwrap_or_default()
    )
    .trim()
    .to_owned()
}

fn inside(scenario: &str) -> Option<PathBuf> {
    (std::env::var(SCENARIO).ok().as_deref() == Some(scenario))
        .then(|| PathBuf::from(std::env::var_os(HOST).expect("the host root")))
}

/// Runs the case `case` again inside a user and mount namespace in which `stage` ran: shell
/// text with `$H` naming a fresh host root and `$mount`, `$mkdir`, `$cp`, `$chmod` and `$true`
/// set. Returns what that run printed after `<NAME> <scenario> inside: `, or why the case is
/// absent.
fn staged(case: &str, scenario: &str, stage: &str) -> Result<Vec<String>, String> {
    delegated()?;
    let tool = |name: &str| {
        find(name)
            .map(|path| text(&path))
            .ok_or_else(|| format!("no {name} on this host"))
    };
    let (unshare, setpriv, shell) = (tool("unshare")?, tool("setpriv")?, tool("sh")?);
    let tools = format!(
        "mount={} mkdir={} cp={} chmod={} true={}",
        tool("mount")?,
        tool("mkdir")?,
        tool("cp")?,
        tool("chmod")?,
        tool("true")?
    );
    let directory = tempfile::tempdir().expect("host root");
    let host = directory
        .path()
        .canonicalize()
        .expect("canonical host root");
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
            "H=\"$1\"; shift; {tools}; {{ {stage}; }} >&2 || exit {STAGE_FAILED}; exec \"$@\""
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
    // A stage may have closed a directory; open it again so the host root can be removed.
    let locked = host.join("tools/locked");
    if locked.is_dir() {
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    }
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

fn observed(lines: &[String], what: &str) -> String {
    let prefix = format!("{what} ");
    lines
        .iter()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .map_or_else(
            || panic!("no `{what}` line among {lines:?}"),
            |rest| rest.trim().to_owned(),
        )
}

struct Inside {
    adapter: Arc<dyn ConfinementAdapter>,
    workspace: PathBuf,
    /// Runs a program of `/usr/bin` and prints its status.
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

/// A start the driver refused before anything was dispatched, for a place.
const REFUSED_UNPLACED: &str = "refused exec.sandbox-place-unserved";
const CONTROL_DENIED: &str = "ran \"126\\n\"";
const RAN: &str = "ran \"0\\n\"";

/// One scenario whose every observation is a start with the root `<host>/tools`: prints
/// `<what> <outcome>` for each `(what, entry, script)`; an empty script is the scenario's own.
async fn observe(scenario: &str, host: &Path, starts: &[(&str, &str, &str)]) {
    let Some(inside) = enter(scenario, host) else {
        return;
    };
    let tools = host.join("tools");
    for (what, entry, script) in starts {
        let script = if script.is_empty() {
            inside.script.as_str()
        } else {
            script
        };
        println!(
            "{NAME} {scenario} inside: {what} {}",
            run(&inside.adapter, &inside.workspace, &tools, entry, script).await
        );
    }
}

/// **The decision's own words, "never refuses", for a root the count classes `beneath`.**
/// Beneath the read-only root `<host>/tools` lies a mount a run cannot reach through it, whose
/// table row names a directory of the workspace root:
///
/// - `covered`: `<workspace root>/ws_other` bound at `tools/hid`, and a tmpfs stacked on it;
/// - `closed`: the same bind at `tools/locked/m`, and `tools/locked` closed to this user
///   (mode 000), so neither the driver nor a run can open the mount point.
///
/// Nothing a run can write is shown through the root. The count classes the root `beneath`
/// all the same, which is the stricter class the decision accepts: an entry with a link in
/// that root would be refused. This start lists `/usr/bin` and names nothing in the root.
/// A `beneath` root is passed to the helper as `--writable`, and for a `--writable` root the
/// helper asks the kernel and fails closed: status 125, after dispatch.
///
/// **Changed by the implementor in the third round of the fix, on the coordinator's
/// decision.** The case was written to expect the start to run ("a host is not refused for a
/// mount nothing writable touches") and found it ended 125 by the helper. The decision: a
/// source whose reach as written overlaps a writable location is from then on a writable
/// location for the driver too, its reach is asked strictly, and where that fails the start
/// is refused before dispatch `exec.sandbox-place-unserved`. "Never refuses" holds for a root
/// that overlaps nothing writable. So both starts are expected to be refused by the driver:
/// `refused <code>` is printed only for a start the adapter did not dispatch, and a run the
/// helper ended would print `ran "" substrate-exec-gate: ...`.
#[tokio::test(flavor = "multi_thread")]
async fn adversary6_a_root_the_count_classes_beneath_is_refused_before_dispatch() {
    const CASE: &str = "adversary6_a_root_the_count_classes_beneath_is_refused_before_dispatch";
    const SHAPES: [(&str, &str); 2] = [
        (
            "covered",
            "$mkdir -p \"$H/workspaces/ws_other\" \"$H/tools/hid\" \
             && $mount --bind \"$H/workspaces/ws_other\" \"$H/tools/hid\" \
             && $mount -t tmpfs tmpfs \"$H/tools/hid\"",
        ),
        (
            "closed",
            "$mkdir -p \"$H/workspaces/ws_other\" \"$H/tools/locked/m\" \
             && $mount --bind \"$H/workspaces/ws_other\" \"$H/tools/locked/m\" \
             && $chmod 000 \"$H/tools/locked\"",
        ),
    ];
    for (scenario, _) in SHAPES {
        if let Some(host) = inside(scenario) {
            observe(
                scenario,
                &host,
                &[("control", "bin", ""), ("listed", "/usr/bin", "")],
            )
            .await;
            return;
        }
    }
    let mut failures = Vec::new();
    for (scenario, stage) in SHAPES {
        let lines = match staged(CASE, scenario, stage) {
            Ok(lines) => lines,
            Err(reason) => return println!("{NAME} beneath by the count: ABSENT ({reason})"),
        };
        // Neither start is dispatched: the root's reach cannot be asked, whatever is listed.
        for (what, wanted) in [("control", REFUSED_UNPLACED), ("listed", REFUSED_UNPLACED)] {
            let seen = observed(&lines, what);
            println!("{NAME} beneath by the count: {scenario}: {what}: {seen}");
            if seen != wanted {
                failures.push(format!("{scenario}: {what}: {seen}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "a root the count classes `beneath`, with a mount beneath it that cannot be asked, was \
         not refused before dispatch:\n{}",
        failures.join("\n")
    );
    println!("{NAME} beneath by the count: executed");
}

/// **An entry the true class refuses and the counted class admits.** `<host>/tools/vol` is a
/// `ramfs` that only the root shows: no run can write it. The entry `/opt/k/vol/tool` ends on
/// it.
///
/// - `alone`: the root is classed `follow`, the entry's end has no place on a type outside
///   the list, and the start is refused `exec.sandbox-place-unserved`;
/// - `counted`: beside it, beneath the same root, lies a covered mount: a second mount of a
///   `ramfs` that is also mounted beneath the workspace root, with a tmpfs stacked on it. No
///   run reaches it through the root. Its row is counted, the root is classed `beneath`, and
///   a `beneath` entry is opened without a place being asked.
///
/// The sentence under test says the count can only make a class stricter. The class is
/// stricter; the answer for this entry is not: the two starts must answer alike.
#[tokio::test(flavor = "multi_thread")]
async fn adversary6_counting_a_covered_mount_changes_no_entrys_answer() {
    const CASE: &str = "adversary6_counting_a_covered_mount_changes_no_entrys_answer";
    const VOLUME: &str = "$mkdir -p \"$H/tools/vol\" && $mount -t ramfs ramfs \"$H/tools/vol\" \
                          && $cp \"$true\" \"$H/tools/vol/tool\"";
    const COVERED: &str = "$mkdir -p \"$H/workspaces/ws_other/media\" \"$H/tools/hid\" \
                           && $mount -t ramfs ramfs \"$H/workspaces/ws_other/media\" \
                           && $mount --bind \"$H/workspaces/ws_other/media\" \"$H/tools/hid\" \
                           && $mount -t tmpfs tmpfs \"$H/tools/hid\"";
    for scenario in ["alone", "counted"] {
        if let Some(host) = inside(scenario) {
            observe(
                scenario,
                &host,
                &[
                    ("control", "bin", ""),
                    ("entry", "/opt/k/vol/tool", "/opt/k/vol/tool; echo $?"),
                ],
            )
            .await;
            return;
        }
    }
    let mut seen = Vec::new();
    for (scenario, stage) in [
        ("alone", VOLUME.to_owned()),
        ("counted", format!("{VOLUME} && {COVERED}")),
    ] {
        let lines = match staged(CASE, scenario, &stage) {
            Ok(lines) => lines,
            Err(reason) => return println!("{NAME} entry on an unlisted type: ABSENT ({reason})"),
        };
        assert_eq!(observed(&lines, "control"), CONTROL_DENIED, "{scenario}");
        let entry = observed(&lines, "entry");
        println!("{NAME} entry on an unlisted type: {scenario}: {entry}");
        seen.push(entry);
    }
    assert_eq!(
        seen[0], "refused exec.sandbox-place-unserved",
        "alone: the entry ends on a type outside the list"
    );
    assert_eq!(
        seen[1], seen[0],
        "counted: a covered mount no run reaches was counted, and the entry's answer changed"
    );
    println!("{NAME} entry on an unlisted type: executed");
}

/// **Every symbolic link a followed entry reads is placed, however the walk comes to it.**
/// `<host>/tools/media` is a `ramfs` that only the root shows, so the root is classed
/// `follow`; `media/link` on it points at `/usr/bin`. The entries reach that link directly,
/// as a middle name, through another link, through `..` in a link's target, and through a
/// link with an absolute target. Each is refused `exec.sandbox-place-unserved`.
///
/// Beside them: a link on the root's own file system is followed; a chain of 40 links is
/// followed and one of 41 is refused; a link into `/proc/self/root` is refused.
#[tokio::test(flavor = "multi_thread")]
async fn adversary6_a_link_without_a_place_is_refused_however_the_walk_reaches_it() {
    const CASE: &str = "adversary6_a_link_without_a_place_is_refused_however_the_walk_reaches_it";
    const SCENARIO_NAME: &str = "links";
    const UNPLACED: [(&str, &str); 5] = [
        ("last", "/opt/k/media/link"),
        ("middle", "/opt/k/media/link/sed"),
        ("through-a-link", "/opt/k/l1"),
        ("through-dot-dot", "/opt/k/l2"),
        ("absolute", "/opt/k/abs"),
    ];
    if let Some(host) = inside(SCENARIO_NAME) {
        let tools = host.join("tools");
        std::fs::create_dir_all(tools.join("dir")).expect("dir");
        for (link, target) in [
            ("media/link", "/usr/bin".to_owned()),
            ("l1", "media/link".to_owned()),
            ("l2", "dir/../media/link".to_owned()),
            ("abs", "/opt/k/media/link".to_owned()),
            ("placed", "/usr/bin".to_owned()),
            ("magic", "/proc/self/root/usr/bin".to_owned()),
            ("c40", "/usr/bin".to_owned()),
        ] {
            symlink(target, tools.join(link)).expect("a link");
        }
        // c1 resolves through 40 links, c0 through 41.
        for index in (0..40).rev() {
            symlink(format!("c{}", index + 1), tools.join(format!("c{index}"))).expect("a link");
        }
        let mut starts = vec![
            ("control", "bin", ""),
            ("placed", "/opt/k/placed", ""),
            ("forty", "/opt/k/c1", ""),
            ("forty-one", "/opt/k/c0", ""),
            ("magic", "/opt/k/magic", ""),
        ];
        starts.extend(UNPLACED.iter().map(|(what, entry)| (*what, *entry, "")));
        observe(SCENARIO_NAME, &host, &starts).await;
        return;
    }
    let lines = match staged(
        CASE,
        SCENARIO_NAME,
        "$mkdir -p \"$H/tools/media\" && $mount -t ramfs ramfs \"$H/tools/media\"",
    ) {
        Ok(lines) => lines,
        Err(reason) => return println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})"),
    };
    assert_eq!(observed(&lines, "control"), CONTROL_DENIED);
    let mut failures = Vec::new();
    let mut expect = |what: &str, wanted: &str| {
        let seen = observed(&lines, what);
        println!("{NAME} {SCENARIO_NAME}: {what}: {seen}");
        if seen != wanted {
            failures.push(format!("{what}: {seen}, not {wanted}"));
        }
    };
    expect("placed", RAN);
    expect("forty", RAN);
    expect("forty-one", "refused exec.sandbox-path-unrepresentable");
    expect("magic", "refused exec.sandbox-path-unrepresentable");
    for (what, _) in UNPLACED {
        expect(what, "refused exec.sandbox-place-unserved");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME} {SCENARIO_NAME}: executed");
}
