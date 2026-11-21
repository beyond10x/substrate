//! Security review of design 35 § 18, *Three decisions after the second review of that branch*,
//! in the shipped `substrate-exec-gate`: **`..` asks the kernel nothing.**
//!
//! `security6_gate.rs` held the helper under `strace` at the `..` it asked the kernel for and
//! moved a directory of the writable mount meanwhile. Since the third round a path through the
//! writable mount is refused, so that case's mover never acts, and what it measured is held by
//! a unit case of the walk alone. This file holds the same thing of the shipped program, on
//! the one shape that is still admitted: a trusted link whose target has `..` and never enters
//! the writable mount.
//!
//! No case has a privilege or makes a mount. Where the host has no `strace`, the kernel has no
//! Landlock, or the scratch directory is on a file-system type outside the list, the case
//! prints `ABSENT` and the reason and is **not passed**.

#![cfg(target_os = "linux")]

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use substrate_exec_gate::{Class, Lists, Located};

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

/// A base tool as a `follow` path of the helper.
fn base(path: &str) -> Located {
    let (root, rel) = path
        .strip_prefix("/usr/")
        .map(|rel| ("/usr", rel))
        .or_else(|| path.strip_prefix("/bin/").map(|rel| ("/bin", rel)))
        .expect("a base tool");
    Located::new(Class::Follow, root, rel).expect("a base path")
}

/// The processes `pid` started.
fn children(pid: u32) -> Vec<u32> {
    std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|child| child.parse().ok())
        .collect()
}

/// Whether process `pid` holds a descriptor of `path`.
fn holds(pid: u32, path: &Path) -> bool {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|descriptor| std::fs::read_link(descriptor.path()).is_ok_and(|held| held == path))
}

/// A scratch root: `fs/w`, the mount the run can write; `fs/ro/beside`, a trusted link to
/// `../ro2/a/d/../../../apart`; the directories `fs/ro2/a/d`, which no run can write; and
/// `fs/apart/true` and `apart/true`. The link's text names `fs/apart`. A `..` asked of the
/// kernel from `d` after `d` was moved to `fs/ro2/d` would lead to `apart`.
fn scratch(tool: &Path) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    for name in ["fs/w", "fs/ro", "fs/ro2/a/d", "fs/apart", "apart"] {
        std::fs::create_dir_all(root.join(name)).expect(name);
    }
    std::fs::copy(tool, root.join("fs/apart/true")).expect("copy true");
    std::fs::copy(tool, root.join("apart/true")).expect("copy true");
    symlink("../ro2/a/d/../../../apart", root.join("fs/ro/beside")).expect("the trusted link");
    (directory, root)
}

/// **The shipped helper asks the kernel for no `..`, and a directory moved under its walk does
/// not move the end** (design 35 § 18: "The helper now also resolves `..` as the driver does,
/// on the directories its walk holds open").
///
/// The entry is `fs/ro/beside`, a link to `../ro2/a/d/../../../apart`, with `fs/w` writable.
/// Nothing on the link's way is in the writable mount, so the path is admitted and the program
/// of `fs/apart` runs.
///
/// - **Traced**: under `strace`, no `openat` or `openat2` of the helper before it executes the
///   program has `..` as its name, and the walk is in the trace (the open of `apart`).
/// - **Moved**: `strace` holds the helper where its open of `d` returns, the one `openat`
///   whose directory is `fs/ro2/a`. The case then moves `fs/ro2/a/d` to `fs/ro2/d`. No run
///   can do that, the directory is not in a writable mount: the case stands in for nobody, and
///   holds the mechanism. The walk ends at `fs/apart` all the same, where the driver, which
///   resolves the same text before dispatch, judged it. With the kernel's `..` it would end at
///   `apart`, and the program of `apart` would run in place of the one of `fs/apart`.
// One case on purpose: the traced start and the moved start are two readings of one walk.
#[allow(clippy::too_many_lines)]
#[test]
fn security7_the_helper_asks_the_kernel_for_no_dot_dot_and_a_moved_directory_does_not_move_the_end()
{
    const NAME: &str = "security7 exec gate walk: no `..` asked of the kernel";
    // How long the helper is held where its open of `d` returns, in microseconds.
    const HELD: &str = "1000000";
    let absent = |reason: &str| println!("{NAME}: ABSENT ({reason})");
    let (Some(strace), Some(shell), Some(env), Some(tool)) =
        (find("strace"), find("sh"), find("env"), find("true"))
    else {
        return absent("no strace, sh, env or true on this host");
    };
    let script = "fs/apart/true; echo $?; apart/true; echo $?";
    let lists = |root: &Path| Lists {
        program: base(&text(&shell)),
        loader: substrate_exec_gate::elf_interpreter(&std::fs::read(&env).expect("env")),
        allow: vec![
            Located::new(Class::Follow, &text(root), "fs/ro/beside").expect("a follow path"),
        ],
        writable: vec![text(&root.join("fs/w"))],
        deny: Vec::new(),
    };
    let helper = |command: &mut Command, root: &Path| {
        let lists = lists(root);
        command
            .arg(env!("CARGO_BIN_EXE_substrate-exec-gate"))
            .args(lists.options())
            .arg(lists.program.path())
            .args(["-c", script])
            .current_dir(root)
            .env_clear()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    };
    let unusable = |diagnostic: &str| {
        diagnostic.contains("strace:")
            || diagnostic.contains("Landlock is unavailable")
            || diagnostic.contains("file-system-unlisted")
    };

    // Traced: every open of the helper before it executes the program.
    let (_directory, root) = scratch(&tool);
    let trace = root.join("trace");
    let mut command = Command::new(&strace);
    command
        .args(["-f", "-e", "trace=openat,openat2,execve", "-o"])
        .arg(&trace);
    helper(&mut command, &root);
    let output = command.output().expect("strace starts");
    let diagnostic = String::from_utf8_lossy(&output.stderr).into_owned();
    if unusable(&diagnostic) {
        return absent(diagnostic.trim());
    }
    assert_eq!(
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).as_ref()
        ),
        (Some(0), "0\n126\n"),
        "traced: the program of fs/apart runs and no other: {diagnostic}"
    );
    let traced = std::fs::read_to_string(&trace).expect("the trace");
    // The first `execve` starts the helper and the second is the helper's own, of the program.
    let before: Vec<&str> = traced
        .lines()
        .scan(0, |executed, line| {
            if line.contains("execve(") {
                *executed += 1;
            }
            (*executed < 2).then_some(line)
        })
        .filter(|line| line.contains("openat"))
        .collect();
    assert!(
        before.iter().any(|line| line.contains("\"apart\"")),
        "the walk is not in the trace: {traced}"
    );
    let asked: Vec<&&str> = before
        .iter()
        .filter(|line| line.contains("\"..\"") || line.contains("/..\""))
        .collect();
    assert!(
        asked.is_empty(),
        "the helper asked the kernel for `..`: {asked:?}"
    );
    println!(
        "{NAME}: traced: {} opens before the program, none of `..`",
        before.len()
    );

    // Moved: the helper held where its open of `d` returns, and `d` moved meanwhile.
    let (_directory, root) = scratch(&tool);
    let held = root.join("fs/ro2/a/d");
    let mut command = Command::new(&strace);
    command
        .args(["-f", "-o", "/dev/null", "-e", "trace=openat", "-P"])
        .arg(root.join("fs/ro2/a"))
        .args(["-e", &format!("inject=openat:delay_exit={HELD}")]);
    helper(&mut command, &root);
    let mut child = command.spawn().expect("strace starts");
    let mut moved = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && child.try_wait().expect("strace is waited for").is_none() {
        if children(child.id()).iter().any(|pid| holds(*pid, &held)) {
            std::fs::rename(&held, root.join("fs/ro2/d")).expect("move d");
            moved = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let output = child.wait_with_output().expect("strace ends");
    let diagnostic = String::from_utf8_lossy(&output.stderr).into_owned();
    if unusable(&diagnostic) {
        return absent(diagnostic.trim());
    }
    if !moved {
        return absent("the helper was not seen holding fs/ro2/a/d while strace held it");
    }
    assert_eq!(
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).as_ref()
        ),
        (Some(0), "0\n126\n"),
        "fs/ro2/a/d was moved to fs/ro2/d while the helper stood between opening it and the \
         `..` after it, and the walk did not end at fs/apart, where the link's text and the \
         driver put it: {diagnostic}"
    );
    println!("{NAME}: moved: the walk ended at fs/apart");
    println!("{NAME}: executed");
}
