//! Security review, second look: design 35 D28 (`docs/design/35-executable-lists.md`, § 6 *The
//! run's variables*) as an invariant of the `linux` adapter, through the public confinement API
//! and the shipped helper.
//!
//! > **D28. On a start with a list, no variable of the run reaches a process that runs before
//! > `landlock_restrict_self` returned.**
//!
//! What the committed cases already hold is not repeated here: the helper's own refusal and its
//! operands (`crates/substrate-exec-gate/tests/gate.rs`), the program's environment read from
//! `/proc/<shell>/environ` and an object that acts in the first process that loads it
//! (`tests/design35/exec_lists.rs`, `tests/adversary_exec_lists.rs`), and the same through
//! `systemd` (`tests/ess_confinement_systemd.rs`). These cases ask what those do not:
//!
//! - **the routes D28 names as unchecked**: a declared read-only file at the path a loader reads
//!   beside its variables, and a declared file at the helper's own mount;
//! - **where an object the run names is loaded**, as a list of processes and not only by what
//!   one object does;
//! - **where a value of the run can be read** in the view, against an unlisted start;
//! - **rule 7**, the argv of an unlisted start as it was before the lists, `PWD` stage included;
//! - **the other argv of a listed start**: a run with piped input and one with live output.
//!
//! The cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this
//! process is inside, bubblewrap, and `SUBSTRATE_EXEC_GATE` naming the helper. Without the first
//! two each prints `<name>: ABSENT (<reason>)` and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    OutputDelivery, ReadOnlyFile, RunState, StdinMode, linux::LinuxRequest,
};

/// Where the driver binds the helper's bytes in a run's view.
const GATE: &str = "/.substrate-exec-gate";

/// The list every run of these cases is held to, and what it prints when the list holds.
const SCRIPT: &str = "bin/tool; echo listed:$?; other/tool 2>/dev/null; echo unlisted:$?";
const HELD: &str = "listed:0\nunlisted:126\n";

/// The delegated cgroup v2 subtree this process is inside, or `None` after printing why not.
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

/// The host's copy of a base tool, where this distribution keeps it.
fn tool(name: &str) -> PathBuf {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {name} in /usr/bin or /bin"))
}

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8").to_owned()
}

struct Lane {
    host: tempfile::TempDir,
    adapter: Arc<dyn ConfinementAdapter>,
    workspace: PathBuf,
}

/// The adapter opened with the lane's helper, having proved `exec_subpaths`, and a workspace
/// with `bin/tool` and `other/tool`, both copies of the host's `true`.
fn lane(name: &str) -> Option<Lane> {
    let cgroup_root = delegated(name)?;
    let host = tempfile::tempdir().expect("host root");
    let mut config = HostConfig::minimum(host.path().join("workspaces"));
    config.exec_gate = Some(PathBuf::from(
        std::env::var_os("SUBSTRATE_EXEC_GATE")
            .expect("SUBSTRATE_EXEC_GATE is unset: the lane names the helper it built"),
    ));
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .expect("linux opens");
    assert!(
        adapter.facts().proves(&Fact::ExecSubpaths),
        "{name}: linux opened with the helper did not prove exec_subpaths: {:?}",
        adapter.facts()
    );
    let workspace = host.path().join("workspaces").join("ws_security2");
    for directory in ["bin", "other"] {
        std::fs::create_dir_all(workspace.join(directory)).expect(directory);
        std::fs::copy(tool("true"), workspace.join(directory).join("tool")).expect("copy true");
    }
    Some(Lane {
        host,
        adapter,
        workspace,
    })
}

fn profile(workspace: &Path, entries: &[&str]) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        exec_subpaths: entries.iter().map(|entry| (*entry).to_owned()).collect(),
        ..ConfinementProfile::default()
    }
}

fn shell(script: &str, variables: &[(&str, &str)]) -> Command {
    Command {
        argv: vec![text(&tool("sh")), "-c".into(), script.to_owned()],
        env: variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

/// Admits `profile` and runs `command`: the exit code, the stdout and the stderr, or the
/// refusal that answered before anything was dispatched.
async fn run(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    command: &Command,
) -> Result<(Option<i32>, String, String), ConfinementError> {
    let admitted = adapter.admit(profile)?;
    let run = adapter.start(&admitted, command).await?;
    if matches!(profile.stdin, StdinMode::Piped { .. }) {
        adapter.close_stdin(&run).await.expect("the input closes");
    }
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    Ok((
        ended.exit.as_ref().and_then(|exit| exit.code),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// A shared object that does two things in every process that loads it. It appends that
/// process's `argv[0]` to `/workspace/loaded.log`. And in a process that is the helper, or that
/// was given the helper's mount as an argument, it executes whatever follows the last `--`: the
/// run's program, directly, where no ruleset has been applied yet.
const OBJECT: &str = r#"
#define _GNU_SOURCE
#include <fcntl.h>
#include <string.h>
#include <unistd.h>

static void __attribute__((constructor)) start(int argc, char **argv, char **envp) {
    int helper = -1, last = 0;
    int log = open("/workspace/loaded.log", O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
    if (log >= 0 && argc > 0) {
        if (write(log, argv[0], strlen(argv[0])) < 0 || write(log, "\n", 1) < 0) {
            /* The log is evidence only. */
        }
        close(log);
    }
    for (int i = 0; i < argc; i++) {
        if (helper < 0 && strstr(argv[i], "substrate-exec-gate") != NULL) {
            helper = i;
        }
        if (helper >= 0 && strcmp(argv[i], "--") == 0) {
            last = i;
        }
    }
    if (helper >= 0 && last != 0 && last + 1 < argc) {
        execve(argv[last + 1], argv + last + 1, envp);
    }
}
"#;

/// Builds [`OBJECT`] as `pre.so` in `workspace`; `false` after printing why it could not.
fn object(name: &str, workspace: &Path) -> bool {
    let source = workspace.join("pre.c");
    std::fs::write(&source, OBJECT).expect("the source");
    let built = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(workspace.join("pre.so"))
        .arg(&source)
        .output();
    match built {
        Ok(output) if output.status.success() => true,
        other => {
            println!("{name}: ABSENT (no shared object could be built: {other:?})");
            false
        }
    }
}

/// The processes the object was loaded in during the last run, by their `argv[0]`, in order.
fn loaded(workspace: &Path) -> Vec<String> {
    let log = workspace.join("loaded.log");
    let lines = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let _ = std::fs::remove_file(log);
    lines
}

/// **Where an object the run names is loaded** (D28): in the run's program and in what the
/// program starts, and in no process before it. The object acts where it finds the helper or a
/// stage that was given the helper, so the list holding is the second observation.
#[tokio::test(flavor = "multi_thread")]
async fn security2_an_object_the_run_names_is_loaded_in_no_process_before_the_program() {
    const NAME: &str = "security2 exec gate loaded where";
    let Some(lane) = lane(NAME) else {
        return;
    };
    if !object(NAME, &lane.workspace) {
        return;
    }
    let (_, stdout, stderr) = run(
        &lane.adapter,
        &profile(&lane.workspace, &["bin"]),
        &shell(SCRIPT, &[("LD_PRELOAD", "/workspace/pre.so")]),
    )
    .await
    .expect("started");
    let loaded = loaded(&lane.workspace);
    assert_eq!(stdout, HELD, "the list: {stderr} (loaded in {loaded:?})");
    assert_eq!(
        loaded,
        [text(&tool("sh")), "bin/tool".to_owned()],
        "the object was loaded in the program and in the listed tool it started, and nowhere \
         else"
    );
    println!("{NAME}: executed");
}

/// **A loader's other input: `/etc/ld.so.preload`** (design 35 § 6, *What D28 does not cover*:
/// "Whether a declared read-only file may be mounted at that path was not checked").
///
/// The profile declares a read-only file at that path, as it may declare one anywhere outside
/// the mounts the driver owns. The file names `/workspace/pre.so`. No variable is involved, so
/// neither the driver's rule nor the helper's refusal sees it; a dynamically linked helper's
/// loader reads the file before the helper's first instruction, as it would a variable. A
/// profile with a list never runs without it (D19), so the list holds.
///
/// **One assertion was tightened by the implementor**, on the coordinator's word and by the
/// design's seventh revision (D29, rule 2: the driver "does not refuse a declared mount at
/// `/etc/ld.so.preload`"). The review took "refused by name before dispatch" as a pass. The
/// defence is a helper with no loader, so the start must be admitted and dispatched, and a
/// refusal for the path is now a failure.
#[tokio::test(flavor = "multi_thread")]
async fn security2_a_declared_file_at_the_loaders_preload_path_does_not_lift_the_list() {
    const NAME: &str = "security2 exec gate ld.so.preload";
    let Some(lane) = lane(NAME) else {
        return;
    };
    if !object(NAME, &lane.workspace) {
        return;
    }
    let list = lane.host.path().join("preload.list");
    std::fs::write(&list, "/workspace/pre.so\n").expect("the list of objects");
    let declared = ConfinementProfile {
        read_only_files: vec![ReadOnlyFile {
            host_path: text(&list.canonicalize().expect("canonical")),
            mount: "/etc/ld.so.preload".to_owned(),
        }],
        ..profile(&lane.workspace, &["bin"])
    };
    match run(&lane.adapter, &declared, &shell(SCRIPT, &[])).await {
        Err(refusal) => {
            panic!("D29 refuses no declared mount for its path, and this start was: {refusal}")
        }
        Ok((_, stdout, stderr)) => {
            let loaded = loaded(&lane.workspace);
            assert_eq!(
                stdout, HELD,
                "a read-only file declared at /etc/ld.so.preload names an object of the \
                 workspace, and the run executed a program outside its list: the object was \
                 loaded in {loaded:?}, the helper's own process among them, before \
                 landlock_restrict_self (stderr: {stderr:?})"
            );
            assert!(
                !loaded.iter().any(|process| process.contains("exec-gate")),
                "the object was loaded in the helper: {loaded:?}"
            );
            println!("{NAME}: executed, the list held; loaded in {loaded:?}");
        }
    }
}

/// **A declared file at the helper's own mount does not stand in for the helper** (design 35
/// D20: the helper's bytes are bound "after every other mount, so no declared mount can stand
/// where the helper is started from"). The declared file is a program that would run nothing.
/// Either the start is refused by name, or the run is the real helper's: the list holds.
#[tokio::test(flavor = "multi_thread")]
async fn security2_a_declared_file_at_the_helpers_mount_does_not_stand_in_for_the_helper() {
    const NAME: &str = "security2 exec gate helper mount";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let stand_in = lane.host.path().join("stand-in");
    std::fs::copy(tool("true"), &stand_in).expect("a program that runs nothing");
    let declared = ConfinementProfile {
        read_only_files: vec![ReadOnlyFile {
            host_path: text(&stand_in.canonicalize().expect("canonical")),
            mount: GATE.to_owned(),
        }],
        ..profile(&lane.workspace, &["bin"])
    };
    match run(&lane.adapter, &declared, &shell(SCRIPT, &[])).await {
        Err(refusal) => println!("{NAME}: executed, refused before dispatch: {refusal}"),
        Ok((code, stdout, stderr)) => {
            assert_eq!(
                stdout, HELD,
                "the run was not the helper's (exit {code:?}, stderr {stderr:?})"
            );
            println!("{NAME}: executed, the helper's bytes stood over the declared file");
        }
    }
}

/// Where a run can read the value `s2-marker-7f3a` in its view: each process whose arguments
/// or whose environment hold it. The value is written in three pieces, so the shell's own
/// arguments do not hold it.
const WHERE_READABLE: &str = r#"
value="s2-""marker-""7f3a"
for process in /proc/[0-9]*; do
  read -r name < "$process/comm" 2>/dev/null || continue
  grep -q -a "$value" "$process/cmdline" 2>/dev/null && echo "seen:arguments of $name"
  grep -q -a "$value" "$process/environ" 2>/dev/null && echo "seen:environment of $name"
done
echo scanned
"#;

/// **A value of a listed run can be read nowhere an unlisted run does not show it** (design 35
/// § 6: "A value was in bubblewrap's arguments before and is there now"). On a listed start the
/// values are operands of the helper, where they were `--setenv` operands of bubblewrap. In
/// the view that is the arguments of bubblewrap's own first process in both, and the
/// environment of the run's processes in both. The helper's own arguments are gone with the
/// helper: the program took its place.
#[tokio::test(flavor = "multi_thread")]
async fn security2_a_value_of_a_listed_run_is_readable_where_an_unlisted_run_shows_it() {
    const NAME: &str = "security2 exec gate value visibility";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let variables = [("PATH", "/usr/bin:/bin"), ("MARKER", "s2-marker-7f3a")];
    let seen = |stdout: &str| -> BTreeSet<String> {
        assert!(stdout.ends_with("scanned\n"), "the scan ended: {stdout}");
        stdout
            .lines()
            .filter_map(|line| line.strip_prefix("seen:"))
            // The scan's own `grep` is given the value as its pattern.
            .filter(|place| !place.ends_with(" of grep"))
            .map(str::to_owned)
            .collect()
    };
    let grep = text(tool("grep").parent().expect("a directory"));
    let (_, unlisted, stderr) = run(
        &lane.adapter,
        &profile(&lane.workspace, &[]),
        &shell(WHERE_READABLE, &variables),
    )
    .await
    .expect("the unlisted run");
    let unlisted = seen(&unlisted);
    assert!(
        unlisted.contains("environment of sh"),
        "the control: the program has its variable: {unlisted:?} {stderr}"
    );
    let (_, listed, stderr) = run(
        &lane.adapter,
        &profile(&lane.workspace, &[grep.as_str()]),
        &shell(WHERE_READABLE, &variables),
    )
    .await
    .expect("the listed run");
    let listed = seen(&listed);
    assert!(
        listed.contains("environment of sh"),
        "the program has its variable: {listed:?} {stderr}"
    );
    let new: Vec<&String> = listed.difference(&unlisted).collect();
    assert!(
        new.is_empty(),
        "a listed run shows the value where an unlisted run does not: {new:?} (listed \
         {listed:?}, unlisted {unlisted:?})"
    );
    println!("{NAME}: executed, listed {listed:?}, unlisted {unlisted:?}");
}

/// **Rule 7: a start without a list keeps its argv**, and a start with one has neither a
/// `--setenv` nor an `env` stage, with `PWD` among the run's variables too. The unlisted tail
/// is written out here as `process.rs` built it at the base commit `3d603e63f`: one `--setenv`
/// per variable in name order, bubblewrap's `--`, `env -u PWD --`, the second stage that puts
/// the run's own `PWD` back, and the program.
// One case on purpose: the unlisted golden tail and the three listed profiles share one lane.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn security2_an_unlisted_start_keeps_its_argv_and_a_listed_one_has_no_stage_before_the_helper()
 {
    const NAME: &str = "security2 exec gate rule 7";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let sh = text(&tool("sh"));
    let command = shell(
        "true",
        &[
            ("PATH", "/usr/bin:/bin"),
            ("LD_PRELOAD", "/workspace/pre.so"),
            ("PWD", "/workspace/bin"),
        ],
    );
    let render = |profile: &ConfinementProfile| {
        let admitted = lane.adapter.admit(profile).expect("admitted");
        lane.adapter
            .render(&admitted, &command)
            .expect("rendered")
            .args
    };

    let unlisted = render(&profile(&lane.workspace, &[]));
    let first = unlisted
        .iter()
        .position(|part| part == "--setenv")
        .unwrap_or_else(|| panic!("an unlisted start sets no variable: {unlisted:?}"));
    assert_eq!(unlisted[first - 2], "--block-fd", "{unlisted:?}");
    assert_eq!(
        unlisted[first..],
        [
            "--setenv",
            "LD_PRELOAD",
            "/workspace/pre.so",
            "--setenv",
            "PATH",
            "/usr/bin:/bin",
            "--setenv",
            "PWD",
            "/workspace/bin",
            "--",
            "/usr/bin/env",
            "-u",
            "PWD",
            "--",
            "/usr/bin/env",
            "--",
            "PWD=/workspace/bin",
            sh.as_str(),
            "-c",
            "true",
        ],
        "the argv of an unlisted start changed"
    );
    assert!(
        !unlisted.iter().any(|part| part.contains("exec-gate")),
        "{unlisted:?}"
    );

    // Every other argv a listed start has: plain, with piped input, with live output.
    for (what, variant) in [
        ("plain", profile(&lane.workspace, &["bin"])),
        (
            "piped input",
            ConfinementProfile {
                stdin: StdinMode::Piped { limit_bytes: 1_024 },
                ..profile(&lane.workspace, &["bin"])
            },
        ),
        (
            "live output",
            ConfinementProfile {
                output_delivery: OutputDelivery::Live { queued_chunks: 4 },
                ..profile(&lane.workspace, &["bin"])
            },
        ),
    ] {
        let listed = render(&variant);
        assert!(
            !listed.iter().any(|part| part == "--setenv"),
            "{what}: a listed start gives bubblewrap a variable: {listed:?}"
        );
        let separator = listed
            .iter()
            .position(|part| part == "--")
            .expect("bubblewrap's --");
        assert_eq!(
            listed[separator + 1],
            GATE,
            "{what}: the helper is not the first program bubblewrap starts: {listed:?}"
        );
        assert!(
            !listed[..=separator]
                .iter()
                .any(|part| part.contains("pre.so") || part == "/usr/bin:/bin"),
            "{what}: a value of the run is an argument of bubblewrap's own: {listed:?}"
        );
        let end = separator
            + 1
            + listed[separator + 1..]
                .iter()
                .position(|part| part == "--")
                .expect("the end of the helper's options");
        let mut operands = BTreeMap::new();
        let mut options = listed[separator + 2..end].iter();
        while let Some(option) = options.next() {
            if option == "--env" {
                let name = options.next().expect("a name").clone();
                let value = options.next().expect("a value").clone();
                assert!(operands.insert(name, value).is_none(), "{what}: {listed:?}");
            }
        }
        assert_eq!(
            operands, command.env,
            "{what}: the helper's operands are the command's variables, PWD among them"
        );
        assert_eq!(
            listed[end + 1..],
            [sh.as_str(), "-c", "true"],
            "{what}: the program follows the helper directly: {listed:?}"
        );
    }
    println!("{NAME}: executed");
}

/// **The same for a start with piped input, run once** (D28): the start that carries a pipe is
/// built by the same argv, so an object the run names acts in no process before the program.
#[tokio::test(flavor = "multi_thread")]
async fn security2_a_listed_run_with_piped_input_is_held_with_a_preload_in_its_environment() {
    const NAME: &str = "security2 exec gate piped";
    let Some(lane) = lane(NAME) else {
        return;
    };
    if !object(NAME, &lane.workspace) {
        return;
    }
    let piped = ConfinementProfile {
        stdin: StdinMode::Piped { limit_bytes: 1_024 },
        ..profile(&lane.workspace, &["bin"])
    };
    let (_, stdout, stderr) = run(
        &lane.adapter,
        &piped,
        &shell(SCRIPT, &[("LD_PRELOAD", "/workspace/pre.so")]),
    )
    .await
    .expect("started");
    let loaded = loaded(&lane.workspace);
    assert_eq!(stdout, HELD, "the list: {stderr} (loaded in {loaded:?})");
    assert_eq!(loaded, [text(&tool("sh")), "bin/tool".to_owned()]);
    println!("{NAME}: executed");
}
