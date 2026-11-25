//! Security review of `story:linux-executable-allow-list` (design 35): conformance checks of the
//! `linux` adapter with `substrate-exec-gate`, through the public confinement API and the shipped
//! helper, against what the design and `AGENTS.md` say.
//!
//! - **The floor is unchanged** (`AGENTS.md` § *Safety envelope*; design 35 D5: "A start whose
//!   profile has an empty `exec_subpaths` gets no helper and no bind"): a listed start renders the
//!   argv of an unlisted one plus the helper's bind and the helper, and a listed run has the
//!   posture, the mounts and the descriptors of an unlisted one.
//! - **The run cannot remove the ruleset** (D4), also not through the environment its command
//!   names.
//! - **A kernel without Landlock proves no fact** (D8, D19), and a listed profile is then refused.
//! - **The driver publishes the two list facts it proved and nothing else of the helper**
//!   (design 37 § 5, which ends design 35 D16).
//!
//! The cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this
//! process is inside, bubblewrap, and `SUBSTRATE_EXEC_GATE` naming the helper. Without the first
//! two each prints `<name>: ABSENT (<reason>)` and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    RunState, linux::LinuxRequest,
};
use substrate_host::{Driver as _, HostConfig, HostDriver};

/// Where the driver binds the helper's bytes in a run's view.
const GATE: &str = "/.substrate-exec-gate";

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

/// The helper the lane built. A lane with the delegated root and no helper fails here.
fn helper() -> PathBuf {
    PathBuf::from(
        std::env::var_os("SUBSTRATE_EXEC_GATE")
            .expect("SUBSTRATE_EXEC_GATE is unset: the lane names the helper it built"),
    )
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

/// The directories that hold the tools the cases run, as list entries.
fn base_directories() -> Vec<String> {
    let mut directories: Vec<String> = ["sh", "true", "env", "sort", "ls", "readlink", "chmod"]
        .iter()
        .map(|name| text(tool(name).parent().expect("a directory")))
        .collect();
    directories.sort();
    directories.dedup();
    directories
}

fn config(host: &Path, cgroup_root: &Path, gate: Option<PathBuf>) -> HostConfig {
    let mut config = HostConfig::minimum(host.join("workspaces"));
    config.cgroup_root = Some(cgroup_root.to_path_buf());
    config.exec_gate = gate;
    config
}

fn open_with(
    host: &Path,
    cgroup_root: &Path,
    gate: Option<PathBuf>,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    let mut config = HostConfig::minimum(host.join("workspaces"));
    config.exec_gate = gate;
    confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root.to_path_buf())),
    )))
}

struct Lane {
    _host: tempfile::TempDir,
    adapter: Arc<dyn ConfinementAdapter>,
    workspace: PathBuf,
}

/// The adapter opened with the lane's helper, having proved `exec_subpaths`, and a workspace
/// with `bin/tool` and `other/tool`, both copies of the host's `true`.
fn lane(name: &str) -> Option<Lane> {
    let cgroup_root = delegated(name)?;
    let host = tempfile::tempdir().expect("host root");
    let adapter = open_with(host.path(), &cgroup_root, Some(helper())).expect("linux opens");
    assert!(
        adapter.facts().proves(&Fact::ExecSubpaths),
        "{name}: linux opened with the helper did not prove exec_subpaths: {:?}",
        adapter.facts()
    );
    let workspace = host.path().join("workspaces").join("ws_security");
    for directory in ["bin", "other"] {
        std::fs::create_dir_all(workspace.join(directory)).expect(directory);
        std::fs::copy(tool("true"), workspace.join(directory).join("tool")).expect("copy true");
    }
    Some(Lane {
        _host: host,
        adapter,
        workspace,
    })
}

fn profile(workspace: &Path, entries: &[String]) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        exec_subpaths: entries.to_vec(),
        ..ConfinementProfile::default()
    }
}

fn shell(script: &str, extra: &[(&str, &str)]) -> Command {
    let mut env = BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
    for (name, value) in extra {
        env.insert((*name).to_owned(), (*value).to_owned());
    }
    Command {
        argv: vec![text(&tool("sh")), "-c".into(), script.to_owned()],
        env,
    }
}

/// Admits `profile`, runs `command` and returns its exit code, its stdout and its stderr.
async fn run(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    command: &Command,
) -> (Option<i32>, String, String) {
    let admitted = adapter.admit(profile).expect("admitted");
    let run = adapter.start(&admitted, command).await.expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    (
        ended.exit.as_ref().and_then(|exit| exit.code),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// `args` without the tokens that are descriptor numbers, which differ between two renders.
fn unnumbered(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|part| part.parse::<u32>().is_err())
        .cloned()
        .collect()
}

/// The `--env <name> <value>` operands among a helper's `options`, by name.
fn env_operands(options: &[String]) -> BTreeMap<String, String> {
    let mut operands = BTreeMap::new();
    let mut options = options.iter();
    while let Some(option) = options.next() {
        if option == "--env" {
            let name = options.next().expect("a name").clone();
            let value = options.next().expect("a value").clone();
            assert!(
                operands.insert(name.clone(), value).is_none(),
                "the helper is given {name} twice"
            );
        }
    }
    operands
}

/// An unlisted argv without what D28 moves on a listed start: the argv less its `--setenv`
/// triples and its one `/usr/bin/env -u PWD --` stage, and the variables those triples leave,
/// a later one of a name replacing an earlier one as bubblewrap does.
fn without_the_variables(unlisted: &[String]) -> (Vec<String>, BTreeMap<String, String>) {
    let mut variables = BTreeMap::new();
    let mut floor: Vec<String> = Vec::new();
    let mut parts = unlisted.iter().peekable();
    let mut stages = 0;
    while let Some(part) = parts.next() {
        if part == "--setenv" {
            let name = parts.next().expect("a name").clone();
            let value = parts.next().expect("a value").clone();
            variables.insert(name, value);
        } else if part == "--"
            && floor.iter().all(|kept| kept != "--")
            && parts.peek().is_some_and(|next| *next == "/usr/bin/env")
        {
            // Bubblewrap's own `--`, then the stage.
            floor.push(part.clone());
            let stage: Vec<&String> = parts.by_ref().take(4).collect();
            assert_eq!(stage, ["/usr/bin/env", "-u", "PWD", "--"], "{unlisted:?}");
            stages += 1;
        } else {
            floor.push(part.clone());
        }
    }
    assert_eq!(
        stages, 1,
        "the unlisted start has one env stage: {unlisted:?}"
    );
    (floor, variables)
}

/// **A listed start renders the argv of an unlisted start, plus the helper's bind and the
/// helper, and drops nothing of the floor** (`AGENTS.md` § *Safety envelope*: "The enforced
/// isolation set is a floor, not a menu"; design 35 D5). Every option of the floor is in the
/// listed argv.
///
/// **The stated difference since D28**, and nothing more: the unlisted start gives the run's
/// variables to bubblewrap as `--setenv <name> <value>` and removes bubblewrap's `PWD` with a
/// `/usr/bin/env -u PWD --` stage; the listed start has neither, and the same variables with the
/// same values are the helper's `--env <name> <value>` operands. With the bind and the helper's
/// own words taken out of the one, and the `--setenv` triples and that stage out of the other,
/// the two lists are the same.
#[tokio::test(flavor = "multi_thread")]
async fn security_a_listed_start_renders_the_floor_of_an_unlisted_start_and_adds_only_the_helper() {
    const NAME: &str = "security exec gate floor argv";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let command = shell("true", &[("LD_PRELOAD", "/workspace/pre.so")]);
    let render = |entries: &[String]| {
        let admitted = lane
            .adapter
            .admit(&profile(&lane.workspace, entries))
            .expect("admitted");
        lane.adapter
            .render(&admitted, &command)
            .expect("rendered")
            .args
    };
    let unlisted = render(&[]);
    assert!(
        !unlisted
            .iter()
            .any(|part| part == "--ro-bind-data" || part.contains("substrate-exec-gate")),
        "a start without a list names the helper: {unlisted:?}"
    );
    let mut entries = base_directories();
    entries.push("bin".to_owned());
    let listed = render(&entries);

    for option in [
        "--unshare-user",
        "--disable-userns",
        "--unshare-ipc",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-uts",
        "--new-session",
        "--die-with-parent",
        "--clearenv",
        "--seccomp",
        "--proc",
        "--dev",
        "--tmpfs",
    ] {
        assert!(
            listed.iter().any(|part| part == option),
            "the listed argv lacks {option}: {listed:?}"
        );
    }
    assert!(
        listed
            .windows(3)
            .any(|window| window == ["--ro-bind", "/usr", "/usr"]),
        "{listed:?}"
    );

    // Take out the bind and the helper with its options; what is left is the unlisted argv.
    let bind = listed
        .windows(5)
        .position(|window| {
            window[..3] == ["--perms", "0555", "--ro-bind-data"] && window[4] == GATE
        })
        .unwrap_or_else(|| panic!("no bind of the helper: {listed:?}"));
    let helper = listed
        .iter()
        .rposition(|part| part == GATE)
        .expect("the helper");
    assert!(helper > bind + 4, "{listed:?}");
    let end = helper
        + listed[helper..]
            .iter()
            .position(|part| part == "--")
            .expect("the end of the helper's options");
    let mut without: Vec<String> = listed[..bind].to_vec();
    without.extend_from_slice(&listed[bind + 5..helper]);
    without.extend_from_slice(&listed[end + 1..]);

    // D28, the listed side: bubblewrap sets no variable, and the run's are the helper's.
    assert!(
        !listed.iter().any(|part| part == "--setenv"),
        "a listed start gives bubblewrap a variable: {listed:?}"
    );
    let operands = env_operands(&listed[helper + 1..end]);
    assert_eq!(
        operands.get("LD_PRELOAD").map(String::as_str),
        Some("/workspace/pre.so"),
        "the run's variable is not an operand of the helper: {operands:?}"
    );
    // D28, the unlisted side: its `--setenv` triples and its `env -u PWD --` stage, taken out.
    let (floor, variables) = without_the_variables(&unlisted);
    assert_eq!(
        operands, variables,
        "the helper's operands are not the variables bubblewrap sets on an unlisted start"
    );
    assert_eq!(
        unnumbered(&without),
        unnumbered(&floor),
        "a listed start differs from an unlisted one by more than the helper and where the \
         run's variables are given"
    );
    // No option of the helper is a bubblewrap option: they all follow bubblewrap's own `--`.
    let separator = listed
        .iter()
        .position(|part| part == "--")
        .expect("bubblewrap's --");
    assert!(separator < helper && bind < separator, "{listed:?}");
    println!("{NAME}: executed");
}

/// What a run reports of its own confinement, with the shell's builtins and four base tools.
const POSTURE: &str = r#"
while read -r key value; do
  case "$key" in
    NoNewPrivs:|Seccomp:|CapInh:|CapPrm:|CapEff:|CapBnd:|CapAmb:|Uid:|Gid:) echo "status:$key $value" ;;
  esac
done < /proc/self/status
read -r namespaces < /proc/sys/user/max_user_namespaces
echo "user-namespaces:$namespaces"
while read -r id parent device root point options rest; do
  set -- ${rest#*- }
  echo "mount:$point $options $1"
done < /proc/self/mountinfo
for process in 1 $$; do
  for descriptor in /proc/$process/fd/*; do
    echo "descriptor:$(readlink "$descriptor" 2>/dev/null)"
  done
done
env | sort | while read -r line; do echo "env:$line"; done
ls -a / | while read -r line; do echo "root:$line"; done
"#;

/// What only a listed run is asked: what it can do with the helper's file in its view.
const HELPER_FILE: &str = r#"
[ -r /.substrate-exec-gate ] && echo "gate:readable"
( : >> /.substrate-exec-gate ) 2>/dev/null && echo "gate:WRITABLE"
chmod 0777 /.substrate-exec-gate 2>/dev/null && echo "gate:CHMOD"
( : > /.substrate-exec-gate.new && mv /.substrate-exec-gate.new /.substrate-exec-gate ) 2>/dev/null && echo "gate:REPLACED"
/.substrate-exec-gate --version >/dev/null 2>&1
echo "gate:execute $?"
"#;

fn lines<'a>(stdout: &'a str, prefix: &str) -> Vec<&'a str> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .collect()
}

/// **A listed run has the posture, the mounts, the environment and the descriptors of an
/// unlisted run; the one thing added to its view is the helper's read-only file, which it can
/// neither write, replace nor execute** (`AGENTS.md` § *Safety envelope*; design 35 D20: "The
/// run can read `<gate>` and cannot write or execute it"). The helper's sealed bytes reach no
/// process of the run as a descriptor.
#[tokio::test(flavor = "multi_thread")]
async fn security_a_listed_run_has_the_posture_and_the_view_of_an_unlisted_run() {
    const NAME: &str = "security exec gate floor run";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let (code, unlisted, stderr) = run(
        &lane.adapter,
        &profile(&lane.workspace, &[]),
        &shell(POSTURE, &[]),
    )
    .await;
    assert_eq!(code, Some(0), "the unlisted run: {stderr}");
    let script = format!("{POSTURE}{HELPER_FILE}");
    let (code, listed, stderr) = run(
        &lane.adapter,
        &profile(&lane.workspace, &base_directories()),
        &shell(&script, &[]),
    )
    .await;
    assert_eq!(code, Some(0), "the listed run: {stderr}\n{listed}");

    let status = lines(&unlisted, "status:");
    assert!(
        status.contains(&"NoNewPrivs: 1") && status.contains(&"Seccomp: 2"),
        "the floor of an unlisted run: {status:?}"
    );
    assert_eq!(lines(&listed, "status:"), status, "the process posture");
    assert_eq!(
        lines(&listed, "user-namespaces:"),
        lines(&unlisted, "user-namespaces:"),
        "nested user namespaces"
    );
    assert_eq!(
        lines(&listed, "env:"),
        lines(&unlisted, "env:"),
        "the environment"
    );

    // The mounts: the unlisted run's, in order, and one more row for the helper's file.
    let mounts = lines(&listed, "mount:");
    let (gate, others): (Vec<&str>, Vec<&str>) = mounts
        .iter()
        .copied()
        .partition(|row| row.starts_with("/.substrate-exec-gate "));
    assert_eq!(others, lines(&unlisted, "mount:"), "every other mount");
    assert_eq!(gate.len(), 1, "one mount of the helper: {mounts:?}");
    let options: Vec<&str> = gate[0]
        .split(' ')
        .nth(1)
        .expect("the options of the helper's mount")
        .split(',')
        .collect();
    assert!(
        options.contains(&"ro") && options.contains(&"nosuid") && options.contains(&"nodev"),
        "the helper's mount: {gate:?}"
    );

    // The view's root: the same names, and the helper's.
    let root: Vec<&str> = lines(&listed, "root:")
        .into_iter()
        .filter(|name| *name != ".substrate-exec-gate")
        .collect();
    assert_eq!(
        root,
        lines(&unlisted, "root:"),
        "the names of the view's root"
    );

    // No process of the run holds the sealed bytes, bubblewrap's own first process included.
    let descriptors = lines(&listed, "descriptor:");
    assert!(!descriptors.is_empty(), "{listed}");
    assert!(
        !descriptors
            .iter()
            .any(|link| link.contains("memfd") || link.contains("substrate-exec-gate")),
        "a descriptor of the helper's bytes reached the run: {descriptors:?}"
    );
    assert_eq!(
        descriptors.len(),
        lines(&unlisted, "descriptor:").len(),
        "the run holds as many descriptors as an unlisted run"
    );

    assert_eq!(
        lines(&listed, "gate:"),
        ["readable", "execute 126"],
        "what the run can do with the helper's file"
    );
    println!("{NAME}: executed");
}

/// A shared object whose constructor, in a process named `substrate-exec-gate`, executes
/// whatever follows `--` in that process's arguments: the run's program, directly.
const PRELOAD: &str = r#"
#define _GNU_SOURCE
#include <string.h>
#include <unistd.h>

static void __attribute__((constructor)) start(int argc, char **argv, char **envp) {
    if (argc < 1 || strstr(argv[0], "substrate-exec-gate") == NULL) {
        return;
    }
    for (int i = 1; i + 1 < argc; i++) {
        if (strcmp(argv[i], "--") == 0) {
            execve(argv[i + 1], argv + i + 1, envp);
            return;
        }
    }
}
"#;

/// **A variable of the run's own environment does not lift the list** (design 35 D4: "Every
/// process of the run descends from the helper and inherits the domain … The run cannot remove
/// it"; D19: "A profile that carries a list never runs without it").
///
/// The command's environment is the caller's (`Command.env`), and the driver's rule admits any
/// upper-case name that names no credential (`substrate_guarded::environment::check`). The
/// driver sets it with bubblewrap's `--setenv` before `env` and the helper start, so the helper
/// starts with it. Here it names `LD_PRELOAD=/workspace/pre.so`, an object in the workspace, as
/// an earlier run or a checkout could have left it. The profile lists `bin`; `other/tool` must
/// still end with status 126.
#[tokio::test(flavor = "multi_thread")]
async fn security_a_preload_named_by_the_commands_environment_does_not_lift_the_list() {
    const NAME: &str = "security exec gate preload";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let source = lane.workspace.join("pre.c");
    std::fs::write(&source, PRELOAD).expect("the source");
    let built = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(lane.workspace.join("pre.so"))
        .arg(&source)
        .output();
    match built {
        Ok(output) if output.status.success() => {}
        other => {
            println!("{NAME}: ABSENT (no shared object could be built: {other:?})");
            return;
        }
    }
    let listed = profile(&lane.workspace, &["bin".to_owned()]);
    let script = "bin/tool; echo listed:$?; other/tool 2>/dev/null; echo unlisted:$?";

    // The control: without the variable the list holds.
    let (_, stdout, stderr) = run(&lane.adapter, &listed, &shell(script, &[])).await;
    assert_eq!(stdout, "listed:0\nunlisted:126\n", "the control: {stderr}");

    let (_, stdout, stderr) = run(
        &lane.adapter,
        &listed,
        &shell(script, &[("LD_PRELOAD", "/workspace/pre.so")]),
    )
    .await;
    assert_eq!(
        stdout, "listed:0\nunlisted:126\n",
        "with LD_PRELOAD=/workspace/pre.so in the command's environment the run executed a \
         program outside its list: the object's constructor ran in the helper's process before \
         landlock_restrict_self and started the program without the ruleset (stderr: {stderr:?})"
    );
    println!("{NAME}: executed");
}

/// As [`PRELOAD`], for the program that runs before the helper: in a process whose arguments
/// name the helper's mount and that is not the helper itself (`/usr/bin/env`, which the driver
/// places before it), the constructor executes whatever follows the last `--`. The helper is
/// then never started.
const PRELOAD_BEFORE_THE_HELPER: &str = r#"
#define _GNU_SOURCE
#include <string.h>
#include <unistd.h>

static void __attribute__((constructor)) start(int argc, char **argv, char **envp) {
    int helper = 0, last = 0;
    if (argc < 1 || strstr(argv[0], "substrate-exec-gate") != NULL) {
        return;
    }
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "/.substrate-exec-gate") == 0) {
            helper = i;
        }
        if (helper != 0 && strcmp(argv[i], "--") == 0) {
            last = i;
        }
    }
    if (helper != 0 && last != 0 && last + 1 < argc) {
        execve(argv[last + 1], argv + last + 1, envp);
    }
}
"#;

/// **The same, for the program the driver runs before the helper** (design 35 D5: "Everything
/// bubblewrap, `setsid` and `env` execute happens before the ruleset exists"). `env` starts with
/// the run's environment too, so an object the environment names runs there, where no ruleset
/// exists yet, and can start the run's program itself. The helper's own linking does not decide
/// this case: the helper is never started. The list must hold all the same.
#[tokio::test(flavor = "multi_thread")]
async fn security_a_preload_that_acts_before_the_helper_starts_does_not_lift_the_list() {
    const NAME: &str = "security exec gate preload before the helper";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let source = lane.workspace.join("pre.c");
    std::fs::write(&source, PRELOAD_BEFORE_THE_HELPER).expect("the source");
    let built = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(lane.workspace.join("pre.so"))
        .arg(&source)
        .output();
    match built {
        Ok(output) if output.status.success() => {}
        other => {
            println!("{NAME}: ABSENT (no shared object could be built: {other:?})");
            return;
        }
    }
    let listed = profile(&lane.workspace, &["bin".to_owned()]);
    let script = "bin/tool; echo listed:$?; other/tool 2>/dev/null; echo unlisted:$?";
    let (_, stdout, stderr) = run(
        &lane.adapter,
        &listed,
        &shell(script, &[("LD_PRELOAD", "/workspace/pre.so")]),
    )
    .await;
    assert_eq!(
        stdout, "listed:0\nunlisted:126\n",
        "with LD_PRELOAD=/workspace/pre.so in the command's environment the run executed a \
         program outside its list: the object's constructor ran in /usr/bin/env, before the \
         helper, and started the program itself; the helper never ran (stderr: {stderr:?})"
    );
    println!("{NAME}: executed");
}

const INNER: &str = "SECURITY_EXEC_GATE_WITHOUT_LANDLOCK";
const WITHOUT_LANDLOCK: &str = "security exec gate without landlock";

/// **A host without Landlock proves no `exec_subpaths`, and a listed profile is refused there
/// before anything runs** (design 35 D8, D19; `AGENTS.md` invariant 3). This process starts
/// itself again under a seccomp filter that answers `ENOSYS` to `landlock_create_ruleset`, as a
/// kernel without Landlock does; every process the driver starts inherits it.
#[test]
fn security_a_host_without_landlock_proves_no_fact_and_refuses_a_listed_profile() {
    if std::env::var_os(INNER).is_some() {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(without_landlock());
        return;
    }
    if delegated(WITHOUT_LANDLOCK).is_none() {
        return;
    }
    let number = u32::try_from(libc::SYS_landlock_create_ruleset).expect("a system call number");
    let errno = u32::try_from(libc::ENOSYS).expect("an errno");
    let program = [
        // Load the system call number; fail that one call; allow every other.
        libc::sock_filter {
            code: 0x20,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 1,
            k: number,
        },
        libc::sock_filter {
            code: 0x06,
            jt: 0,
            jf: 0,
            k: 0x0005_0000 | errno,
        },
        libc::sock_filter {
            code: 0x06,
            jt: 0,
            jf: 0,
            k: 0x7fff_0000,
        },
    ];
    let mut again = std::process::Command::new(std::env::current_exe().expect("this test"));
    again
        .args([
            "--exact",
            "security_a_host_without_landlock_proves_no_fact_and_refuses_a_listed_profile",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(INNER, "1");
    // SAFETY: the closure runs after the fork and makes two `prctl` calls on memory built before
    // it; both are async-signal-safe.
    unsafe {
        again.pre_exec(move || {
            let filter = libc::sock_fprog {
                len: 4,
                filter: program.as_ptr().cast_mut(),
            };
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                || libc::prctl(
                    libc::PR_SET_SECCOMP,
                    libc::SECCOMP_MODE_FILTER,
                    &raw const filter,
                ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = again.output().expect("this test starts again");
    let stdout = String::from_utf8_lossy(&output.stdout);
    print!("{stdout}");
    assert!(
        output.status.success() && stdout.contains("security exec gate without landlock: inner"),
        "the run under the filter: {output:?}"
    );
    println!("{WITHOUT_LANDLOCK}: executed");
}

async fn without_landlock() {
    let cgroup_root = delegated(WITHOUT_LANDLOCK).expect("the outer case found the lane");
    let host = tempfile::tempdir().expect("host root");
    let adapter = open_with(host.path(), &cgroup_root, Some(helper()))
        .expect("a kernel without Landlock is not a configuration error: the adapter opens");
    let facts = adapter.facts();
    assert!(
        !facts.proves(&Fact::ExecSubpaths),
        "exec_subpaths was proved on a kernel that refuses landlock_create_ruleset: {facts:?}"
    );
    assert!(facts.withheld.is_empty(), "no place is at fault: {facts:?}");
    let workspace = host.path().join("workspaces").join("ws_security");
    std::fs::create_dir_all(workspace.join("bin")).expect("workspace");
    let marker = workspace.join("ran");

    let error = adapter
        .admit(&profile(&workspace, &["bin".to_owned()]))
        .expect_err("a listed profile on a host that did not prove the list");
    assert!(
        matches!(
            &error,
            ConfinementError::FactUnserved { fact, withheld: None, .. }
                if *fact == Fact::ExecSubpaths
        ),
        "{error:?}"
    );
    assert_eq!(error.code(), "confinement.fact-unserved");
    assert_eq!(error.status(), 501);
    assert!(!marker.exists());

    // A profile without a list runs as it always did.
    let (code, _, stderr) = run(
        &adapter,
        &profile(&workspace, &[]),
        &shell(": > /workspace/ran", &[]),
    )
    .await;
    assert_eq!(code, Some(0), "{stderr}");
    assert!(marker.exists(), "the unlisted run ran");
    println!("{WITHOUT_LANDLOCK}: inner executed");
}

/// **A helper inside a workspace is refused however the workspace root is spelled** (design 35
/// D21: "A helper a run could have written is refused"; D25: "Whether one path lies in another
/// is decided by their places, not by their text").
///
/// The driver places `workspace_root` before it creates it. Here the configured root is
/// `<ws>/new/..`: `<ws>` exists and holds a workspace with the helper's file in it, and `new`
/// does not exist yet, so the kernel does not resolve the configured text. The driver would
/// create `<ws>/new` directly after the check, and from then on its workspace root would be
/// `<ws>`, with the helper in a workspace of it. So the open must not accept it.
///
/// **The assertion on the code was changed by the implementor**, on the coordinator's word and
/// by the design's sixth revision (D25, § 6 *A root spelled through a name that does not exist
/// yet is not compared; the open is refused*). The review wrote `config.exec-gate-invalid`, the
/// answer for the root spelled `<ws>`. The design answers `config.exec-gate-unserved` (501)
/// with the cause `path-unopenable`: the driver cannot say that the helper lies in a location
/// it could not place, and it does not create a directory beneath an unchecked path in order to
/// place it. The property the review asked for is kept: the open is refused, and nothing was
/// created. Portable: the check precedes every probe.
#[test]
fn security_a_helper_in_a_workspace_is_refused_however_the_workspace_root_is_spelled() {
    const NAME: &str = "security exec gate root spelling";
    let host = tempfile::tempdir().expect("host root");
    let host_path = host.path().canonicalize().expect("canonical host root");
    let root = host_path.join("workspaces");
    let workspace = root.join("ws_earlier");
    std::fs::create_dir_all(&workspace).expect("an earlier run's workspace");
    let gate = workspace.join("substrate-exec-gate");
    std::fs::write(&gate, b"what an earlier run left").expect("the helper's file");
    let open = |workspace_root: PathBuf| {
        let mut config = HostConfig::minimum(workspace_root);
        config.exec_gate = Some(gate.clone());
        confinement::open(AdapterRequest::Linux(Box::new(LinuxRequest::new(config))))
    };

    // The control: the root spelled as it is.
    match open(root.clone()) {
        Err(error) => assert_eq!(error.code(), "config.exec-gate-invalid", "{error}"),
        Ok(adapter) if !adapter.facts().withheld.is_empty() => {
            println!(
                "{NAME}: ABSENT (this host withholds the lists: {:?})",
                adapter.facts().withheld
            );
            return;
        }
        Ok(adapter) => panic!(
            "a helper in a workspace of the plainly spelled root was accepted: {:?}",
            adapter.facts()
        ),
    }

    let spelled = root.join("new").join("..");
    assert!(!root.join("new").exists());
    let opened = open(spelled.clone());
    assert!(
        root.join("new").exists() || opened.is_err(),
        "the driver did not create the configured root"
    );
    match opened {
        Err(error) => {
            assert_eq!(
                (error.code(), error.status()),
                ("config.exec-gate-unserved", 501),
                "{error}"
            );
            let message = error.to_string();
            assert!(
                message.contains("path-unopenable") && message.contains("new/.."),
                "the refusal does not name the cause and the path: {message}"
            );
            assert!(
                !root.join("new").exists(),
                "a refused open created a directory beneath the unchecked root"
            );
        }
        Ok(_) => panic!(
            "the open accepted the helper {} with workspace_root {}: the helper lies in a \
             workspace of that root, and its place was compared with the text of a root that \
             did not exist yet",
            gate.display(),
            spelled.display()
        ),
    }
    println!("{NAME}: executed");
}

/// **A driver publishes the two list facts it proved and nothing else of the helper** (design
/// 37 § 5, which ends design 35 D16; `AGENTS.md` invariant 4). With a helper the published
/// facts hold `confinement.exec-subpaths`, and `confinement.exec-denied-paths` where the
/// deny-list was proved; without one they hold neither. Apart from those two members the
/// published facts of the two drivers are equal: no path of the helper and no other word of
/// its configuration reaches a client.
#[tokio::test(flavor = "multi_thread")]
async fn security_a_driver_publishes_the_list_facts_it_proved_and_nothing_else_of_the_helper() {
    const NAME: &str = "security exec gate published facts";
    const PUBLISHED: [&str; 2] = ["confinement.exec-subpaths", "confinement.exec-denied-paths"];
    let Some(cgroup_root) = delegated(NAME) else {
        return;
    };
    // What the driver proved is read where the confinement API states it, in the adapter's
    // facts: the driver's own accessor of the proof is private to the crate, and the adapter's
    // open is the same open with the same configuration.
    let published = |gate: Option<PathBuf>| {
        let host = tempfile::tempdir().expect("host root");
        let adapter = open_with(host.path(), &cgroup_root, gate.clone()).expect("linux opens");
        let facts = adapter.facts();
        let proved = (
            facts.proves(&Fact::ExecSubpaths),
            facts.proves(&Fact::ExecDeniedPaths),
            facts.withheld.clone(),
        );
        drop(adapter);
        let host = tempfile::tempdir().expect("host root");
        let driver = HostDriver::open(config(host.path(), &cgroup_root, gate)).expect("opens");
        let machine = driver.machine();
        drop(driver);
        (
            proved,
            (
                machine.facts.confinement_exec_subpaths,
                machine.facts.confinement_exec_denied_paths,
            ),
            serde_json::to_value(&machine.facts).expect("facts serialize"),
        )
    };
    let ((subpaths, denied, withheld), properties, mut with_helper) = published(Some(helper()));
    assert!(
        subpaths && withheld.is_empty(),
        "the driver proved the helper: {withheld:?}"
    );
    assert_eq!(
        properties,
        (Some(true), denied.then_some(true)),
        "a driver publishes each list fact it proved, and no other"
    );
    let ((subpaths, denied, withheld), properties, without_helper) = published(None);
    assert!(!subpaths && !denied && withheld.is_empty(), "{withheld:?}");
    assert_eq!(properties, (None, None));
    for name in PUBLISHED {
        assert!(
            !without_helper.to_string().contains(name),
            "a driver without a helper names {name}: {without_helper}"
        );
        with_helper
            .as_object_mut()
            .expect("the facts are an object")
            .remove(name);
    }
    assert_eq!(
        with_helper, without_helper,
        "the published facts differ in more than the two list facts with a helper configured"
    );
    println!("{NAME}: executed");
}
