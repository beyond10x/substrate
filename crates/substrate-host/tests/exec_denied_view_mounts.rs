//! Design 35 D30, *Through the view's mounts*, for the deny-list of
//! `story:executable-deny-list`, through the public confinement API of the `linux` adapter:
//! **a deny entry holds across the mounts of the view.**
//!
//! A host mount beneath a declared read-only root is carried into a run's view by the
//! recursive bind of the root. Its files are beneath the root in the view, and by place they
//! are on another file system, or on the root's own under other names. The one case here
//! declares a root that holds five such mounts, made in a user and mount namespace of its
//! own, and opens the real adapter there with the lane's statically linked helper:
//!
//! - a second directory of the root's file system bound at `held/sub`, and a tmpfs at
//!   `deep/in`: a program denied in either is not executed although the directory above it
//!   is listed, a listed mount beneath a denied directory is not executed, and a start
//!   whose program lies in a mount beneath a denied directory is refused before dispatch;
//! - a ramfs at `odd/media`, a file-system type outside the list: beneath a denied directory
//!   the start is refused, beneath a listed directory it is compared with nothing;
//! - two tmpfs stacked on `stack/m`: what a denied directory reaches, and what a listed
//!   directory of a start with a deny-list reaches, cannot be established, and the start is
//!   refused before dispatch. With an allow-list alone the same start runs.
//!
//! The case needs `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this
//! process is inside, bubblewrap, `SUBSTRATE_EXEC_GATE` naming the static helper, and a host
//! whose user namespaces may mount. Without one of them it prints
//! `<name>: ABSENT (<reason>)` and is absent, never passed.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, Fact,
    ReadOnlyRoot, RunState, linux::LinuxRequest,
};

const NAME: &str = "ESS confinement linux exec deny view mounts";

/// The one case of this file, as the harness names it.
const CASE: &str = "a_deny_entry_holds_across_the_mounts_of_the_view";

/// Names the host root, in the process this file starts inside a mount namespace of its own.
const INNER: &str = "SUBSTRATE_DENY_VIEW_MOUNTS_HOST";

/// What the stage of that namespace exits with when a mount failed.
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

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8").to_owned()
}

/// One start under a profile that declares `tools` at `/opt/t`: `argv` with the lists
/// `allow` and `deny`. What the run printed, or the refusal with its code and its address.
async fn run(
    adapter: &Arc<dyn ConfinementAdapter>,
    host: &Path,
    allow: &[&str],
    deny: &[&str],
    argv: &[&str],
) -> String {
    let owned = |list: &[&str]| list.iter().map(|entry| (*entry).to_owned()).collect();
    let profile = ConfinementProfile {
        workspace: Some(host.join("workspaces/ws_exec")),
        read_only_roots: vec![ReadOnlyRoot {
            host_path: text(&host.join("tools")),
            mount: "/opt/t".to_owned(),
        }],
        exec_subpaths: owned(allow),
        exec_denied_paths: owned(deny),
        ..ConfinementProfile::default()
    };
    let command = Command {
        argv: owned(argv),
        env: BTreeMap::new(),
    };
    let refused = |error: ConfinementError| {
        let address = match &error {
            ConfinementError::Driver(driver) => driver.address.clone().unwrap_or_default(),
            _ => String::new(),
        };
        format!("refused {} at {address}: {error}", error.code())
    };
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
    format!("ran {:?}", String::from_utf8_lossy(&output.stdout))
}

/// The adapter opened inside the namespace, and the host root it was opened on.
struct Inside<'a> {
    adapter: &'a Arc<dyn ConfinementAdapter>,
    host: &'a Path,
}

impl Inside<'_> {
    /// One start, printed as one line: `what`, then what the run printed or the refusal.
    async fn observe(&self, what: &str, allow: &[&str], deny: &[&str], argv: &[String]) {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        println!(
            "{NAME} inside: {what} {}",
            run(self.adapter, self.host, allow, deny, &argv).await
        );
    }
}

/// The case inside its namespace, where the five mounts beneath `<host>/tools` exist. Prints
/// one line per observation.
async fn inside(host: &Path) {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("{NAME} inside: ABSENT ({reason})");
            return;
        }
    };
    let mut config = HostConfig::minimum(host.join("workspaces"));
    config.exec_gate = Some(PathBuf::from(
        std::env::var_os("SUBSTRATE_EXEC_GATE").expect(
            "SUBSTRATE_EXEC_GATE is unset: `bash scripts/delegated-lane.sh` builds \
         substrate-exec-gate and names it",
        ),
    ));
    let opened = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )));
    let adapter = match opened {
        Ok(adapter) if adapter.facts().proves(&Fact::ExecDeniedPaths) => adapter,
        Ok(adapter) => {
            println!(
                "{NAME} inside: ABSENT (the adapter did not prove exec_denied_paths: {:?})",
                adapter.facts()
            );
            return;
        }
        Err(error) => {
            println!("{NAME} inside: ABSENT (linux did not open: {error})");
            return;
        }
    };
    std::fs::create_dir_all(host.join("workspaces/ws_exec")).expect("the workspace");
    let shell = text(&find("sh").expect("a shell"));
    let script = |script: &str| [shell.clone(), "-c".to_owned(), script.to_owned()];
    let seen = Inside {
        adapter: &adapter,
        host,
    };
    let programs = script(
        "/opt/t/held/sub/prog 2>/dev/null; echo bind:$?; /opt/t/held/sub/other; echo beside:$?; \
         /opt/t/deep/in/prog 2>/dev/null; echo tmpfs:$?; /opt/t/deep/in/other; echo beside:$?",
    );
    let held = ["/opt/t/held", "/opt/t/deep", "/opt/t/bin"];
    // An allow-list alone, on a root that holds every mount of the case: unchanged by D30.
    seen.observe("allow-list", &["/opt/t"], &[], &programs)
        .await;
    // E20 for a declared root: a program denied in a mount beneath a listed directory.
    seen.observe(
        "denied",
        &held,
        &["/opt/t/held/sub/prog", "/opt/t/deep/in/prog"],
        &programs,
    )
    .await;
    // E19 through the driver: a listed mount beneath a denied directory.
    let beneath_denied = script(
        "/opt/t/held/sub/other 2>/dev/null; echo held:$?; \
         /opt/t/deep/in/other 2>/dev/null; echo deep:$?; /opt/t/bin/tool; echo beside:$?",
    );
    seen.observe(
        "listed-mount",
        &["/opt/t/held/sub", "/opt/t/deep/in", "/opt/t/bin"],
        &["/opt/t/held", "/opt/t/deep"],
        &beneath_denied,
    )
    .await;
    // The program itself in a mount beneath a denied directory, and its control.
    let program = ["/opt/t/held/sub/prog".to_owned()];
    seen.observe("program-control", &["/opt/t/held/sub"], &[], &program)
        .await;
    seen.observe("program", &["/opt/t/held/sub"], &["/opt/t/held"], &program)
        .await;
    // A type outside the list: beneath a denied directory, and beneath a listed one.
    let tools =
        script("/opt/t/bin/tool 2>/dev/null; echo denied:$?; /opt/t/bin/other; echo beside:$?");
    seen.observe("unlisted-denied", &["/opt/t/bin"], &["/opt/t/odd"], &tools)
        .await;
    seen.observe(
        "unlisted-listed",
        &["/opt/t/odd", "/opt/t/bin"],
        &["/opt/t/bin/tool"],
        &tools,
    )
    .await;
    // A stack: beneath a denied directory, beneath a listed directory of a start with a
    // deny-list, and beneath a listed directory of a start without one.
    seen.observe("covered-denied", &["/opt/t/bin"], &["/opt/t/stack"], &tools)
        .await;
    seen.observe(
        "covered-listed",
        &["/opt/t/stack", "/opt/t/bin"],
        &["/opt/t/bin/tool"],
        &tools,
    )
    .await;
    seen.observe(
        "covered-alone",
        &["/opt/t/stack", "/opt/t/bin"],
        &[],
        &tools,
    )
    .await;
}

/// What one observation of the case must be.
enum Expected {
    /// The start was dispatched and the run printed this.
    Ran(&'static str),
    /// The start was refused before dispatch: the code, the address, and what the text holds.
    Refused(&'static str, &'static str, &'static str),
}

impl Expected {
    fn holds(&self, seen: &str) -> bool {
        match self {
            Self::Ran(printed) => seen == format!("ran {printed:?}"),
            Self::Refused(code, address, cause) => {
                seen.starts_with(&format!("refused {code} at {address}: ")) && seen.contains(cause)
            }
        }
    }
}

/// Every observation of the case: its name, what it must be, and what that means.
const EXPECTED: [(&str, Expected, &str); 10] = [
    (
        "allow-list",
        Expected::Ran("bind:0\nbeside:0\ntmpfs:0\nbeside:0\n"),
        "every program of the root's mounts to run",
    ),
    (
        "denied",
        Expected::Ran("bind:126\nbeside:0\ntmpfs:126\nbeside:0\n"),
        "each denied program refused in its mount beneath a listed directory",
    ),
    (
        "listed-mount",
        Expected::Ran("held:126\ndeep:126\nbeside:0\n"),
        "no program of a listed mount beneath a denied directory to run",
    ),
    ("program-control", Expected::Ran(""), "the program to run"),
    (
        "program",
        Expected::Refused(
            "exec.sandbox-program-unlisted",
            "argv",
            "is denied by the deny-list entry \"/opt/t/held\"",
        ),
        "the start refused before dispatch for its denied program",
    ),
    (
        "unlisted-denied",
        Expected::Refused(
            "exec.sandbox-place-unserved",
            "exec_denied_paths",
            "tools/odd/media is attached: file-system-unlisted ramfs",
        ),
        "the start refused for the ramfs beneath the denied directory",
    ),
    (
        "unlisted-listed",
        Expected::Ran("denied:126\nbeside:0\n"),
        "a ramfs beneath a listed directory to refuse nothing",
    ),
    (
        "covered-denied",
        Expected::Refused(
            "exec.sandbox-place-unserved",
            "exec_denied_paths",
            "tools/stack/m is attached: mount-covered",
        ),
        "the start refused for the stack beneath the denied directory",
    ),
    (
        "covered-listed",
        Expected::Refused(
            "exec.sandbox-place-unserved",
            "exec_subpaths",
            "tools/stack/m is attached: mount-covered",
        ),
        "the start refused for the stack beneath a listed directory of a start with a deny-list",
    ),
    (
        "covered-alone",
        Expected::Ran("denied:0\nbeside:0\n"),
        "an allow-list alone to run over the same stack",
    ),
];

/// **A deny entry holds across the mounts of the view, and fails closed where a mount
/// cannot be compared** (design 35 D30, § 8.2, § 16).
///
/// - `allow-list`: the root listed whole and no deny entry. Every program of its mounts
///   runs: an allow-list alone is as it was.
/// - `denied`: `held`, `deep` and `bin` listed, and `held/sub/prog` and `deep/in/prog`
///   denied, each in a mount beneath its listed directory. Both end with status 126 and
///   their neighbours run. The start is dispatched: the mounts are of listed types, and a
///   refusal would be no answer.
/// - `listed-mount`: `held` and `deep` denied, and the mounts beneath them listed. No
///   program of either mount runs, and a listed program beside them does.
/// - `program`: the program is `held/sub/prog` and `held` is denied. The start is refused
///   `exec.sandbox-program-unlisted`, address `argv`, before dispatch. Its control, without
///   the deny entry, runs.
/// - `unlisted-denied`: `odd` is denied and holds a ramfs. The start is refused
///   `exec.sandbox-place-unserved`, address `exec_denied_paths`, with the mount point and
///   `file-system-unlisted ramfs`. `unlisted-listed`: the same directory listed, beside a
///   deny entry elsewhere, refuses nothing, and the deny entry holds.
/// - `covered-denied` and `covered-listed`: `stack` holds two mounts on one mount point.
///   Denied, or listed beside a deny entry, it refuses the start
///   `exec.sandbox-place-unserved` with `mount-covered`, address `exec_denied_paths` and
///   `exec_subpaths`. `covered-alone`: listed with no deny entry, the start runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_deny_entry_holds_across_the_mounts_of_the_view() {
    if let Some(host) = std::env::var_os(INNER) {
        inside(Path::new(&host)).await;
        return;
    }
    let stdout = match staged() {
        Ok(stdout) => stdout,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let inside = format!("{NAME} inside: ");
    let line = |what: &str| {
        stdout
            .lines()
            // The harness writes `test <name> ... ` before the first line of a case.
            .filter_map(|line| line.split_once(inside.as_str()).map(|(_, rest)| rest))
            .find_map(|line| line.strip_prefix(&format!("{what} ")))
            .map(|rest| rest.trim().to_owned())
    };
    if let Some(reason) = line("ABSENT") {
        println!("{NAME}: ABSENT (inside the namespace: {reason})");
        return;
    }
    let mut failures = Vec::new();
    for (what, expected, means) in EXPECTED {
        let seen = line(what).unwrap_or_else(|| {
            panic!("the run inside the namespace printed no `{what}` line: {stdout}")
        });
        println!("{NAME}: {what}: {seen}");
        if !expected.holds(&seen) {
            failures.push(format!("{what}: expected {means}, observed {seen}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME}: executed");
}

/// Runs this case again inside a user and mount namespace in which the five mounts are made:
/// what that run printed, with its diagnostics after it, or why the case is absent.
fn staged() -> Result<String, String> {
    delegated()?;
    let (Some(unshare), Some(mount), Some(setpriv), Some(shell), Some(cp), Some(true_file)) = (
        find("unshare"),
        find("mount"),
        find("setpriv"),
        find("sh"),
        find("cp"),
        find("true"),
    ) else {
        return Err("no unshare, mount, setpriv, sh, cp or true on this host".to_owned());
    };
    let directory = tempfile::tempdir().expect("host root");
    let host = directory
        .path()
        .canonicalize()
        .expect("canonical host root");
    let tools = host.join("tools");
    let extra = host.join("extra");
    for directory in [
        "tools/bin",
        "tools/held/sub",
        "tools/deep/in",
        "tools/odd/media",
        "tools/stack/m",
        "extra",
    ] {
        std::fs::create_dir_all(host.join(directory)).expect("a directory of the root");
    }
    for name in [
        "tools/bin/tool",
        "tools/bin/other",
        "extra/prog",
        "extra/other",
    ] {
        std::fs::copy(&true_file, host.join(name)).expect("copy true");
    }
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
            "{{ {mount} --bind \"$1\" \"$2/held/sub\" && {mount} -t tmpfs tmpfs \"$2/deep/in\" \
             && {cp} \"$1/prog\" \"$1/other\" \"$2/deep/in/\" \
             && {mount} -t ramfs ramfs \"$2/odd/media\" \
             && {mount} -t tmpfs tmpfs \"$2/stack/m\" && {mount} -t tmpfs tmpfs \"$2/stack/m\"; }} \
             >&2 || exit {STAGE_FAILED}; shift 2; exec \"$@\"",
            mount = mount.display(),
            cp = cp.display()
        ))
        .arg("sh")
        .arg(&extra)
        .arg(&tools)
        .arg(setpriv)
        .args(["--inh-caps=-all", "--ambient-caps=-all", "--"])
        .arg(std::env::current_exe().expect("this test binary"))
        .args(["--exact", CASE, "--nocapture", "--test-threads=1"])
        .env(INNER, &host)
        .output()
        .expect("unshare starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(STAGE_FAILED) {
        return Err(format!("no mount in a user namespace: {}", stderr.trim()));
    }
    // `unshare` itself ended: this host gives this user no user or mount namespace.
    if output.status.code() == Some(1) && stderr.trim_start().starts_with("unshare:") {
        return Err(format!("no user and mount namespace: {}", stderr.trim()));
    }
    Ok(format!("{stdout}\n{stderr}"))
}
