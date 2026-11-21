//! Adversary pass on the fix `f402165ad` (design 35 D30, *Through the view's mounts*), in the
//! helper: **places a run can write that `MountTree::reaches` has to find**, and places it must
//! not refuse.
//!
//! As `tests/view_mounts_gate.rs`: each run makes its mounts in a user and mount namespace of
//! its own, `unshare --user --map-root-user --mount`, and starts the shipped
//! `substrate-exec-gate` there with an empty environment. Where the namespace, a mount of the
//! stage, Landlock or the scratch directory's file system is missing, a case prints `ABSENT`
//! with the reason and is **not passed**.
//!
//! The scratch root of a run holds:
//!
//! - `fs/w`, the mount the run is told it can write (`--writable`);
//! - `fs/ro/tool` and `fs/ro/dir/tool`, copies of `true`, which nothing writable shows unless a
//!   stage makes it so.

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

/// One run of the helper.
struct Run<'a> {
    /// Shell text run in the scratch root inside the namespace, with `$mount`, `$mkdir`, `$cp`
    /// and `$ln` set, and `$ODD` naming the directory [`ODD`] beneath `fs/w`.
    stage: &'a str,
    /// The allow entries, `follow` paths beneath the scratch root.
    allow: &'a [&'a str],
    /// The `--writable` roots, beneath the scratch root.
    writable: &'a [&'a str],
    /// What the shell runs under the ruleset.
    script: &'a str,
    /// Whether a refusal that names a file-system type outside the list is the helper's answer
    /// to this run, and not a reason the run is absent on this host.
    typed: bool,
}

/// A directory name with every character the kernel escapes in a mount table (a space, a tab,
/// a newline, a backslash), and the suffix a removed directory has elsewhere in `/proc`.
const ODD: &str = "a b\tc\\d\ne (deleted)";

impl Run<'_> {
    /// The helper's output, or why the run is absent on this host.
    fn output(&self) -> Result<Output, String> {
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
        for name in ["fs/ro/tool", "fs/ro/dir/tool"] {
            std::fs::copy(tool("true")?, root.join(name)).expect("copy true");
        }
        let odd = root.join("fs/w").join(ODD);
        std::fs::create_dir(&odd).expect("the odd directory");
        let root_text = root.to_str().expect("UTF-8");
        let lists = Lists {
            program: base(&sh),
            loader: substrate_exec_gate::elf_interpreter(&std::fs::read(&env).expect("env")),
            allow: self
                .allow
                .iter()
                .map(|rel| Located::new(Class::Follow, root_text, rel).expect("a follow path"))
                .collect(),
            writable: self
                .writable
                .iter()
                .map(|rel| format!("{root_text}/{rel}"))
                .collect(),
        };
        let output = Command::new(unshare)
            .args(["--user", "--map-root-user", "--mount", "--"])
            .arg(&sh)
            .arg("-c")
            .arg(format!(
                "mount={mount} mkdir={mkdir} cp={cp} ln={ln}; {{ {}; }} >&2 \
                 || exit {STAGE_FAILED}; exec \"$@\"",
                self.stage
            ))
            .arg("sh")
            .arg(&env)
            .arg("-i")
            .arg(env!("CARGO_BIN_EXE_substrate-exec-gate"))
            .args(lists.options())
            .arg(&sh)
            .args(["-c", self.script])
            .current_dir(&root)
            .env_clear()
            .env("ODD", &odd)
            .output()
            .expect("unshare starts");
        let diagnostic = text(&output.stderr);
        match output.status.code() {
            Some(STAGE_FAILED) => Err(format!("the stage failed: {}", diagnostic.trim())),
            Some(1) if diagnostic.trim_start().starts_with("unshare:") => Err(format!(
                "no user and mount namespace: {}",
                diagnostic.trim()
            )),
            Some(REFUSED)
                // Only the place of a path: a mount of the stage that is refused for its type is
                // an answer of the helper, not a property of this host.
                if diagnostic.contains("Landlock is unavailable")
                    || (diagnostic.contains("file-system-unlisted")
                        && diagnostic.contains("cannot establish the place of")
                        && !self.typed) =>
            {
                Err(diagnostic.trim().to_owned())
            }
            _ => Ok(output),
        }
    }
}

/// What a run ended with: its status, its output and its diagnostic.
type Seen = (Option<i32>, String, String);

fn seen(output: &Output) -> Seen {
    (
        output.status.code(),
        text(&output.stdout),
        text(&output.stderr).trim().to_owned(),
    )
}

/// One shape: with `stage`, the entry `entry` names something the run can write through
/// `fs/w`. Three runs: the control without a writable mount executes `probe` and prints its
/// status 0; the control that lists `fs/ro/dir` beside the writable mount does too; the entry
/// with `fs/w` writable is refused with the line of D24 and nothing runs. Returns the failures,
/// or `Err` with why the shape is absent.
fn refused_only_where_writable(
    stage: &str,
    entry: &str,
    probe: &str,
) -> Result<Vec<String>, String> {
    let mut failures = Vec::new();
    let script = format!("{probe}; echo $?");
    let control = Run {
        stage,
        allow: &[entry],
        writable: &[],
        script: &script,
        typed: false,
    }
    .output()?;
    if seen(&control).0 != Some(0) || text(&control.stdout) != "0\n" {
        failures.push(format!(
            "the control without a writable mount did not run {probe}: {:?}",
            seen(&control)
        ));
    }
    let apart = Run {
        stage,
        allow: &["fs/ro/dir"],
        writable: &["fs/w"],
        script: "fs/ro/dir/tool; echo $?",
        typed: false,
    }
    .output()?;
    if seen(&apart).0 != Some(0) || text(&apart.stdout) != "0\n" {
        failures.push(format!(
            "the control apart from the writable mount did not run: {:?}",
            seen(&apart)
        ));
    }
    let attacked = seen(
        &Run {
            stage,
            allow: &[entry],
            writable: &["fs/w"],
            script: &script,
            typed: true,
        }
        .output()?,
    );
    if (attacked.0, attacked.1.as_str()) != (Some(REFUSED), "") {
        failures.push(format!(
            "the entry {entry} names what the run can write through fs/w and was admitted: \
             {attacked:?}"
        ));
    } else if !attacked.2.contains("in the writable mount") {
        failures.push(format!("{entry}: not the line of D24: {}", attacked.2));
    }
    Ok(failures)
}

fn hold(name: &str, shapes: &[(&str, &str, &str, &str)]) {
    let mut failures = Vec::new();
    for (what, stage, entry, probe) in shapes {
        match refused_only_where_writable(stage, entry, probe) {
            Ok(found) => failures.extend(found.into_iter().map(|line| format!("{what}: {line}"))),
            Err(reason) => {
                println!("adversary5 exec gate view mounts {name}: ABSENT ({what}: {reason})");
                return;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("adversary5 exec gate view mounts {name}: executed");
}

/// **A single file bound beneath the writable mount.** `fs/ro/tool` is bound at `fs/w/slot`:
/// the run writes the listed program's bytes through that name. The root of such a mount is a
/// file, and the entry is that file under its own name.
#[test]
fn adversary5_a_file_bound_beneath_a_writable_mount_lies_in_it() {
    hold(
        "file bind",
        &[(
            "a file bound at fs/w/slot",
            ": > fs/w/slot && $mount --bind fs/ro/tool fs/w/slot",
            "fs/ro/tool",
            "fs/ro/tool",
        )],
    );
}

/// **Deeper than three, and a mount point whose name the table escapes.** Four tmpfs, each
/// mounted in the one before it, beneath `fs/w`; and one tmpfs at a directory named with a
/// space, a tab, a newline, a backslash and ` (deleted)`. The entry is a link into the last.
#[test]
fn adversary5_a_deep_chain_and_an_escaped_mount_point_are_reached() {
    hold(
        "chain and escapes",
        &[
            (
                "four tmpfs in a chain",
                "$mkdir fs/w/a && $mount -t tmpfs tmpfs fs/w/a && $mkdir fs/w/a/b && $mount -t tmpfs tmpfs fs/w/a/b \
                 && $mkdir fs/w/a/b/c && $mount -t tmpfs tmpfs fs/w/a/b/c && $mkdir fs/w/a/b/c/d \
                 && $mount -t tmpfs tmpfs fs/w/a/b/c/d && $cp fs/ro/tool fs/w/a/b/c/d/tool \
                 && $ln -s ../w/a/b/c/d fs/ro/deep",
                "fs/ro/deep",
                "fs/ro/deep/tool",
            ),
            (
                "a mount point with escaped characters",
                "$mount -t tmpfs tmpfs \"$ODD\" && $cp fs/ro/tool \"$ODD/tool\" \
                 && $ln -s \"$ODD\" fs/ro/odd",
                "fs/ro/odd",
                "fs/ro/odd/tool",
            ),
        ],
    );
}

/// **Beneath a mount of a type outside the list.** D30 does not compare such a mount. What is
/// mounted beneath it still lies beneath `fs/w`: a tmpfs stacked on a `devpts` at `fs/w/pts`,
/// and a file bound onto a node of a `devpts` at `fs/w/pts`.
#[test]
fn adversary5_a_mount_beneath_an_unlisted_mount_beneath_a_writable_mount_is_reached() {
    hold(
        "beneath an unlisted type",
        &[
            (
                "a tmpfs stacked on a devpts",
                "$mkdir fs/w/pts && $mount -t devpts devpts fs/w/pts \
                 && $mount -t tmpfs tmpfs fs/w/pts && $cp fs/ro/tool fs/w/pts/tool \
                 && $ln -s ../w/pts fs/ro/pts",
                "fs/ro/pts",
                "fs/ro/pts/tool",
            ),
            (
                "a file bound onto a node of a devpts",
                "$mkdir fs/w/pts && $mount -t devpts devpts fs/w/pts \
                 && $mount --bind fs/ro/tool fs/w/pts/ptmx",
                "fs/ro/tool",
                "fs/ro/tool",
            ),
        ],
    );
}

/// **The writable mount is itself a mount point**, with a mount beneath it; and the writable
/// mount is a second mount of a directory that holds one, so the file system beneath it is
/// reached under its other name.
#[test]
fn adversary5_a_writable_root_that_is_a_mount_point_reaches_what_is_beneath_it() {
    hold(
        "writable root is a mount point",
        &[
            (
                "a tmpfs at fs/w and one beneath it",
                "$mount -t tmpfs tmpfs fs/w && $mkdir fs/w/deep && $mount -t tmpfs tmpfs fs/w/deep \
                 && $cp fs/ro/tool fs/w/deep/tool && $ln -s ../w/deep fs/ro/deep",
                "fs/ro/deep",
                "fs/ro/deep/tool",
            ),
            (
                "fs/w a recursive bind of a directory that holds a tmpfs",
                "$mkdir -p fs/src/held && $mount -t tmpfs tmpfs fs/src/held \
                 && $cp fs/ro/tool fs/src/held/tool && $mount --rbind fs/src fs/w \
                 && $ln -s ../src/held fs/ro/held",
                "fs/ro/held",
                "fs/ro/held/tool",
            ),
        ],
    );
}

/// **A file system of a type outside the list that the run can write, and a link on it.** A
/// `ramfs` is mounted at `fs/w/media`, beneath the writable mount, and a second time at
/// `fs/ro/media`. The run of an earlier start left `link` on it, a link to `fs/ro/dir`. The
/// entry is `fs/ro/media/link`: a path through a link the run can replace, which admits
/// whatever the run points it at. This is K10's planted link. D30 skips a mount of an unlisted
/// type beneath a writable location because "whatever is compared with a writable place is
/// placed first, and has no place on such a type"; the link is never placed, only what it
/// resolves to is.
#[test]
fn adversary5_a_link_on_an_unlisted_file_system_beneath_a_writable_mount_is_not_followed() {
    const NAME: &str = "planted link on an unlisted type";
    const STAGE: &str = "$mkdir fs/w/media fs/ro/media && $mount -t ramfs ramfs fs/w/media \
                         && $mount --bind fs/w/media fs/ro/media && $ln -s ../dir fs/w/media/link";
    let absent = |reason: String| {
        println!("adversary5 exec gate view mounts {NAME}: ABSENT ({reason})");
    };
    // The control: with the link's own target listed, `fs/ro/dir/tool` runs and `fs/ro/tool`
    // does not, with the same mounts and the same writable mount.
    let control = match (Run {
        stage: STAGE,
        allow: &["fs/ro/dir"],
        writable: &["fs/w"],
        script: "fs/ro/dir/tool; echo $?; fs/ro/tool 2>/dev/null; echo $?",
        typed: false,
    })
    .output()
    {
        Ok(output) => output,
        Err(reason) => return absent(reason),
    };
    assert_eq!(text(&control.stdout), "0\n126\n", "{:?}", seen(&control));
    let attacked = match (Run {
        stage: STAGE,
        allow: &["fs/ro/media/link"],
        writable: &["fs/w"],
        script: "fs/ro/media/link/tool; echo $?",
        typed: true,
    })
    .output()
    {
        Ok(output) => seen(&output),
        Err(reason) => return absent(reason),
    };
    assert_eq!(
        (attacked.0, attacked.1.as_str()),
        (Some(REFUSED), ""),
        "the entry is a link on a file system the run writes through fs/w/media; the helper \
         followed it: {attacked:?}"
    );
    println!("adversary5 exec gate view mounts {NAME}: executed");
}

/// **Mounts the kernel does not show where the table puts them, beneath the writable mount,
/// fail closed; elsewhere they refuse nothing.** Two tmpfs stacked on `fs/w/deep`, and a tmpfs
/// at `fs/w/a/b` hidden by a later one at `fs/w/a`: the helper exits 125 and names
/// `mount-covered`. The same stacks beneath `fs/ro` do not concern a writable mount, and the
/// run starts; so does a run whose writable mount is itself the upper of a stack.
#[test]
fn adversary5_covered_mounts_refuse_beneath_a_writable_mount_and_nowhere_else() {
    const NAME: &str = "covered mounts";
    let stacked = |at: &str| {
        format!(
            "$mkdir {at}/deep && $mount -t tmpfs tmpfs {at}/deep && $mount -t tmpfs tmpfs {at}/deep"
        )
    };
    let hidden = |at: &str| {
        format!(
            "$mkdir -p {at}/a/b && $mount -t tmpfs tmpfs {at}/a/b && $mount -t tmpfs tmpfs {at}/a"
        )
    };
    let mut failures = Vec::new();
    for (what, stage, covered) in [
        ("a stack beneath fs/w", stacked("fs/w"), true),
        ("a hidden mount beneath fs/w", hidden("fs/w"), true),
        ("a stack beneath fs/ro", stacked("fs/ro"), false),
        ("a hidden mount beneath fs/ro", hidden("fs/ro"), false),
        (
            "fs/w itself the upper of a stack",
            "$mount -t tmpfs tmpfs fs/w && $mount -t tmpfs tmpfs fs/w".to_owned(),
            false,
        ),
    ] {
        let observed = match (Run {
            stage: &stage,
            allow: &["fs/ro/dir"],
            writable: &["fs/w"],
            script: "fs/ro/dir/tool; echo $?",
            typed: false,
        })
        .output()
        {
            Ok(output) => seen(&output),
            Err(reason) => {
                println!("adversary5 exec gate view mounts {NAME}: ABSENT ({what}: {reason})");
                return;
            }
        };
        let held = if covered {
            (observed.0, observed.1.as_str()) == (Some(REFUSED), "")
                && observed.2.contains("mount-covered")
        } else {
            (observed.0, observed.1.as_str()) == (Some(0), "0\n")
        };
        if !held {
            failures.push(format!("{what}: {observed:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("adversary5 exec gate view mounts {NAME}: executed");
}

/// **The stated limit, measured for D24: the layers of an overlay** (design 35 § 6, *A limit
/// the place does not close*). An overlay at `fs/ro/ov` has its upper directory in the
/// writable mount. The entry `fs/ro/ov/tool` is admitted, the run rewrites the upper file and
/// the listed path runs the new bytes. The place of the overlay's file is on the overlay's own
/// device, and no mount is beneath `fs/w`. Not a finding: the design specifies no refusal.
#[test]
fn adversary5_limit_an_overlay_whose_upper_directory_the_run_writes_is_followed() {
    const NAME: &str = "limit: overlay upper";
    let Some(sh) = find("sh") else {
        println!("adversary5 exec gate view mounts {NAME}: ABSENT (no sh on this host)");
        return;
    };
    let script = format!(
        "printf '#!{}\\necho replaced\\n' > fs/w/up/tool; fs/ro/ov/tool; echo $?",
        sh.display()
    );
    let observed = match (Run {
        stage: "$mkdir fs/ro/low fs/ro/ov fs/w/up fs/w/work && $cp fs/ro/tool fs/ro/low/tool \
                && $cp fs/ro/tool fs/w/up/tool && $mount -t overlay overlay \
                -o lowerdir=fs/ro/low,upperdir=fs/w/up,workdir=fs/w/work fs/ro/ov",
        allow: &["fs/ro/ov/tool"],
        writable: &["fs/w"],
        script: &script,
        typed: false,
    })
    .output()
    {
        Ok(output) => seen(&output),
        Err(reason) => {
            println!("adversary5 exec gate view mounts {NAME}: ABSENT ({reason})");
            return;
        }
    };
    println!("adversary5 exec gate view mounts {NAME}: observed {observed:?}");
    assert_eq!(
        (observed.0, observed.1.as_str()),
        (Some(0), "replaced\n0\n"),
        "the limit as the design states it: {observed:?}"
    );
    println!("adversary5 exec gate view mounts {NAME}: executed");
}
