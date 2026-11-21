//! Adversary pass on `story:executable-deny-list` (design 35 § 8), commit `aafb9823a`.
//!
//! Every case starts runs through `ConfinementAdapter` on the real `linux` adapter, opened with
//! the helper the lane built, and reads what the run itself printed.
//!
//! The cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this
//! process is inside, bubblewrap, and `SUBSTRATE_EXEC_GATE` naming the statically linked
//! `substrate-exec-gate`. Without the first each prints `<name>: ABSENT (<reason>)` and is
//! absent, never passed.
//!
//! No case lists a directory of the base system and calls a tool of it by name: every tool a
//! script runs is a copy in the workspace, beneath a listed directory of its own.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    ReadOnlyFile, ReadOnlyRoot, RunState, linux::LinuxRequest,
};
use substrate_host::seatbelt::{PATH_UNREPRESENTABLE, PROGRAM_UNLISTED};

fn delegated(name: &str) -> Option<PathBuf> {
    let absent = |reason: &str| {
        println!("{name}: ABSENT ({reason})");
        None
    };
    let Some(root) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT").map(PathBuf::from) else {
        return absent("SUBSTRATE_VECTORS_CGROUP_ROOT is unset");
    };
    if std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").is_err() {
        return absent("the delegated cgroup root does not accept controllers");
    }
    if !Path::new("/usr/bin/bwrap").is_file() {
        return absent("no bubblewrap at /usr/bin/bwrap");
    }
    Some(root)
}

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

struct Lane {
    host: tempfile::TempDir,
    adapter: Arc<dyn ConfinementAdapter>,
    workspace: PathBuf,
}

/// The adapter opened with the lane's helper, having proved both list facts, and a workspace
/// with `bin/{tool,other}`, copies of the host's `true`, and `tools/{mv,ln}`, copies of the
/// host's own.
fn lane(name: &str) -> Option<Lane> {
    let cgroup_root = delegated(name)?;
    let gate = PathBuf::from(std::env::var_os("SUBSTRATE_EXEC_GATE").expect(
        "SUBSTRATE_EXEC_GATE is unset: `bash scripts/delegated-lane.sh` builds the helper",
    ));
    let host = tempfile::tempdir().expect("host root");
    let mut config = HostConfig::minimum(host.path().join("workspaces"));
    config.exec_gate = Some(gate);
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .expect("linux opens");
    for fact in [Fact::ExecSubpaths, Fact::ExecDeniedPaths] {
        assert!(
            adapter.facts().proves(&fact),
            "{name}: linux opened with the helper did not prove {fact:?}: {:?}",
            adapter.facts()
        );
    }
    let workspace = host.path().join("workspaces").join("ws_adversary4");
    for directory in ["bin", "other", "tools"] {
        std::fs::create_dir_all(workspace.join(directory)).expect(directory);
    }
    for name in ["bin/tool", "bin/other", "other/tool"] {
        std::fs::copy(tool("true"), workspace.join(name)).expect("copy true");
    }
    for name in ["mv", "ln"] {
        std::fs::copy(tool(name), workspace.join("tools").join(name)).expect("copy a tool");
    }
    Some(Lane {
        host,
        adapter,
        workspace,
    })
}

fn lists(workspace: &Path, allow: &[&str], deny: &[&str]) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        exec_subpaths: allow.iter().map(|entry| (*entry).to_owned()).collect(),
        exec_denied_paths: deny.iter().map(|entry| (*entry).to_owned()).collect(),
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

/// Admits `profile` and starts `argv` with an empty environment.
async fn start(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    argv: &[&str],
) -> Outcome {
    let refused = |error: &ConfinementError| Outcome::Refused(format!("{}: {error}", error.code()));
    let admitted = match adapter.admit(profile) {
        Ok(admitted) => admitted,
        Err(error) => return refused(&error),
    };
    let command = Command {
        argv: argv.iter().map(|part| (*part).to_owned()).collect(),
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

/// [`start`] of the host's shell with `script`.
async fn shell(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    script: &str,
) -> Outcome {
    start(adapter, profile, &[&text(&tool("sh")), "-c", script]).await
}

/// **Every other name of a denied file, tried inside the run.** `bin` is listed and `bin/tool`
/// denied. On the host before the start: `bin/link`, a symbolic link to it in the listed
/// directory; `bin/hard`, a hard link beside it; `other/link`, a symbolic link from a listed
/// directory that was not expanded. Inside the run: the names through `/proc/self/root` and
/// `/proc/<pid>/cwd`, through `..`, after a rename within the directory, and a hard link the
/// run makes beside it. A hard link into the other listed directory must not be made. The
/// control is the neighbour `bin/other`, which runs.
#[tokio::test(flavor = "multi_thread")]
async fn adversary4_every_other_name_of_a_denied_file_stays_denied() {
    const NAME: &str = "adversary4 exec deny other names";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let bin = lane.workspace.join("bin");
    std::os::unix::fs::symlink("tool", bin.join("link")).expect("a link");
    std::fs::hard_link(bin.join("tool"), bin.join("hard")).expect("a hard link");
    std::os::unix::fs::symlink("../bin/tool", lane.workspace.join("other/link")).expect("a link");

    let profile = lists(&lane.workspace, &["bin", "other", "tools"], &["bin/tool"]);
    // One line per attempt: a label and the status the shell saw.
    let script = "bin/other; echo \"neighbour $?\"; \
        try() { label=$1; shift; \"$@\" 2>/dev/null; echo \"$label $?\"; }; \
        try entry bin/tool; try link bin/link; try hard bin/hard; try other-link other/link; \
        try dotdot bin/../bin/tool; try slashes ./bin//tool; \
        try proc-root /proc/self/root/workspace/bin/tool; try proc-cwd /proc/$$/cwd/bin/tool; \
        try absolute /workspace/bin/tool; \
        tools/mv bin/tool bin/renamed; try renamed bin/renamed; \
        tools/ln bin/renamed bin/again 2>/dev/null; try again bin/again; \
        tools/ln bin/renamed other/linked 2>/dev/null; echo \"link-into-other $?\"; \
        try linked other/linked";
    let Outcome::Ran(_, stdout, stderr) = shell(&lane.adapter, &profile, script).await else {
        panic!("the start was refused");
    };
    assert_eq!(
        stdout,
        "neighbour 0\nentry 126\nlink 126\nhard 126\nother-link 126\ndotdot 126\nslashes 126\n\
         proc-root 126\nproc-cwd 126\nabsolute 126\nrenamed 126\nagain 126\n\
         link-into-other 1\nlinked 127\n",
        "stderr: {stderr}"
    );
    println!("{NAME}: executed");
}

/// **The limit of design 35 § 8.2 and § 8.5: a deny entry absent at the start, then created
/// by the run** in a listed directory the run can write. The expansion writes its rules on
/// files, not on names. A new file under the entry's name has no rule and is refused:
/// `copied 126`. A hard link to a listed program of the same directory under that name, and
/// that program renamed to it, are the file they were, with its rule, and run: `linked 0`,
/// `renamed 0`. A client that denies a name in a directory the run can write gets a rule
/// about new files there, not about that name.
///
/// **What runs is bounded.** Only a program the run could already start under its own name
/// runs under the denied one. A program outside the list cannot be given the entry's name:
/// a hard link and a rename from an unlisted directory into the listed one fail (`EXDEV`,
/// the refer rule), and a copy of it is a new file and ends with status 126.
///
/// This was the adversary's `adversary4_a_deny_entry_the_run_creates_later_is_denied`, which
/// required 126 of all three and found the two that run.
#[tokio::test(flavor = "multi_thread")]
async fn limit_a_deny_entry_the_run_creates_by_a_link_or_a_rename_runs() {
    const NAME: &str = "adversary4 exec deny limit created later";
    let Some(lane) = lane(NAME) else {
        return;
    };
    std::fs::copy(tool("cp"), lane.workspace.join("tools/cp")).expect("copy cp");
    let profile = lists(
        &lane.workspace,
        &["bin", "tools"],
        &[
            "bin/copied",
            "bin/linked",
            "bin/renamed",
            "bin/deep/er/later",
        ],
    );
    let script = "bin/tool; echo \"listed $?\"; \
        tools/cp bin/tool bin/copied; bin/copied 2>/dev/null; echo \"copied $?\"; \
        tools/ln bin/tool bin/linked 2>/dev/null; bin/linked 2>/dev/null; echo \"linked $?\"; \
        tools/mv bin/other bin/renamed; bin/renamed 2>/dev/null; echo \"renamed $?\"";
    let Outcome::Ran(_, stdout, stderr) = shell(&lane.adapter, &profile, script).await else {
        panic!("the start was refused: a deny entry need not exist at the start");
    };
    assert_eq!(
        stdout, "listed 0\ncopied 126\nlinked 0\nrenamed 0\n",
        "a new file under the entry's name is refused; a listed program of the directory \
         linked or renamed to it is the file it was, and runs: {stderr}"
    );

    // The bound of the limit: `outside` is not listed, and no program of it runs under the
    // name of a deny entry, whether the run links it there, renames it there or copies it.
    // `bin` is expanded and has no rule of its own, so the kernel may let the link and the
    // rename happen; the file has no rule either way.
    std::fs::create_dir_all(lane.workspace.join("outside")).expect("outside");
    for name in ["a", "b", "c"] {
        std::fs::copy(tool("true"), lane.workspace.join("outside").join(name)).expect("copy true");
    }
    let profile = lists(
        &lane.workspace,
        &["bin", "tools"],
        &["bin/from-link", "bin/from-rename", "bin/from-copy"],
    );
    let script = "tools/ln outside/a bin/from-link 2>/dev/null; \
        bin/from-link 2>/dev/null; echo \"linked $?\"; \
        tools/mv outside/b bin/from-rename 2>/dev/null; \
        bin/from-rename 2>/dev/null; echo \"renamed $?\"; \
        tools/cp outside/c bin/from-copy; bin/from-copy 2>/dev/null; echo \"copied $?\"";
    let Outcome::Ran(_, outside, stderr) = shell(&lane.adapter, &profile, script).await else {
        panic!("the start was refused: a deny entry need not exist at the start");
    };
    let lines: Vec<&str> = outside.lines().collect();
    assert_eq!(lines.len(), 3, "{outside:?}: {stderr}");
    for (line, how) in lines.iter().zip(["linked", "renamed", "copied"]) {
        // 126: the file is there and has no rule. 127: the kernel refused to put it there.
        assert!(
            *line == format!("{how} 126") || *line == format!("{how} 127"),
            "a program outside the list ran under the name of a deny entry: {outside:?}: {stderr}"
        );
    }
    println!("{NAME}: a program outside the list: {outside:?}");
    println!("{NAME}: executed, copied 126, linked 0, renamed 0");
}

/// **A denied directory, renamed by the run, stays denied**, and so does a denied directory
/// that holds a listed directory.
#[tokio::test(flavor = "multi_thread")]
async fn adversary4_a_denied_directory_renamed_by_the_run_stays_denied() {
    const NAME: &str = "adversary4 exec deny directory renamed";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let inner = lane.workspace.join("bin/sub/inner");
    std::fs::create_dir_all(&inner).expect("inner");
    std::fs::copy(tool("true"), inner.join("tool")).expect("copy true");
    std::fs::copy(tool("true"), lane.workspace.join("bin/sub/tool")).expect("copy true");

    let profile = lists(
        &lane.workspace,
        &["bin", "bin/sub/inner", "tools"],
        &["bin/sub"],
    );
    let script = "bin/tool; echo $?; bin/sub/tool 2>/dev/null; echo $?; \
        bin/sub/inner/tool 2>/dev/null; echo $?; \
        tools/mv bin/sub bin/moved; bin/moved/tool 2>/dev/null; echo $?; \
        bin/moved/inner/tool 2>/dev/null; echo $?";
    let Outcome::Ran(_, stdout, stderr) = shell(&lane.adapter, &profile, script).await else {
        panic!("the start was refused");
    };
    assert_eq!(stdout, "0\n126\n126\n126\n126\n", "{stderr}");
    println!("{NAME}: executed");
}

/// **Deny entries in other spellings.** Each names `bin/tool` in a form the lexical rule of
/// § 5 does not take, or takes as the same path. A spelling is either refused by name before
/// anything runs, or it denies the file. It is never admitted and without effect.
#[tokio::test(flavor = "multi_thread")]
async fn adversary4_a_deny_entry_in_another_spelling_is_refused_or_denies() {
    const NAME: &str = "adversary4 exec deny spellings";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let mut seen = Vec::new();
    for spelling in [
        "bin/tool/",
        "bin//tool",
        "./bin/tool",
        "bin/./tool",
        "bin/../bin/tool",
        "/workspace/bin/tool",
        "/workspace/bin/tool/",
        "/workspace/./bin/tool",
        "/workspace/bin/../bin/tool",
        "/proc/self/root/workspace/bin/tool",
    ] {
        let profile = lists(&lane.workspace, &["bin"], &[spelling]);
        let outcome = shell(
            &lane.adapter,
            &profile,
            "bin/tool 2>/dev/null; echo $?; bin/other; echo $?",
        )
        .await;
        match &outcome {
            Outcome::Refused(why) => {
                assert!(
                    why.starts_with("exec.sandbox-")
                        || why.starts_with("confinement.")
                        || why.starts_with("request."),
                    "{spelling:?}: a refusal without a name: {why}"
                );
                seen.push(format!(
                    "{spelling}: {}",
                    why.split(':').next().unwrap_or_default()
                ));
            }
            Outcome::Ran(_, stdout, stderr) => {
                assert_eq!(
                    stdout, "126\n0\n",
                    "{spelling:?} was admitted and dispatched, and bin/tool ran: {stderr}"
                );
                seen.push(format!("{spelling}: denies"));
            }
        }
    }
    println!("{NAME}: executed: {seen:?}");
}

/// **Deny entries whose names look like options of the helper.** `bin/--allow`, `bin/--`,
/// `bin/--deny` and `bin/-x` are files; each is denied in turn and must not run, the listed
/// neighbour must, and the helper must not read a name as an option of its own.
#[tokio::test(flavor = "multi_thread")]
async fn adversary4_deny_entries_named_like_options_are_held() {
    const NAME: &str = "adversary4 exec deny option names";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let names = ["--allow", "--", "--deny", "-x", "--env"];
    for name in names {
        std::fs::copy(tool("true"), lane.workspace.join("bin").join(name)).expect("copy true");
    }
    for name in names {
        let entry = format!("bin/{name}");
        let profile = lists(&lane.workspace, &["bin"], &[&entry]);
        let script = format!("./{entry} 2>/dev/null; echo $?; bin/tool; echo $?");
        match shell(&lane.adapter, &profile, &script).await {
            Outcome::Refused(why) => assert!(
                why.starts_with(PATH_UNREPRESENTABLE),
                "{entry:?}: refused otherwise than by the lexical rule: {why}"
            ),
            Outcome::Ran(_, stdout, stderr) => {
                assert_eq!(stdout, "126\n0\n", "{entry:?}: {stderr}");
            }
        }
    }
    println!("{NAME}: executed");
}

/// A directory outside the workspace root, for a read-only root: `bin/real` and `bin/other`,
/// copies of the host's shell, and `bin/sh`, a symbolic link to `real`.
fn shells(host: &Path) -> PathBuf {
    let shells = host.join("shells");
    let bin = shells.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let shell = std::fs::canonicalize(tool("sh")).expect("the shell's file");
    for name in ["real", "other"] {
        std::fs::copy(&shell, bin.join(name)).expect("copy the shell");
    }
    std::os::unix::fs::symlink("real", bin.join("sh")).expect("a link");
    shells
}

fn with_root(mut profile: ConfinementProfile, host_path: &Path, mount: &str) -> ConfinementProfile {
    profile.read_only_roots.push(ReadOnlyRoot {
        host_path: text(host_path),
        mount: mount.to_owned(),
    });
    profile
}

/// **A denied program is refused before dispatch, however the deny entry reaches it**
/// (design 35 § 8.2): the file itself, a symbolic link to it as the deny entry, its directory,
/// the root that holds the directory, and a deny entry equal to the allow entry. Each start
/// must be refused `exec.sandbox-program-unlisted`, and none may run the program.
#[tokio::test(flavor = "multi_thread")]
async fn adversary4_a_denied_program_is_refused_before_dispatch_under_every_deny_entry() {
    const NAME: &str = "adversary4 exec deny program refused";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let shells = shells(lane.host.path());
    for (program, deny) in [
        ("/opt/t/bin/real", "/opt/t/bin/real"),
        ("/opt/t/bin/real", "/opt/t/bin/sh"),
        ("/opt/t/bin/sh", "/opt/t/bin/real"),
        ("/opt/t/bin/real", "/opt/t/bin"),
        ("/opt/t/bin/real", "/opt/t"),
    ] {
        let profile = with_root(
            lists(&lane.workspace, &["/opt/t/bin"], &[deny]),
            &shells,
            "/opt/t",
        );
        let outcome = start(
            &lane.adapter,
            &profile,
            &[program, "-c", ": > /workspace/ran"],
        )
        .await;
        let Outcome::Refused(why) = &outcome else {
            panic!("{program} with {deny} denied was dispatched: {outcome:?}");
        };
        assert!(
            why.starts_with(PROGRAM_UNLISTED),
            "{program} with {deny} denied: {why}"
        );
        assert!(!lane.workspace.join("ran").exists(), "the program ran");
    }
    // The control: the neighbour of the denied file is a program the start takes.
    let profile = with_root(
        lists(&lane.workspace, &["/opt/t/bin"], &["/opt/t/bin/real"]),
        &shells,
        "/opt/t",
    );
    let outcome = start(
        &lane.adapter,
        &profile,
        &[
            "/opt/t/bin/other",
            "-c",
            "/opt/t/bin/real -c : 2>/dev/null; echo $?; /opt/t/bin/sh -c : 2>/dev/null; echo $?",
        ],
    )
    .await;
    let Outcome::Ran(_, stdout, stderr) = &outcome else {
        panic!("the control was refused: {outcome:?}");
    };
    assert_eq!(stdout, "126\n126\n", "{stderr}");
    println!("{NAME}: executed");
}

/// **A deny entry equal to an allow entry**, for a directory of the workspace and for a
/// declared read-only file: the denied path wins (design 35 D10, N8), and a listed neighbour
/// still runs.
#[tokio::test(flavor = "multi_thread")]
async fn adversary4_a_deny_entry_equal_to_an_allow_entry_wins() {
    const NAME: &str = "adversary4 exec deny equals allow";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let single = lane.host.path().join("single");
    std::fs::copy(tool("true"), &single).expect("copy true");
    let mut profile = lists(
        &lane.workspace,
        &["bin", "other", "/opt/single"],
        &["bin", "/opt/single"],
    );
    profile.read_only_files.push(ReadOnlyFile {
        host_path: text(&single),
        mount: "/opt/single".to_owned(),
    });
    let outcome = shell(
        &lane.adapter,
        &profile,
        "bin/tool 2>/dev/null; echo $?; /opt/single 2>/dev/null; echo $?; other/tool; echo $?",
    )
    .await;
    let Outcome::Ran(_, stdout, stderr) = &outcome else {
        panic!("the start was refused: {outcome:?}");
    };
    assert_eq!(stdout, "126\n126\n0\n", "{stderr}");
    println!("{NAME}: executed");
}

/// **A deny entry that is a symbolic link in the workspace** names a file the run could have
/// pointed anywhere. It is refused by name before dispatch, or it denies what it points at. It
/// is never admitted and without effect.
#[tokio::test(flavor = "multi_thread")]
async fn adversary4_a_deny_entry_that_is_a_link_in_the_workspace_is_refused_or_denies() {
    const NAME: &str = "adversary4 exec deny link entry";
    let Some(lane) = lane(NAME) else {
        return;
    };
    std::os::unix::fs::symlink("bin/tool", lane.workspace.join("link")).expect("a link");
    std::os::unix::fs::symlink("tool", lane.workspace.join("bin/alias")).expect("a link");
    for entry in ["link", "bin/alias"] {
        let profile = lists(&lane.workspace, &["bin"], &[entry]);
        match shell(
            &lane.adapter,
            &profile,
            "bin/tool 2>/dev/null; echo $?; bin/other; echo $?",
        )
        .await
        {
            Outcome::Refused(why) => {
                assert!(why.starts_with(PATH_UNREPRESENTABLE), "{entry}: {why}");
                println!("{NAME}: {entry}: refused before dispatch");
            }
            Outcome::Ran(code, stdout, stderr) => {
                assert_eq!(
                    stdout, "126\n0\n",
                    "{entry}: dispatched (status {code:?}) and bin/tool ran: {stderr}"
                );
                println!("{NAME}: {entry}: denies its target");
            }
        }
    }
    println!("{NAME}: executed");
}

/// **The stop of the expansion** (design 35 § 8.2: "the helper stops at 65536 rules and exits
/// 125"). A listed directory with more entries than that holds one deny entry. Whatever the
/// start answers, the run's program does not execute: the start is refused, or the run ends
/// with the helper's status 125 and printed nothing.
#[tokio::test(flavor = "multi_thread")]
async fn adversary4_a_directory_past_the_rule_stop_runs_nothing() {
    const NAME: &str = "adversary4 exec deny rule stop";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let many = lane.workspace.join("many");
    std::fs::create_dir_all(&many).expect("many");
    for index in 0..65_540_u32 {
        std::fs::write(many.join(format!("f{index}")), b"").expect("an entry");
    }
    let profile = lists(&lane.workspace, &["bin", "many"], &["many/f0"]);
    let outcome = shell(&lane.adapter, &profile, "echo ran; bin/tool; echo $?").await;
    match &outcome {
        Outcome::Refused(why) => println!("{NAME}: executed, refused before dispatch: {why}"),
        Outcome::Ran(code, stdout, stderr) => {
            assert_eq!(
                (stdout.as_str(), *code),
                ("", Some(125)),
                "past the stop the helper executes nothing: {stderr}"
            );
            println!(
                "{NAME}: executed, dispatched and ended by the helper: {}",
                stderr.trim()
            );
        }
    }
}

/// **A deny entry beneath a host mount inside a read-only root** (design 35 § 8.5: "A deny
/// entry denies a path, and every mount of that path"). The root's host directory holds a
/// mount point, `deep`, with another host directory bound on it; bubblewrap binds a root with
/// its mounts. The profile lists the root and denies `/opt/t/deep/prog`, the one path the
/// program has in the view.
///
/// The case needs to make a mount, so it runs only where this process may: started inside
/// `unshare --user --map-root-user --mount`. Elsewhere it is absent, never passed. The helper's
/// own case of the same is `crates/substrate-exec-gate/tests/adversary4_namespace_gate.rs`.
#[tokio::test(flavor = "multi_thread")]
async fn adversary4_a_deny_entry_beneath_a_host_mount_in_a_listed_root_is_enforced() {
    const NAME: &str = "adversary4 exec deny nested host mount";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let tools = lane.host.path().join("tools");
    let extra = lane.host.path().join("extra");
    for directory in [tools.join("bin"), tools.join("deep"), extra.clone()] {
        std::fs::create_dir_all(directory).expect("a directory");
    }
    let shell_file = std::fs::canonicalize(tool("sh")).expect("the shell's file");
    std::fs::copy(&shell_file, tools.join("bin/real")).expect("copy the shell");
    for name in ["prog", "other"] {
        std::fs::copy(tool("true"), extra.join(name)).expect("copy true");
    }
    let Some(mount) = find("mount") else {
        println!("{NAME}: ABSENT (no mount on this host)");
        return;
    };
    let bound = std::process::Command::new(mount)
        .arg("--bind")
        .arg(&extra)
        .arg(tools.join("deep"))
        .output()
        .expect("mount starts");
    if !bound.status.success() {
        println!(
            "{NAME}: ABSENT (this process may not mount: {})",
            String::from_utf8_lossy(&bound.stderr).trim()
        );
        return;
    }
    let profile = with_root(
        lists(&lane.workspace, &["/opt/t"], &["/opt/t/deep/prog"]),
        &tools,
        "/opt/t",
    );
    let outcome = start(
        &lane.adapter,
        &profile,
        &[
            "/opt/t/bin/real",
            "-c",
            "/opt/t/deep/prog 2>/dev/null; echo $?; /opt/t/deep/other; echo $?",
        ],
    )
    .await;
    match &outcome {
        Outcome::Refused(why) => {
            assert!(
                why.starts_with("exec.sandbox-"),
                "a refusal without a name: {why}"
            );
            println!("{NAME}: executed, refused before dispatch: {why}");
        }
        Outcome::Ran(code, stdout, stderr) => {
            assert_eq!(
                stdout, "126\n0\n",
                "the deny entry's own path executed (status {code:?}): {stderr}"
            );
            println!("{NAME}: executed");
        }
    }
}
