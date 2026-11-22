//! Security review, second round, of `story:executable-deny-list` (design 35 § 8, D11 with
//! D30), through the real `linux` adapter and the lane's statically linked helper. What the
//! committed cases do not ask:
//!
//! - **the created-later limit, whole** (§ 8.2, § 8.5): the stated figures are for a file the
//!   run copies, links or renames to the name of an absent deny entry. Here the entry's parent
//!   directory is absent too, and the run renames a directory of the listed directory to it.
//!   The file ran under the denied name. The stakeholder decided to state that limit exactly
//!   and to change no mechanism (S8-1), so the case is a `limit_` case that holds the figures;
//! - **what the expansion cannot read** (D26, invariant 3 of `AGENTS.md`): an earlier run
//!   takes the read permission from a directory on the way to a deny entry;
//! - **what the expansion cannot place**: a mount of a file-system type outside the list that
//!   is itself an entry of an expanded directory. The driver refuses that start before
//!   dispatch (S8-2), and the case requires the refusal;
//! - **more names of a denied file in the view at the start**: a denied directory that is a
//!   mount point and is mounted a second time beneath a listed directory, a denied file bound
//!   over a second name, and a deny entry that is a symbolic link in a read-only root.
//!
//! The first two cases make no mount. The third makes its mounts in a user and mount
//! namespace of its own and runs itself again there. All need `SUBSTRATE_VECTORS_CGROUP_ROOT`
//! naming a delegated cgroup v2 subtree this process is inside, bubblewrap and
//! `SUBSTRATE_EXEC_GATE` naming the static helper. Without one of them a case prints
//! `<name>: ABSENT (<reason>)` and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    ReadOnlyFile, ReadOnlyRoot, RunState, linux::LinuxRequest,
};

/// Names the host root, in the process a case starts inside a mount namespace of its own.
const INNER: &str = "SECURITY5_DENIED_HOST";

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

/// A tool of the host, where this distribution keeps it.
fn find(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
}

fn tool(name: &str) -> PathBuf {
    find(name).unwrap_or_else(|| panic!("no {name} on this host"))
}

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8").to_owned()
}

/// The adapter over `workspace_root` with the lane's helper, both list facts proved, or why
/// the case is absent.
fn open(workspace_root: &Path) -> Result<Arc<dyn ConfinementAdapter>, String> {
    let cgroup_root = delegated()?;
    let mut config = HostConfig::minimum(workspace_root.to_path_buf());
    config.exec_gate = Some(PathBuf::from(
        std::env::var_os("SUBSTRATE_EXEC_GATE")
            .expect("SUBSTRATE_EXEC_GATE is unset: the lane names the helper it built"),
    ));
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .map_err(|error| format!("linux did not open: {error}"))?;
    for fact in [Fact::ExecSubpaths, Fact::ExecDeniedPaths] {
        if !adapter.facts().proves(&fact) {
            return Err(format!(
                "the adapter did not prove {fact:?}: {:?}",
                adapter.facts()
            ));
        }
    }
    Ok(adapter)
}

fn lists(workspace: &Path, allow: &[&str], deny: &[&str]) -> ConfinementProfile {
    let owned = |list: &[&str]| list.iter().map(|entry| (*entry).to_owned()).collect();
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        exec_subpaths: owned(allow),
        exec_denied_paths: owned(deny),
        ..ConfinementProfile::default()
    }
}

/// What one start came to.
#[derive(Debug)]
enum Outcome {
    /// Refused at admission or before dispatch: the refusal's own text, which starts with its
    /// code.
    Refused(String),
    /// Dispatched: the exit code, standard output and standard error of the run.
    Ran(Option<i32>, String, String),
}

impl Outcome {
    /// The outcome on one line, as a case prints it.
    fn line(&self) -> String {
        match self {
            Self::Refused(why) => format!("refused {why}"),
            Self::Ran(code, stdout, stderr) => format!(
                "ran {code:?} {:?} {:?}",
                stdout.replace('\n', " ").trim_end(),
                stderr.trim_end()
            ),
        }
    }
}

/// Admits `profile` and starts the host's shell with `script` and an empty environment.
async fn shell(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    script: &str,
) -> Outcome {
    let refused = |error: &ConfinementError| Outcome::Refused(format!("{}: {error}", error.code()));
    let admitted = match adapter.admit(profile) {
        Ok(admitted) => admitted,
        Err(error) => return refused(&error),
    };
    let command = Command {
        argv: vec![text(&tool("sh")), "-c".to_owned(), script.to_owned()],
        env: BTreeMap::new(),
    };
    let run = match adapter.start(&admitted, &command).await {
        Ok(run) => run,
        Err(error) => return refused(&error),
    };
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    Outcome::Ran(
        ended.exit.as_ref().and_then(|exit| exit.code),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A host root with a workspace that holds `bin/{tool,other}` and `bin/lib/helper`, copies of
/// the host's `true`; `outside/a`, one more, in a directory no profile here lists; and
/// `tools`, copies of the host's own `names`.
fn workspace(names: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let host = tempfile::tempdir().expect("host root");
    let workspace = host
        .path()
        .canonicalize()
        .expect("canonical host root")
        .join("workspaces/ws_security5");
    for directory in ["bin/lib", "outside", "tools"] {
        std::fs::create_dir_all(workspace.join(directory)).expect(directory);
    }
    for name in ["bin/tool", "bin/other", "bin/lib/helper", "outside/a"] {
        std::fs::copy(tool("true"), workspace.join(name)).expect("copy true");
    }
    for name in names {
        std::fs::copy(tool(name), workspace.join("tools").join(name)).expect("copy a tool");
    }
    (host, workspace)
}

/// **The limit of a deny entry in a listed directory the run can write** (design 35 § 8.2
/// and § 8.5, as the security review of round 2 measured it, S8-1; the stakeholder decided to
/// state the limit exactly and to change no mechanism). The review wrote this case with the
/// expectation `copied after a rename 126`, which the shipped helper does not meet; it is a
/// `limit_` case now and holds every figure it measured.
///
/// **In a listed directory a run can write, a deny entry holds for the files that exist at
/// the start, under the names they have then.** A directory inside an expanded directory
/// that holds no denied path gets one rule. What the run creates in it executes, and the
/// rule moves with a rename. So a file the run creates or moves can run under any name, the
/// denied name included. Nothing runs that the run could not already run by copying it into
/// a listed directory it can write.
///
/// `bin` is listed and can be written by the run. The deny entry is `bin/sub/tool`, and
/// neither it nor `bin/sub` exists at the start. `bin/lib` is a directory of `bin` that holds
/// no denied path, so the expansion of `bin` gives it one rule, on the directory.
///
/// - `listed 0`: a program of `bin/lib` runs.
/// - `new directory 126`: the run makes `bin/fresh` and copies a program no entry lists into
///   it. The directory is new, in the expanded `bin`, and has no rule.
/// - `copied 126`: the run makes `bin/sub` and copies that program to `bin/sub/tool`, the deny
///   entry's own name.
/// - `copied after a rename 0`: the run removes its `bin/sub`, renames `bin/lib` to `bin/sub`
///   and copies the same program to `bin/sub/tool`. The directory is the one that got a rule,
///   under the name the deny entry's parent was to have, and the file runs under the denied
///   name.
/// - `a link to a program of another listed directory 0`: a symbolic link the run makes
///   under the name of a second absent deny entry, `bin/later`, to a program of `tools`.
///
/// A second start, with a deny entry that exists, `bin/held/tool`:
///
/// - `denied 126`: the denied file.
/// - `new file in a directory of the expanded one 0`: a program no entry lists, copied into
///   `bin/lib`.
/// - `the denied path after a rename 0`: the run moves `bin/held` away, renames `bin/lib` to
///   `bin/held` and copies that program to `bin/held/tool`.
/// - `the denied file 126`: the file that was denied at the start, under its new name
///   `bin/gone/tool`. The rule is on the file: it stays denied wherever the run moves it.
#[tokio::test(flavor = "multi_thread")]
async fn limit_a_file_the_run_creates_or_moves_runs_under_a_denied_name() {
    const NAME: &str = "security5 exec denied limit created or moved by the run";
    let started = Instant::now();
    let (host, workspace) = workspace(&["mkdir", "cp", "mv", "rm", "ln"]);
    let adapter = match open(&host.path().join("workspaces")) {
        Ok(adapter) => adapter,
        Err(reason) => return println!("{NAME}: ABSENT ({reason})"),
    };
    let script = "bin/lib/helper; echo \"listed $?\"; \
        tools/mkdir bin/fresh; tools/cp outside/a bin/fresh/tool; \
        bin/fresh/tool 2>/dev/null; echo \"new directory $?\"; \
        tools/mkdir bin/sub; tools/cp outside/a bin/sub/tool; \
        bin/sub/tool 2>/dev/null; echo \"copied $?\"; \
        tools/rm -r bin/sub; tools/mv bin/lib bin/sub; tools/cp outside/a bin/sub/tool; \
        bin/sub/tool 2>/dev/null; echo \"copied after a rename $?\"; \
        tools/ln -s ../tools/mkdir bin/later; bin/later --version >/dev/null 2>&1; \
        echo \"a link to a program of another listed directory $?\"";
    let outcome = shell(
        &adapter,
        &lists(
            &workspace,
            &["bin", "tools"],
            &["bin/sub/tool", "bin/later"],
        ),
        script,
    )
    .await;
    println!("{NAME}: {}", outcome.line());
    let Outcome::Ran(_, stdout, stderr) = outcome else {
        panic!("the start was refused: a deny entry need not exist at the start");
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "listed 0",
            "new directory 126",
            "copied 126",
            "copied after a rename 0",
            "a link to a program of another listed directory 0",
        ],
        "an entry that is absent at the start, the limit as stated: {stderr}"
    );

    // The same with a deny entry that exists at the start: `bin/held/tool` is denied, the
    // run moves `bin/held` away, renames `bin/lib` to `bin/held` and copies a program no
    // entry lists to the denied path.
    let second = workspace.with_file_name("ws_security5_existing");
    for directory in ["bin/held", "bin/lib", "outside", "tools"] {
        std::fs::create_dir_all(second.join(directory)).expect(directory);
    }
    for name in ["bin/held/tool", "bin/lib/helper", "outside/a"] {
        std::fs::copy(tool("true"), second.join(name)).expect("copy true");
    }
    for name in ["cp", "mv"] {
        std::fs::copy(tool(name), second.join("tools").join(name)).expect("copy a tool");
    }
    let existing = shell(
        &adapter,
        &lists(&second, &["bin", "tools"], &["bin/held/tool"]),
        "bin/held/tool 2>/dev/null; echo \"denied $?\"; \
         tools/cp outside/a bin/lib/new; bin/lib/new 2>/dev/null; \
         echo \"new file in a directory of the expanded one $?\"; \
         tools/mv bin/held bin/gone; tools/mv bin/lib bin/held; \
         tools/cp outside/a bin/held/tool; bin/held/tool 2>/dev/null; \
         echo \"the denied path after a rename $?\"; \
         bin/gone/tool 2>/dev/null; echo \"the denied file $?\"",
    )
    .await;
    println!("{NAME}: an entry that exists: {}", existing.line());
    let Outcome::Ran(_, stdout, stderr) = existing else {
        panic!("the start with a deny entry that exists was refused");
    };
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            "denied 126",
            "new file in a directory of the expanded one 0",
            "the denied path after a rename 0",
            "the denied file 126",
        ],
        "an entry that exists at the start, the limit as stated: {stderr}"
    );
    println!("{NAME}: executed in {:?}", started.elapsed());
}

/// **A directory the expansion cannot read ends the start with nothing run** (design 35
/// D26, § 12: "After dispatch there is no refusal: a helper failure is exit status 125";
/// invariant 3 of `AGENTS.md`).
///
/// `bin` is listed and `bin/held/tool` denied, so the helper expands `bin` and then
/// `bin/held`. An earlier run with the same workspace and no deny-list takes the read
/// permission from `bin/held`. The names in it still open, so the driver places the deny
/// entry and the listed directory as before, and only the helper, which has to read the
/// directory's entries, finds out.
///
/// Held: no program of `bin/held` runs, the denied one and its neighbour alike, and the
/// start is refused before dispatch or ends with status 125 and nothing printed. Which of
/// the two is printed, because a run causes it for every later start of that profile.
#[tokio::test(flavor = "multi_thread")]
async fn security5_a_directory_an_earlier_run_made_unreadable_runs_nothing_of_it() {
    const NAME: &str = "security5 exec denied unreadable expanded directory";
    let started = Instant::now();
    let (host, workspace) = workspace(&["chmod"]);
    std::fs::create_dir_all(workspace.join("bin/held")).expect("held");
    for name in ["tool", "other"] {
        std::fs::copy(tool("true"), workspace.join("bin/held").join(name)).expect("copy true");
    }
    let adapter = match open(&host.path().join("workspaces")) {
        Ok(adapter) => adapter,
        Err(reason) => return println!("{NAME}: ABSENT ({reason})"),
    };
    let listed = lists(&workspace, &["bin"], &["bin/held/tool"]);
    let script =
        "bin/held/tool 2>/dev/null; echo \"denied $?\"; bin/held/other; echo \"beside $?\"";
    let before = shell(&adapter, &listed, script).await;
    println!("{NAME}: before: {}", before.line());
    assert!(
        matches!(&before, Outcome::Ran(Some(0), stdout, _) if stdout == "denied 126\nbeside 0\n"),
        "the control, the directory as it was: {before:?}"
    );
    let earlier = shell(
        &adapter,
        &lists(&workspace, &["tools"], &[]),
        "tools/chmod u-r bin/held; echo $?",
    )
    .await;
    assert!(
        matches!(&earlier, Outcome::Ran(Some(0), stdout, _) if stdout == "0\n"),
        "the earlier run takes the read permission from its own directory: {earlier:?}"
    );
    let after = shell(&adapter, &listed, script).await;
    // The scratch root is removed when the case ends, and that needs the permission back.
    std::fs::set_permissions(
        workspace.join("bin/held"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("the permission back");
    println!("{NAME}: after: {}", after.line());
    match &after {
        Outcome::Refused(_) => {}
        Outcome::Ran(code, stdout, stderr) => {
            assert_eq!(
                (*code, stdout.as_str()),
                (Some(125), ""),
                "a start whose expansion cannot read a directory ran something: {stderr}"
            );
            assert!(stderr.contains("cannot read the directory"), "{stderr}");
        }
    }
    println!("{NAME}: executed in {:?}", started.elapsed());
}

/// **A program in a declared mount beneath a denied directory is refused before dispatch**
/// (design 35 § 8.2: "A denied program is refused before dispatch"; D30: "the denied places
/// include the root of every … mount beneath a denied directory"; the helper "is the second
/// line").
///
/// The mount beneath the denied directory is one the profile itself declares, so it is in
/// the run's view and not in the driver's own mount table: a read-only root at
/// `/workspace/held/k` with `held` denied, and a read-only root at `/opt/a/b` beneath another
/// at `/opt/a` with `/opt/a` denied, and a read-only file at `/opt/a/prog` beneath that
/// root. In each the program is the file of the inner mount.
///
/// Held: the start is refused `exec.sandbox-program-unlisted`, as it is for a host mount
/// beneath a denied directory, and never dispatched for the helper to find the program
/// without a rule. Where the profile's own mounts cannot be declared that way, that is what
/// is held: the control is refused as a mount no profile can declare, and so the driver's
/// table shows every mount of the view that lies beneath another.
// One case on purpose: three shapes of one question, each with its control.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn security5_a_program_in_a_declared_mount_beneath_a_denied_directory_is_refused() {
    const NAME: &str = "security5 exec denied program in a declared mount";
    let started = Instant::now();
    let (host, workspace) = workspace(&[]);
    std::fs::create_dir_all(workspace.join("held/k")).expect("the mount point");
    let root = host.path().canonicalize().expect("canonical host root");
    for directory in ["inner", "outer/b"] {
        std::fs::create_dir_all(root.join(directory)).expect(directory);
    }
    std::fs::copy(tool("true"), root.join("inner/prog")).expect("copy true");
    let adapter = match open(&host.path().join("workspaces")) {
        Ok(adapter) => adapter,
        Err(reason) => return println!("{NAME}: ABSENT ({reason})"),
    };
    // Each declared mount: its source beneath the host root, its mount, and whether it is a
    // single file.
    let declared = |mounts: &[(&str, &str, bool)], allow: &[&str], deny: &[&str]| {
        let mut profile = lists(&workspace, allow, deny);
        for (source, mount, file) in mounts {
            let (host_path, mount) = (text(&root.join(source)), (*mount).to_owned());
            if *file {
                profile
                    .read_only_files
                    .push(ReadOnlyFile { host_path, mount });
            } else {
                profile
                    .read_only_roots
                    .push(ReadOnlyRoot { host_path, mount });
            }
        }
        profile
    };
    let mut failures = Vec::new();
    for (what, mounts, program, allow, deny) in [
        (
            "a root beneath a denied directory of the workspace",
            &[("inner", "/workspace/held/k", false)][..],
            "/workspace/held/k/prog",
            &["bin", "/workspace/held/k"][..],
            &["held"][..],
        ),
        (
            "a root beneath a denied root",
            &[("outer", "/opt/a", false), ("inner", "/opt/a/b", false)][..],
            "/opt/a/b/prog",
            &["bin", "/opt/a/b"][..],
            &["/opt/a"][..],
        ),
        (
            "a file beneath a denied root",
            &[
                ("outer", "/opt/a", false),
                ("inner/prog", "/opt/a/prog", true),
            ][..],
            "/opt/a/prog",
            &["bin", "/opt/a/prog"][..],
            &["/opt/a"][..],
        ),
    ] {
        let start = |deny: &[&str]| {
            let profile = declared(mounts, allow, deny);
            let adapter = adapter.clone();
            async move {
                let refused = |error: &ConfinementError| {
                    Outcome::Refused(format!("{}: {error}", error.code()))
                };
                let admitted = match adapter.admit(&profile) {
                    Ok(admitted) => admitted,
                    Err(error) => return refused(&error),
                };
                let command = Command {
                    argv: vec![program.to_owned()],
                    env: BTreeMap::new(),
                };
                let run = match adapter.start(&admitted, &command).await {
                    Ok(run) => run,
                    Err(error) => return refused(&error),
                };
                let ended = adapter.wait(&run).await.expect("waited");
                let output = adapter.output(&run).await.expect("output");
                Outcome::Ran(
                    ended.exit.as_ref().and_then(|exit| exit.code),
                    String::from_utf8_lossy(&output.stdout).into_owned(),
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                )
            }
        };
        let control = start(&[]).await;
        println!("{NAME}: {what}: without the deny entry: {}", control.line());
        if !matches!(&control, Outcome::Ran(Some(0), _, _)) {
            // A shape no profile can declare: then no mount of the view is beneath another
            // that the driver's own table does not show.
            let invalid = matches!(
                &control,
                Outcome::Refused(why)
                    if why.starts_with("exec.read-only-root-invalid")
                        || why.starts_with("exec.read-only-file")
            );
            if !invalid {
                failures.push(format!(
                    "{what}: the control neither ran nor was refused as a mount no profile can \
                     declare: {}",
                    control.line()
                ));
            }
            continue;
        }
        let outcome = start(deny).await;
        println!("{NAME}: {what}: with {deny:?} denied: {}", outcome.line());
        match &outcome {
            Outcome::Refused(why) if why.starts_with("exec.sandbox-program-unlisted") => {}
            Outcome::Ran(Some(0), _, _) => failures.push(format!(
                "{what}: the denied program ran: {}",
                outcome.line()
            )),
            _ => failures.push(format!(
                "{what}: the program lies in a mount beneath the denied {deny:?}, and the start \
                 was not refused exec.sandbox-program-unlisted before dispatch: {}",
                outcome.line()
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME}: executed in {:?}", started.elapsed());
}

/// Runs `case` again inside a user and mount namespace after `stage`, a shell fragment whose
/// first positional parameter is `tools`: what that run printed, with its diagnostics after
/// it, or why the case is absent.
fn staged(case: &str, host: &Path, stage: &str, tools: &Path) -> Result<String, String> {
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
        .arg(tools)
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
            "no tmpfs, ramfs or bind mount in a user namespace: {}",
            stderr.trim()
        ));
    }
    if output.status.code() == Some(1) && stderr.trim_start().starts_with("unshare:") {
        return Err(format!("no user and mount namespace: {}", stderr.trim()));
    }
    Ok(format!("{stdout}\n{stderr}"))
}

const MOUNTS: &str = "security5 exec denied names across mounts";

/// The observation `what` of the run inside the namespace, from what it printed.
fn observed(stdout: &str, what: &str) -> Option<String> {
    let inside = format!("{MOUNTS} inside: ");
    stdout
        .lines()
        // The harness writes `test <name> ... ` before the first line of a case.
        .filter_map(|line| line.split_once(inside.as_str()).map(|(_, rest)| rest))
        .find_map(|line| line.strip_prefix(&format!("{what}: ")))
        .map(|rest| rest.trim().to_owned())
}

/// One start of the staged case: its name, the allow-list, the deny-list and the script.
type Row = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static str,
);

/// The starts of the staged case. The root `<host>/tools` is declared at `/opt/k`; the stage
/// is in the case's own comment.
const ROWS: [Row; 6] = [
    (
        "mount point",
        &["/opt/k"],
        &["/opt/k/vol"],
        "/opt/k/vol/prog 2>/dev/null; echo \"vol $?\"; /opt/k/again/prog 2>/dev/null; \
         echo \"again $?\"; /opt/k/bin/other; echo \"beside $?\"",
    ),
    (
        "file bind",
        &["/opt/k/alias", "/opt/k/bin"],
        &["/opt/k/bin/tool"],
        "/opt/k/bin/tool 2>/dev/null; echo \"tool $?\"; /opt/k/alias/name 2>/dev/null; \
         echo \"name $?\"; /opt/k/alias/plain; echo \"plain $?\"",
    ),
    (
        "link entry",
        &["/opt/k/bin"],
        &["/opt/k/link"],
        "/opt/k/bin/tool 2>/dev/null; echo \"tool $?\"; /opt/k/link 2>/dev/null; \
         echo \"link $?\"; /opt/k/bin/other; echo \"beside $?\"",
    ),
    (
        "unlisted beside",
        &["/opt/k/odd", "/opt/k/bin"],
        &["/opt/k/bin/tool"],
        "/opt/k/odd/prog; echo \"prog $?\"; /opt/k/odd/media/prog; echo \"media $?\"; \
         /opt/k/bin/tool 2>/dev/null; echo \"tool $?\"",
    ),
    (
        "unlisted entry",
        &["/opt/k/odd", "/opt/k/bin"],
        &["/opt/k/odd/prog"],
        "/opt/k/odd/prog 2>/dev/null; echo \"prog $?\"; /opt/k/odd/beside; echo \"beside $?\"; \
         /opt/k/bin/other; echo \"other $?\"",
    ),
    (
        "unlisted entry, allow-list alone",
        &["/opt/k/odd", "/opt/k/bin"],
        &[],
        "/opt/k/odd/prog; echo \"prog $?\"; /opt/k/odd/media/prog; echo \"media $?\"",
    ),
];

/// The staged case inside its namespace.
async fn mounts_inside(host: &Path) {
    let adapter = match open(&host.join("workspaces")) {
        Ok(adapter) => adapter,
        Err(reason) => return println!("{MOUNTS} inside: ABSENT: ({reason})"),
    };
    let workspace = host.join("workspaces/ws_security5");
    std::fs::create_dir_all(&workspace).expect("the workspace");
    for (what, allow, deny, script) in ROWS {
        let profile = ConfinementProfile {
            read_only_roots: vec![ReadOnlyRoot {
                host_path: text(&host.join("tools")),
                mount: "/opt/k".to_owned(),
            }],
            ..lists(&workspace, allow, deny)
        };
        println!(
            "{MOUNTS} inside: {what}: {}",
            shell(&adapter, &profile, script).await.line()
        );
    }
}

/// **More names of a denied file in the view at the start, and a mount the expansion cannot
/// place** (design 35 D11 with D30, D23, D26).
///
/// A declared read-only root, `<host>/tools` at `/opt/k`, holds `bin/{tool,other}`,
/// `alias/plain`, `odd/{prog,beside}` and `link`, a symbolic link to `bin/tool`. The stage
/// mounts a tmpfs at `vol` with `prog` in it and binds it a second time at `again`; binds the
/// file `bin/tool` over `alias/name`; and mounts a ramfs, a type outside the list, at
/// `odd/media`, with `prog` in it.
///
/// - `mount point`: the root is listed and `vol`, a mount point, is denied. Its program does
///   not run under `vol` or under `again`, the second mount of the same file system, and a
///   program beside them runs.
/// - `file bind`: `alias` and `bin` are listed and `bin/tool` is denied. The file does not
///   run under `alias/name`, the mount of it, and `alias/plain` runs.
/// - `link entry`: the deny entry is `link`. The program it points at does not run under
///   either name.
/// - `unlisted beside`: `odd` is listed and the deny entry lies elsewhere. `odd` is not
///   expanded, the ramfs beneath it is compared with nothing, and its program runs.
/// - `unlisted entry`: the deny entry is `odd/prog`, so `odd` is expanded and the ramfs mount
///   point is one of its entries, with no place. Held: nothing runs, and the start is
///   refused before dispatch, `exec.sandbox-place-unserved`, with the mount point and the
///   cause (S8-2). Its control, the same lists without the deny entry,
///   runs both programs.
#[tokio::test(flavor = "multi_thread")]
async fn security5_a_denied_file_is_denied_under_the_names_mounts_give_it() {
    const CASE: &str = "security5_a_denied_file_is_denied_under_the_names_mounts_give_it";
    if let Some(host) = std::env::var_os(INNER) {
        mounts_inside(Path::new(&host)).await;
        return;
    }
    let started = Instant::now();
    let (Some(mount), Some(cp), Some(program)) = (find("mount"), find("cp"), find("true")) else {
        return println!("{MOUNTS}: ABSENT (no mount, cp or true on this host)");
    };
    let directory = tempfile::tempdir().expect("host root");
    let host = directory
        .path()
        .canonicalize()
        .expect("canonical host root");
    let tools = host.join("tools");
    for name in ["bin", "alias", "odd/media", "vol", "again"] {
        std::fs::create_dir_all(tools.join(name)).expect(name);
    }
    for name in [
        "bin/tool",
        "bin/other",
        "alias/plain",
        "odd/prog",
        "odd/beside",
    ] {
        std::fs::copy(&program, tools.join(name)).expect("copy true");
    }
    std::fs::write(tools.join("alias/name"), b"").expect("the file a mount covers");
    std::os::unix::fs::symlink("bin/tool", tools.join("link")).expect("the link");
    let stage = format!(
        "{mount} -t tmpfs tmpfs \"$1/vol\" && {cp} \"$1/bin/tool\" \"$1/vol/prog\" \
         && {mount} --bind \"$1/vol\" \"$1/again\" \
         && {mount} --bind \"$1/bin/tool\" \"$1/alias/name\" \
         && {mount} -t ramfs ramfs \"$1/odd/media\" && {cp} \"$1/bin/tool\" \"$1/odd/media/prog\"",
        mount = mount.display(),
        cp = cp.display()
    );
    let stdout = match staged(CASE, &host, &stage, &tools) {
        Ok(stdout) => stdout,
        Err(reason) => return println!("{MOUNTS}: ABSENT ({reason})"),
    };
    if let Some(reason) = observed(&stdout, "ABSENT") {
        return println!("{MOUNTS}: ABSENT (inside the namespace: {reason})");
    }
    let line = |what: &str| {
        let answer = observed(&stdout, what).unwrap_or_else(|| panic!("no `{what}`: {stdout}"));
        println!("{MOUNTS}: {what}: {answer}");
        answer
    };
    let mut failures = Vec::new();
    for (what, expected) in [
        ("mount point", "ran Some(0) \"vol 126 again 126 beside 0\""),
        ("file bind", "ran Some(0) \"tool 126 name 126 plain 0\""),
        ("link entry", "ran Some(0) \"tool 126 link 126 beside 0\""),
        ("unlisted beside", "ran Some(0) \"prog 0 media 0 tool 126\""),
        (
            "unlisted entry, allow-list alone",
            "ran Some(0) \"prog 0 media 0\"",
        ),
    ] {
        let answer = line(what);
        if !answer.starts_with(expected) {
            failures.push(format!("{what}: {answer}, not {expected}"));
        }
    }
    // S8-2, decided as a fix: the driver has the row, the listed place and the denied place
    // in its own table, so the start is refused before dispatch. A start that is dispatched
    // and ends with the helper's status 125 was accepted here before that decision.
    let unlisted = line("unlisted entry");
    if !(unlisted.starts_with("refused exec.sandbox-place-unserved")
        && unlisted.contains("odd/media: file-system-unlisted ramfs"))
    {
        failures.push(format!(
            "unlisted entry: not refused before dispatch with the mount point and the cause: \
             {unlisted}"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!(
        "{MOUNTS}: executed in {:?}; the unlisted entry: refused before dispatch",
        started.elapsed()
    );
}
