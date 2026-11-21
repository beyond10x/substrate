//! Design 35 (`docs/design/35-executable-lists.md`), `story:linux-executable-allow-list`: runs on
//! the `linux` adapter held to an executable allow-list.
//!
//! Each case opens the real adapter with `substrate-exec-gate`, starts one run and reads what the
//! run itself observed. A case named `limit_…` asserts a limit the design states as it is, so a
//! kernel or tool change that moves it is seen.
//!
//! The cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this
//! process is inside, and bubblewrap. Without them each prints `<name>: ABSENT (<reason>)` and is
//! **absent, never reported as passed** (invariant 3). With them and without `SUBSTRATE_EXEC_GATE`
//! naming the helper they fail: `bash scripts/delegated-lane.sh` builds the helper and names it.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_exec_gate::{Class, Lists, Located};
use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementAdapter, ConfinementError,
    ConfinementProfile, Fact, FactWithheld, ReadOnlyRoot, RunState, linux::LinuxRequest,
};
use substrate_host::seatbelt::{EXEC_SUBPATH_UNDECLARED, PATH_UNREPRESENTABLE};

/// Where the driver binds the helper's bytes in a run's view.
const GATE: &str = "/.substrate-exec-gate";

/// The helper the lane built. A lane with the delegated root and no helper fails here.
fn helper() -> PathBuf {
    super::exec_gate().expect(
        "SUBSTRATE_EXEC_GATE is unset: `bash scripts/delegated-lane.sh` builds \
         substrate-exec-gate and names it",
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

/// The directories that hold the shell and the base tools the cases run, as list entries.
fn base_directories() -> Vec<String> {
    let mut directories: Vec<String> =
        ["sh", "true", "mv", "ln", "rm", "mkdir", "cp", "stat", "cat"]
            .iter()
            .map(|name| text(tool(name).parent().expect("a directory")))
            .collect();
    directories.sort();
    directories.dedup();
    directories
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

/// A workspace with `bin/tool` and `other/tool`, both copies of the host's `true`.
fn workspace(host: &Path) -> PathBuf {
    let workspace = host.join("workspaces").join("ws_exec");
    for directory in ["bin", "other"] {
        std::fs::create_dir_all(workspace.join(directory)).expect(directory);
        std::fs::copy(tool("true"), workspace.join(directory).join("tool")).expect("copy true");
    }
    workspace
}

/// The delegated root, or `None` after printing `<name>: ABSENT (<reason>)`.
fn delegated(name: &str) -> Option<PathBuf> {
    match super::delegated() {
        Ok(root) => Some(root),
        Err(reason) => {
            println!("{name}: ABSENT ({reason})");
            None
        }
    }
}

struct Lane {
    host: tempfile::TempDir,
    adapter: Arc<dyn ConfinementAdapter>,
    workspace: PathBuf,
}

/// The adapter opened with the lane's helper, having proved `exec_subpaths`, and one workspace;
/// or `None` after printing `<name>: ABSENT (<reason>)`.
fn lane(name: &str) -> Option<Lane> {
    let cgroup_root = delegated(name)?;
    let host = tempfile::tempdir().expect("host root");
    let adapter = open_with(host.path(), &cgroup_root, Some(helper())).expect("linux opens");
    assert!(
        adapter.facts().proves(&Fact::ExecSubpaths),
        "{name}: linux opened with the helper did not prove exec_subpaths: {:?}",
        adapter.facts()
    );
    let workspace = workspace(host.path());
    Some(Lane {
        host,
        adapter,
        workspace,
    })
}

fn listed(workspace: &Path, entries: &[&str]) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        exec_subpaths: entries.iter().map(|entry| (*entry).to_owned()).collect(),
        ..ConfinementProfile::default()
    }
}

fn shell(script: &str) -> Command {
    Command {
        argv: vec![text(&tool("sh")), "-c".into(), script.to_owned()],
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
    }
}

/// Admits `profile`, runs `script` under the shell and returns its exit code and its stdout.
async fn run(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    script: &str,
) -> (Option<i32>, String) {
    let admitted = adapter.admit(profile).expect("admitted");
    let run = adapter
        .start(&admitted, &shell(script))
        .await
        .expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    (
        ended.exit.as_ref().and_then(|exit| exit.code),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// The records of one profile's sink, as JSON.
type Records = Arc<parking_lot::Mutex<Vec<serde_json::Value>>>;

fn audited(profile: ConfinementProfile) -> (ConfinementProfile, Records) {
    let records = Records::default();
    let sink = Arc::clone(&records);
    (
        ConfinementProfile {
            audit: Some(AuditSink::new(move |record: &AuditRecord| {
                sink.lock()
                    .push(serde_json::to_value(record).expect("record"));
            })),
            ..profile
        },
        records,
    )
}

/// Starts `argv` under `profile` and requires the start refused before dispatch with `code` and
/// `status`, its `profile_refused` record after the admission's, and `marker` never written.
async fn assert_start_refused(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: ConfinementProfile,
    program: &str,
    (code, status): (&str, u16),
    marker: &Path,
    what: &str,
) -> ConfinementError {
    let (profile, records) = audited(profile);
    let admitted = adapter
        .admit(&profile)
        .unwrap_or_else(|error| panic!("{what}: admission reads no path: {error}"));
    let error = adapter
        .start(
            &admitted,
            &Command {
                argv: vec![program.to_owned(), "-c".into(), ": > /workspace/ran".into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect_err(what);
    assert_eq!(error.code(), code, "{what}: {error}");
    assert_eq!(error.status(), status, "{what}: {error}");
    let records = records.lock();
    let events: Vec<_> = records.iter().map(|record| &record["event"]).collect();
    assert_eq!(
        events,
        ["profile_admitted", "profile_refused"],
        "{what}: {records:?}"
    );
    assert_eq!(records[1]["code"], code, "{what}");
    assert!(!marker.exists(), "{what}: the program ran");
    error
}

/// The story's acceptance: a profile listing one directory executes a program beneath it, and an
/// execution of a program outside the list fails inside the run with status 126. Before this
/// story the same profile was refused `confinement.fact-unserved`.
#[tokio::test(flavor = "multi_thread")]
async fn a_listed_directory_runs_its_program_and_a_program_outside_it_fails_inside_the_run() {
    const NAME: &str = "ESS confinement linux exec allow-list";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let profile = listed(&lane.workspace, &["bin"]);
    let base = tool("true");
    let script = format!(
        "bin/tool; echo $?; other/tool 2>/dev/null; echo $?; {} 2>/dev/null; echo $?; \
         exec other/tool",
        base.display()
    );
    let (code, stdout) = run(&lane.adapter, &profile, &script).await;
    assert_eq!(
        stdout, "0\n126\n126\n",
        "the listed program ran; a workspace program and a base tool outside the list did not"
    );
    assert_eq!(code, Some(126), "the run ends with the refused execution");

    // The render shows the argv the start runs: the helper bound from its bytes and placed
    // directly before the program, with one option per entry.
    let admitted = lane.adapter.admit(&profile).expect("admitted");
    let rendered = lane
        .adapter
        .render(&admitted, &shell("true"))
        .expect("rendered");
    let args = rendered.args;
    let bind = args
        .windows(5)
        .find(|window| window[4] == GATE)
        .unwrap_or_else(|| panic!("no bind at {GATE}: {args:?}"));
    assert_eq!(
        &bind[..3],
        ["--perms", "0555", "--ro-bind-data"],
        "{args:?}"
    );
    let sh = text(&tool("sh"));
    let gate = args
        .iter()
        .rposition(|part| part == GATE)
        .expect("the helper");
    assert_eq!(args[gate - 1], "--", "after env: {args:?}");
    assert_eq!(args[gate + 1], "--program", "{args:?}");
    let allow = args
        .windows(4)
        .filter(|window| window[0] == "--allow")
        .collect::<Vec<_>>();
    assert_eq!(
        allow,
        [["--allow", "beneath", "/workspace", "bin"]],
        "{args:?}"
    );
    let end = gate
        + args[gate..]
            .iter()
            .position(|part| part == "--")
            .expect("the end of the options");
    assert_eq!(&args[end + 1..], [sh.as_str(), "-c", "true"], "{args:?}");
    println!("{NAME}: executed");
}

/// D2 and D3: an entry may name one file; an absolute entry may lie in the workspace mount; and
/// the program alone as the list runs nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_entry_an_absolute_entry_and_the_program_alone() {
    const NAME: &str = "ESS confinement linux exec entries";
    const SCRIPT: &str = "bin/tool 2>/dev/null; echo $?; bin/sibling 2>/dev/null; echo $?";
    let Some(lane) = lane(NAME) else {
        return;
    };
    std::fs::copy(tool("true"), lane.workspace.join("bin/sibling")).expect("sibling");
    let cases = [
        (vec!["bin/tool".to_owned()], "0\n126\n"),
        (vec!["/workspace/bin".to_owned()], "0\n0\n"),
        (vec![text(&tool("sh"))], "126\n126\n"),
    ];
    for (entries, expected) in cases {
        let entries: Vec<&str> = entries.iter().map(String::as_str).collect();
        let (code, stdout) = run(&lane.adapter, &listed(&lane.workspace, &entries), SCRIPT).await;
        assert_eq!(stdout, expected, "entries {entries:?}");
        assert_eq!(
            code,
            Some(0),
            "entries {entries:?}: the shell's builtins ran"
        );
    }
    println!("{NAME}: executed");
}

/// The workspace `lane` holds, with an executable `#!` script outside every list.
fn script_workspace(lane: &Lane) -> Vec<String> {
    let script = lane.workspace.join("other/script.sh");
    std::fs::write(
        &script,
        format!("#!{}\necho the script ran\n", tool("sh").display()),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("mode");
    base_directories()
}

/// A6: with the base tools listed, a `#!` script outside the list is refused, although its
/// interpreter is allowed: the kernel asks the execute right of the script file.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_with_an_interpreter_line_outside_the_list_is_refused() {
    const NAME: &str = "ESS confinement linux exec script refused";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let entries = script_workspace(&lane);
    let entries: Vec<&str> = entries.iter().map(String::as_str).collect();
    let (code, stdout) = run(
        &lane.adapter,
        &listed(&lane.workspace, &entries),
        "other/script.sh 2>/dev/null; echo $?",
    )
    .await;
    assert_eq!(stdout, "126\n", "the script did not run");
    assert_eq!(code, Some(0));
    println!("{NAME}: executed");
}

/// The limit of design 35 § 7 (A7): an allowed interpreter runs any script it can read. `sh
/// script` does not execute the script file, so the list does not see it.
#[tokio::test(flavor = "multi_thread")]
async fn limit_an_allowed_interpreter_runs_a_script_outside_the_list() {
    const NAME: &str = "ESS confinement linux exec limit interpreter";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let entries = script_workspace(&lane);
    let entries: Vec<&str> = entries.iter().map(String::as_str).collect();
    let (code, stdout) = run(
        &lane.adapter,
        &listed(&lane.workspace, &entries),
        "sh other/script.sh; echo $?",
    )
    .await;
    assert_eq!(stdout, "the script ran\n0\n", "the limit holds as stated");
    assert_eq!(code, Some(0));
    println!("{NAME}: executed, sh runs a script outside the list");
}

/// The limit of design 35 § 7 (A8): the loader, started directly, runs any program file the run
/// can read. A dynamically linked program cannot start unless the loader is executable.
#[tokio::test(flavor = "multi_thread")]
async fn limit_the_loader_runs_a_program_outside_the_list() {
    const NAME: &str = "ESS confinement linux exec limit loader";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let env = std::fs::read("/usr/bin/env").expect("/usr/bin/env");
    let Some(loader) = substrate_exec_gate::elf_interpreter(&env) else {
        // A host whose base programs are static grants no loader, so the limit has no subject.
        println!("{NAME}: executed, the host's base programs name no loader");
        return;
    };
    let (code, stdout) = run(
        &lane.adapter,
        &listed(&lane.workspace, &["bin"]),
        &format!("other/tool 2>/dev/null; echo $?; {loader} other/tool; echo $?"),
    )
    .await;
    assert_eq!(
        stdout, "126\n0\n",
        "refused by path, run by the loader: the limit holds as stated"
    );
    assert_eq!(code, Some(0));
    println!("{NAME}: executed, {loader} runs a program outside the list");
}

/// D6: refer is granted on `/`, so a run with a list still renames a file within one directory
/// and across two unlisted directories, and links across them. The inode shows a rename and not
/// a copy.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_with_a_list_renames_within_a_directory_and_across_two_unlisted_ones() {
    const NAME: &str = "ESS confinement linux exec rename";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let entries = base_directories();
    let entries: Vec<&str> = entries.iter().map(String::as_str).collect();
    let (code, stdout) = run(
        &lane.adapter,
        &listed(&lane.workspace, &entries),
        "mkdir a b && echo x > a/f || exit 20; i=$(stat -c %i a/f); \
         mv a/f a/g || exit 21; [ \"$(stat -c %i a/g)\" = \"$i\" ] || exit 22; \
         mv a/g b/h || exit 23; [ \"$(stat -c %i b/h)\" = \"$i\" ] || exit 24; \
         ln b/h a/l || exit 25; echo renamed",
    )
    .await;
    assert_eq!(stdout, "renamed\n");
    assert_eq!(code, Some(0));
    assert!(lane.workspace.join("a/l").is_file());
    println!("{NAME}: executed");
}

/// The limit of design 35 § 6 (R4 to R6): a rename or link from a directory outside the list
/// into a listed one is refused `EXDEV`. `ln` fails; `mv` falls back to a copy, which the new
/// inode shows; the opposite direction is a rename.
#[tokio::test(flavor = "multi_thread")]
async fn limit_a_rename_into_a_listed_directory_is_refused_and_mv_copies() {
    const NAME: &str = "ESS confinement linux exec limit rename into";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let mut entries = base_directories();
    entries.push("bin".to_owned());
    let entries: Vec<&str> = entries.iter().map(String::as_str).collect();
    let (code, stdout) = run(
        &lane.adapter,
        &listed(&lane.workspace, &entries),
        "echo x > other/f; i=$(stat -c %i other/f); \
         ln other/f bin/l 2>/dev/null; echo \"ln $?\"; \
         mv other/f bin/m; echo \"mv $?\"; \
         [ \"$(stat -c %i bin/m)\" != \"$i\" ] && echo copied; j=$(stat -c %i bin/m); \
         mv bin/m other/back && [ \"$(stat -c %i other/back)\" = \"$j\" ] && echo renamed-out",
    )
    .await;
    assert_eq!(
        stdout, "ln 1\nmv 0\ncopied\nrenamed-out\n",
        "the limit holds as stated"
    );
    assert_eq!(code, Some(0));
    println!("{NAME}: executed, a link into a listed directory is refused and mv copies");
}

/// The limit of design 35 § 6 (W11): a listed directory the run removes and creates again is no
/// longer listed. The rule belonged to the removed directory.
#[tokio::test(flavor = "multi_thread")]
async fn limit_a_listed_directory_removed_and_created_again_is_no_longer_listed() {
    const NAME: &str = "ESS confinement linux exec limit recreated";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let mut entries = base_directories();
    entries.push("bin".to_owned());
    let entries: Vec<&str> = entries.iter().map(String::as_str).collect();
    let (code, stdout) = run(
        &lane.adapter,
        &listed(&lane.workspace, &entries),
        "bin/tool; echo $?; rm -r bin && mkdir bin && cp other/tool bin/tool || exit 20; \
         bin/tool 2>/dev/null; echo $?",
    )
    .await;
    assert_eq!(stdout, "0\n126\n", "the limit holds as stated");
    assert_eq!(code, Some(0));
    println!("{NAME}: executed, a recreated directory is not listed");
}

/// A directory outside the workspace root with `bin/tool` and `lib/tool`, copies of `true`:
/// what a case declares as read-only roots.
fn tools(host: &Path) -> PathBuf {
    let tools = host.join("tools");
    for directory in ["bin", "lib"] {
        std::fs::create_dir_all(tools.join(directory)).expect(directory);
        std::fs::copy(tool("true"), tools.join(directory).join("tool")).expect("copy true");
    }
    tools
}

fn root(host_path: &Path, mount: &str) -> ReadOnlyRoot {
    ReadOnlyRoot {
        host_path: text(host_path),
        mount: mount.to_owned(),
    }
}

/// The limit of design 35 § 5 (K7): a listed directory does not reach through a second mount
/// of a part of it. One directory is the read-only root `/opt/t` and its `bin` is `/opt/x`;
/// with `/opt/t` listed, `/opt/t/bin/tool` runs and the same file as `/opt/x/tool` does not.
#[tokio::test(flavor = "multi_thread")]
async fn limit_a_listed_directory_does_not_reach_through_a_second_mount_of_a_part_of_it() {
    const NAME: &str = "ESS confinement linux exec limit second mount";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let tools = tools(lane.host.path());
    let profile = ConfinementProfile {
        read_only_roots: vec![root(&tools, "/opt/t"), root(&tools.join("bin"), "/opt/x")],
        ..listed(&lane.workspace, &["/opt/t"])
    };
    let (code, stdout) = run(
        &lane.adapter,
        &profile,
        "/opt/t/bin/tool; echo $?; /opt/x/tool 2>/dev/null; echo $?",
    )
    .await;
    assert_eq!(stdout, "0\n126\n", "the limit holds as stated");
    assert_eq!(code, Some(0));
    println!("{NAME}: executed, a rule on /opt/t does not reach /opt/x");
}

/// D3: each entry rule is a named refusal before dispatch, with its audit record and nothing
/// run.
#[tokio::test(flavor = "multi_thread")]
async fn an_entry_or_a_program_the_view_cannot_carry_is_refused_at_the_start() {
    const NAME: &str = "ESS confinement linux exec refused at start";
    let Some(lane) = lane(NAME) else {
        return;
    };
    std::os::unix::fs::symlink("/usr/bin", lane.workspace.join("link")).expect("link");
    let marker = lane.workspace.join("ran");
    let sh = text(&tool("sh"));
    let cases: [(&str, &str, &str, &str); 5] = [
        (
            "link",
            &sh,
            PATH_UNREPRESENTABLE,
            "a symbolic link in the workspace",
        ),
        (
            "link/true",
            &sh,
            PATH_UNREPRESENTABLE,
            "a path through a link",
        ),
        ("target", &sh, PATH_UNREPRESENTABLE, "an absent entry"),
        (
            "/tmp/tools",
            &sh,
            EXEC_SUBPATH_UNDECLARED,
            "an entry under /tmp",
        ),
        ("bin", "sh", PATH_UNREPRESENTABLE, "a relative program"),
    ];
    for (entry, program, code, what) in cases {
        assert_start_refused(
            &lane.adapter,
            listed(&lane.workspace, &[entry]),
            program,
            (code, 422),
            &marker,
            what,
        )
        .await;
    }
    // The lexical rule is admission's: an entry no adapter can carry admits nothing.
    let error = lane
        .adapter
        .admit(&listed(&lane.workspace, &["bin/../other"]))
        .expect_err("not normal");
    assert_eq!(error.code(), PATH_UNREPRESENTABLE, "{error}");
    assert_eq!(error.status(), 422);
    println!("{NAME}: executed");
}

/// D24, the class comes from the host source (K10): a read-only root whose host directory lies
/// in the workspace root holds `link`, a symbolic link to `/usr/bin` that a run could have
/// planted. Listing it is refused before dispatch, and nothing of `/usr/bin` runs through it.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_in_a_read_only_root_inside_the_workspace_root_is_refused() {
    const NAME: &str = "ESS confinement linux exec root in a workspace";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let planted = lane.host.path().join("workspaces/ws_other/tools");
    std::fs::create_dir_all(&planted).expect("a directory another run wrote");
    std::os::unix::fs::symlink("/usr/bin", planted.join("link")).expect("a planted link");
    let error = assert_start_refused(
        &lane.adapter,
        ConfinementProfile {
            read_only_roots: vec![root(&planted, "/opt/k")],
            ..listed(&lane.workspace, &["/opt/k/link"])
        },
        &text(&tool("sh")),
        (PATH_UNREPRESENTABLE, 422),
        &lane.workspace.join("ran"),
        "a link in a root inside the workspace root",
    )
    .await;
    assert!(error.to_string().contains("symbolic link"), "{error}");
    println!("{NAME}: executed");
}

/// `substrate-exec-gate` in a bubblewrap view of its own, without the driver's check before it:
/// `ro` at `/opt/ro`, `workspace` at `/workspace`, and `allow` as the one entry.
fn gate_in_a_view(ro: &Path, workspace: &Path, allow: Located) -> std::process::Output {
    let sh = tool("sh");
    let lists = Lists {
        program: Located::new(
            Class::Follow,
            sh.parent().and_then(Path::to_str).expect("a directory"),
            "sh",
        )
        .expect("the shell"),
        loader: substrate_exec_gate::elf_interpreter(
            &std::fs::read("/usr/bin/env").expect("/usr/bin/env"),
        ),
        allow: vec![allow],
        writable: vec![
            "/workspace".to_owned(),
            "/tmp".to_owned(),
            "/dev".to_owned(),
        ],
    };
    std::process::Command::new("/usr/bin/bwrap")
        .args([
            "--unshare-user",
            "--unshare-pid",
            "--die-with-parent",
            "--clearenv",
        ])
        .args(["--ro-bind", "/usr", "/usr"])
        .args(["--ro-bind-try", "/bin", "/bin"])
        .args(["--ro-bind-try", "/lib", "/lib"])
        .args(["--ro-bind-try", "/lib64", "/lib64"])
        .args(["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"])
        .arg("--ro-bind")
        .arg(ro)
        .arg("/opt/ro")
        .arg("--bind")
        .arg(workspace)
        .arg("/workspace")
        .arg("--ro-bind")
        .arg(helper())
        .arg(GATE)
        .args(["--chdir", "/workspace", "--", GATE])
        .args(lists.options())
        .arg(&sh)
        .args(["-c", "echo ran; : > /workspace/ran"])
        .env_clear()
        .output()
        .expect("bubblewrap starts")
}

/// D24, a followed path into a writable mount (K9). A read-only root outside every writable
/// location holds a symbolic link into `/workspace`, and the profile lists it: the start is
/// refused before dispatch. The helper given the same arguments in a sandbox, without the
/// driver's check, exits 125 with the line of D24 and executes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_followed_entry_into_the_workspace_is_refused_by_the_driver_and_by_the_helper() {
    const NAME: &str = "ESS confinement linux exec follow into a writable mount";
    let Some(lane) = lane(NAME) else {
        return;
    };
    std::fs::create_dir_all(lane.workspace.join("tools/bin")).expect("tools");
    let tools = tools(lane.host.path());
    std::os::unix::fs::symlink("/workspace/tools/bin", tools.join("link")).expect("a link");
    let marker = lane.workspace.join("ran");
    let error = assert_start_refused(
        &lane.adapter,
        ConfinementProfile {
            read_only_roots: vec![root(&tools, "/opt/ro")],
            ..listed(&lane.workspace, &["/opt/ro/link"])
        },
        &text(&tool("sh")),
        (PATH_UNREPRESENTABLE, 422),
        &marker,
        "a link into the workspace",
    )
    .await;
    assert!(
        error
            .to_string()
            .contains("resolves to /workspace/tools/bin, in the writable mount /workspace"),
        "{error}"
    );

    let output = gate_in_a_view(
        &tools,
        &lane.workspace,
        Located::new(Class::Follow, "/opt/ro", "link").expect("the entry"),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "substrate-exec-gate: --allow /opt/ro/link resolves to /workspace/tools/bin, in the \
         writable mount /workspace\n"
    );
    assert_eq!(output.status.code(), Some(125));
    assert!(
        output.stdout.is_empty() && !marker.exists(),
        "the program ran"
    );
    println!("{NAME}: executed");
}

/// D24 and § 5 (K10), the helper's side of a link swapped in after the driver's check: the
/// workspace entry `bin` is a symbolic link to `/usr/bin` when the helper opens it. The helper
/// exits 125, and nothing of `/usr/bin` runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_entry_that_is_a_link_when_the_helper_opens_it_ends_the_run_125() {
    const NAME: &str = "ESS confinement linux exec swapped link";
    let Some(lane) = lane(NAME) else {
        return;
    };
    std::fs::remove_dir_all(lane.workspace.join("bin")).expect("remove bin");
    std::os::unix::fs::symlink("/usr/bin", lane.workspace.join("bin")).expect("the swap");
    let output = gate_in_a_view(
        &tools(lane.host.path()),
        &lane.workspace,
        Located::new(Class::Beneath, "/workspace", "bin").expect("the entry"),
    );
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(125), "{diagnostic}");
    assert!(
        diagnostic.starts_with("substrate-exec-gate: cannot open \"bin\" beneath \"/workspace\"")
            && diagnostic.trim_end().lines().count() == 1,
        "{diagnostic}"
    );
    assert!(
        output.stdout.is_empty() && !lane.workspace.join("ran").exists(),
        "the program ran"
    );
    println!("{NAME}: executed");
}

/// D26 through the driver (U3): the host's `/proc/sys` declared as a read-only root has a row
/// of type `proc`, outside the list. The start of a profile with a list is refused
/// `exec.sandbox-place-unserved` before dispatch, with its audit record and nothing run; the
/// same roots without a list start as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_whose_place_cannot_be_established_refuses_a_listed_start_only() {
    const NAME: &str = "ESS confinement linux exec place unserved";
    let Some(lane) = lane(NAME) else {
        return;
    };
    let roots = vec![root(Path::new("/proc/sys"), "/opt/p")];
    let error = assert_start_refused(
        &lane.adapter,
        ConfinementProfile {
            read_only_roots: roots.clone(),
            ..listed(&lane.workspace, &["bin"])
        },
        &text(&tool("sh")),
        ("exec.sandbox-place-unserved", 501),
        &lane.workspace.join("ran"),
        "a root on procfs",
    )
    .await;
    let message = error.to_string();
    for holds in [
        "file-system-unlisted proc",
        "ext4, overlay, tmpfs",
        "/opt/p",
    ] {
        assert!(message.contains(holds), "{message}");
    }
    // No list, no place asked: the run starts and reads the root.
    let (code, stdout) = run(
        &lane.adapter,
        &ConfinementProfile {
            read_only_roots: roots,
            ..listed(&lane.workspace, &[])
        },
        "cat /opt/p/kernel/ostype",
    )
    .await;
    assert_eq!(stdout, "Linux\n");
    assert_eq!(code, Some(0));
    println!("{NAME}: executed");
}

/// D20: the driver read the helper once, at open. The file replaced and then removed on disk
/// changes no later start, two starts of one opened adapter both run under the helper (K3), and
/// inside the run the helper can be neither executed nor written.
#[tokio::test(flavor = "multi_thread")]
async fn the_helper_replaced_and_removed_after_the_open_changes_no_later_start() {
    const NAME: &str = "ESS confinement linux exec helper after open";
    const EXPECTED: &str = "0\n126\n126\nread-only\n";
    let Some(cgroup_root) = delegated(NAME) else {
        return;
    };
    let host = tempfile::tempdir().expect("host root");
    let installed = host.path().join("substrate-exec-gate");
    std::fs::copy(helper(), &installed).expect("install the helper");
    let adapter = open_with(host.path(), &cgroup_root, Some(installed.clone())).expect("opens");
    assert!(adapter.facts().proves(&Fact::ExecSubpaths));
    let workspace = workspace(host.path());
    let profile = listed(&workspace, &["bin"]);
    let script = format!(
        "bin/tool; echo $?; other/tool 2>/dev/null; echo $?; {GATE} --version 2>/dev/null; \
         echo $?; if (echo x >> {GATE}) 2>/dev/null; then echo written; else echo read-only; fi"
    );
    std::fs::write(&installed, "#!/bin/sh\nexec \"$@\"\n").expect("replace the helper");
    let (_, stdout) = run(&adapter, &profile, &script).await;
    assert_eq!(stdout, EXPECTED, "a helper replaced on disk");
    std::fs::remove_file(&installed).expect("remove the helper");
    let (_, stdout) = run(&adapter, &profile, &script).await;
    assert_eq!(stdout, EXPECTED, "a helper removed from disk");
    println!("{NAME}: executed");
}

/// The refusal of a listed profile on an adapter that did not prove the fact: status 501, naming
/// `exec_subpaths`, with its `profile_refused` record.
fn assert_fact_unserved(adapter: &Arc<dyn ConfinementAdapter>, workspace: &Path, what: &str) {
    assert!(
        !adapter.facts().proves(&Fact::ExecSubpaths),
        "{what}: {:?}",
        adapter.facts()
    );
    let (profile, records) = audited(listed(workspace, &["bin"]));
    let error = adapter.admit(&profile).expect_err(what);
    assert!(
        matches!(
            &error,
            ConfinementError::FactUnserved { fact, .. } if *fact == Fact::ExecSubpaths
        ),
        "{what}: {error}"
    );
    assert_eq!(error.code(), confinement::FACT_UNSERVED, "{what}");
    assert_eq!(error.status(), 501, "{what}");
    let records = records.lock();
    assert_eq!(records.len(), 1, "{what}: {records:?}");
    assert_eq!(records[0]["event"], "profile_refused", "{what}");
    assert_eq!(records[0]["code"], confinement::FACT_UNSERVED, "{what}");
    assert_eq!(records[0]["fact"], "exec_subpaths", "{what}");
}

/// D19, first row: an adapter opened without `exec_gate` proves no executable-list fact. A
/// profile with a list is refused at admission and nothing runs with a weaker list; a profile
/// without one runs as before.
#[tokio::test(flavor = "multi_thread")]
async fn without_a_helper_a_listed_profile_is_refused_and_an_unlisted_one_runs() {
    const NAME: &str = "ESS confinement linux exec helper unset";
    let Some(cgroup_root) = delegated(NAME) else {
        return;
    };
    let host = tempfile::tempdir().expect("host root");
    let adapter = open_with(host.path(), &cgroup_root, None).expect("linux opens");
    let workspace = workspace(host.path());
    assert_fact_unserved(&adapter, &workspace, "no helper configured");
    let (code, stdout) = run(
        &adapter,
        &listed(&workspace, &[]),
        "bin/tool; echo $?; other/tool; echo $?",
    )
    .await;
    assert_eq!(
        stdout, "0\n0\n",
        "an empty list runs every program of the view"
    );
    assert_eq!(code, Some(0));
    println!("{NAME}: executed");
}

/// A stand-in helper: it answers the protocol line with `protocol`, and otherwise executes the
/// program without restricting anything.
fn stand_in(host: &Path, protocol: &str) -> PathBuf {
    let path = host.join("stand-in-gate");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo {protocol}; exit 0; fi\n\
             while [ \"$1\" != -- ]; do shift; done\nshift\nexec \"$@\"\n"
        ),
    )
    .expect("stand-in");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("mode");
    path
}

/// D19, fourth row, and D15: a helper of the protocol that does not restrict leaves the fact
/// absent, because the probe saw the refused half happen. The adapter opens and refuses a listed
/// profile by the fact's name.
#[tokio::test(flavor = "multi_thread")]
async fn a_helper_that_does_not_restrict_withholds_the_fact() {
    const NAME: &str = "ESS confinement linux exec helper unrestricting";
    let Some(cgroup_root) = delegated(NAME) else {
        return;
    };
    let host = tempfile::tempdir().expect("host root");
    let gate = stand_in(host.path(), "substrate-exec-gate/1");
    let adapter = open_with(host.path(), &cgroup_root, Some(gate)).expect("linux opens");
    let workspace = workspace(host.path());
    assert_fact_unserved(&adapter, &workspace, "a helper that does not restrict");
    println!("{NAME}: executed");
}

/// D19, third row: a helper that does not print the protocol line in the throwaway sandbox
/// refuses the open, naming the path. No adapter exists, so nothing is admitted.
#[tokio::test(flavor = "multi_thread")]
async fn a_helper_of_another_protocol_refuses_the_open() {
    const NAME: &str = "ESS confinement linux exec helper protocol";
    let Some(cgroup_root) = delegated(NAME) else {
        return;
    };
    let host = tempfile::tempdir().expect("host root");
    let gate = stand_in(host.path(), "substrate-exec-gate/0");
    let Err(error) = open_with(host.path(), &cgroup_root, Some(gate.clone())) else {
        panic!("a helper of another protocol opened");
    };
    assert_eq!(error.code(), "config.exec-gate-unserved", "{error}");
    assert_eq!(error.status(), 501, "{error}");
    assert!(
        error.to_string().contains(&text(&gate)),
        "the message names the path: {error}"
    );
    println!("{NAME}: executed");
}

/// D21 through the adapter's open: a helper in a workspace, and one with a second hard link in
/// one, each refuse the open `config.exec-gate-invalid`, 422. The checks themselves are the
/// driver's unit cases; this is the path an embedder takes.
#[tokio::test(flavor = "multi_thread")]
async fn a_helper_a_run_could_have_written_refuses_the_open() {
    const NAME: &str = "ESS confinement linux exec helper writable";
    let Some(cgroup_root) = delegated(NAME) else {
        return;
    };
    let host = tempfile::tempdir().expect("host root");
    let workspace = workspace(host.path());
    let inside = workspace.join("bin/gate");
    std::fs::copy(helper(), &inside).expect("a helper in a workspace");
    let outside = host.path().join("substrate-exec-gate");
    std::fs::copy(helper(), &outside).expect("a helper outside");
    std::fs::hard_link(&outside, workspace.join("other/second-name")).expect("a second link");
    for (gate, holds) in [
        (&inside, "which a run can write"),
        (&outside, "has 2 links"),
    ] {
        let Err(error) = open_with(host.path(), &cgroup_root, Some(gate.clone())) else {
            panic!("{} opened", gate.display());
        };
        assert_eq!(error.code(), "config.exec-gate-invalid", "{error}");
        assert_eq!(error.status(), 422, "{error}");
        assert!(error.to_string().contains(holds), "{error}");
    }
    println!("{NAME}: executed");
}

/// The fields of one logged event, by name.
type Fields = BTreeMap<String, String>;

/// Runs `open` and returns its value with the fields of every `warn` and `error` event this
/// thread logged meanwhile. The subscriber is this thread's for the call and no other's.
fn warnings<T>(open: impl FnOnce() -> T) -> (T, Vec<Fields>) {
    #[derive(Default)]
    struct Visitor(Fields);

    impl tracing::field::Visit for Visitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }
    }

    struct Capture(Arc<parking_lot::Mutex<Vec<Fields>>>);

    impl tracing::Subscriber for Capture {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            *metadata.level() <= tracing::Level::WARN
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut visitor = Visitor::default();
            event.record(&mut visitor);
            self.0.lock().push(visitor.0);
        }

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let value = tracing::subscriber::with_default(Capture(Arc::clone(&seen)), open);
    let seen = seen.lock().clone();
    (value, seen)
}

/// A profile with a list on an adapter that withheld the fact for `entry`: refused
/// `confinement.fact-unserved`, 501, with the entry and its words in the text, and one
/// `profile_refused` record of the shape it always had.
fn assert_refused_as_withheld(
    adapter: &Arc<dyn ConfinementAdapter>,
    workspace: &Path,
    entry: &FactWithheld,
) {
    let (profile, records) = audited(listed(workspace, &["bin"]));
    let error = adapter
        .admit(&profile)
        .expect_err("a list on an adapter that withheld the fact");
    match &error {
        ConfinementError::FactUnserved { fact, withheld, .. } => {
            assert_eq!(*fact, Fact::ExecSubpaths);
            assert_eq!(withheld.as_ref(), Some(entry));
        }
        other => panic!("expected exec_subpaths unserved, got {other:?}"),
    }
    assert_eq!(error.code(), confinement::FACT_UNSERVED);
    assert_eq!(error.status(), 501);
    let message = error.to_string();
    for holds in [
        "file-system-unlisted",
        "/proc/sys",
        "config.writable_roots",
        "is on proc",
    ] {
        assert!(message.contains(holds), "{message}");
    }
    {
        let records = records.lock();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0]["event"], "profile_refused");
        assert_eq!(records[0]["code"], confinement::FACT_UNSERVED);
        assert_eq!(records[0]["fact"], "exec_subpaths");
        let members: Vec<_> = records[0]
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            ["adapter", "code", "event", "fact"],
            "the record keeps its shape"
        );
    }
}

/// D27, through the driver and with no privilege. The lane's configuration with one more
/// declared writable root, at `/proc/sys`, which no start names: it lies on the type `proc`,
/// outside the list. The adapter opens with the helper configured and proves no executable-list
/// fact; its proof says why; a profile with a list is refused with that reason and nothing
/// runs; and a profile without one runs as before, with no helper in its argv.
#[tokio::test(flavor = "multi_thread")]
async fn a_writable_root_on_an_unlisted_file_system_withholds_the_lists_and_says_why() {
    const NAME: &str = "ESS confinement linux exec withheld";
    let Some(cgroup_root) = delegated(NAME) else {
        return;
    };
    let host = tempfile::tempdir().expect("host root");
    let mut config = HostConfig::minimum(host.path().join("workspaces"));
    config.exec_gate = Some(helper());
    config.writable_roots = vec![confinement::DeclaredWritableRoot {
        name: "kernel".to_owned(),
        path: PathBuf::from("/proc/sys"),
    }];
    let (adapter, warnings) = warnings(|| {
        confinement::open(AdapterRequest::Linux(Box::new(
            LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
        )))
    });
    let adapter =
        adapter.expect("linux opens with a writable root on a file system outside the list");
    let entry = FactWithheld {
        fact: Fact::ExecSubpaths,
        cause: "file-system-unlisted",
        address: "config.writable_roots",
        path: "/proc/sys".to_owned(),
        file_system: Some("proc".to_owned()),
    };
    // Visible by default: the open wrote one `warn` line for the entry, with its fields.
    let withheld: Vec<_> = warnings
        .iter()
        .filter(|fields| {
            fields
                .get("message")
                .is_some_and(|message| message.contains("executable lists withheld"))
        })
        .collect();
    assert_eq!(withheld.len(), 1, "one line per entry: {warnings:?}");
    for (field, value) in [
        ("fact", "ExecSubpaths"),
        ("cause", "file-system-unlisted"),
        ("address", "config.writable_roots"),
        ("path", "/proc/sys"),
        ("file_system", "proc"),
    ] {
        assert_eq!(
            withheld[0].get(field).map(String::as_str),
            Some(value),
            "{field}: {withheld:?}"
        );
    }
    let facts = adapter.facts();
    assert!(!facts.proves(&Fact::ExecSubpaths), "{facts:?}");
    assert_eq!(facts.withheld, std::slice::from_ref(&entry), "{facts:?}");
    // The floor itself is proved: only the lists are withheld.
    assert!(facts.proves(&Fact::WholeTreeKill), "{facts:?}");

    // A profile with a list: refused at admission with the reason, recorded, and nothing runs.
    let workspace = workspace(host.path());
    assert_refused_as_withheld(&adapter, &workspace, &entry);

    // A profile without a list: admitted, rendered with no helper, and run.
    let unlisted = listed(&workspace, &[]);
    let admitted = adapter.admit(&unlisted).expect("a profile without a list");
    let rendered = adapter.render(&admitted, &shell("true")).expect("rendered");
    assert!(
        !rendered.args.iter().any(|part| part.contains(GATE)),
        "no helper in the argv of a run without a list: {:?}",
        rendered.args
    );
    let (code, stdout) = run(
        &adapter,
        &unlisted,
        "bin/tool; echo $?; other/tool; echo $?",
    )
    .await;
    assert_eq!(stdout, "0\n0\n", "a run without a list runs every program");
    assert_eq!(code, Some(0));
    println!("{NAME}: executed");
}
