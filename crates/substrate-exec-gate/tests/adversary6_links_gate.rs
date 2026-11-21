//! Adversary round on `d36436a0f` (design 35 § 18, *Two decisions after the first review of
//! that branch*), in the helper: **every symbolic link a followed path resolves through is
//! placed**, and one without a place, or in a place a writable mount reaches, ends the helper
//! with status 125.
//!
//! As `tests/view_mounts_gate.rs`: each run makes its mounts in a user and mount namespace of
//! its own and starts the shipped `substrate-exec-gate` there with an empty environment. Where
//! the namespace, a mount of the stage, Landlock or the scratch directory's file system is
//! missing, a case prints `ABSENT` with the reason and is **not passed**.
//!
//! The scratch root of a run holds `fs/w`, the mount the run is told it can write; `fs/ro/dir`
//! with `tool`, a copy of `true`; and `fs/ro/u`, where a stage mounts a `ramfs`: a file system
//! of a type outside the list, which nothing writable shows.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use substrate_exec_gate::{Class, Lists, Located, REFUSED};

const STAGE_FAILED: i32 = 97;

fn find(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn base(path: &str) -> Located {
    let (root, rel) = path
        .strip_prefix("/usr/")
        .map(|rel| ("/usr", rel))
        .or_else(|| path.strip_prefix("/bin/").map(|rel| ("/bin", rel)))
        .expect("a base tool");
    Located::new(Class::Follow, root, rel).expect("a base path")
}

/// What a run ended with: its status, its output and its diagnostic.
type Seen = (Option<i32>, String, String);

/// One run of the helper: `stage` is shell text run in the scratch root inside the namespace,
/// with `$mount`, `$mkdir`, `$cp` and `$ln` set and `$R` naming the scratch root; `entry` is
/// the one allow entry, a `follow` path beneath the scratch root; `writable` whether `fs/w`
/// is passed as `--writable`. The shell then runs `script`. `Err` holds why the run is absent
/// on this host; a refusal that names a file-system type is the helper's answer, unless it is
/// about the place of the scratch root's own file system.
fn run(stage: &str, entry: &str, writable: bool, script: &str) -> Result<Seen, String> {
    let tool = |name: &str| {
        find(name)
            .map(|path| path.to_str().expect("UTF-8").to_owned())
            .ok_or_else(|| format!("no {name} on this host"))
    };
    let (unshare, sh, env) = (tool("unshare")?, tool("sh")?, tool("env")?);
    let (mount, mkdir, cp, ln) = (tool("mount")?, tool("mkdir")?, tool("cp")?, tool("ln")?);
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    for name in ["fs/w", "fs/ro/dir", "fs/ro/u"] {
        std::fs::create_dir_all(root.join(name)).expect(name);
    }
    std::fs::copy(tool("true")?, root.join("fs/ro/dir/tool")).expect("copy true");
    let root_text = root.to_str().expect("UTF-8");
    let lists = Lists {
        program: base(&sh),
        loader: substrate_exec_gate::elf_interpreter(&std::fs::read(&env).expect("env")),
        allow: vec![Located::new(Class::Follow, root_text, entry).expect("a follow path")],
        writable: if writable {
            vec![format!("{root_text}/fs/w")]
        } else {
            Vec::new()
        },
    };
    let output: Output = Command::new(unshare)
        .args(["--user", "--map-root-user", "--mount", "--"])
        .arg(&sh)
        .arg("-c")
        .arg(format!(
            "mount={mount} mkdir={mkdir} cp={cp} ln={ln}; {{ {stage}; }} >&2 \
             || exit {STAGE_FAILED}; exec \"$@\""
        ))
        .arg("sh")
        .arg(&env)
        .arg("-i")
        .arg(env!("CARGO_BIN_EXE_substrate-exec-gate"))
        .args(lists.options())
        .arg(&sh)
        .args(["-c", script])
        .current_dir(&root)
        .env_clear()
        .env("R", &root)
        .output()
        .expect("unshare starts");
    let diagnostic = text(&output.stderr).trim().to_owned();
    match output.status.code() {
        Some(STAGE_FAILED) => Err(format!("the stage failed: {diagnostic}")),
        Some(1) if diagnostic.starts_with("unshare:") => {
            Err(format!("no user and mount namespace: {diagnostic}"))
        }
        Some(REFUSED) if diagnostic.contains("Landlock is unavailable") => Err(diagnostic),
        code => Ok((code, text(&output.stdout), diagnostic)),
    }
}

/// The `ramfs` at `fs/ro/u`, which every stage of this file starts with.
const RAMFS: &str = "$mount -t ramfs ramfs fs/ro/u";

/// The control of a stage: with `fs/ro/dir` listed and `fs/w` writable the tool runs. `Err`
/// with why the case is absent: the stage, or the scratch root's own file system.
fn control(stage: &str) -> Result<(), String> {
    let seen = run(stage, "fs/ro/dir", true, "fs/ro/dir/tool; echo $?")?;
    if seen.0 == Some(REFUSED) && seen.2.contains("file-system-unlisted") {
        return Err(seen.2);
    }
    assert_eq!((seen.0, seen.1.as_str()), (Some(0), "0\n"), "{seen:?}");
    Ok(())
}

/// Runs each `(what, stage, entry)` with `fs/w` writable and returns those that were not
/// ended with status 125 and no output, or whose diagnostic lacks `line`.
fn refused(shapes: &[(&str, String, &str)], line: &str) -> Result<Vec<String>, String> {
    let mut failures = Vec::new();
    for (what, stage, entry) in shapes {
        control(stage)?;
        let seen = run(stage, entry, true, "echo ran")?;
        if (seen.0, seen.1.as_str()) != (Some(REFUSED), "") {
            failures.push(format!("{what}: the entry {entry} was admitted: {seen:?}"));
        } else if !seen.2.contains(line) {
            failures.push(format!("{what}: not the expected line: {}", seen.2));
        }
    }
    Ok(failures)
}

fn finish(name: &str, outcome: Result<Vec<String>, String>) {
    match outcome {
        Ok(failures) => {
            assert!(failures.is_empty(), "{}", failures.join("\n"));
            println!("adversary6 exec gate links {name}: executed");
        }
        Err(reason) => println!("adversary6 exec gate links {name}: ABSENT ({reason})"),
    }
}

/// **A link on a file system without a place, however the walk comes to it**: as the last
/// name, as a middle name, through another link, through a chain of two, through `..` in a
/// link's target, and with an absolute target. `fs/ro/u/link` points at `fs/ro/dir`.
#[test]
fn adversary6_a_link_without_a_place_is_refused_however_the_walk_reaches_it() {
    let stage = |more: &str| format!("{RAMFS} && $ln -s ../dir fs/ro/u/link && {more}");
    finish(
        "unplaced link",
        refused(
            &[
                ("last name", stage(":"), "fs/ro/u/link"),
                ("middle name", stage(":"), "fs/ro/u/link/tool"),
                (
                    "through a link",
                    stage("$ln -s u/link fs/ro/l1"),
                    "fs/ro/l1",
                ),
                (
                    "through two links",
                    stage("$ln -s u/link fs/ro/l1 && $ln -s l1 fs/ro/l2"),
                    "fs/ro/l2",
                ),
                (
                    "through .. in a target",
                    stage("$ln -s dir/../u/link fs/ro/l3"),
                    "fs/ro/l3",
                ),
                (
                    "an absolute target on the ramfs",
                    format!("{RAMFS} && $ln -s \"$R/fs/ro/dir\" fs/ro/u/abs"),
                    "fs/ro/u/abs",
                ),
                (
                    "an absolute target to the link",
                    stage("$ln -s \"$R/fs/ro/u/link\" fs/ro/l4"),
                    "fs/ro/l4",
                ),
            ],
            "file-system-unlisted",
        ),
    );
}

/// **A link in a place the writable mount reaches, in the middle of a chain.** `fs/w/l`
/// points at `fs/ro/dir`, and the entry reaches it through `fs/ro/l1`. The path ends where no
/// run can write; the link it read is the run's to replace.
#[test]
fn adversary6_a_link_in_a_writable_place_is_refused_in_the_middle_of_a_chain() {
    finish(
        "writable link",
        refused(
            &[
                (
                    "a link in fs/w",
                    "$ln -s ../ro/dir fs/w/l && $ln -s ../w/l fs/ro/l1".to_owned(),
                    "fs/ro/l1",
                ),
                (
                    "a link on a tmpfs beneath fs/w",
                    "$mkdir fs/w/t && $mount -t tmpfs tmpfs fs/w/t \
                     && $ln -s \"$R/fs/ro/dir\" fs/w/t/l && $ln -s ../w/t/l fs/ro/l1"
                        .to_owned(),
                    "fs/ro/l1",
                ),
            ],
            "resolves through the symbolic link",
        ),
    );
}

/// **Magic links.** `/proc/<pid>/cwd` and `/proc/<pid>/root` of the helper's own process (the
/// stage's shell becomes the helper, so `$$` is its process ID), and `/proc/self`, which is a
/// link itself. Each is a link on a file system without a place.
#[test]
fn adversary6_a_magic_link_is_a_link_without_a_place() {
    finish(
        "magic links",
        refused(
            &[
                (
                    "/proc/<pid>/cwd",
                    "$ln -s /proc/$$/cwd/fs/ro/dir fs/ro/m".to_owned(),
                    "fs/ro/m",
                ),
                (
                    "/proc/<pid>/root",
                    "$ln -s \"/proc/$$/root$R/fs/ro/dir\" fs/ro/m".to_owned(),
                    "fs/ro/m",
                ),
                (
                    "/proc/self/cwd",
                    "$ln -s /proc/self/cwd/fs/ro/dir fs/ro/m".to_owned(),
                    "fs/ro/m",
                ),
            ],
            "file-system-unlisted proc",
        ),
    );
}

/// **The bound on links, and what is not a link.** A chain of 40 links is followed and one of
/// 41 ends the helper, as the kernel's own walk does. A target that passes through a directory
/// of the writable mount by `..` reads no link there and ends where no run can write. A link
/// hidden beneath a later mount is never read: the walk sees what the view shows.
///
/// **Changed by the implementor in the third round of the fix, on the coordinator's
/// decision.** The row `through a writable directory by ..` expected the path to be followed
/// and the tool to run. The decision: a followed path whose walk visits any directory in a
/// place a writable location reaches is refused, by the helper with status 125, because a
/// run that can write the directory can move it while the path is resolved through it. The
/// row expects the refusal now.
///
/// **Changed by the implementor in the closing round.** The row `forty links` had the shell
/// execute `fs/ro/c1/tool`, a path on which the kernel itself follows 40 links in one walk.
/// That failed once in the suite's parallel run, status 126 with `Too many levels of symbolic
/// links` from the shell, after the helper had admitted the entry. The kernel counts the
/// links of a walk in a field it sets once and does not reset when it abandons its lockless
/// attempt and walks again (`total_link_count`, `fs/namei.c`); a mount made meanwhile, as
/// the cases beside this one make, abandons it, and a walk of exactly 40 links then ends
/// `ELOOP`. The row executes `fs/ro/dir/tool` now, the file the entry admits by the
/// directory the chain ends at, so it holds what the helper answers for 40 links and asks the
/// kernel to follow none.
#[test]
fn adversary6_the_walk_follows_what_the_kernel_follows() {
    const NAME: &str = "walk";
    // c1 resolves through 40 links, c0 through 41.
    let chain = "$ln -s dir fs/ro/c40; i=39; while [ $i -ge 0 ]; do \
                 $ln -s c$((i+1)) fs/ro/c$i || exit 1; i=$((i-1)); done";
    let mut failures = Vec::new();
    for (what, stage, entry, script, wanted) in [
        (
            "forty links",
            chain.to_owned(),
            "fs/ro/c1",
            "fs/ro/dir/tool; echo $?",
            (Some(0), "0\n"),
        ),
        (
            "forty-one links",
            chain.to_owned(),
            "fs/ro/c0",
            "echo ran",
            (Some(REFUSED), ""),
        ),
        (
            "through a writable directory by ..",
            "$mkdir fs/w/sub && $ln -s ../w/sub/../../ro/dir fs/ro/l".to_owned(),
            "fs/ro/l",
            "fs/ro/l/tool; echo $?",
            (Some(REFUSED), ""),
        ),
        (
            "a link hidden beneath a later mount",
            format!(
                "{RAMFS} && $ln -s ../dir fs/ro/u/link && $mount -t tmpfs tmpfs fs/ro/u \
                 && $mkdir fs/ro/u/link && $cp fs/ro/dir/tool fs/ro/u/link/tool"
            ),
            "fs/ro/u/link",
            "fs/ro/u/link/tool; echo $?",
            (Some(0), "0\n"),
        ),
    ] {
        let seen = match control(&stage).and_then(|()| run(&stage, entry, true, script)) {
            Ok(seen) => seen,
            Err(reason) => {
                return println!("adversary6 exec gate links {NAME}: ABSENT ({what}: {reason})");
            }
        };
        if (seen.0, seen.1.as_str()) != wanted {
            failures.push(format!("{what}: {seen:?}, not {wanted:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("adversary6 exec gate links {NAME}: executed");
}

/// **Without `--writable` the helper reads no table and places no link** (its own
/// documentation). The link on the `ramfs` is followed and the tool runs. Recorded, not a
/// finding: the driver passes `--writable` for every start, the workspace first.
#[test]
fn adversary6_without_a_writable_mount_no_link_is_placed() {
    const NAME: &str = "no writable mount";
    let stage = format!("{RAMFS} && $ln -s ../dir fs/ro/u/link");
    let seen = match control(&stage)
        .and_then(|()| run(&stage, "fs/ro/u/link", false, "fs/ro/u/link/tool; echo $?"))
    {
        Ok(seen) => seen,
        Err(reason) => return println!("adversary6 exec gate links {NAME}: ABSENT ({reason})"),
    };
    println!("adversary6 exec gate links {NAME}: observed {seen:?}");
    assert_eq!((seen.0, seen.1.as_str()), (Some(0), "0\n"), "{seen:?}");
    println!("adversary6 exec gate links {NAME}: executed");
}
