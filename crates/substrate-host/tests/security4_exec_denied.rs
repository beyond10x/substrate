//! Security review of `story:executable-deny-list` (design 35 § 8, D10 to D13), through the
//! public confinement API of the `linux` adapter and the lane's statically linked helper.
//!
//! The committed cases hold the deny-list where a denied path and the listed directory that
//! holds it are one file system seen under one or two names. These cases ask what they do not:
//!
//! - **a host mount beneath a listed directory.** The driver binds `/usr` and every declared
//!   root recursively, so a mount the host has beneath one of them is in every view. The helper
//!   decides "a denied path lies in this directory" by place (D25), and the kernel grants by
//!   walking up through mount points. Where the two differ, a denied program runs;
//! - **a deny entry whose place cannot be established** is refused before dispatch, through
//!   the driver and not only on synthetic mount-table text;
//! - **D28 and D29 with `--deny` operands present**: a traced start with a deny-list executes
//!   bubblewrap and the helper and nothing else before its ruleset, and the helper maps no file.
//!
//! The cases need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this
//! process is inside, bubblewrap, and `SUBSTRATE_EXEC_GATE` naming the static helper. Without the
//! first two each prints `<name>: ABSENT (<reason>)` and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    ReadOnlyFile, ReadOnlyRoot, RunState, linux::LinuxRequest,
};

/// Where the driver binds the helper's bytes in a run's view.
const GATE: &str = "/.substrate-exec-gate";

/// Set, in the process this file starts inside a mount namespace of its own, to say so.
const INNER: &str = "SECURITY4_INNER";

/// What the stage of that namespace exits with when its mount failed.
const STAGE_FAILED: i32 = 97;

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

struct Lane {
    host: tempfile::TempDir,
    adapter: Arc<dyn ConfinementAdapter>,
    workspace: PathBuf,
}

/// The adapter opened with the lane's helper, and a workspace with `bin/tool` and `bin/other`,
/// both copies of the host's `true`. `prepare` is given the host root before the adapter opens
/// and may name another sandbox program. `Err` holds why the adapter did not open, or did not
/// prove the deny-list.
fn lane_or(
    prepare: impl FnOnce(&Path) -> Option<PathBuf>,
    cgroup_root: PathBuf,
) -> Result<Lane, String> {
    let host = tempfile::tempdir().expect("host root");
    let mut config = HostConfig::minimum(host.path().join("workspaces"));
    config.exec_gate = Some(PathBuf::from(
        std::env::var_os("SUBSTRATE_EXEC_GATE")
            .expect("SUBSTRATE_EXEC_GATE is unset: the lane names the helper it built"),
    ));
    if let Some(bubblewrap) = prepare(host.path()) {
        config.bubblewrap = bubblewrap;
    }
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .map_err(|error| format!("linux did not open: {error}"))?;
    if !adapter.facts().proves(&Fact::ExecDeniedPaths) {
        return Err(format!(
            "linux opened with the helper did not prove exec_denied_paths: {:?}",
            adapter.facts()
        ));
    }
    let workspace = host.path().join("workspaces").join("ws_security4");
    std::fs::create_dir_all(workspace.join("bin")).expect("bin");
    for name in ["tool", "other"] {
        std::fs::copy(tool("true"), workspace.join("bin").join(name)).expect("copy true");
    }
    Ok(Lane {
        host,
        adapter,
        workspace,
    })
}

/// [`lane_or`] on the lane itself, where an adapter that does not prove the fact is a failure.
fn lane(name: &str, prepare: impl FnOnce(&Path) -> Option<PathBuf>) -> Option<Lane> {
    let cgroup_root = delegated(name)?;
    Some(lane_or(prepare, cgroup_root).unwrap_or_else(|why| panic!("{name}: {why}")))
}

fn profile(workspace: &Path, entries: &[&str], denied: &[&str]) -> ConfinementProfile {
    let owned = |list: &[&str]| list.iter().map(|entry| (*entry).to_owned()).collect();
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        exec_subpaths: owned(entries),
        exec_denied_paths: owned(denied),
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
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    Ok((
        ended.exit.as_ref().and_then(|exit| exit.code),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// The directory beneath `/usr` the mount case covers with a mount of its own, inside its own
/// mount namespace only.
const COVERED: &str = "/usr/local/src";

/// What the two runs of the mount case print, each line a program and its status.
const MOUNT_SCRIPT: &str = "/usr/local/src/tool 2>/dev/null; echo denied:$?; \
                            /usr/local/src/other; echo beside:$?";

/// The mount case inside its namespace: [`COVERED`] is a mount there. Opens the adapter and
/// prints what a run without the deny entry and a run with it printed, one line each.
async fn mount_case_inside(name: &str) {
    let Some(cgroup_root) = delegated(name) else {
        return;
    };
    let lane = match lane_or(|_| None, cgroup_root) {
        Ok(lane) => lane,
        Err(why) => {
            println!("{name} inside: ABSENT ({why})");
            return;
        }
    };
    for (what, denied) in [
        ("control", &[][..]),
        ("denied", &["/usr/local/src/tool"][..]),
    ] {
        let listed = profile(&lane.workspace, &["bin", "/usr/local"], denied);
        match run(&lane.adapter, &listed, &shell(MOUNT_SCRIPT, &[])).await {
            Ok((_, stdout, _)) => println!("{name} inside: {what} {stdout:?}"),
            Err(refusal) => println!("{name} inside: {what} refused {refusal}"),
        }
    }
}

/// **A program denied in a host mount beneath a listed directory is not executed** (design 35
/// D10: "A denied path wins. It wins over a listed directory that holds it"; § 8.5: "A deny
/// entry denies a path, and every mount of that path").
///
/// The host has a mount beneath `/usr`: this case makes one at `/usr/local/src`, in a user and
/// mount namespace of its own, and runs itself again there, so the driver opens on a host that
/// has it and binds it into the view with `/usr`. The profile lists `/usr/local` and denies
/// `/usr/local/src/tool`. Without the deny entry both programs of the mount run. With it the
/// denied one ends with status 126 and its neighbour runs. A refusal before dispatch is no
/// passing answer: the mount is of a listed type, and D30 serves it (design 35 § 16, E20).
///
/// A host that mounts a tool directory beneath `/usr`, a container whose runtime binds files
/// there, and an operator's declared root with a mount inside it have this shape without any
/// namespace.
#[tokio::test(flavor = "multi_thread")]
async fn security4_a_program_denied_in_a_host_mount_beneath_a_listed_directory_is_not_executed() {
    const NAME: &str = "security4 exec denied host mount";
    const CASE: &str =
        "security4_a_program_denied_in_a_host_mount_beneath_a_listed_directory_is_not_executed";
    if std::env::var_os(INNER).is_some() {
        mount_case_inside(NAME).await;
        return;
    }
    if delegated(NAME).is_none() {
        return;
    }
    let (Some(unshare), Some(mount), Some(setpriv)) =
        (find("unshare"), find("mount"), find("setpriv"))
    else {
        println!("{NAME}: ABSENT (no unshare, mount or setpriv on this host)");
        return;
    };
    if !Path::new(COVERED).is_dir() {
        println!("{NAME}: ABSENT (this host has no {COVERED} to mount over)");
        return;
    }
    let host = tempfile::tempdir().expect("host root");
    let held = host.path().join("held");
    std::fs::create_dir(&held).expect("held");
    for name in ["tool", "other"] {
        std::fs::copy(tool("true"), held.join(name)).expect("copy true");
    }
    // The namespace maps this user to itself and keeps its capabilities for the one mount;
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
        .arg(tool("sh"))
        .arg("-c")
        .arg(format!(
            "{} --bind \"$1\" {COVERED} >&2 || exit {STAGE_FAILED}; shift; exec \"$@\"",
            mount.display()
        ))
        .arg("sh")
        .arg(&held)
        .arg(setpriv)
        .args(["--inh-caps=-all", "--ambient-caps=-all", "--"])
        .arg(std::env::current_exe().expect("this test binary"))
        .args(["--exact", CASE, "--nocapture", "--test-threads=1"])
        .env(INNER, "1")
        .output()
        .expect("unshare starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(STAGE_FAILED) {
        println!(
            "{NAME}: ABSENT (no mount in a user namespace: {})",
            stderr.trim()
        );
        return;
    }
    // `unshare` itself ended: this host gives this user no user and mount namespace.
    if output.status.code() == Some(1) && stderr.trim_start().starts_with("unshare:") {
        println!(
            "{NAME}: ABSENT (no user and mount namespace: {})",
            stderr.trim()
        );
        return;
    }
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
    let (Some(control), Some(denied)) = (line("control"), line("denied")) else {
        panic!("the run inside the namespace printed neither line: {stdout}\n{stderr}");
    };
    assert_eq!(
        control, "\"denied:0\\nbeside:0\\n\"",
        "the control, without the deny entry: both programs of the mount run"
    );
    assert_eq!(
        denied, "\"denied:126\\nbeside:0\\n\"",
        "`/usr/local` is listed, `/usr/local/src` is a mount of the host and \
         `/usr/local/src/tool` is denied: the denied program ran, or the start was refused \
         although the mount is of a listed type"
    );
    println!("{NAME}: executed, {denied}");
}

/// **A deny entry whose place cannot be established is refused before dispatch** (design 35
/// D26, § 12: `exec.sandbox-place-unserved`, 501, address `exec_denied_paths`), through the
/// driver. A declared read-only root shows the host's `/boot`, and the host has its EFI system
/// partition mounted beneath it: a `vfat` file system, a type outside the closed list, which
/// the recursive bind of the root carries into the view. The deny entry is that mount. The
/// control is the same profile without the entry, which runs: the root alone refuses nothing.
#[tokio::test(flavor = "multi_thread")]
async fn security4_a_deny_entry_in_an_unlisted_file_system_is_refused_before_dispatch() {
    const NAME: &str = "security4 exec denied unplaced";
    let Some(lane) = lane(NAME, |_| None) else {
        return;
    };
    let mounted = std::fs::read_to_string("/proc/self/mountinfo")
        .unwrap_or_default()
        .lines()
        .any(|row| row.contains(" /boot/efi ") && row.contains(" - vfat "));
    if !mounted {
        println!("{NAME}: ABSENT (this host has no vfat mount at /boot/efi)");
        return;
    }
    let with = |denied: &[&str]| ConfinementProfile {
        read_only_roots: vec![ReadOnlyRoot {
            host_path: "/boot".to_owned(),
            mount: "/opt/unlisted".to_owned(),
        }],
        ..profile(&lane.workspace, &["bin"], denied)
    };
    let script = shell(": > /workspace/ran; bin/tool; echo ran:$?", &[]);
    match run(&lane.adapter, &with(&[]), &script).await {
        Ok((_, stdout, _)) => assert_eq!(stdout, "ran:0\n", "the control"),
        Err(refusal) => {
            println!("{NAME}: ABSENT (the root alone is refused on this host: {refusal})");
            return;
        }
    }
    std::fs::remove_file(lane.workspace.join("ran")).expect("the control ran");
    let refusal = run(&lane.adapter, &with(&["/opt/unlisted/efi"]), &script)
        .await
        .expect_err("a deny entry on a file system outside the list was dispatched");
    assert_eq!(
        (refusal.code(), refusal.status()),
        ("exec.sandbox-place-unserved", 501),
        "{refusal}"
    );
    assert!(
        refusal.to_string().contains("file-system-unlisted"),
        "the refusal names the cause: {refusal}"
    );
    assert!(
        !lane.workspace.join("ran").exists(),
        "the refused start ran its program"
    );
    println!("{NAME}: executed, {refusal}");
}

/// One finished system call of a trace: the process, the call's name, the whole line with an
/// interrupted call joined to its end, and whether it succeeded.
struct Call {
    pid: u32,
    name: String,
    line: String,
    succeeded: bool,
}

impl Call {
    /// The first quoted argument: the path of an `execve` or an `openat`.
    fn path(&self) -> &str {
        self.line
            .split_once('"')
            .and_then(|(_, rest)| rest.split_once('"'))
            .map_or("", |(path, _)| path)
    }
}

/// The finished calls of an `strace -f -o` file, in the order they began or resumed.
fn calls(trace: &str) -> Vec<Call> {
    let mut interrupted: BTreeMap<u32, String> = BTreeMap::new();
    let mut calls = Vec::new();
    for line in trace.lines() {
        let Some((pid, rest)) = line.split_once(' ') else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let rest = rest.trim_start();
        let line = if let Some(head) = rest.strip_suffix("<unfinished ...>") {
            interrupted.insert(pid, head.trim_end().to_owned());
            continue;
        } else if rest.starts_with("<... ") {
            let tail = rest.split_once("resumed>").map_or("", |(_, tail)| tail);
            format!("{}{tail}", interrupted.remove(&pid).unwrap_or_default())
        } else {
            rest.to_owned()
        };
        let Some((name, _)) = line.split_once('(') else {
            continue;
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let succeeded = line
            .rsplit_once(" = ")
            .is_some_and(|(_, result)| !result.starts_with('-'));
        calls.push(Call {
            pid,
            name: name.to_owned(),
            line,
            succeeded,
        });
    }
    calls
}

/// A sandbox program for the driver that is bubblewrap under `strace -f`, one trace file per
/// start in `traces`. The driver clears its environment before it spawns the sandbox, so the
/// script names everything by path.
fn traced_bubblewrap(host: &Path, traces: &Path) -> PathBuf {
    std::fs::create_dir_all(traces).expect("the trace directory");
    let script = host.join("traced-bwrap");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nexec /usr/bin/strace -f -qq -s 512 -y -o '{}'/trace.$$ \
             -e trace=execve,execveat,open,openat,openat2,mmap,landlock_restrict_self,\
             read,readv,pread64,preadv,recvmsg,recvfrom \
             /usr/bin/bwrap \"$@\"\n",
            traces.display()
        ),
    )
    .expect("the script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("mode");
    script
}

/// **D28 and D29 hold with a deny-list** (design 35 § 6, *Before the ruleset*): the expansion
/// of D11 reads directories and compares places before the ruleset, and it executes and maps
/// nothing.
///
/// The driver's own start of one run with both lists is traced. `bin` is listed and
/// `bin/tool` is denied, so the helper expands a directory the run can write. The profile
/// declares a file at `/etc/ld.so.preload` and the command sets `LD_PRELOAD`; both name a path
/// of the workspace. Up to the first successful `landlock_restrict_self`:
///
/// - the `execve` calls are bubblewrap and the helper's mount, each once and successful, and
///   the helper is started with one variable;
/// - every mapping of the helper is anonymous and none is executable;
/// - every open of the helper is `O_PATH`, a directory opened to list it, or the mount table,
///   and the helper reads from the mount table only.
///
/// After it the denied program ends with status 126 and its neighbour runs.
// One case on purpose: one traced start, read once.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn security4_a_start_with_a_deny_list_executes_and_maps_nothing_before_the_ruleset() {
    const NAME: &str = "security4 exec denied trace";
    const MARKER: &str = "s4-marker-a09e";
    if !Path::new("/usr/bin/strace").is_file() {
        println!("{NAME}: ABSENT (no strace at /usr/bin/strace)");
        return;
    }
    let mut traces = PathBuf::new();
    let Some(lane) = lane(NAME, |host| {
        traces = host.join("traces");
        Some(traced_bubblewrap(host, &traces))
    }) else {
        return;
    };
    let list = lane.host.path().join("preload.list");
    std::fs::write(&list, "/workspace/no-such-object.so\n").expect("the list of objects");
    let declared = ConfinementProfile {
        read_only_files: vec![ReadOnlyFile {
            host_path: text(&list.canonicalize().expect("canonical")),
            mount: "/etc/ld.so.preload".to_owned(),
        }],
        ..profile(&lane.workspace, &["bin"], &["bin/tool", "bin/later"])
    };
    let script =
        format!(": {MARKER}; bin/tool 2>/dev/null; echo denied:$?; bin/other; echo beside:$?");
    let command = shell(&script, &[("LD_PRELOAD", "/workspace/no-such-object.so")]);
    let (code, stdout, stderr) = run(&lane.adapter, &declared, &command)
        .await
        .expect("the start is admitted and dispatched");
    assert_eq!(
        stdout, "denied:126\nbeside:0\n",
        "the deny-list did not hold under the trace (exit {code:?}, stderr {stderr:?})"
    );
    let of_the_run: Vec<String> = std::fs::read_dir(&traces)
        .expect("the trace directory")
        .map(|entry| std::fs::read(entry.expect("an entry").path()).expect("a trace"))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .filter(|trace| trace.contains(MARKER))
        .collect();
    assert_eq!(of_the_run.len(), 1, "one start carried the marker");
    if let Some(keep) = std::env::var_os("SECURITY4_TRACE_OUT") {
        std::fs::write(keep, &of_the_run[0]).expect("the trace is kept");
    }
    let calls = calls(&of_the_run[0]);
    let ruleset = calls
        .iter()
        .position(|call| call.name == "landlock_restrict_self" && call.succeeded)
        .expect("the trace holds a successful landlock_restrict_self");
    let helper = calls[ruleset].pid;
    let started = calls[..ruleset]
        .iter()
        .position(|call| {
            call.pid == helper && call.name == "execve" && call.succeeded && call.path() == GATE
        })
        .expect("the process that applied the ruleset was started as the helper");
    let executed: Vec<(&str, bool)> = calls[..ruleset]
        .iter()
        .filter(|call| matches!(call.name.as_str(), "execve" | "execveat"))
        .map(|call| (call.path(), call.succeeded))
        .collect();
    assert_eq!(
        executed,
        [("/usr/bin/bwrap", true), (GATE, true)],
        "the programs executed before the ruleset"
    );
    assert!(
        calls[started].line.contains("--deny") && calls[started].line.contains("/* 1 var */"),
        "the helper was started with its deny operands and PWD alone: {}",
        calls[started].line
    );
    let of_helper = || {
        calls[started + 1..ruleset]
            .iter()
            .filter(|c| c.pid == helper)
    };
    let mapped: Vec<&str> = of_helper()
        .filter(|call| call.name == "mmap")
        .filter(|call| !call.line.contains("MAP_ANONYMOUS") || call.line.contains("PROT_EXEC"))
        .map(|call| call.line.as_str())
        .collect();
    assert!(
        mapped.is_empty(),
        "the helper mapped a file, or memory as code, before the ruleset: {mapped:#?}"
    );
    let opened: Vec<&str> = of_helper()
        .filter(|call| matches!(call.name.as_str(), "open" | "openat" | "openat2"))
        .map(|call| call.line.as_str())
        .collect();
    let for_contents: Vec<&&str> = opened
        .iter()
        .filter(|line| {
            !line.contains("O_PATH")
                && !line.contains("O_DIRECTORY")
                && !line.contains("\"/proc/self/mountinfo\"")
        })
        .collect();
    assert!(
        for_contents.is_empty(),
        "the helper opened a file for its contents before the ruleset: {for_contents:#?}"
    );
    let listed = opened
        .iter()
        .filter(|line| line.contains("O_DIRECTORY") && !line.contains("O_PATH"))
        .count();
    assert!(
        listed >= 1,
        "the helper listed no directory, so nothing was expanded: {opened:#?}"
    );
    let read: Vec<&str> = of_helper()
        .filter(|call| {
            matches!(
                call.name.as_str(),
                "read" | "readv" | "pread64" | "preadv" | "recvmsg" | "recvfrom"
            )
        })
        .map(|call| call.line.split_once(',').map_or("", |(first, _)| first))
        .filter(|descriptor| !descriptor.ends_with("/mountinfo>"))
        .collect();
    assert!(
        read.is_empty(),
        "the helper read from another descriptor than the mount table before the ruleset: \
         {read:?}"
    );
    println!(
        "{NAME}: executed, before the ruleset {executed:?}; the helper made {} opens, {listed} \
         of them to list a directory, and no mapping of a file",
        opened.len()
    );
}
