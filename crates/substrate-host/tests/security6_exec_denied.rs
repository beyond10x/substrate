//! Security review, third look, of `story:executable-deny-list`: the refusal before dispatch
//! for a mount of a file-system type outside the list that is an entry of a directory the
//! helper would expand (design 35 § 6 *Through the view's mounts*, D11 step 3 with D30; S8-2),
//! through the real `linux` adapter and the lane's statically linked helper. One staged case,
//! fourteen starts, three questions:
//!
//! - **does the guard refuse a start the helper would have run?** A mount of the other type
//!   that a mount of a listed type covers at its own mount point, one stacked on a second
//!   mount of the listed directory's file system, and one a mount of a listed type covers
//!   from the directory above, with the mount point still there and with it gone;
//! - **is a start still dispatched that the helper ends with status 125?** A deny entry that
//!   is absent two directories down, two deny entries whose expansions meet, a file that is a
//!   mount point, and a directory of the expanded one that is a second mount of a file system
//!   in which a covered mount sits elsewhere;
//! - **is a name a run can create read as a row of the mount table?** A directory whose name
//!   holds a line break and the text of a row, on the way to a mount.
//!
//! The case makes its mounts in a user and mount namespace of its own and runs itself again
//! there. It needs `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this
//! process is inside, bubblewrap and `SUBSTRATE_EXEC_GATE` naming the static helper. Without
//! one of them it prints `<name>: ABSENT (<reason>)` and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    ReadOnlyRoot, RunState, linux::LinuxRequest,
};

/// Names the host root, in the process the case starts inside a mount namespace of its own.
const INNER: &str = "SECURITY6_DENIED_HOST";

/// What the stage of that namespace exits with when one of its commands failed.
const STAGE_FAILED: i32 = 97;

const NAME: &str = "security6 exec denied the refusal for a mount among the entries";

/// A directory name a run can create: a line break, then the text of a row of the table.
const ROW_IN_A_NAME: &str = "o d\n901 1 0:99 x y rw - ramfs ramfs rw";

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

/// What one start came to, on one line: `refused <code>: <text>` for a refusal at admission
/// or before dispatch, `ran <exit code> <standard output> <standard error>` for a run.
async fn shell(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    script: &str,
) -> String {
    let refused = |error: &ConfinementError| format!("refused {}: {error}", error.code());
    let admitted = match adapter.admit(profile) {
        Ok(admitted) => admitted,
        Err(error) => return refused(&error),
    };
    let sh = find("sh").expect("a shell on this host");
    let command = Command {
        argv: vec![text(&sh), "-c".to_owned(), script.to_owned()],
        env: BTreeMap::new(),
    };
    let run = match adapter.start(&admitted, &command).await {
        Ok(run) => run,
        Err(error) => return refused(&error),
    };
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    format!(
        "ran {:?} {:?} {:?}",
        ended.exit.as_ref().and_then(|exit| exit.code),
        String::from_utf8_lossy(&output.stdout)
            .replace('\n', " ")
            .trim_end(),
        String::from_utf8_lossy(&output.stderr).trim_end()
    )
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

/// The observation `what` of the run inside the namespace, from what it printed.
fn observed(stdout: &str, what: &str) -> Option<String> {
    let inside = format!("{NAME} inside: ");
    stdout
        .lines()
        // The harness writes `test <name> ... ` before the first line of a case.
        .filter_map(|line| line.split_once(inside.as_str()).map(|(_, rest)| rest))
        .find_map(|line| line.strip_prefix(&format!("{what}: ")))
        .map(|rest| rest.trim().to_owned())
}

/// What a start must come to.
enum Held {
    /// Dispatched, the shell ended 0 and printed this.
    Runs(&'static str),
    /// Refused before dispatch `exec.sandbox-place-unserved`, the text holding this.
    Refused(&'static str),
    /// Either: refused with the first, or run with the second. Never dispatched and ended by
    /// the helper.
    RefusedOrRuns(&'static str, &'static str),
}

impl Held {
    fn by(&self, answer: &str) -> bool {
        let runs = |printed: &str| answer.starts_with(&format!("ran Some(0) \"{printed}\""));
        let refused = |holding: &str| {
            answer.starts_with("refused exec.sandbox-place-unserved") && answer.contains(holding)
        };
        match self {
            Self::Runs(printed) => runs(printed),
            Self::Refused(holding) => refused(holding),
            Self::RefusedOrRuns(holding, printed) => refused(holding) || runs(printed),
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Runs(printed) => format!("a run that prints {printed:?}"),
            Self::Refused(holding) => {
                format!("a refusal exec.sandbox-place-unserved holding {holding:?}")
            }
            Self::RefusedOrRuns(holding, printed) => format!(
                "a refusal exec.sandbox-place-unserved holding {holding:?}, or a run that \
                 prints {printed:?}"
            ),
        }
    }
}

/// One start of the staged case.
struct Start {
    what: &'static str,
    allow: &'static [&'static str],
    deny: &'static [&'static str],
    script: &'static str,
    held: Held,
}

/// The denied program and its neighbour of one directory.
macro_rules! pair {
    ($directory:literal) => {
        concat!(
            $directory,
            "/tool 2>/dev/null; echo \"tool $?\"; ",
            $directory,
            "/prog; echo \"prog $?\""
        )
    };
}

/// The starts. The root `<host>/tools` is declared at `/opt/k`; the stage is in the case's
/// own comment.
const STARTS: [Start; 14] = [
    Start {
        what: "covered at its mount point",
        allow: &["/opt/k/cov"],
        deny: &["/opt/k/cov/tool"],
        script: pair!("/opt/k/cov"),
        held: Held::Runs("tool 126 prog 0"),
    },
    Start {
        what: "stacked on a second mount",
        allow: &["/opt/k/vol"],
        deny: &["/opt/k/vol/tool"],
        script: pair!("/opt/k/vol"),
        held: Held::Runs("tool 126 prog 0"),
    },
    Start {
        what: "covered from above, mount point kept",
        allow: &["/opt/k/kept"],
        deny: &["/opt/k/kept/odd/tool"],
        script: pair!("/opt/k/kept/odd"),
        held: Held::Runs("tool 126 prog 0"),
    },
    Start {
        what: "covered from above, mount point gone",
        allow: &["/opt/k/gone"],
        deny: &["/opt/k/gone/odd/tool"],
        script: pair!("/opt/k/gone/odd"),
        held: Held::Runs("tool 126 prog 0"),
    },
    Start {
        what: "listed mount point of another type",
        allow: &["/opt/k/odd/media", "/opt/k/bin"],
        deny: &["/opt/k/bin/tool"],
        script: pair!("/opt/k/bin"),
        held: Held::Refused("odd/media: file-system-unlisted ramfs"),
    },
    Start {
        what: "masked file",
        allow: &["/opt/k/mask"],
        deny: &["/opt/k/mask/tool"],
        script: pair!("/opt/k/mask"),
        held: Held::RefusedOrRuns("mask/su: file-system-unlisted", "tool 126 prog 0"),
    },
    Start {
        what: "masked file, deny entry elsewhere",
        allow: &["/opt/k/mask", "/opt/k/bin"],
        deny: &["/opt/k/bin/tool"],
        script: "/opt/k/mask/prog; echo \"prog $?\"; /opt/k/bin/tool 2>/dev/null; echo \"tool $?\"",
        held: Held::Runs("prog 0 tool 126"),
    },
    Start {
        what: "absent two levels down",
        allow: &["/opt/k/odd"],
        deny: &["/opt/k/odd/a/b/later"],
        script: "/opt/k/odd/prog; echo \"prog $?\"",
        held: Held::Refused("odd/media: file-system-unlisted ramfs"),
    },
    Start {
        what: "absent two levels down, listed above",
        allow: &["/opt/k/far"],
        deny: &["/opt/k/far/in/x/y"],
        script: "/opt/k/far/prog; echo \"prog $?\"",
        held: Held::Refused("far/in/media: file-system-unlisted ramfs"),
    },
    Start {
        what: "absent beside the holder",
        allow: &["/opt/k/far"],
        deny: &["/opt/k/far/x/y"],
        script: "/opt/k/far/prog; echo \"prog $?\"; /opt/k/far/in/prog; echo \"in $?\"; \
                 /opt/k/far/in/media/prog; echo \"media $?\"",
        held: Held::Runs("prog 0 in 0 media 0"),
    },
    Start {
        what: "two deny entries meet",
        allow: &["/opt/k/far", "/opt/k/bin"],
        deny: &["/opt/k/far/tool", "/opt/k/far/in/later"],
        script: "/opt/k/far/prog; echo \"prog $?\"",
        held: Held::Refused("far/in/media: file-system-unlisted ramfs"),
    },
    Start {
        what: "listed twice, one expansion",
        allow: &["/opt/k/far/in", "/opt/k/far"],
        deny: &["/opt/k/far/tool"],
        script: "/opt/k/far/tool 2>/dev/null; echo \"tool $?\"; /opt/k/far/in/prog; \
                 echo \"in $?\"; /opt/k/far/in/media/prog; echo \"media $?\"",
        held: Held::Runs("tool 126 in 0 media 0"),
    },
    Start {
        what: "second mount of a reached file system",
        allow: &["/opt/k/a"],
        deny: &["/opt/k/a/tool"],
        script: pair!("/opt/k/a"),
        held: Held::RefusedOrRuns("", "tool 126 prog 0"),
    },
    Start {
        what: "a row in a name",
        allow: &["/opt/k/names"],
        deny: &["/opt/k/names/tool"],
        script: "/opt/k/names/tool 2>/dev/null; echo \"tool $?\"; /opt/k/names/prog; \
                 echo \"prog $?\"; for p in /opt/k/names/*/prog; do \"$p\"; echo \"inner $?\"; \
                 done; for p in /opt/k/names/*/vol/prog; do \"$p\"; echo \"vol $?\"; done",
        held: Held::Runs("tool 126 prog 0 inner 0 vol 0"),
    },
];

/// The staged case inside its namespace.
async fn inside(host: &Path) {
    let adapter = match open(&host.join("workspaces")) {
        Ok(adapter) => adapter,
        Err(reason) => return println!("{NAME} inside: ABSENT: ({reason})"),
    };
    let workspace = host.join("workspaces/ws_security6");
    std::fs::create_dir_all(&workspace).expect("the workspace");
    for start in &STARTS {
        let owned = |list: &[&str]| list.iter().map(|entry| (*entry).to_owned()).collect();
        let profile = ConfinementProfile {
            workspace: Some(workspace.clone()),
            exec_subpaths: owned(start.allow),
            exec_denied_paths: owned(start.deny),
            read_only_roots: vec![ReadOnlyRoot {
                host_path: text(&host.join("tools")),
                mount: "/opt/k".to_owned(),
            }],
            ..ConfinementProfile::default()
        };
        let began = Instant::now();
        let answer = shell(&adapter, &profile, start.script).await;
        println!("{NAME} inside: {}: {answer}", start.what);
        println!(
            "{NAME} inside: time of {}: {:?}",
            start.what,
            began.elapsed()
        );
    }
}

/// **The refusal before dispatch for a mount among the entries of an expanded directory
/// refuses what the helper would end, and no more** (design 35 § 6, the row amended
/// 2026-11-22; D11 step 3 with D30; S8-2).
///
/// A declared read-only root, `<host>/tools` at `/opt/k`, holds copies of the host's `true`
/// as `tool` and `prog` in each directory named below. The stage mounts:
///
/// - `cov/media`: a ramfs, and a tmpfs on top of it;
/// - `vol`: a tmpfs, bound a second time at `again`, and a ramfs on top of `again`;
/// - `kept/odd/media` and `gone/odd/media`: a ramfs each, then a tmpfs over `kept/odd` and
///   over `gone/odd`. The tmpfs over `kept/odd` has a directory `media`, the other has none;
/// - `odd/media` and `far/in/media`: a ramfs each;
/// - `a/back`: a bind of `b`, not recursive; then two tmpfs on `b/sub`, one on the other;
/// - `mask/su`: a bind of `/dev/null`, as an operator masks a program;
/// - `names/<a name with a line break and the text of a row>/vol`: a tmpfs.
///
/// What each start must come to:
///
/// - `covered at its mount point`, `stacked on a second mount`, `covered from above, mount
///   point kept` and `covered from above, mount point gone`: the helper never meets the ramfs
///   as an entry, so the start runs: the denied program 126, its neighbour 0;
/// - `listed mount point of another type`: the listed entry itself has no place (D26):
///   refused;
/// - `masked file`: the deny entry lies beside the masked file, so the helper would meet a
///   mount of `devtmpfs` among the entries: refused with the mount point, or run where this
///   host keeps `/dev/null` on a listed type. With the deny entry elsewhere it runs;
/// - `absent two levels down`, alone and with the listed directory above the holder, and
///   `two deny entries meet`: the directory that holds the ramfs is on the way to a deny
///   entry: refused, naming the mount point. `absent beside the holder` and `listed twice,
///   one expansion` leave that directory one rule: they run;
/// - `second mount of a reached file system`: the helper expands `a`, asks what `a/back`
///   reaches, and that is the place of `b`, beneath which the covered tmpfs sits. Refused
///   before dispatch or run, never dispatched and ended by the helper;
/// - `a row in a name`: the name is no row. The start runs.
// One case: the stage, and one line per start.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn security6_the_refusal_for_a_mount_among_the_entries_is_what_the_helper_would_end() {
    const CASE: &str =
        "security6_the_refusal_for_a_mount_among_the_entries_is_what_the_helper_would_end";
    if let Some(host) = std::env::var_os(INNER) {
        inside(Path::new(&host)).await;
        return;
    }
    let started = Instant::now();
    let (Some(mount), Some(cp), Some(mkdir), Some(program)) =
        (find("mount"), find("cp"), find("mkdir"), find("true"))
    else {
        return println!("{NAME}: ABSENT (no mount, cp, mkdir or true on this host)");
    };
    let directory = tempfile::tempdir().expect("host root");
    let host = directory
        .path()
        .canonicalize()
        .expect("canonical host root");
    let tools = host.join("tools");
    let row = format!("names/{ROW_IN_A_NAME}");
    for name in [
        "bin",
        "cov/media",
        "vol",
        "again",
        "kept/odd/media",
        "gone/odd/media",
        "odd/media",
        "far/in/media",
        "a/back",
        "b/sub",
        "mask",
        &format!("{row}/vol"),
    ] {
        std::fs::create_dir_all(tools.join(name)).expect(name);
    }
    for directory in ["bin", "cov", "odd", "far", "far/in", "a", "mask", "names"] {
        for name in ["tool", "prog"] {
            std::fs::copy(&program, tools.join(directory).join(name)).expect("copy true");
        }
    }
    std::fs::copy(&program, tools.join(&row).join("prog")).expect("copy true");
    std::fs::copy(&program, tools.join("mask/su")).expect("copy true");
    let stage = format!(
        "{mount} -t ramfs ramfs \"$1/cov/media\" && {mount} -t tmpfs tmpfs \"$1/cov/media\" \
         && {mount} -t tmpfs tmpfs \"$1/vol\" && {cp} \"$1/bin/tool\" \"$1/bin/prog\" \"$1/vol/\" \
         && {mount} --bind \"$1/vol\" \"$1/again\" && {mount} -t ramfs ramfs \"$1/again\" \
         && for d in kept gone; do \
              {mount} -t ramfs ramfs \"$1/$d/odd/media\" && {mount} -t tmpfs tmpfs \"$1/$d/odd\" \
              && {cp} \"$1/bin/tool\" \"$1/bin/prog\" \"$1/$d/odd/\" || exit {STAGE_FAILED}; \
            done \
         && {mkdir} \"$1/kept/odd/media\" \
         && {mount} -t ramfs ramfs \"$1/odd/media\" \
         && {mount} -t ramfs ramfs \"$1/far/in/media\" \
         && {cp} \"$1/bin/prog\" \"$1/far/in/media/prog\" \
         && {mount} --bind \"$1/b\" \"$1/a/back\" \
         && {mount} -t tmpfs tmpfs \"$1/b/sub\" && {mount} -t tmpfs tmpfs \"$1/b/sub\" \
         && {mount} --bind /dev/null \"$1/mask/su\" \
         && for d in \"$1\"/names/*/; do \
              {mount} -t tmpfs tmpfs \"${{d}}vol\" && {cp} \"$1/bin/prog\" \"${{d}}vol/prog\" \
              || exit {STAGE_FAILED}; \
            done",
        mount = mount.display(),
        cp = cp.display(),
        mkdir = mkdir.display()
    );
    let stdout = match staged(CASE, &host, &stage, &tools) {
        Ok(stdout) => stdout,
        Err(reason) => return println!("{NAME}: ABSENT ({reason})"),
    };
    if let Some(reason) = observed(&stdout, "ABSENT") {
        return println!("{NAME}: ABSENT (inside the namespace: {reason})");
    }
    let mut failures = Vec::new();
    for start in &STARTS {
        let answer = observed(&stdout, start.what)
            .unwrap_or_else(|| panic!("no `{}`: {stdout}", start.what));
        let time = observed(&stdout, &format!("time of {}", start.what)).unwrap_or_default();
        println!("{NAME}: {}: {answer} ({time})", start.what);
        if !start.held.by(&answer) {
            failures.push(format!(
                "{}: allow {:?}, deny {:?}: {answer}, not {}",
                start.what,
                start.allow,
                start.deny,
                start.held.text()
            ));
        }
    }
    println!("{NAME}: executed in {:?}", started.elapsed());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
