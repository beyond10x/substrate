//! Adversary round on `ec87d1554` (design 35 § 18, *Three decisions after the second review of
//! that branch*), in the helper: the walk of a followed path resolves `..` on the directories
//! it holds open, refuses a path through a directory the run can write or one without a place,
//! and every entry is placed where it ends, whatever its class.
//!
//! As `tests/view_mounts_gate.rs`: each run makes its mounts in a user and mount namespace of
//! its own and starts the shipped `substrate-exec-gate` there with an empty environment. Where
//! the namespace, a mount of the stage, Landlock or the scratch directory's file system is
//! missing, a case prints `ABSENT` with the reason and is **not passed**.
//!
//! The scratch root of a run holds `fs/w`, the mount the run is told it can write, and
//! `fs/ro/dir` with `tool`, a copy of `true`.

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

/// The one allow entry of a run: a `follow` path beneath the scratch root, or a `beneath`
/// path beneath the writable mount `fs/w`.
#[derive(Clone, Copy)]
enum Entry<'a> {
    Follow(&'a str),
    Beneath(&'a str),
}

/// One run of the helper: `stage` is shell text run in the scratch root inside the namespace,
/// with `$mount`, `$mkdir`, `$cp` and `$ln` set and `$R` naming the scratch root. `fs/w` is
/// passed as `--writable`. The shell then runs `script`. `Err` holds why the run is absent on
/// this host.
fn run(stage: &str, entry: Entry<'_>, script: &str) -> Result<Seen, String> {
    let tool = |name: &str| {
        find(name)
            .map(|path| path.to_str().expect("UTF-8").to_owned())
            .ok_or_else(|| format!("no {name} on this host"))
    };
    let (unshare, sh, env) = (tool("unshare")?, tool("sh")?, tool("env")?);
    let (mount, mkdir, cp, ln) = (tool("mount")?, tool("mkdir")?, tool("cp")?, tool("ln")?);
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    for name in ["fs/w", "fs/ro/dir"] {
        std::fs::create_dir_all(root.join(name)).expect(name);
    }
    std::fs::copy(tool("true")?, root.join("fs/ro/dir/tool")).expect("copy true");
    let root_text = root.to_str().expect("UTF-8");
    let writable = format!("{root_text}/fs/w");
    let entry = match entry {
        Entry::Follow(rel) => Located::new(Class::Follow, root_text, rel),
        Entry::Beneath(rel) => Located::new(Class::Beneath, &writable, rel),
    }
    .expect("a path of the gate");
    let lists = Lists {
        program: base(&sh),
        loader: substrate_exec_gate::elf_interpreter(&std::fs::read(&env).expect("env")),
        allow: vec![entry],
        writable: vec![writable],
        deny: Vec::new(),
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

/// The control of a stage: with `fs/ro/dir` listed the tool runs. `Err` with why the case is
/// absent: the stage, or the scratch root's own file system.
fn control(stage: &str) -> Result<(), String> {
    let seen = run(stage, Entry::Follow("fs/ro/dir"), "fs/ro/dir/tool; echo $?")?;
    if seen.0 == Some(REFUSED) && seen.2.contains("file-system-unlisted") {
        return Err(seen.2);
    }
    assert_eq!((seen.0, seen.1.as_str()), (Some(0), "0\n"), "{seen:?}");
    Ok(())
}

/// What a shape must end with: the tool ran, or the helper ended the run with status 125, no
/// output and a diagnostic that holds this text.
#[derive(Clone, Copy)]
enum Wanted {
    Runs,
    Refused(&'static str),
}

/// Runs each `(what, stage, entry, script, wanted)` after its control and prints the case as
/// executed, or as absent with the reason.
fn hold(name: &str, shapes: &[(&str, String, Entry<'_>, &str, Wanted)]) {
    let mut failures = Vec::new();
    for (what, stage, entry, script, wanted) in shapes {
        let seen = match control(stage).and_then(|()| run(stage, *entry, script)) {
            Ok(seen) => seen,
            Err(reason) => {
                return println!("adversary7 exec gate walk {name}: ABSENT ({what}: {reason})");
            }
        };
        let held = match wanted {
            Wanted::Runs => (seen.0, seen.1.as_str()) == (Some(0), "0\n"),
            Wanted::Refused(line) => {
                (seen.0, seen.1.as_str()) == (Some(REFUSED), "") && seen.2.contains(line)
            }
        };
        if !held {
            failures.push(format!("{what}: {seen:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("adversary7 exec gate walk {name}: executed");
}

/// **`..`, `.` and empty names, as the kernel takes them.** The rule must land where the
/// kernel's own walk of the same path ends, or the listed tool does not run:
///
/// - `..` after a link is taken from where the link led: `x/a/../tools`, with `x/a` a link to
///   `fs/other/deep`, is `fs/other/tools`, and not the empty `fs/ro/x/tools` beside the link;
/// - `..` at the root of the view stays there;
/// - `.` and doubled or trailing slashes name nothing, so `dir/./../dir` is `dir` and not the
///   empty `dir/dir`.
#[test]
fn adversary7_dot_dot_is_taken_from_where_the_walk_stands() {
    let up = "../".repeat(40);
    hold(
        "dot dot",
        &[
            (
                "after a link",
                "$mkdir -p fs/ro/x/tools fs/other/deep fs/other/tools \
                 && $cp fs/ro/dir/tool fs/other/tools/tool \
                 && $ln -s ../../other/deep fs/ro/x/a && $ln -s x/a/../tools fs/ro/e"
                    .to_owned(),
                Entry::Follow("fs/ro/e"),
                "fs/ro/e/tool; echo $?",
                Wanted::Runs,
            ),
            (
                "above the root",
                "$ln -s \"/../..$R/fs/ro/dir\" fs/ro/e".to_owned(),
                Entry::Follow("fs/ro/e"),
                "fs/ro/e/tool; echo $?",
                Wanted::Runs,
            ),
            (
                "far above, relative",
                format!("$ln -s \"{up}$R/fs/ro/dir\" fs/ro/e"),
                Entry::Follow("fs/ro/e"),
                "fs/ro/e/tool; echo $?",
                Wanted::Runs,
            ),
            (
                "a dot before dot dot",
                "$mkdir fs/ro/dir/dir && $ln -s ./dir/./../dir fs/ro/e".to_owned(),
                Entry::Follow("fs/ro/e"),
                "fs/ro/e/tool; echo $?",
                Wanted::Runs,
            ),
            (
                "dots and slashes",
                "$ln -s ./dir/.// fs/ro/e".to_owned(),
                Entry::Follow("fs/ro/e"),
                "fs/ro/e/tool; echo $?",
                Wanted::Runs,
            ),
        ],
    );
}

/// **A directory on the way that the run can write, or that has no place, refuses the path**,
/// although the path leaves it again and ends where no run can write:
///
/// - through `fs/w/sub`, reached by a link to a link;
/// - through a directory of a tmpfs mounted beneath `fs/w`;
/// - through a directory of a `ramfs`, a type outside the list, that nothing writable shows.
#[test]
fn adversary7_a_directory_on_the_way_refuses_the_path() {
    hold(
        "passed directory",
        &[
            (
                "a writable directory, through two links",
                "$mkdir fs/w/sub && $ln -s ../w/sub/../../ro/dir fs/ro/l2 && $ln -s l2 fs/ro/l1"
                    .to_owned(),
                Entry::Follow("fs/ro/l1"),
                "echo ran",
                Wanted::Refused("passes through"),
            ),
            (
                "a directory on a tmpfs beneath the writable mount",
                "$mkdir fs/w/t && $mount -t tmpfs tmpfs fs/w/t && $mkdir fs/w/t/sub \
                 && $ln -s ../w/t/sub/../../../ro/dir fs/ro/l1"
                    .to_owned(),
                Entry::Follow("fs/ro/l1"),
                "echo ran",
                Wanted::Refused("passes through"),
            ),
            (
                "a directory without a place",
                "$mkdir fs/ro/u && $mount -t ramfs ramfs fs/ro/u && $mkdir fs/ro/u/d \
                 && $ln -s u/d/../../dir fs/ro/l1"
                    .to_owned(),
                Entry::Follow("fs/ro/l1"),
                "echo ran",
                Wanted::Refused("file-system-unlisted"),
            ),
        ],
    );
}

/// **A `beneath` entry is placed where it ends.** In the writable mount `fs/w`: an entry on a
/// `ramfs` mounted in it, and that mount point itself, have no place and end the helper; an
/// entry on a tmpfs mounted in it, and that mount point itself, run their tool.
#[test]
fn adversary7_a_beneath_entry_is_placed_where_it_ends() {
    const STAGE: &str = "$mkdir fs/w/vol fs/w/t && $mount -t ramfs ramfs fs/w/vol \
                         && $mount -t tmpfs tmpfs fs/w/t && $cp fs/ro/dir/tool fs/w/vol/tool \
                         && $cp fs/ro/dir/tool fs/w/t/tool";
    hold(
        "beneath ends",
        &[
            (
                "a file on a ramfs",
                STAGE.to_owned(),
                Entry::Beneath("vol/tool"),
                "echo ran",
                Wanted::Refused("file-system-unlisted"),
            ),
            (
                "the ramfs mount point",
                STAGE.to_owned(),
                Entry::Beneath("vol"),
                "echo ran",
                Wanted::Refused("file-system-unlisted"),
            ),
            (
                "a file on a tmpfs",
                STAGE.to_owned(),
                Entry::Beneath("t/tool"),
                "fs/w/t/tool; echo $?",
                Wanted::Runs,
            ),
            (
                "the tmpfs mount point",
                STAGE.to_owned(),
                Entry::Beneath("t"),
                "fs/w/t/tool; echo $?",
                Wanted::Runs,
            ),
        ],
    );
}
