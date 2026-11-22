//! Adversary round 2 on `story:executable-deny-list` (design 35 § 8, D30), commit `4653b2f9c`,
//! through the public confinement API of the `linux` adapter with the lane's static helper.
//!
//! Two cases, each opening the adapter once:
//!
//! - `adversary5_no_other_way_to_a_denied_file_runs_it`, on the host as it is: the routes to a
//!   denied file that need no name of it (a descriptor, `execveat` with an empty path, an
//!   unnamed file linked into the directory), a deny entry beneath a file, and the stated
//!   limit N4 set up by an earlier run instead of by the host.
//! - `adversary5_a_deny_entry_beside_a_mount_the_helper_cannot_place`, in a user and mount
//!   namespace of its own: a declared root whose directories hold a mount of a file-system
//!   type outside the list, as a direct entry of a directory the deny-list expands, one level
//!   deeper, and above a mount of a listed type; and a denied root of a file system with a
//!   part of it bound beneath a listed directory.
//!
//! Both need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this process
//! is inside, bubblewrap and `SUBSTRATE_EXEC_GATE` naming the static helper; the second also a
//! host whose user namespaces may mount. Without one a case prints
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

/// Names the host root, in the process the staged case starts inside its namespace.
const INNER: &str = "SUBSTRATE_ADVERSARY5_DENIED_HOST";

/// What the stage of that namespace exits with when a mount failed.
const STAGE_FAILED: i32 = 97;

/// The delegated cgroup v2 subtree this process is inside, or why a case is absent.
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

fn find(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
}

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8").to_owned()
}

/// The `linux` adapter on `host`, opened with the lane's helper, having proved the deny-list.
fn open(host: &Path) -> Result<Arc<dyn ConfinementAdapter>, String> {
    let cgroup_root = delegated()?;
    let mut config = HostConfig::minimum(host.join("workspaces"));
    config.exec_gate = Some(PathBuf::from(
        std::env::var_os("SUBSTRATE_EXEC_GATE")
            .ok_or("SUBSTRATE_EXEC_GATE is unset: the lane builds substrate-exec-gate")?,
    ));
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .map_err(|error| format!("linux did not open: {error}"))?;
    if !adapter.facts().proves(&Fact::ExecDeniedPaths) {
        return Err(format!(
            "the adapter did not prove exec_denied_paths: {:?}",
            adapter.facts()
        ));
    }
    Ok(adapter)
}

/// One start. `ran <exit code> <standard output> | <first line of standard error>`, or
/// `refused <code> at <address>: <text>` for a refusal at admission or before dispatch.
async fn run(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    script: &str,
) -> String {
    let shell = text(&find("sh").expect("a shell"));
    let command = Command {
        argv: vec![shell, "-c".to_owned(), script.to_owned()],
        env: BTreeMap::new(),
    };
    let refused = |error: ConfinementError| {
        let address = match &error {
            ConfinementError::Driver(driver) => driver.address.clone().unwrap_or_default(),
            _ => String::new(),
        };
        format!("refused {} at {address}: {error}", error.code())
    };
    let admitted = match adapter.admit(profile) {
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
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    format!(
        "ran {:?} {:?} | {}",
        ended.exit.as_ref().and_then(|exit| exit.code),
        String::from_utf8_lossy(&output.stdout),
        diagnostic.lines().next().unwrap_or_default()
    )
}

fn owned(list: &[&str]) -> Vec<String> {
    list.iter().map(|entry| (*entry).to_owned()).collect()
}

/// What a forked child of the Python part ends with: 0 when the program ran, 100 when the
/// kernel answered `EACCES`, 101 for another error, 102 when the call returned.
///
/// `unnamed_linked` passes a directory descriptor with an absolute source path. The kernel
/// ignores the descriptor; Python before 3.14 calls `linkat` with `AT_SYMLINK_FOLLOW` only when
/// one is given, and otherwise `link`, which links the `/proc` symlink itself and fails `EXDEV`,
/// so the unnamed file was never linked there and the row read 101.
const PYTHON: &str = r#"
import os, sys

def attempt(label, call):
    pid = os.fork()
    if pid == 0:
        try:
            call()
        except OSError as error:
            os._exit(100 if error.errno == 13 else 101)
        os._exit(102)
    print(label, os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]), flush=True)

def by_descriptor(flags):
    def call():
        os.execve(os.open("bin/tool", flags), ["tool"], {})
    return call

def unnamed():
    writer = os.open("bin", os.O_TMPFILE | os.O_RDWR, 0o755)
    with open("bin/tool", "rb") as denied:
        os.write(writer, denied.read())
    return writer

def unnamed_by_descriptor():
    writer = unnamed()
    reader = os.open("/proc/self/fd/%d" % writer, os.O_RDONLY)
    os.close(writer)
    os.execve(reader, ["tool"], {})

def unnamed_linked():
    writer = unnamed()
    here = os.open(".", os.O_RDONLY)
    os.link("/proc/self/fd/%d" % writer, "bin/from-unnamed", src_dir_fd=here, follow_symlinks=True)
    os.close(writer)
    os.execv("bin/from-unnamed", ["tool"])

attempt("execveat-read", by_descriptor(os.O_RDONLY))
attempt("execveat-path", by_descriptor(os.O_PATH))
attempt("unnamed-by-descriptor", unnamed_by_descriptor)
attempt("unnamed-linked", unnamed_linked)
"#;

/// **No other way to a denied file runs it** (design 35 § 8.2 D11 step 1, § 8.5). `bin` is
/// listed and holds the deny entry `bin/tool`; `other` and `tools` are listed and hold none.
///
/// - `beneath-a-file`: the deny entry is `bin/tool/x`, a name beneath a file. It names nothing
///   that can exist, so it denies nothing (§ 8.1, "a deny entry that no allowed path holds
///   changes nothing and is not an error") and the start is not ended by the helper: the
///   program and its neighbour run, or the start is refused before dispatch.
/// - `routes`: the denied file through a descriptor of the run (`/proc/self/fd/3`,
///   `/proc/<pid>/fd/3`), linked by that descriptor into the listed `other` (must fail) and
///   into `bin` (a name of the same file, 126). `mount` and `unshare` inside the run make
///   no mount (§ 8.2 rests on that: a run cannot put a mount beneath a listed directory).
/// - `python`, where the host has `python3`: `execveat` with an empty path on a read and on an
///   `O_PATH` descriptor of the denied file, an unnamed file (`O_TMPFILE`) in `bin` holding
///   the denied file's bytes executed by its descriptor, and the same linked into `bin`. Each
///   must end `EACCES` (100): "a file created during the run in an expanded directory is not
///   executable" (§ 8.2).
/// - `earlier-run`, a stated limit (§ 8.5, N4) and not a defect: a start without the deny
///   entry links `bin/tool` into `other`; the next start, with the deny entry, runs that
///   link. § 8.2's "the run cannot link the denied file into another listed directory" holds
///   for the run that has the deny entry and for no earlier one.
// One case on purpose: every route starts from the one workspace, and the last two starts
// need the link the one before them left.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn adversary5_no_other_way_to_a_denied_file_runs_it() {
    const NAME: &str = "adversary5 exec deny routes";
    let host = tempfile::tempdir().expect("host root");
    let adapter = match open(host.path()) {
        Ok(adapter) => adapter,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let workspace = host.path().join("workspaces/ws_adversary5");
    for directory in ["bin", "other", "tools"] {
        std::fs::create_dir_all(workspace.join(directory)).expect(directory);
    }
    let true_file = find("true").expect("true");
    for name in ["bin/tool", "bin/other", "other/keep"] {
        std::fs::copy(&true_file, workspace.join(name)).expect("copy true");
    }
    for name in ["ln", "mount", "unshare"] {
        std::fs::copy(find(name).expect(name), workspace.join("tools").join(name)).expect(name);
    }
    std::fs::write(workspace.join("tools/routes.py"), PYTHON).expect("the Python part");
    let profile = |allow: &[&str], deny: &[&str]| ConfinementProfile {
        workspace: Some(workspace.clone()),
        exec_subpaths: owned(allow),
        exec_denied_paths: owned(deny),
        ..ConfinementProfile::default()
    };
    let listed = ["bin", "other", "tools"];
    let mut failures = Vec::new();
    let mut expect = |what: &str, seen: &str, accepted: &[&str]| {
        println!("{NAME}: {what}: {seen}");
        if !accepted.iter().any(|prefix| seen.starts_with(prefix)) {
            failures.push(format!(
                "{what}: observed {seen}, expected one of {accepted:?}"
            ));
        }
    };

    let seen = run(
        &adapter,
        &profile(&listed, &["bin/tool/x"]),
        "bin/tool; echo \"tool $?\"; bin/other; echo \"other $?\"",
    )
    .await;
    expect(
        "beneath-a-file",
        &seen,
        &[
            "ran Some(0) \"tool 0\\nother 0\\n\"",
            "refused exec.sandbox-path-unrepresentable at exec_denied_paths",
        ],
    );

    let seen = run(
        &adapter,
        &profile(&listed, &["bin/tool"]),
        "exec 3<bin/tool; \
         /proc/self/fd/3 2>/dev/null; echo \"fd $?\"; \
         /proc/$$/fd/3 2>/dev/null; echo \"pid-fd $?\"; \
         tools/ln -L /proc/self/fd/3 other/by-fd 2>/dev/null; echo \"link-into-other $?\"; \
         other/by-fd 2>/dev/null; echo \"other-by-fd $?\"; \
         tools/ln -L /proc/self/fd/3 bin/by-fd 2>/dev/null; bin/by-fd 2>/dev/null; \
         echo \"same-directory $?\"; \
         tools/mount -t tmpfs none other >/dev/null 2>&1; echo \"mount $?\"; \
         tools/unshare --user --map-root-user --mount tools/ln --version >/dev/null 2>&1; \
         [ $? -ne 0 ]; echo \"no-namespace $?\"; \
         other/keep; echo \"neighbour $?\"",
    )
    .await;
    // `mount` without the privilege ends 1 (not root) or 32 (the call failed): either is no
    // mount, so the line is read apart from the rest.
    let mounted = seen.contains("\\nmount 0\\n");
    expect(
        "routes",
        &seen
            .replace("\\nmount 1\\n", "\\nmount -\\n")
            .replace("\\nmount 32\\n", "\\nmount -\\n"),
        &[
            "ran Some(0) \"fd 126\\npid-fd 126\\nlink-into-other 1\\nother-by-fd 127\\n\
           same-directory 126\\nmount -\\nno-namespace 0\\nneighbour 0\\n\"",
        ],
    );
    assert!(!mounted, "{NAME}: the run mounted a file system");

    if let Some(python) = find("python3") {
        let python = text(&python);
        let seen = run(
            &adapter,
            &profile(&["bin", "other", "tools", &python], &["bin/tool"]),
            &format!("{python} tools/routes.py 2>&1"),
        )
        .await;
        expect(
            "python",
            &seen,
            &["ran Some(0) \"execveat-read 100\\nexecveat-path 100\\n\
               unnamed-by-descriptor 100\\nunnamed-linked 100\\n\""],
        );
    } else {
        println!("{NAME}: python: ABSENT (this host has no python3)");
    }

    let seen = run(
        &adapter,
        &profile(&listed, &[]),
        "tools/ln bin/tool other/kept; echo \"linked $?\"",
    )
    .await;
    expect("earlier-run", &seen, &["ran Some(0) \"linked 0\\n\""]);
    let seen = run(
        &adapter,
        &profile(&listed, &["bin/tool"]),
        "bin/tool 2>/dev/null; echo \"entry $?\"; other/kept; echo \"kept $?\"",
    )
    .await;
    expect(
        "limit-earlier-run",
        &seen,
        &["ran Some(0) \"entry 126\\nkept 0\\n\""],
    );

    assert!(failures.is_empty(), "{NAME}:\n{}", failures.join("\n"));
    println!("{NAME}: executed");
}

/// The staged case inside its namespace. Prints one line per observation.
async fn inside(name: &str, host: &Path) {
    let adapter = match open(host) {
        Ok(adapter) => adapter,
        Err(reason) => {
            println!("{name} inside: ABSENT ({reason})");
            return;
        }
    };
    std::fs::create_dir_all(host.join("workspaces/ws_exec")).expect("the workspace");
    let profile = |allow: &[&str], deny: &[&str]| ConfinementProfile {
        workspace: Some(host.join("workspaces/ws_exec")),
        read_only_roots: vec![ReadOnlyRoot {
            host_path: text(&host.join("tools")),
            mount: "/opt/t".to_owned(),
        }],
        exec_subpaths: owned(allow),
        exec_denied_paths: owned(deny),
        ..ConfinementProfile::default()
    };
    let pair = |directory: &str| {
        format!(
            "/opt/t/{directory}/tool 2>/dev/null; echo denied:$?; \
             /opt/t/{directory}/other; echo beside:$?"
        )
    };
    let observations: [(&str, &[&str], &[&str], String); 6] = [
        // The root's directories with their mounts, and no deny entry.
        (
            "control",
            &["/opt/t/odd", "/opt/t/far", "/opt/t/bin"],
            &[],
            format!("{}; {}", pair("odd"), pair("far")),
        ),
        // The deny entry is beside the mount of a type outside the list, one directory.
        (
            "beside-unlisted",
            &["/opt/t/odd", "/opt/t/bin"],
            &["/opt/t/odd/tool"],
            pair("odd"),
        ),
        // The same with a deny entry that does not exist at the start.
        (
            "beside-unlisted-absent",
            &["/opt/t/odd", "/opt/t/bin"],
            &["/opt/t/odd/later"],
            pair("odd"),
        ),
        // The same mount one directory deeper than the deny entry.
        (
            "deeper-unlisted",
            &["/opt/t/far", "/opt/t/bin"],
            &["/opt/t/far/tool"],
            pair("far"),
        ),
        // A mount of a listed type above which the mount of the other type sits: the denied
        // file is `extra/prog`, named through its other mount at `held/sub`.
        (
            "listed-above-unlisted",
            &["/opt/t/nest", "/opt/t/held", "/opt/t/bin"],
            &["/opt/t/held/sub/prog"],
            "/opt/t/nest/media/inner/prog 2>/dev/null; echo denied:$?; \
             /opt/t/bin/other; echo beside:$?"
                .to_owned(),
        ),
        // The denied directory is the root of a file system; a part of that file system is
        // bound beneath a listed directory.
        (
            "denied-root",
            &["/opt/t/u", "/opt/t/bin"],
            &["/opt/t/whole/t"],
            "/opt/t/u/s/prog 2>/dev/null; echo denied:$?; /opt/t/u/other; echo beside:$?"
                .to_owned(),
        ),
    ];
    for (what, allow, deny, script) in observations {
        println!(
            "{name} inside: {what} {}",
            run(&adapter, &profile(allow, deny), &script).await
        );
    }
}

/// **A deny entry beside a mount the helper cannot place** (design 35 D30, § 6 *Through the
/// view's mounts*, § 12).
///
/// D30: "A mount of a type outside the list is not compared and, but for the last row, not
/// refused", the last row being a mount beneath a *denied* directory; and "it fails closed, in
/// D26's form. Where an attachment point cannot be established, a start with a list is
/// refused `exec.sandbox-place-unserved` (501) before dispatch". Step 3 of D11 gives an entry
/// of an expanded directory that is a mount point "the place its own row gives it", and a
/// row of a type outside the list gives none.
///
/// A declared root `tools`, mounted at `/opt/t`, holds:
///
/// - `odd/{tool,other}` and a ramfs at `odd/media`: with `/opt/t/odd` listed and
///   `/opt/t/odd/tool` denied (`beside-unlisted`), or a deny entry that does not exist yet
///   (`beside-unlisted-absent`), the start must run with the denied file refused and its
///   neighbour run, or be refused before dispatch. A start that is dispatched and ends with
///   the helper's status 125 and nothing run is neither;
/// - `far/{tool,other}` and a ramfs at `far/in/media`, the same one directory deeper
///   (`deeper-unlisted`): must run, `denied:126`, `beside:0`;
/// - a ramfs at `nest/media` with `extra` bound at `nest/media/inner`, and `extra` bound at
///   `held/sub` (`listed-above-unlisted`): the file denied as `/opt/t/held/sub/prog` must not
///   run as `/opt/t/nest/media/inner/prog`;
/// - a tmpfs at `whole/t` with `sub/prog`, and its `sub` bound at `u/s` (`denied-root`): with
///   the root of that file system denied, `/opt/t/u/s/prog` must not run and `u/other` must.
#[tokio::test(flavor = "multi_thread")]
async fn adversary5_a_deny_entry_beside_a_mount_the_helper_cannot_place() {
    const NAME: &str = "adversary5 exec deny beside unlisted mount";
    const CASE: &str = "adversary5_a_deny_entry_beside_a_mount_the_helper_cannot_place";
    const HELD: &str = "ran Some(0) \"denied:126\\nbeside:0\\n\"";
    const UNSERVED: &str = "refused exec.sandbox-place-unserved at ";
    if let Some(host) = std::env::var_os(INNER) {
        inside(NAME, Path::new(&host)).await;
        return;
    }
    let stdout = match staged(CASE) {
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
            .filter_map(|line| line.split_once(inside.as_str()).map(|(_, rest)| rest))
            .find_map(|line| line.strip_prefix(&format!("{what} ")))
            .map(|rest| rest.trim().to_owned())
    };
    if let Some(reason) = line("ABSENT") {
        println!("{NAME}: ABSENT (inside the namespace: {reason})");
        return;
    }
    let expected: [(&str, &[&str]); 6] = [
        (
            "control",
            &["ran Some(0) \"denied:0\\nbeside:0\\ndenied:0\\nbeside:0\\n\""],
        ),
        ("beside-unlisted", &[HELD, UNSERVED]),
        (
            "beside-unlisted-absent",
            &["ran Some(0) \"denied:0\\nbeside:0\\n\"", UNSERVED],
        ),
        ("deeper-unlisted", &[HELD]),
        ("listed-above-unlisted", &[HELD, UNSERVED]),
        ("denied-root", &[HELD]),
    ];
    let mut failures = Vec::new();
    for (what, accepted) in expected {
        let seen = line(what).unwrap_or_else(|| {
            panic!("the run inside the namespace printed no `{what}` line: {stdout}")
        });
        println!("{NAME}: {what}: {seen}");
        if !accepted.iter().any(|prefix| seen.starts_with(prefix)) {
            failures.push(format!(
                "{what}: observed {seen}, expected one of {accepted:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{NAME}:\n{}", failures.join("\n"));
    println!("{NAME}: executed");
}

/// Runs `case` again inside a user and mount namespace in which the mounts beneath
/// `<host>/tools` are made: what that run printed, or why the case is absent.
fn staged(case: &str) -> Result<String, String> {
    delegated()?;
    let (Some(unshare), Some(mount), Some(setpriv), Some(shell), Some(cp), Some(mkdir), Some(tool)) = (
        find("unshare"),
        find("mount"),
        find("setpriv"),
        find("sh"),
        find("cp"),
        find("mkdir"),
        find("true"),
    ) else {
        return Err("no unshare, mount, setpriv, sh, cp, mkdir or true on this host".to_owned());
    };
    let directory = tempfile::tempdir().expect("host root");
    let host = directory
        .path()
        .canonicalize()
        .expect("canonical host root");
    for directory in [
        "tools/bin",
        "tools/odd/media",
        "tools/far/in/media",
        "tools/nest/media",
        "tools/held/sub",
        "tools/whole/t",
        "tools/u/s",
        "extra",
    ] {
        std::fs::create_dir_all(host.join(directory)).expect("a directory of the root");
    }
    for name in [
        "tools/bin/tool",
        "tools/bin/other",
        "tools/odd/tool",
        "tools/odd/other",
        "tools/far/tool",
        "tools/far/other",
        "tools/u/other",
        "extra/prog",
        "extra/other",
    ] {
        std::fs::copy(&tool, host.join(name)).expect("copy true");
    }
    // The namespace maps this user to itself and keeps its capabilities for the mounts;
    // `setpriv` then drops them all, because bubblewrap refuses to start with any.
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
            "{{ {mount} -t ramfs ramfs \"$2/odd/media\" \
             && {mount} -t ramfs ramfs \"$2/far/in/media\" \
             && {mount} -t ramfs ramfs \"$2/nest/media\" && {mkdir} \"$2/nest/media/inner\" \
             && {mount} --bind \"$1\" \"$2/nest/media/inner\" \
             && {mount} --bind \"$1\" \"$2/held/sub\" \
             && {mount} -t tmpfs tmpfs \"$2/whole/t\" && {mkdir} \"$2/whole/t/sub\" \
             && {cp} \"$1/prog\" \"$2/whole/t/sub/prog\" \
             && {mount} --bind \"$2/whole/t/sub\" \"$2/u/s\"; }} \
             >&2 || exit {STAGE_FAILED}; shift 2; exec \"$@\"",
            mount = mount.display(),
            mkdir = mkdir.display(),
            cp = cp.display()
        ))
        .arg("sh")
        .arg(host.join("extra"))
        .arg(host.join("tools"))
        .arg(setpriv)
        .args(["--inh-caps=-all", "--ambient-caps=-all", "--"])
        .arg(std::env::current_exe().expect("this test binary"))
        .args(["--exact", case, "--nocapture", "--test-threads=1"])
        .env(INNER, &host)
        .output()
        .expect("unshare starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(STAGE_FAILED) {
        return Err(format!("no mount in a user namespace: {}", stderr.trim()));
    }
    if output.status.code() == Some(1) && stderr.trim_start().starts_with("unshare:") {
        return Err(format!("no user and mount namespace: {}", stderr.trim()));
    }
    Ok(format!("{stdout}\n{stderr}"))
}
