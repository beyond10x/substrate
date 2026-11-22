//! Design 35 D30, *Through the view's mounts*, in the helper: **a followed path that resolves
//! into a mount beneath a `--writable` root is refused**, as one that resolves into the root
//! itself is (D24). What is mounted beneath a directory has a place on another file system, or
//! on the same one under other names, and lies in that directory in the view all the same.
//!
//! **Every symbolic link such a path resolves through is placed as well**: a link the run can
//! replace is refused, and so is one that has no place, on a mount of a file-system type
//! outside the list. A mount of such a type beneath a writable root refuses nothing by itself.
//!
//! **So is every directory the path passes through**: a path through a directory the run can
//! write is refused although it ends where the run cannot, and **every path is placed where
//! it ends, whatever its class**: a `beneath` path that ends on a mount of a type outside the
//! list is refused like a `follow` path.
//!
//! No case has a privilege. Each run makes its mounts in a user and mount namespace of its own,
//! `unshare --user --map-root-user --mount`, and starts the shipped `substrate-exec-gate` there
//! with an empty environment, so the helper reads that namespace's mount table as it reads a
//! view's.
//!
//! Where the namespace cannot be created, a mount is refused there, the kernel has no Landlock
//! or the scratch directory is on a file-system type outside the list, a case prints `ABSENT`
//! and the reason and is **not passed**.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use substrate_exec_gate::{Class, Lists, Located, REFUSED};

/// What a stage exits with when one of its own commands failed, before the helper starts.
const STAGE_FAILED: i32 = 97;

/// A tool of the host, where this distribution keeps it.
fn find(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The paths of the tools a stage and a run need, or the one this host lacks.
fn tools<const N: usize>(names: [&str; N]) -> Result<[String; N], String> {
    let mut found = Vec::with_capacity(N);
    for name in names {
        let path = find(name).ok_or_else(|| format!("no {name} on this host"))?;
        found.push(path.to_str().expect("UTF-8").to_owned());
    }
    Ok(found.try_into().expect("one path per name"))
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

/// One run in a fresh scratch root. The root holds `fs/tree`, `fs/beside/true`,
/// `fs/apart/true` and `fs/links/{held,beside,apart}`, links to `fs/tree/held`, `fs/beside`
/// and `fs/apart`; the empty `fs/unlisted`; `fs/tree/out`, a link out of `fs/tree` to
/// `fs/apart`; `fs/loop`, a link to itself; and `fs/dangling`, a link to nothing. `stage` runs
/// in the
/// root inside the namespace and mounts what the case names at `fs/tree/held`; the helper then
/// starts the shell with `script`, `entry` as its one allow entry and `writable` as the mounts
/// the run can write. `Err` holds why the case is absent on this host.
fn run(stage: &str, entry: &str, writable: &[&str], script: &str) -> Result<Output, String> {
    run_entry(stage, (Class::Follow, "", entry), writable, script)
}

/// [`run`] with an entry of any class: the class, the entry's root beneath the scratch root
/// (empty for the scratch root itself) and the path beneath that root.
fn run_entry(
    stage: &str,
    (class, within, entry): (Class, &str, &str),
    writable: &[&str],
    script: &str,
) -> Result<Output, String> {
    let [unshare, sh, env, mount, cp, ln, tool] =
        tools(["unshare", "sh", "env", "mount", "cp", "ln", "true"])?;
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    for name in [
        "fs/tree/held",
        "fs/beside",
        "fs/apart",
        "fs/links",
        "fs/unlisted",
    ] {
        std::fs::create_dir_all(root.join(name)).expect(name);
    }
    for (name, target) in [
        ("fs/tree/out", "../apart"),
        ("fs/loop", "loop"),
        ("fs/dangling", "nothing"),
    ] {
        std::os::unix::fs::symlink(target, root.join(name)).expect("a link");
    }
    std::fs::copy(&tool, root.join("fs/beside/true")).expect("copy true");
    std::fs::copy(&tool, root.join("fs/apart/true")).expect("copy true");
    for (name, target) in [
        ("held", "../tree/held"),
        ("beside", "../beside"),
        ("apart", "../apart"),
    ] {
        std::os::unix::fs::symlink(target, root.join("fs/links").join(name)).expect("a link");
    }
    let at = |rel: &str| {
        let path = if rel.is_empty() {
            root.clone()
        } else {
            root.join(rel)
        };
        path.to_str().expect("UTF-8").to_owned()
    };
    let lists = Lists {
        program: base(&sh),
        loader: substrate_exec_gate::elf_interpreter(&std::fs::read(&env).expect("env")),
        allow: vec![Located::new(class, &at(within), entry).expect("a path of the gate")],
        writable: writable.iter().map(|rel| at(rel)).collect(),
        deny: Vec::new(),
    };
    let stage = stage
        .replace("{mount}", &mount)
        .replace("{cp}", &cp)
        .replace("{ln}", &ln);
    let output = Command::new(unshare)
        .args(["--user", "--map-root-user", "--mount", "--"])
        .arg(&sh)
        .arg("-c")
        .arg(format!(
            "{{ {stage}; }} >&2 || exit {STAGE_FAILED}; exec \"$@\""
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
        .output()
        .expect("unshare starts");
    let diagnostic = text(&output.stderr);
    match output.status.code() {
        Some(STAGE_FAILED) => Err(format!(
            "no mount in a user namespace: {}",
            diagnostic.trim()
        )),
        // `unshare` itself ended: this host gives this user no user or mount namespace.
        Some(1) if diagnostic.trim_start().starts_with("unshare:") => Err(format!(
            "no user and mount namespace: {}",
            diagnostic.trim()
        )),
        // The scratch directory on a type outside the list. A `ramfs` is what a stage
        // mounts, and a refusal that names it is the helper's answer to the case.
        Some(REFUSED)
            if diagnostic.contains("Landlock is unavailable")
                || (diagnostic.contains("file-system-unlisted")
                    && !diagnostic.contains("file-system-unlisted ramfs")) =>
        {
            Err(diagnostic.trim().to_owned())
        }
        _ => Ok(output),
    }
}

/// The two mounts a case stages at `fs/tree/held`, each with `true` in it: a file system of its
/// own, and a second mount of the directory `fs/beside` of the scratch file system. The third
/// member is what the helper's status is for the entry `fs/links/beside` while `fs/tree` is
/// writable: `fs/beside` is apart from a tmpfs, and it is what the bind mount shows.
const STAGES: [(&str, &str, Option<i32>); 2] = [
    (
        "a tmpfs",
        "{mount} -t tmpfs tmpfs fs/tree/held && {cp} fs/beside/true fs/tree/held/true",
        Some(0),
    ),
    (
        "a bind mount",
        "{mount} --bind fs/beside fs/tree/held",
        Some(REFUSED),
    ),
];

/// D30 on D24: `fs/tree` is a mount the run can write, and `fs/tree/held` is a mount beneath
/// it. The entry `fs/links/held` is a link to that mount. It is refused with the line of D24
/// and nothing runs. By place alone the mount is outside `fs/tree`: a tmpfs is another device,
/// and the bind mount shows `fs/beside`.
///
/// Two controls, each of which runs: the same entry with no writable mount, and the entry
/// `fs/links/apart`, which resolves apart from the writable mount and from what is mounted
/// beneath it, with it.
///
/// The directory a bind mount shows lies in the writable mount under its own name as well: the
/// run writes `fs/beside/true` through `fs/tree/held/true`. So the entry `fs/links/beside` is
/// refused where `fs/beside` is bound beneath `fs/tree`, and runs where a tmpfs is mounted
/// there.
#[test]
fn a_followed_entry_into_a_mount_beneath_a_writable_mount_is_refused() {
    const NAME: &str = "follow into a mount beneath a writable mount";
    // Each stage is measured, so a helper that fails one says so for both.
    let mut failures = Vec::new();
    for (what, stage, beside) in STAGES {
        let absent = |reason: String| println!("exec gate view mounts {NAME}: ABSENT ({reason})");
        let controls = [
            ("links/held", &[][..], "fs/links/held/true; echo $?"),
            (
                "links/apart",
                &["fs/tree"][..],
                "fs/links/apart/true; echo $?",
            ),
        ];
        for (entry, writable, script) in controls {
            match run(stage, &format!("fs/{entry}"), writable, script) {
                Ok(output) => assert_eq!(
                    text(&output.stdout),
                    "0\n",
                    "{what}: the control {entry} with {writable:?} writable: {}",
                    text(&output.stderr)
                ),
                Err(reason) => return absent(reason),
            }
        }
        let output = match run(stage, "fs/links/held", &["fs/tree"], "echo ran") {
            Ok(output) => output,
            Err(reason) => return absent(reason),
        };
        let diagnostic = text(&output.stderr);
        let observed = (output.status.code(), text(&output.stdout));
        if observed != (Some(REFUSED), String::new()) {
            failures.push(format!(
                "{what} beneath a writable mount: the link into it was admitted: \
                 {observed:?} {diagnostic}"
            ));
        } else if !diagnostic.contains("in the writable mount") {
            failures.push(format!("{what}: not the line of D24: {diagnostic}"));
        }
        match run(stage, "fs/links/beside", &["fs/tree"], "exit 0") {
            Ok(output) if output.status.code() == beside => {}
            Ok(output) => failures.push(format!(
                "{what}: the directory beside the writable mount ended {:?}, not {beside:?}: {}",
                output.status.code(),
                text(&output.stderr)
            )),
            Err(reason) => return absent(reason),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("exec gate view mounts {NAME}: executed");
}

/// A `ramfs`, a type outside the list, at `fs/unlisted`, with `link` on it, a link to
/// `fs/apart`.
const RAMFS: &str = "{mount} -t ramfs ramfs fs/unlisted && {ln} -s ../apart fs/unlisted/link";

/// D30: **every symbolic link a followed path resolves through is placed.**
///
/// - A link on a mount of a type outside the list has no place. No rule compares such a mount,
///   so whether the run can write the link cannot be said, and the helper exits 125 with the
///   cause and the link. With no writable mount nothing is the run's to write, no table is
///   read, and the same entry runs.
/// - A link in the writable mount that points out of it is refused: the run can point it
///   anywhere. So is one on a tmpfs mounted beneath the writable mount.
/// - A link nothing writable reaches is followed, as before, and the walk answers a link to
///   itself and a link to nothing as the kernel does.
#[test]
fn every_symbolic_link_a_followed_path_reads_is_placed() {
    const NAME: &str = "links of a followed path";
    const TMPFS: &str =
        "{mount} -t tmpfs tmpfs fs/tree/held && {ln} -s ../../apart fs/tree/held/out";
    let through = "resolves through the symbolic link";
    let mut failures = Vec::new();
    for (what, stage, entry, writable, script, expected, holds) in [
        (
            "a link nothing writable reaches",
            ":",
            "fs/links/apart",
            &["fs/tree"][..],
            "fs/links/apart/true; echo $?",
            (Some(0), "0\n"),
            &[][..],
        ),
        (
            "a link on a ramfs, with a writable mount",
            RAMFS,
            "fs/unlisted/link",
            &["fs/tree"][..],
            "fs/unlisted/link/true; echo $?",
            (Some(REFUSED), ""),
            &[
                "cannot establish the place of",
                "fs/unlisted/link: file-system-unlisted ramfs",
            ][..],
        ),
        (
            "a link on a ramfs, with no writable mount",
            RAMFS,
            "fs/unlisted/link",
            &[][..],
            "fs/unlisted/link/true; echo $?",
            (Some(0), "0\n"),
            &[][..],
        ),
        (
            "a link in the writable mount that points out of it",
            ":",
            "fs/tree/out",
            &["fs/tree"][..],
            "fs/tree/out/true; echo $?",
            (Some(REFUSED), ""),
            &[through, "fs/tree/out, in the writable mount"][..],
        ),
        (
            "a link on a tmpfs beneath the writable mount that points out of it",
            TMPFS,
            "fs/links/held/out",
            &["fs/tree"][..],
            "fs/links/held/out/true; echo $?",
            (Some(REFUSED), ""),
            &[through, "fs/tree/held/out, in the writable mount"][..],
        ),
        (
            "a link to itself",
            ":",
            "fs/loop",
            &["fs/tree"][..],
            "echo ran",
            (Some(REFUSED), ""),
            &["cannot open", "Too many levels of symbolic links"][..],
        ),
        (
            "a link to nothing",
            ":",
            "fs/dangling",
            &["fs/tree"][..],
            "echo ran",
            (Some(REFUSED), ""),
            &["cannot open", "No such file or directory"][..],
        ),
    ] {
        let output = match run(stage, entry, writable, script) {
            Ok(output) => output,
            Err(reason) => {
                return println!("exec gate view mounts {NAME}: ABSENT ({what}: {reason})");
            }
        };
        let diagnostic = text(&output.stderr);
        let observed = (output.status.code(), text(&output.stdout));
        if (observed.0, observed.1.as_str()) != expected {
            failures.push(format!(
                "{what}: {observed:?}, not {expected:?}: {diagnostic}"
            ));
        }
        for part in holds {
            if !diagnostic.contains(part) {
                failures.push(format!("{what}: no `{part}` in: {diagnostic}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("exec gate view mounts {NAME}: executed");
}

/// D30: **a followed path that passes through a directory the run can write is refused**,
/// although no link lies there and the path ends where the run cannot write. The run can
/// rename what the directory holds while the path is resolved through it, so where the path
/// ends is the run's to decide.
///
/// - `fs/links/through` is a link to `../tree/held/../../apart`: it enters the writable
///   `fs/tree`, leaves it with `..` and ends at `fs/apart`. Refused with the directory and the
///   mount. With no writable mount the same entry runs.
/// - `fs/links/around` is a link to `../beside/../apart`: `..` through a directory nothing
///   writable reaches. It runs: the helper resolves `..` itself, by dropping the directory it
///   holds, and ends where the kernel would.
/// - `fs/links/via` passes through a directory on a `ramfs`. It has no place, so whether the
///   run can write it cannot be said: refused with the cause.
/// - A path that ends in the writable mount keeps the line of D24: the end is judged first.
#[test]
fn a_followed_path_through_a_writable_directory_is_refused() {
    const NAME: &str = "path through a writable directory";
    const THROUGH: &str = "{ln} -s ../tree/held/../../apart fs/links/through";
    let mut failures = Vec::new();
    for (what, stage, entry, writable, expected, holds) in [
        (
            "through the writable mount and out of it by ..",
            THROUGH,
            "fs/links/through",
            &["fs/tree"][..],
            (Some(REFUSED), String::new()),
            &["passes through", "fs/tree, in the writable mount"][..],
        ),
        (
            "the same link with no writable mount",
            THROUGH,
            "fs/links/through",
            &[][..],
            (Some(0), "0\n".to_owned()),
            &[][..],
        ),
        (
            "by .. through a directory nothing writable reaches",
            "{ln} -s ../beside/../apart fs/links/around",
            "fs/links/around",
            &["fs/tree"][..],
            (Some(0), "0\n".to_owned()),
            &[][..],
        ),
        (
            "through a directory on a ramfs",
            "{mount} -t ramfs ramfs fs/unlisted && {ln} -s ../unlisted/../apart fs/links/via",
            "fs/links/via",
            &["fs/tree"][..],
            (Some(REFUSED), String::new()),
            &[
                "cannot establish the place of",
                "fs/unlisted: file-system-unlisted ramfs",
            ][..],
        ),
        (
            "a path that ends in the writable mount",
            ":",
            "fs/links/held",
            &["fs/tree"][..],
            (Some(REFUSED), String::new()),
            &["resolves to", "fs/tree/held, in the writable mount"][..],
        ),
    ] {
        let script = format!("{entry}/true; echo $?");
        let output = match run(stage, entry, writable, &script) {
            Ok(output) => output,
            Err(reason) => {
                return println!("exec gate view mounts {NAME}: ABSENT ({what}: {reason})");
            }
        };
        let diagnostic = text(&output.stderr);
        let observed = (output.status.code(), text(&output.stdout));
        if observed != expected {
            failures.push(format!(
                "{what}: {observed:?}, not {expected:?}: {diagnostic}"
            ));
        }
        for part in holds {
            if !diagnostic.contains(part) {
                failures.push(format!("{what}: no `{part}` in: {diagnostic}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("exec gate view mounts {NAME}: executed");
}

/// D26: **every path is placed where it ends, whatever its class.** A `beneath` path lies in a
/// mount the run can write, so nothing is asked of its place but that it has one. On a `ramfs`
/// mounted beneath the writable `fs/tree` it has none: such a mount is compared with nothing,
/// and the helper exits 125 with the cause, for a directory and for a file. On a tmpfs there
/// the same entry runs, and with no writable mount no table is read and nothing is placed.
#[test]
fn a_beneath_path_is_placed_where_it_ends() {
    const NAME: &str = "beneath path placed";
    const RAMFS: &str =
        "{mount} -t ramfs ramfs fs/tree/held && {cp} fs/beside/true fs/tree/held/true";
    let unlisted = "file-system-unlisted ramfs";
    let mut failures = Vec::new();
    for (what, stage, entry, writable, expected, holds) in [
        (
            "a directory on a ramfs",
            RAMFS,
            "held",
            &["fs/tree"][..],
            (Some(REFUSED), String::new()),
            &["cannot establish the place of", "fs/tree/held: ", unlisted][..],
        ),
        (
            "a file on a ramfs",
            RAMFS,
            "held/true",
            &["fs/tree"][..],
            (Some(REFUSED), String::new()),
            &[
                "cannot establish the place of",
                "fs/tree/held/true: ",
                unlisted,
            ][..],
        ),
        (
            "a directory on a tmpfs",
            STAGES[0].1,
            "held",
            &["fs/tree"][..],
            (Some(0), "0\n".to_owned()),
            &[][..],
        ),
        (
            "a directory on a ramfs, with no writable mount",
            RAMFS,
            "held",
            &[][..],
            (Some(0), "0\n".to_owned()),
            &[][..],
        ),
    ] {
        let output = match run_entry(
            stage,
            (Class::Beneath, "fs/tree", entry),
            writable,
            "fs/tree/held/true; echo $?",
        ) {
            Ok(output) => output,
            Err(reason) => {
                return println!("exec gate view mounts {NAME}: ABSENT ({what}: {reason})");
            }
        };
        let diagnostic = text(&output.stderr);
        let observed = (output.status.code(), text(&output.stdout));
        if observed != expected {
            failures.push(format!(
                "{what}: {observed:?}, not {expected:?}: {diagnostic}"
            ));
        }
        for part in holds {
            if !diagnostic.contains(part) {
                failures.push(format!("{what}: no `{part}` in: {diagnostic}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("exec gate view mounts {NAME}: executed");
}

/// D30: **a mount of a type outside the list beneath a writable mount refuses nothing by
/// itself.** It is compared with no place, and a start whose entries stay apart from it runs.
/// Bubblewrap binds each device node of `/dev` as such a mount beneath the writable `/dev`, so
/// a helper that refused here would refuse every start with a list.
#[test]
fn a_mount_of_an_unlisted_type_beneath_a_writable_mount_refuses_no_start() {
    const NAME: &str = "unlisted mount beneath a writable mount";
    let output = match run(
        "{mount} -t ramfs ramfs fs/tree/held",
        "fs/links/apart",
        &["fs/tree"],
        "fs/links/apart/true; echo $?",
    ) {
        Ok(output) => output,
        Err(reason) => return println!("exec gate view mounts {NAME}: ABSENT ({reason})"),
    };
    assert_eq!(
        (output.status.code(), text(&output.stdout).as_str()),
        (Some(0), "0\n"),
        "a ramfs beneath the writable mount, and an entry apart from it: {}",
        text(&output.stderr)
    );
    println!("exec gate view mounts {NAME}: executed");
}
