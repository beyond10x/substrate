//! Security review, third look: design 35 D29 (`docs/design/35-executable-lists.md`, § 6 *Before
//! the ruleset*) as an invariant of the `linux` adapter, through the public confinement API and
//! the lane's statically linked helper.
//!
//! > On a start with a list, before `landlock_restrict_self` returns, the processes in the view
//! > are bubblewrap's own and the static helper; [...] nothing the run or the profile names is
//! > loaded as code.
//!
//! The design lists what that sentence rests on and did not run (§ 18). These cases run the
//! parts a workstation can settle:
//!
//! - **a system-call trace of one listed start**, taken by the driver's own spawn of bubblewrap
//!   under `strace -f`: which programs are executed before the ruleset, what the helper opens
//!   and maps before it, and that bubblewrap executes the helper's path once, without a search;
//! - **a binary-format handler**: a run cannot mount `binfmt_misc`, write its `register` file or
//!   make a user namespace to do either in;
//! - **an image the kernel does not start**: the open's byte test is about a loader, and an ELF
//!   image of a type the kernel refuses has none. Bubblewrap starts a program with `execvp`,
//!   which hands such a file to `/bin/sh`. The open must still refuse it.
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
    ReadOnlyFile, RunState, linux::LinuxRequest,
};

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

/// The lane's helper: the statically linked build `SUBSTRATE_EXEC_GATE` names.
fn helper() -> PathBuf {
    PathBuf::from(
        std::env::var_os("SUBSTRATE_EXEC_GATE")
            .expect("SUBSTRATE_EXEC_GATE is unset: the lane names the helper it built"),
    )
}

struct Lane {
    host: tempfile::TempDir,
    adapter: Arc<dyn ConfinementAdapter>,
    workspace: PathBuf,
}

/// The adapter opened with the lane's helper and `bubblewrap` as its sandbox program, having
/// proved `exec_subpaths`, and a workspace with `bin/tool` and `other/tool`, both copies of the
/// host's `true`. `prepare` is given the host root before the adapter opens and may name another
/// sandbox program.
fn lane(name: &str, prepare: impl FnOnce(&Path) -> Option<PathBuf>) -> Option<Lane> {
    let cgroup_root = delegated(name)?;
    let host = tempfile::tempdir().expect("host root");
    let mut config = HostConfig::minimum(host.path().join("workspaces"));
    config.exec_gate = Some(helper());
    if let Some(bubblewrap) = prepare(host.path()) {
        config.bubblewrap = bubblewrap;
    }
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .expect("linux opens");
    assert!(
        adapter.facts().proves(&Fact::ExecSubpaths),
        "{name}: linux opened with the helper did not prove exec_subpaths: {:?}",
        adapter.facts()
    );
    let workspace = host.path().join("workspaces").join("ws_security3");
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

/// Builds the C `source` as `name` in `directory` with the host's compiler; `false` after
/// printing why it could not.
fn compiled(case: &str, directory: &Path, name: &str, flags: &[&str], source: &str) -> bool {
    let file = directory.join(format!("{name}.c"));
    std::fs::write(&file, source).expect("the source");
    let built = std::process::Command::new("cc")
        .args(flags)
        .arg("-o")
        .arg(directory.join(name))
        .arg(&file)
        .output();
    match built {
        Ok(output) if output.status.success() => true,
        other => {
            println!("{case}: ABSENT (the host's cc built nothing: {other:?})");
            false
        }
    }
}

/// A shared object that appends the `argv[0]` of every process that loads it to
/// `/workspace/loaded.log`.
const OBJECT: &str = r#"
#include <fcntl.h>
#include <string.h>
#include <unistd.h>

static void __attribute__((constructor)) start(int argc, char **argv) {
    int log = open("/workspace/loaded.log", O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
    if (log >= 0 && argc > 0) {
        if (write(log, argv[0], strlen(argv[0])) < 0 || write(log, "\n", 1) < 0) {
            /* The log is evidence only. */
        }
        close(log);
    }
}
"#;

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

/// What a traced listed start did before its ruleset, read from the trace.
struct Before {
    /// Every `execve` attempted before the ruleset, as `path` and whether it succeeded.
    executed: Vec<(String, bool)>,
    /// The line of the `execve` that started the helper.
    helper_started: String,
    /// What the helper's process opened between its start and the ruleset.
    helper_opened: Vec<String>,
    /// The lines of the helper's `mmap` calls in that span.
    helper_mapped: Vec<String>,
    /// The descriptor of every read the helper made in that span, as `strace -y` names it.
    helper_read: Vec<String>,
    /// What bubblewrap's processes opened inside the view: after the last `pivot_root`.
    bubblewrap_opened: Vec<String>,
    /// The lines of bubblewrap's `mmap` calls inside the view.
    bubblewrap_mapped: Vec<String>,
    /// What the helper's process, by then the run's program, opened after the ruleset.
    program_opened: Vec<String>,
}

/// Splits a trace at the first successful `landlock_restrict_self`. Panics, naming what is
/// missing, when the trace is not that of a listed start.
fn before_the_ruleset(calls: &[Call]) -> Before {
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
    let in_view = calls[..started]
        .iter()
        .rposition(|call| call.name == "pivot_root" && call.succeeded)
        .expect("bubblewrap changed its root before it started the helper");
    let opens = |call: &&Call| matches!(call.name.as_str(), "open" | "openat" | "openat2");
    let of_helper = || {
        calls[started + 1..ruleset]
            .iter()
            .filter(|c| c.pid == helper)
    };
    let of_bubblewrap = || calls[in_view + 1..started].iter();
    Before {
        executed: calls[..ruleset]
            .iter()
            .filter(|call| matches!(call.name.as_str(), "execve" | "execveat"))
            .map(|call| (call.path().to_owned(), call.succeeded))
            .collect(),
        helper_started: calls[started].line.clone(),
        helper_opened: of_helper()
            .filter(opens)
            .map(|call| call.line.clone())
            .collect(),
        helper_mapped: of_helper()
            .filter(|call| call.name == "mmap")
            .map(|call| call.line.clone())
            .collect(),
        helper_read: of_helper()
            .filter(|call| {
                matches!(
                    call.name.as_str(),
                    "read" | "readv" | "pread64" | "preadv" | "recvmsg" | "recvfrom"
                )
            })
            .map(|call| {
                let descriptor = call.line.split_once(',').map_or("", |(first, _)| first);
                descriptor.to_owned()
            })
            .collect(),
        bubblewrap_opened: of_bubblewrap()
            .filter(opens)
            .map(|call| call.line.clone())
            .collect(),
        bubblewrap_mapped: of_bubblewrap()
            .filter(|call| call.name == "mmap")
            .map(|call| call.line.clone())
            .collect(),
        program_opened: calls[ruleset..]
            .iter()
            .filter(|call| call.pid == helper)
            .filter(opens)
            .filter(|call| call.succeeded)
            .map(|call| call.path().to_owned())
            .collect(),
    }
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
             -e trace=execve,execveat,open,openat,openat2,mmap,pivot_root,landlock_restrict_self,\
             read,readv,pread64,preadv,recvmsg,recvfrom \
             /usr/bin/bwrap \"$@\"\n",
            traces.display()
        ),
    )
    .expect("the script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("mode");
    script
}

/// **Before the ruleset, a listed start executes bubblewrap and the helper and nothing else,
/// and the helper maps no file** (D29, the sentence of § 6 and four of the lines it rests on).
///
/// The driver's own start of one run is traced: its sandbox program is bubblewrap under
/// `strace -f`. The profile declares a read-only file at `/etc/ld.so.preload` that names an
/// object of the workspace, and the command names the same object in `LD_PRELOAD`, so a loader
/// anywhere before the ruleset would show as an `openat` of that file or an `mmap` of the object.
/// Read from the trace, up to the first successful `landlock_restrict_self`:
///
/// - the `execve` calls are bubblewrap and the helper's mount, each once and each successful: no
///   stage between them, and no second attempt that a search or a fallback would make;
/// - the helper is started with one variable, maps anonymous memory only, and opens one file
///   for its contents, `/proc/self/mountinfo`, the only descriptor it reads from; every other
///   open is `O_PATH`;
/// - bubblewrap, once its root is the view, maps no file and opens entries of `/proc` only.
///
/// And after it, the trap was armed: the run's program opened `/etc/ld.so.preload` and the
/// object, and the list held.
// One case on purpose: one traced start, read once.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn security3_before_the_ruleset_a_listed_start_executes_bubblewrap_and_the_helper_only() {
    const NAME: &str = "security3 exec gate trace";
    const MARKER: &str = "s3-marker-7d41";
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
    if !compiled(
        NAME,
        &lane.workspace,
        "pre.so",
        &["-shared", "-fPIC"],
        OBJECT,
    ) {
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
    let script =
        format!(": {MARKER}; bin/tool; echo listed:$?; other/tool 2>/dev/null; echo unlisted:$?");
    let command = Command {
        argv: vec![text(&tool("sh")), "-c".into(), script],
        env: BTreeMap::from([("LD_PRELOAD".to_owned(), "/workspace/pre.so".to_owned())]),
    };
    let (code, stdout, stderr) = run(&lane.adapter, &declared, &command)
        .await
        .expect("the start is admitted and dispatched");
    assert_eq!(
        stdout, "listed:0\nunlisted:126\n",
        "the list did not hold under the trace (exit {code:?}, stderr {stderr:?})"
    );

    let of_the_run: Vec<String> = std::fs::read_dir(&traces)
        .expect("the trace directory")
        .map(|entry| std::fs::read(entry.expect("an entry").path()).expect("a trace"))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .filter(|trace| trace.contains(MARKER))
        .collect();
    assert_eq!(of_the_run.len(), 1, "one start carried the marker");
    if let Some(keep) = std::env::var_os("SECURITY3_TRACE_OUT") {
        std::fs::write(keep, &of_the_run[0]).expect("the trace is kept");
    }
    let before = before_the_ruleset(&calls(&of_the_run[0]));

    assert_eq!(
        before.executed,
        [("/usr/bin/bwrap".to_owned(), true), (GATE.to_owned(), true)],
        "the programs executed before the ruleset"
    );
    assert!(
        before.helper_started.contains("/* 1 var */"),
        "the helper was started with another environment than PWD alone: {}",
        before.helper_started
    );
    // The helper reads the contents of one file, the view's mount table. Every other open is
    // `O_PATH`: a place to compare or to write a rule for, whose bytes cannot be read from it.
    let read: Vec<&String> = before
        .helper_opened
        .iter()
        .filter(|line| !line.contains("O_PATH"))
        .collect();
    assert!(
        read.len() == 1 && read[0].contains("\"/proc/self/mountinfo\""),
        "the helper opened another file than the mount table for its contents before the \
         ruleset: {:#?}",
        before.helper_opened
    );
    // And it reads from that file alone: none of the descriptors it inherits, which are the
    // run's standard streams, its secret slots and the start's own.
    assert!(
        !before.helper_read.is_empty()
            && before
                .helper_read
                .iter()
                .all(|descriptor| descriptor.ends_with("/mountinfo>")),
        "the helper read from another descriptor than the mount table before the ruleset: {:?}",
        before.helper_read
    );
    // Bubblewrap, once its root is the view, opens entries of `/proc` and nothing of the view.
    assert!(
        before
            .bubblewrap_opened
            .iter()
            .all(|line| line.contains("</proc")),
        "bubblewrap opened a file of the view before it started the helper: {:#?}",
        before.bubblewrap_opened
    );
    for (who, mapped) in [
        ("the helper", &before.helper_mapped),
        ("bubblewrap in the view", &before.bubblewrap_mapped),
    ] {
        assert!(
            mapped
                .iter()
                .all(|line| line.contains("MAP_ANONYMOUS") && !line.contains("PROT_EXEC")),
            "{who} mapped a file, or memory as code, before the ruleset: {mapped:#?}"
        );
    }
    for armed in ["/etc/ld.so.preload", "/workspace/pre.so"] {
        assert!(
            before.program_opened.iter().any(|path| path == armed),
            "the run's program did not open {armed} after the ruleset, so the case shows \
             nothing: {:?}",
            before.program_opened
        );
    }
    println!(
        "{NAME}: executed, executed before the ruleset {:?}; the helper opened {:#?}, read {} \
         times and from the mount table only, and made {} anonymous mappings; bubblewrap in the \
         view opened {:#?} and made {} mappings",
        before.executed,
        before.helper_opened,
        before.helper_read.len(),
        before.helper_mapped.len(),
        before.bubblewrap_opened,
        before.bubblewrap_mapped.len()
    );
}

/// A program that tries each way a process could give the kernel a binary-format handler, and
/// prints the error number of each try, 0 for one that worked.
const HANDLER: &str = r#"
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <stdio.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>

static int mounted(const char *at) {
    return mount("binfmt_misc", at, "binfmt_misc", 0, NULL) == 0 ? 0 : errno;
}

int main(void) {
    int file = open("/proc/sys/fs/binfmt_misc/register", O_WRONLY);
    printf("register:%d\n", file < 0 ? errno : 0);
    mkdir("/workspace/handlers", 0755);
    printf("mount:%d\n", mounted("/workspace/handlers"));
    printf("mount-proc:%d\n", mounted("/proc/sys/fs/binfmt_misc"));
    if (unshare(CLONE_NEWUSER | CLONE_NEWNS) != 0) {
        printf("namespace:%d\n", errno);
    } else {
        printf("namespace:0\nnamespace-mount:%d\n", mounted("/workspace/handlers"));
    }
    return 0;
}
"#;

/// **A run cannot register a binary-format handler** (D29, § 6: "A host can register a handler
/// (`binfmt_misc`) that claims a file by its first bytes and starts an interpreter of the
/// host's choosing"). Since Linux 6.7 a user namespace can hold a `binfmt_misc` instance of its
/// own, so the question is real for a process inside the view. A program of the workspace
/// opens the `register` file for writing, mounts the file system on a directory of its own and
/// on `/proc`, and makes a user namespace to mount it in. Each try fails, in a start without a
/// list, which may execute anything in the view, and in a start with one.
#[tokio::test(flavor = "multi_thread")]
async fn security3_a_run_cannot_register_a_binary_format_handler() {
    const NAME: &str = "security3 exec gate binfmt";
    let Some(lane) = lane(NAME, |_| None) else {
        return;
    };
    if !compiled(NAME, &lane.workspace.join("bin"), "handler", &[], HANDLER) {
        return;
    }
    let command = Command {
        argv: vec!["/workspace/bin/handler".to_owned()],
        env: BTreeMap::new(),
    };
    let mut seen = Vec::new();
    for (start, entries) in [("unlisted", &[][..]), ("listed", &["bin"][..])] {
        let (code, stdout, stderr) =
            run(&lane.adapter, &profile(&lane.workspace, entries), &command)
                .await
                .unwrap_or_else(|refusal| panic!("{start}: {refusal}"));
        assert_eq!(code, Some(0), "{start}: {stdout:?} {stderr:?}");
        let tries: Vec<(&str, &str)> = stdout
            .lines()
            .filter_map(|line| line.split_once(':'))
            .collect();
        assert!(
            tries.len() == 4 && tries.iter().all(|(_, error)| *error != "0"),
            "{start}: a try to register a handler did not fail (name:errno, 0 is success): \
             {tries:?}"
        );
        seen.push(format!("{start} {tries:?}"));
    }
    println!("{NAME}: executed, every try failed: {seen:?}");
}

/// **An image the kernel does not start is not taken as the helper** (D29, rule 1: "what
/// starts first there is another program, or is not known").
///
/// The image is the lane's static helper with its ELF type changed from an executable's to a
/// core file's. It is of the driver's class, byte order and machine, its program header table
/// reads whole, and it has no `PT_INTERP`, so the three tests rule 1 had when this case was
/// written passed it. The kernel
/// refuses to execute it (`ENOEXEC`), and bubblewrap starts programs with `execvp`, which then
/// runs `/bin/sh` with the file as its script: a dynamically linked program of the host, before
/// any ruleset. So the open must refuse the file. The byte test now asks the type as well and
/// refuses it there, `config.exec-gate-invalid`, one step before the protocol step that
/// refused it `config.exec-gate-unserved` before.
#[tokio::test(flavor = "multi_thread")]
async fn security3_an_image_the_kernel_does_not_start_is_refused_at_the_open() {
    const NAME: &str = "security3 exec gate unstartable image";
    const ET_CORE: u16 = 4;
    let Some(cgroup_root) = delegated(NAME) else {
        return;
    };
    let mut image = std::fs::read(helper()).expect("the lane's helper");
    assert_eq!(
        substrate_exec_gate::elf_linkage(&image),
        substrate_exec_gate::Linkage::Static,
        "the lane's helper is statically linked"
    );
    image[16..18].copy_from_slice(&ET_CORE.to_le_bytes());
    let linkage = substrate_exec_gate::elf_linkage(&image);
    let host = tempfile::tempdir().expect("host root");
    let install = host.path().join("install");
    std::fs::create_dir_all(&install).expect("install");
    let file = install.join("substrate-exec-gate");
    std::fs::write(&file, &image).expect("the image");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let mut config = HostConfig::minimum(host.path().join("workspaces"));
    config.exec_gate = Some(file);
    let opened = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )));
    match opened {
        Ok(adapter) => panic!(
            "the open took an image the kernel does not start as the helper (the byte test \
             called it {linkage:?}); exec_subpaths proved: {}",
            adapter.facts().proves(&Fact::ExecSubpaths)
        ),
        Err(error) => {
            // The byte test refuses it now, one step before the protocol step that refused it
            // `config.exec-gate-unserved` when this case was written.
            assert_eq!(linkage, substrate_exec_gate::Linkage::Unstartable);
            assert_eq!(
                (error.code(), error.status()),
                ("config.exec-gate-invalid", 422),
                "{error}"
            );
            println!(
                "{NAME}: executed, the byte test called the image {linkage:?} and the open \
                 refused it {} ({}): {error}",
                error.code(),
                error.status()
            );
        }
    }
}
