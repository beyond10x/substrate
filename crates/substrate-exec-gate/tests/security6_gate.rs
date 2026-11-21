//! Security review of design 35 D30 as amended (§ 18, *Two decisions after the first review of
//! that branch*): **every symbolic link a followed path reads is placed, in the helper**, asked
//! of the shipped `substrate-exec-gate` where the committed cases do not ask:
//!
//! - the two followed paths that are not entries, `--program` and `--loader`, are walked by the
//!   same rule: a link in the writable mount on either is refused before the ruleset;
//! - an absolute target starts again at the root and is judged there like a relative one;
//! - a trusted link whose target passes through a *directory* of the writable mount and leaves
//!   it again with `..`: the directory is no link, so it is not judged. What an earlier run
//!   left in the writable mount cannot move the end of such a link: the names of the target
//!   fix how deep the directory is, and a link put in the place of one of them is refused.
//!
//! No case has a privilege or makes a mount: the helper is started with an empty environment
//! on directories of the scratch file system, and reads this process's mount table as it reads
//! a view's. Where the kernel has no Landlock, or the scratch directory is on a file-system
//! type outside the list, a case prints `ABSENT` and the reason and is **not passed**.

#![cfg(target_os = "linux")]

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use substrate_exec_gate::{Class, Lists, Located, REFUSED};

/// A tool of the host, where this distribution keeps it.
fn find(name: &str) -> PathBuf {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {name} on this host"))
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

/// The loader the host's base programs name.
fn loader() -> Option<String> {
    substrate_exec_gate::elf_interpreter(&std::fs::read(find("env")).expect("env"))
}

/// A scratch root: `fs/w`, the mount the run can write; `fs/ro`, which holds the trusted
/// links; and `fs/apart/true` and `apart/true`, two directories no run can write.
struct Scratch {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
        let root = directory.path().canonicalize().expect("canonical scratch");
        for name in ["fs/w", "fs/ro", "fs/apart", "apart"] {
            std::fs::create_dir_all(root.join(name)).expect(name);
        }
        let tool = find("true");
        std::fs::copy(&tool, root.join("fs/apart/true")).expect("copy true");
        std::fs::copy(&tool, root.join("apart/true")).expect("copy true");
        Self {
            _directory: directory,
            root,
        }
    }

    fn at(&self, rel: &str) -> String {
        text(&self.root.join(rel))
    }

    fn link(&self, target: &str, rel: &str) {
        symlink(target, self.root.join(rel)).expect("a link");
    }

    /// The entry `rel`, a `follow` path beneath the scratch root.
    fn entry(&self, rel: &str) -> Located {
        Located::new(Class::Follow, &text(&self.root), rel).expect("a follow path")
    }

    /// The lists of a run of the shell with `fs/w` writable and `allow` as its entries.
    fn lists(&self, allow: &[&str]) -> Lists {
        Lists {
            program: base(&text(&find("sh"))),
            loader: loader(),
            allow: allow.iter().map(|rel| self.entry(rel)).collect(),
            writable: vec![self.at("fs/w")],
        }
    }

    /// Starts the helper with `lists` and the shell, by the path `--program` names, with
    /// `script`: the status, what the run printed and the helper's diagnostic. `Err` holds why
    /// the case is absent on this host.
    fn run(&self, lists: &Lists, script: &str) -> Result<(Option<i32>, String, String), String> {
        let output = Command::new(env!("CARGO_BIN_EXE_substrate-exec-gate"))
            .args(lists.options())
            .arg(lists.program.path())
            .args(["-c", script])
            .current_dir(&self.root)
            .env_clear()
            .output()
            .expect("the helper starts");
        let diagnostic = String::from_utf8_lossy(&output.stderr).into_owned();
        if output.status.code() == Some(REFUSED)
            && (diagnostic.contains("Landlock is unavailable")
                || diagnostic.contains("file-system-unlisted"))
        {
            return Err(diagnostic.trim().to_owned());
        }
        Ok((
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            diagnostic,
        ))
    }
}

/// What a row expects: the status, what the run printed, and parts of the diagnostic.
type Expected<'a> = (Option<i32>, &'a str, &'a [&'a str]);

/// Holds `seen` to `expected`, recording what differs under `what`.
fn hold(
    failures: &mut Vec<String>,
    what: &str,
    seen: &(Option<i32>, String, String),
    expected: Expected<'_>,
) {
    if (seen.0, seen.1.as_str()) != (expected.0, expected.1) {
        failures.push(format!(
            "{what}: {:?}, not {:?}: {}",
            (seen.0, &seen.1),
            (expected.0, expected.1),
            seen.2
        ));
    }
    for part in expected.2 {
        if !seen.2.contains(part) {
            failures.push(format!("{what}: no `{part}` in: {}", seen.2));
        }
    }
}

/// **The program and the loader are followed like an entry** (D24: "the program, the loader
/// and each entry"; D30 as amended: every symbolic link such a path reads is placed).
///
/// `--program` and `--loader` each name a link in the writable mount that points at the host's
/// own shell and loader. The run could point either anywhere, so each is refused before the
/// ruleset and nothing runs. The controls name the same two through links in `fs/ro`, which
/// nothing writable reaches, and run.
#[test]
fn security6_the_program_and_the_loader_are_walked_like_an_entry() {
    const NAME: &str = "security6 exec gate links: program and loader";
    let Some(loader) = loader() else {
        return println!("{NAME}: ABSENT (the host's env names no loader)");
    };
    let scratch = Scratch::new();
    let shell = text(&find("sh"));
    for directory in ["fs/w", "fs/ro"] {
        scratch.link(&shell, &format!("{directory}/sh"));
        scratch.link(&loader, &format!("{directory}/loader"));
    }
    let script = "fs/apart/true; echo $?";
    let through = "resolves through the symbolic link";
    let mut failures = Vec::new();
    let rows: [(&str, &str, &str, Expected<'_>); 4] = [
        (
            "the controls: both through links nothing writable reaches",
            "fs/ro/sh",
            "fs/ro/loader",
            (Some(0), "0\n", &[]),
        ),
        (
            "the program through a link in the writable mount",
            "fs/w/sh",
            "fs/ro/loader",
            (
                Some(REFUSED),
                "",
                &["--program", through, "fs/w/sh, in the writable mount"],
            ),
        ),
        (
            "the loader through a link in the writable mount",
            "fs/ro/sh",
            "fs/w/loader",
            (
                Some(REFUSED),
                "",
                &["--loader", through, "fs/w/loader, in the writable mount"],
            ),
        ),
        (
            "both through links in the writable mount",
            "fs/w/sh",
            "fs/w/loader",
            (Some(REFUSED), "", &[through, "in the writable mount"]),
        ),
    ];
    for (what, program, loader, expected) in rows {
        let lists = Lists {
            program: scratch.entry(program),
            loader: Some(scratch.at(loader)),
            ..scratch.lists(&["fs/apart"])
        };
        match scratch.run(&lists, script) {
            Ok(seen) => hold(&mut failures, what, &seen, expected),
            Err(reason) => return println!("{NAME}: ABSENT ({what}: {reason})"),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME}: executed");
}

/// **An absolute target starts again at the root and is judged there** (D30 as amended).
///
/// `fs/ro/abs-*` are trusted links with absolute targets: to a link in the writable mount that
/// points out of it, to a directory in the writable mount, and to `fs/apart`. The first two
/// are refused, each with its own line, and the third runs.
#[test]
fn security6_an_absolute_target_is_judged_like_a_relative_one() {
    const NAME: &str = "security6 exec gate links: absolute targets";
    let scratch = Scratch::new();
    std::fs::create_dir_all(scratch.root.join("fs/w/held")).expect("held");
    std::fs::copy(find("true"), scratch.root.join("fs/w/held/true")).expect("copy true");
    scratch.link("../apart", "fs/w/out");
    scratch.link(&scratch.at("fs/w/out"), "fs/ro/abs-link");
    scratch.link(&scratch.at("fs/w/held"), "fs/ro/abs-held");
    scratch.link(&scratch.at("fs/apart"), "fs/ro/abs-apart");
    let mut failures = Vec::new();
    let rows: [(&str, &str, Expected<'_>); 3] = [
        (
            "an absolute target that is a link in the writable mount",
            "fs/ro/abs-link",
            (
                Some(REFUSED),
                "",
                &[
                    "resolves through the symbolic link",
                    "fs/w/out, in the writable mount",
                ],
            ),
        ),
        (
            "an absolute target that is a directory in the writable mount",
            "fs/ro/abs-held",
            (
                Some(REFUSED),
                "",
                &["resolves to", "fs/w/held, in the writable mount"],
            ),
        ),
        (
            "an absolute target apart from the writable mount",
            "fs/ro/abs-apart",
            (Some(0), "0\n", &[]),
        ),
    ];
    for (what, entry, expected) in rows {
        let script = format!("{entry}/true; echo $?");
        match scratch.run(&scratch.lists(&[entry]), &script) {
            Ok(seen) => hold(&mut failures, what, &seen, expected),
            Err(reason) => return println!("{NAME}: ABSENT ({what}: {reason})"),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME}: executed");
}

/// What an earlier run did to the writable mount, as renames and then links (target, name),
/// and what the start after it is held to.
type Rearranged<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str)],
    Expected<'a>,
);

/// **What an earlier run left in the writable mount cannot move the end of a trusted link that
/// passes through it.**
///
/// `fs/ro/through` is a trusted link to `../w/a/d/../../../apart`: its target passes through
/// `fs/w/a/d`, a directory of the writable mount, and leaves the mount again with `..`. The
/// entry would end at `fs/apart`. `apart`, one directory further up, is what the entry would
/// name if `d` were one level higher, and `fs/w/apart` what it would name if `d` were one
/// level lower.
///
/// **Pinned by the implementor in the third round of the fix, on the coordinator's
/// decision.** The first row, the directory where the link's author had it, expected the
/// program of `fs/apart` to run: a directory is no link, so the walk did not judge it. The
/// decision: a followed path whose walk visits any directory in a place a writable location
/// reaches is refused, by the helper with status 125. The row expects that refusal and its
/// line now; the three rows a run rearranged are as the reviewer wrote them.
///
/// A run that can write `fs/w` rearranges it before the start under review:
///
/// - `d` moved up, to `fs/w/d`: the name `a/d` is gone and the entry does not open;
/// - `d` moved up and a link `a/d` to it left behind: refused, the link is the run's;
/// - `a` moved down, to `fs/w/x/a`, and a link `a` to it left behind: refused the same way.
///
/// In no row does a program of `apart` or of the writable mount run.
#[test]
fn security6_an_earlier_run_cannot_move_the_end_of_a_trusted_link_through_its_mount() {
    const NAME: &str = "security6 exec gate links: a writable directory on the way";
    let script = "fs/apart/true; echo $?; apart/true; echo $?; fs/w/apart/true; echo $?";
    let through = "resolves through the symbolic link";
    let mut failures = Vec::new();
    let rows: [Rearranged<'_>; 4] = [
        (
            "the directory where the link's author had it",
            &[],
            &[],
            (
                Some(REFUSED),
                "",
                &["passes through", "fs/w, in the writable mount"],
            ),
        ),
        (
            "the directory moved up one level",
            &[("fs/w/a/d", "fs/w/d")],
            &[],
            (
                Some(REFUSED),
                "",
                &["cannot open", "No such file or directory"],
            ),
        ),
        (
            "the directory moved up one level, and a link in its place",
            &[("fs/w/a/d", "fs/w/d")],
            &[("../d", "fs/w/a/d")],
            (
                Some(REFUSED),
                "",
                &[through, "fs/w/a/d, in the writable mount"],
            ),
        ),
        (
            "its parent moved down one level, and a link in its place",
            &[("fs/w/a", "fs/w/x/a")],
            &[("x/a", "fs/w/a")],
            (
                Some(REFUSED),
                "",
                &[through, "fs/w/a, in the writable mount"],
            ),
        ),
    ];
    for (what, renames, links, expected) in rows {
        let scratch = Scratch::new();
        for name in ["fs/w/a/d", "fs/w/x", "fs/w/apart"] {
            std::fs::create_dir_all(scratch.root.join(name)).expect(name);
        }
        std::fs::copy(find("true"), scratch.root.join("fs/w/apart/true")).expect("copy true");
        scratch.link("../w/a/d/../../../apart", "fs/ro/through");
        for (from, to) in renames {
            std::fs::rename(scratch.root.join(from), scratch.root.join(to)).expect("a rename");
        }
        for (target, rel) in links {
            scratch.link(target, rel);
        }
        match scratch.run(&scratch.lists(&["fs/ro/through"]), script) {
            Ok(seen) => {
                // Whatever else: only the program the link's author named runs.
                let ran: Vec<&str> = seen.1.lines().collect();
                if ran.get(1) == Some(&"0") || ran.get(2) == Some(&"0") {
                    failures.push(format!(
                        "{what}: a program the entry does not name ran: {seen:?}"
                    ));
                }
                hold(&mut failures, what, &seen, expected);
            }
            Err(reason) => return println!("{NAME}: ABSENT ({what}: {reason})"),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{NAME}: executed");
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

/// **A run that shares the writable mount cannot move the end of a trusted link while the
/// helper walks it** (design 35 D30 as amended: the helper resolves a `follow` path name by
/// name; D24: the rule of a followed entry lands on what the entry names).
///
/// The same link as in the case before, `fs/ro/through`, a link to `../w/a/d/../../../apart`.
/// The start under review executes nothing before its ruleset, but a second run with the same
/// writable mount does. It moves `fs/w/a/d` to `fs/w/d` after the helper opened `d` and before
/// the helper opens the `..` that follows: `strace` holds the helper at exactly that system
/// call, the one `openat` whose directory is `fs/w/a/d`, and the case moves the directory once
/// it sees the helper hold it. The `..` then names `fs/w`, one level higher than the link's
/// author had it, and the walk ends at `apart` and not at `fs/apart`.
///
/// `apart` is not in the writable mount, so the end is not refused. It is a directory the
/// entry does not name and the driver, which resolves the same text before dispatch, did not
/// judge. Held here: whatever the helper answers, a program of `apart` does not run.
///
/// The control is the same start under the same `strace` with nothing moved.
///
/// **Pinned by the implementor in the third round of the fix, on the coordinator's
/// decision.** The control accepted either answer: the program of `fs/apart` runs and the
/// one of `apart` does not, or the helper refuses. A path through a directory of the writable
/// mount is refused now, so only the refusal is taken. The helper also no longer asks the
/// kernel for `..`: it holds the descriptor of every directory of the walk and drops the
/// last, so no system call of the walk has `fs/w/a/d` as its directory and nothing moved
/// between two system calls changes where the walk stands.
#[test]
fn security6_a_concurrent_writer_cannot_move_the_end_of_a_trusted_link_during_the_walk() {
    const NAME: &str = "security6 exec gate links: a directory moved during the walk";
    // How long the helper is held at the `..`, in microseconds.
    const HELD: &str = "1000000";
    let Some(strace) = ["/usr/bin/strace", "/bin/strace"]
        .iter()
        .map(Path::new)
        .find(|path| path.is_file())
    else {
        return println!("{NAME}: ABSENT (no strace on this host)");
    };
    let script = "fs/apart/true; echo $?; apart/true; echo $?";
    let mut observed = Vec::new();
    for moves in [false, true] {
        let scratch = Scratch::new();
        let held = scratch.root.join("fs/w/a/d");
        std::fs::create_dir_all(&held).expect("d");
        scratch.link("../w/a/d/../../../apart", "fs/ro/through");
        let lists = scratch.lists(&["fs/ro/through"]);
        let mut child = Command::new(strace)
            .args(["-f", "-o", "/dev/null", "-e", "trace=openat", "-P"])
            .arg(&held)
            .args(["-e", &format!("inject=openat:delay_enter={HELD}")])
            .arg(env!("CARGO_BIN_EXE_substrate-exec-gate"))
            .args(lists.options())
            .arg(lists.program.path())
            .args(["-c", script])
            .current_dir(&scratch.root)
            .env_clear()
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("strace starts");
        let mut moved = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while moves
            && std::time::Instant::now() < deadline
            && child.try_wait().expect("strace is waited for").is_none()
        {
            if children(child.id()).iter().any(|pid| holds(*pid, &held)) {
                // What a second run with the same writable mount does.
                std::fs::rename(&held, scratch.root.join("fs/w/d")).expect("move d");
                moved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let output = child.wait_with_output().expect("strace ends");
        let status = output.status.code();
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        let diagnostic = String::from_utf8_lossy(&output.stderr).into_owned();
        if diagnostic.contains("strace:")
            || diagnostic.contains("Landlock is unavailable")
            || diagnostic.contains("file-system-unlisted")
        {
            return println!("{NAME}: ABSENT ({})", diagnostic.trim());
        }
        if moves && !moved && status == Some(0) {
            return println!(
                "{NAME}: ABSENT (the helper was not seen holding fs/w/a/d while strace held it)"
            );
        }
        println!("{NAME}: moved {moved}: {:?}", (status, &printed));
        observed.push((moved, status, printed, diagnostic));
    }
    let (_, status, printed, diagnostic) = &observed[0];
    assert_eq!(
        (*status, printed.as_str()),
        (Some(REFUSED), ""),
        "the control, nothing moved: {diagnostic}"
    );
    let (moved, status, printed, diagnostic) = &observed[1];
    assert_ne!(
        printed.lines().nth(1),
        Some("0"),
        "a second run with the same writable mount moved fs/w/a/d up one level while the helper \
         stood between opening it and opening the `..` after it (moved: {moved}). The walk \
         ended at apart, the execute rule landed there, and a program of apart ran, although \
         the entry names fs/apart and the driver judged fs/apart: {status:?} {printed:?} \
         {diagnostic}"
    );
    println!("{NAME}: executed");
}
