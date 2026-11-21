//! Adversary round on `ec87d1554` (design 35 § 18, *Three decisions after the second review of
//! that branch*), through the real `linux` adapter:
//!
//! - the classes of the shown mounts are decided together, so a root that overlaps only
//!   another root classed `beneath` is `beneath` too, in whatever order they are declared;
//! - every entry is placed where it ends, whatever its class;
//! - the driver's walk of a followed path and the helper's end at the same file, and a path
//!   through a directory a run can write is refused before dispatch.
//!
//! As `tests/exec_lists_view_mounts.rs`: a case starts this binary again inside a user and
//! mount namespace of its own in which the stage made its mounts, and opens the adapter there
//! with the lane's statically linked helper. Without `SUBSTRATE_VECTORS_CGROUP_ROOT`,
//! bubblewrap, `SUBSTRATE_EXEC_GATE` or a host whose user namespaces may mount, a case prints
//! `<name>: ABSENT (<reason>)` and is absent, never passed.
//!
//! An observation is `refused <code>` for a start the adapter did not dispatch, and
//! `ran "<output>"` for one it did, followed by the helper's own line where the helper ended
//! the run: a start the driver admitted and the helper refused shows as `ran "" ...`.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    ReadOnlyRoot, RunState, linux::LinuxRequest,
};

const NAME: &str = "adversary7 exec view mounts";

const SCENARIO: &str = "ADVERSARY7_VIEW_MOUNTS_SCENARIO";
const HOST: &str = "ADVERSARY7_VIEW_MOUNTS_HOST";

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

/// The adapter over `<host>/workspaces`, whatever it proved.
fn open_any(host: &Path) -> Result<Arc<dyn ConfinementAdapter>, String> {
    let cgroup_root = delegated()?;
    let mut config = HostConfig::minimum(host.join("workspaces"));
    config.exec_gate = Some(helper());
    confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .map_err(|error| format!("linux did not open: {error}"))
}

fn open(host: &Path) -> Result<Arc<dyn ConfinementAdapter>, String> {
    let adapter = open_any(host)?;
    if !adapter.facts().proves(&Fact::ExecSubpaths) {
        return Err(format!(
            "the adapter did not prove exec_subpaths: {:?}",
            adapter.facts()
        ));
    }
    Ok(adapter)
}

/// One start: the read-only roots as `(host directory, mount)`, the entries, and the script
/// the shell runs.
struct Start<'a> {
    roots: &'a [(PathBuf, &'a str)],
    entries: &'a [&'a str],
    script: &'a str,
}

async fn run(adapter: &Arc<dyn ConfinementAdapter>, workspace: &Path, start: &Start<'_>) -> String {
    let profile = ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        read_only_roots: start
            .roots
            .iter()
            .map(|(host_path, mount)| ReadOnlyRoot {
                host_path: text(host_path),
                mount: (*mount).to_owned(),
            })
            .collect(),
        exec_subpaths: start
            .entries
            .iter()
            .map(|entry| (*entry).to_owned())
            .collect(),
        ..ConfinementProfile::default()
    };
    let command = Command {
        argv: vec![
            text(&find("sh").expect("a shell")),
            "-c".to_owned(),
            start.script.to_owned(),
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
/// text with `$H` naming a fresh host root and `$mount`, `$mkdir`, `$cp` and `$true` set.
/// Returns what that run printed after `<NAME> <scenario> inside: `, or why the case is
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
        "mount={} mkdir={} cp={} true={}",
        tool("mount")?,
        tool("mkdir")?,
        tool("cp")?,
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

const CONTROL_DENIED: &str = "ran \"126\\n\"";
const RAN: &str = "ran \"0\\n\"";
const UNREPRESENTABLE: &str = "refused exec.sandbox-path-unrepresentable";
const UNPLACED: &str = "refused exec.sandbox-place-unserved";

/// Compares each `(what, wanted)` with what the scenario printed, printing every observation.
fn compare(scenario: &str, lines: &[String], wanted: &[(&str, &str)]) -> Vec<String> {
    let mut failures = Vec::new();
    for (what, wanted) in wanted {
        let seen = observed(lines, what);
        println!("{NAME} {scenario}: {what}: {seen}");
        if seen != *wanted {
            failures.push(format!("{what}: {seen}, not {wanted}"));
        }
    }
    failures
}

/// **A chain of roots, in both orders.** Three tmpfs: the first is mounted beneath the
/// workspace root and again at `<host>/a`; the second in the first, at `<host>/a/t2`, and
/// again at `<host>/b`; the third in the second, at `<host>/b/t3`, and again at `<host>/c`.
/// So the root `a` overlaps the workspace root, `b` overlaps only `a`, and `c` overlaps only
/// `b`. A link to `/usr/bin` lies on the third.
///
/// The profile declares the three roots and lists the link through `c`. Declared `c`, `b`,
/// `a`, the root `c` is asked first, when nothing it overlaps is known to be writable. The
/// start is refused before dispatch in both orders, with the same code.
#[tokio::test(flavor = "multi_thread")]
async fn adversary7_a_chain_of_roots_is_classed_alike_in_either_order() {
    const CASE: &str = "adversary7_a_chain_of_roots_is_classed_alike_in_either_order";
    const SCENARIO_NAME: &str = "chain of roots";
    if let Some(host) = inside(SCENARIO_NAME) {
        let Some(inside) = enter(SCENARIO_NAME, &host) else {
            return;
        };
        symlink("/usr/bin", host.join("c/link")).expect("a link on the third tmpfs");
        let (a, b, c) = (
            (host.join("a"), "/opt/a"),
            (host.join("b"), "/opt/b"),
            (host.join("c"), "/opt/c"),
        );
        for (what, roots, entry) in [
            ("control", vec![a.clone(), b.clone(), c.clone()], "bin"),
            (
                "forward",
                vec![a.clone(), b.clone(), c.clone()],
                "/opt/c/link",
            ),
            (
                "reverse",
                vec![c.clone(), b.clone(), a.clone()],
                "/opt/c/link",
            ),
            ("alone", vec![c.clone()], "/opt/c/link"),
        ] {
            let start = Start {
                roots: &roots,
                entries: &[entry],
                script: &inside.script,
            };
            println!(
                "{NAME} {SCENARIO_NAME} inside: {what} {}",
                run(&inside.adapter, &inside.workspace, &start).await
            );
        }
        return;
    }
    let lines = match staged(
        CASE,
        SCENARIO_NAME,
        "$mkdir -p \"$H/workspaces/ws_other/t1\" \"$H/a\" \"$H/b\" \"$H/c\" \
         && $mount -t tmpfs tmpfs \"$H/workspaces/ws_other/t1\" \
         && $mount --bind \"$H/workspaces/ws_other/t1\" \"$H/a\" \
         && $mkdir \"$H/a/t2\" && $mount -t tmpfs tmpfs \"$H/a/t2\" \
         && $mount --bind \"$H/a/t2\" \"$H/b\" \
         && $mkdir \"$H/b/t3\" && $mount -t tmpfs tmpfs \"$H/b/t3\" \
         && $mount --bind \"$H/b/t3\" \"$H/c\"",
    ) {
        Ok(lines) => lines,
        Err(reason) => return println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})"),
    };
    // Alone, the root `c` overlaps nothing a run can write and its link is followed: what
    // makes it `beneath` in the other two starts is the chain and nothing else.
    let failures = compare(
        SCENARIO_NAME,
        &lines,
        &[
            ("control", CONTROL_DENIED),
            ("alone", RAN),
            ("forward", UNREPRESENTABLE),
            ("reverse", UNREPRESENTABLE),
        ],
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME} {SCENARIO_NAME}: executed");
}

/// **A base directory that shows a workspace.** A directory of the workspace root is bound at
/// `/usr/local`, so `/usr` overlaps a location a run can write and is classed `beneath`. It
/// then joins the locations the other mounts are asked against, and `/bin`, `/lib` and
/// `/lib64`, which a merged `/usr` shows too, follow it.
///
/// Whatever the answer, it is the driver's: each start is refused by name before dispatch, or
/// it runs. A start the helper ends with status 125 would be the two sides disagreeing. The
/// case also prints whether the adapter proved `exec_subpaths` at the open, and whether its
/// proof names a cause for a fact it withheld.
#[tokio::test(flavor = "multi_thread")]
async fn adversary7_a_base_directory_that_shows_a_workspace_is_answered_before_dispatch() {
    const CASE: &str =
        "adversary7_a_base_directory_that_shows_a_workspace_is_answered_before_dispatch";
    const SCENARIO_NAME: &str = "base directory";
    if let Some(host) = inside(SCENARIO_NAME) {
        let absent = |reason: String| println!("{NAME} {SCENARIO_NAME} inside: ABSENT ({reason})");
        let Some(program) = probe_program() else {
            return absent("/usr/bin holds none of the probed programs as a file".to_owned());
        };
        let adapter = match open_any(&host) {
            Ok(adapter) => adapter,
            Err(reason) => return absent(reason),
        };
        let facts = adapter.facts();
        println!(
            "{NAME} {SCENARIO_NAME} inside: opened exec_subpaths proved: {}, proof: {}",
            facts.proves(&Fact::ExecSubpaths),
            format!("{facts:?}")
                .split_once("withheld: ")
                .map_or("?", |(_, rest)| rest)
        );
        let workspace = host.join("workspaces/ws_exec");
        std::fs::create_dir_all(workspace.join("bin")).expect("bin");
        let script = format!("{program} --version >/dev/null 2>&1; echo $?");
        for (what, entry) in [("control", "bin"), ("listed", "/usr/bin")] {
            let start = Start {
                roots: &[],
                entries: &[entry],
                script: &script,
            };
            println!(
                "{NAME} {SCENARIO_NAME} inside: {what} {}",
                run(&adapter, &workspace, &start).await
            );
        }
        return;
    }
    if !Path::new("/usr/local").is_dir() {
        return println!("{NAME} {SCENARIO_NAME}: ABSENT (no /usr/local on this host)");
    }
    let lines = match staged(
        CASE,
        SCENARIO_NAME,
        "$mkdir -p \"$H/workspaces/ws_other\" \
         && $mount --bind \"$H/workspaces/ws_other\" /usr/local",
    ) {
        Ok(lines) => lines,
        Err(reason) => return println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})"),
    };
    println!(
        "{NAME} {SCENARIO_NAME}: opened: {}",
        observed(&lines, "opened")
    );
    let mut failures = Vec::new();
    for what in ["control", "listed"] {
        let seen = observed(&lines, what);
        println!("{NAME} {SCENARIO_NAME}: {what}: {seen}");
        if seen.starts_with("ran \"\"") {
            failures.push(format!(
                "{what}: the driver admitted and the helper refused: {seen}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME} {SCENARIO_NAME}: executed");
}

/// **Every entry is placed where it ends, whatever its class.** In the workspace, which is
/// classed `beneath`: `vol` is a `ramfs` mounted in it and `t` a tmpfs, each with a tool.
///
/// - an entry that ends on the `ramfs`, and the `ramfs` mount point itself as an entry, have
///   no place: `exec.sandbox-place-unserved`;
/// - an entry on the tmpfs, and the tmpfs mount point itself, run their tool;
/// - the program and the loader, listed as entries beside themselves, change nothing;
/// - an entry on the view's own `/tmp` is outside every mount a list can name.
#[tokio::test(flavor = "multi_thread")]
async fn adversary7_an_entry_is_placed_where_it_ends_in_a_mount_a_run_can_write() {
    const CASE: &str = "adversary7_an_entry_is_placed_where_it_ends_in_a_mount_a_run_can_write";
    const SCENARIO_NAME: &str = "ends";
    if let Some(host) = inside(SCENARIO_NAME) {
        let Some(inside) = enter(SCENARIO_NAME, &host) else {
            return;
        };
        let shell = text(&find("sh").expect("a shell"));
        let loader = substrate_exec_gate::elf_interpreter(
            &std::fs::read(&shell).expect("the shell's bytes"),
        );
        let mut both = vec![shell.as_str()];
        both.extend(loader.as_deref());
        for (what, entries, script) in [
            ("on-ramfs", vec!["vol/tool"], "/workspace/vol/tool; echo $?"),
            ("ramfs-itself", vec!["vol"], "/workspace/vol/tool; echo $?"),
            ("on-tmpfs", vec!["t/tool"], "/workspace/t/tool; echo $?"),
            ("tmpfs-itself", vec!["t"], "/workspace/t/tool; echo $?"),
            ("program-and-loader", both, "echo $?"),
            ("view-tmp", vec!["/tmp/tool"], "echo $?"),
        ] {
            let start = Start {
                roots: &[],
                entries: &entries,
                script,
            };
            println!(
                "{NAME} {SCENARIO_NAME} inside: {what} {}",
                run(&inside.adapter, &inside.workspace, &start).await
            );
        }
        return;
    }
    let lines = match staged(
        CASE,
        SCENARIO_NAME,
        "$mkdir -p \"$H/workspaces/ws_exec/vol\" \"$H/workspaces/ws_exec/t\" \
         && $mount -t ramfs ramfs \"$H/workspaces/ws_exec/vol\" \
         && $mount -t tmpfs tmpfs \"$H/workspaces/ws_exec/t\" \
         && $cp \"$true\" \"$H/workspaces/ws_exec/vol/tool\" \
         && $cp \"$true\" \"$H/workspaces/ws_exec/t/tool\"",
    ) {
        Ok(lines) => lines,
        Err(reason) => return println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})"),
    };
    let failures = compare(
        SCENARIO_NAME,
        &lines,
        &[
            ("on-ramfs", UNPLACED),
            ("ramfs-itself", UNPLACED),
            ("on-tmpfs", RAN),
            ("tmpfs-itself", RAN),
            ("program-and-loader", RAN),
            ("view-tmp", "refused exec.sandbox-exec-subpath-undeclared"),
        ],
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME} {SCENARIO_NAME}: executed");
}

/// **The driver's walk and the helper's end at the same file.** The read-only root
/// `<host>/tools` holds links to `/usr/bin` whose targets the two sides must resolve alike:
/// the driver from the text and its own mount table before dispatch, the helper on the
/// directories it holds open in the run's view.
///
/// - `after-a-link`: `x/a/../bin`, where `x/a` is a link to `/usr/lib`: `..` is taken from
///   where the link led, not from where it lies;
/// - `above-the-root`: `/../../usr/bin`;
/// - `dots-and-slashes`: `/usr/./bin//./`;
/// - `up-through-the-view`: `../../usr/bin`, through the view's own root;
/// - `far-above`: more `..` than the link lies deep.
///
/// Each runs a program of `/usr/bin`: the rule landed on `/usr/bin`. A status 126 is a rule
/// that landed elsewhere, and `ran ""` a helper that refused what the driver admitted.
///
/// - `through-writable`: `/workspace/sub/../../usr/bin` passes through a directory of the
///   workspace and leaves it again;
/// - `chain-through-writable`: a link to that link.
///
/// Both are refused before dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn adversary7_the_two_walks_end_at_the_same_file() {
    const CASE: &str = "adversary7_the_two_walks_end_at_the_same_file";
    const SCENARIO_NAME: &str = "walks";
    const LINKS: [(&str, &str); 8] = [
        ("x/a", "/usr/lib"),
        ("after-a-link", "x/a/../bin"),
        ("above-the-root", "/../../usr/bin"),
        ("dots-and-slashes", "/usr/./bin//./"),
        ("up-through-the-view", "../../usr/bin"),
        ("far-above", "../../../../../../usr/bin"),
        ("through-writable", "/workspace/sub/../../usr/bin"),
        ("chain-through-writable", "through-writable"),
    ];
    if let Some(host) = inside(SCENARIO_NAME) {
        let Some(inside) = enter(SCENARIO_NAME, &host) else {
            return;
        };
        let tools = host.join("tools");
        std::fs::create_dir_all(tools.join("x")).expect("x");
        std::fs::create_dir_all(inside.workspace.join("sub")).expect("sub");
        for (link, target) in LINKS {
            symlink(target, tools.join(link)).expect("a link");
        }
        let roots = [(tools, "/opt/k")];
        for (link, _) in &LINKS[1..] {
            let entry = format!("/opt/k/{link}");
            let start = Start {
                roots: &roots,
                entries: &[entry.as_str()],
                script: &inside.script,
            };
            println!(
                "{NAME} {SCENARIO_NAME} inside: {link} {}",
                run(&inside.adapter, &inside.workspace, &start).await
            );
        }
        return;
    }
    let lines = match staged(CASE, SCENARIO_NAME, "$mkdir -p \"$H/tools\"") {
        Ok(lines) => lines,
        Err(reason) => return println!("{NAME} {SCENARIO_NAME}: ABSENT ({reason})"),
    };
    let failures = compare(
        SCENARIO_NAME,
        &lines,
        &[
            ("after-a-link", RAN),
            ("above-the-root", RAN),
            ("dots-and-slashes", RAN),
            ("up-through-the-view", RAN),
            ("far-above", RAN),
            ("through-writable", UNREPRESENTABLE),
            ("chain-through-writable", UNREPRESENTABLE),
        ],
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME} {SCENARIO_NAME}: executed");
}
