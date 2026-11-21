//! Design 35 D30, *Through the view's mounts*, in the helper: **a followed path that resolves
//! into a mount beneath a `--writable` root is refused**, as one that resolves into the root
//! itself is (D24). What is mounted beneath a directory has a place on another file system, or
//! on the same one under other names, and lies in that directory in the view all the same.
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
/// and `fs/apart`. `stage` runs in the
/// root inside the namespace and mounts what the case names at `fs/tree/held`; the helper then
/// starts the shell with `script`, `entry` as its one allow entry and `writable` as the mounts
/// the run can write. `Err` holds why the case is absent on this host.
fn run(stage: &str, entry: &str, writable: &[&str], script: &str) -> Result<Output, String> {
    let [unshare, sh, env, mount, cp, tool] =
        tools(["unshare", "sh", "env", "mount", "cp", "true"])?;
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    for name in ["fs/tree/held", "fs/beside", "fs/apart", "fs/links"] {
        std::fs::create_dir_all(root.join(name)).expect(name);
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
    let at = |rel: &str| root.join(rel).to_str().expect("UTF-8").to_owned();
    let lists = Lists {
        program: base(&sh),
        loader: substrate_exec_gate::elf_interpreter(&std::fs::read(&env).expect("env")),
        allow: vec![
            Located::new(Class::Follow, root.to_str().expect("UTF-8"), entry)
                .expect("a follow path"),
        ],
        writable: writable.iter().map(|rel| at(rel)).collect(),
    };
    let stage = stage.replace("{mount}", &mount).replace("{cp}", &cp);
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
        Some(REFUSED)
            if diagnostic.contains("Landlock is unavailable")
                || diagnostic.contains("file-system-unlisted") =>
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
