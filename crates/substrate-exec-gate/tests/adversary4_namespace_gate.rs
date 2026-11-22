//! Adversary pass on `story:executable-deny-list` (design 35 § 8, D11, D23, D25), commit
//! `aafb9823a`: **a mount nested beneath a listed directory.**
//!
//! Design 35 § 8.5: "A deny entry denies a path, and every mount of that path (D23, D25)."
//! Step 2 of D11 writes one rule on a listed directory when no denied *place* lies in the
//! directory's *place*. A place is a device and the names from the root of that file system. A
//! Landlock rule on a directory reaches further than the directory's place: the kernel walks
//! from the executed file up through every mount point, so the rule also grants whatever is
//! mounted beneath the directory, and what is mounted there has another place.
//!
//! Each case makes its mounts in a user and mount namespace of its own, as
//! `tests/namespace_gate.rs` does, and starts the shipped helper there. Where the namespace or
//! Landlock is missing a case prints `ABSENT` and is not passed.

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

fn tool(name: &str) -> PathBuf {
    find(name).unwrap_or_else(|| panic!("no {name} on this host"))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn base(path: &Path) -> Located {
    let text = path.to_str().expect("UTF-8");
    let (root, rel) = text
        .strip_prefix("/usr/")
        .map(|rel| ("/usr", rel))
        .or_else(|| text.strip_prefix("/bin/").map(|rel| ("/bin", rel)))
        .expect("a base tool");
    Located::new(Class::Follow, root, rel).expect("a base path")
}

/// One run: a tree `fs/{one,tree/bin}/{tool,other}` on the scratch directory's file system,
/// every file a copy of `true`, then `stage` (shell text, run as the namespace's root in the
/// scratch root with `$mount`, `$mkdir` and `$cp` set), then the helper with `allow` and `deny`
/// beneath the root and `sh -c script`. Returns the run's standard output, or `None` after
/// printing the case as absent.
fn run(name: &str, stage: &str, allow: &[&str], deny: &[&str], script: &str) -> Option<String> {
    let absent = |reason: &str| {
        println!("adversary4 exec gate namespace {name}: ABSENT ({reason})");
        None
    };
    let (Some(unshare), Some(mount), Some(mkdir), Some(cp)) =
        (find("unshare"), find("mount"), find("mkdir"), find("cp"))
    else {
        return absent("no unshare, mount, mkdir or cp on this host");
    };
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    for directory in ["fs/one", "fs/tree/bin"] {
        std::fs::create_dir_all(root.join(directory)).expect(directory);
    }
    for name in [
        "fs/one/tool",
        "fs/one/other",
        "fs/tree/bin/tool",
        "fs/tree/bin/other",
    ] {
        std::fs::copy(tool("true"), root.join(name)).expect("copy true");
    }
    let located = |entries: &[&str]| {
        entries
            .iter()
            .map(|rel| {
                Located::new(Class::Follow, root.to_str().expect("UTF-8"), rel)
                    .expect("a follow path")
            })
            .collect()
    };
    let lists = Lists {
        program: base(&tool("sh")),
        loader: substrate_exec_gate::elf_interpreter(&std::fs::read(tool("env")).expect("env")),
        allow: located(allow),
        deny: located(deny),
        writable: Vec::new(),
    };
    let output: Output = Command::new(unshare)
        .args(["--user", "--map-root-user", "--mount", "--"])
        .arg(tool("sh"))
        .arg("-c")
        .arg(format!(
            "mount={} mkdir={} cp={}; {{ {stage}; }} >&2 || exit {STAGE_FAILED}; exec \"$@\"",
            mount.display(),
            mkdir.display(),
            cp.display()
        ))
        .arg("sh")
        .arg(tool("env"))
        .arg("-i")
        .arg(env!("CARGO_BIN_EXE_substrate-exec-gate"))
        .args(lists.options())
        .arg(tool("sh"))
        .args(["-c", script])
        .current_dir(&root)
        .env_clear()
        .output()
        .expect("unshare starts");
    let diagnostic = text(&output.stderr);
    match output.status.code() {
        Some(STAGE_FAILED) => {
            return absent(&format!("the stage failed: {}", diagnostic.trim()));
        }
        // `unshare` itself ended: this host gives this user no user and mount namespace.
        Some(1) if diagnostic.trim_start().starts_with("unshare:") => {
            return absent(&format!(
                "no user and mount namespace: {}",
                diagnostic.trim()
            ));
        }
        Some(REFUSED)
            if diagnostic.contains("Landlock is unavailable")
                || diagnostic.contains("file-system-unlisted") =>
        {
            return absent(diagnostic.trim());
        }
        _ => {}
    }
    println!(
        "adversary4 exec gate namespace {name}: status {:?}, stderr {:?}",
        output.status.code(),
        diagnostic.trim()
    );
    Some(text(&output.stdout))
}

/// `fs/tree/bin` bound a second time at `fs/one/deep`, inside a listed directory that holds no
/// denied place of its own.
const BIN_BENEATH_ONE: &str = "$mkdir fs/one/deep && $mount --bind fs/tree/bin fs/one/deep";

/// **Another mount of a denied path, nested in a listed directory, runs the denied program.**
/// `fs` is listed and `fs/tree/bin/tool` denied. `fs/tree/bin` is also mounted at
/// `fs/one/deep`. The walk of D11 expands `fs` and `fs/tree` and `fs/tree/bin`; `fs/one` holds
/// no denied place, so it gets one rule, and that rule grants `fs/one/deep/tool`, which is the
/// denied file under its second mount.
///
/// The control is the first line of the script: the denied name itself is refused (126).
#[test]
fn adversary4_a_denied_program_runs_through_a_second_mount_nested_in_a_listed_directory() {
    const NAME: &str = "second mount nested in a listed directory";
    let Some(stdout) = run(
        NAME,
        BIN_BENEATH_ONE,
        &["fs"],
        &["fs/tree/bin/tool"],
        "fs/tree/bin/tool 2>/dev/null; echo $?; fs/one/deep/tool 2>/dev/null; echo $?; \
         fs/one/deep/other; echo $?; fs/one/other; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        stdout, "126\n126\n0\n0\n",
        "the denied file must be denied under every mount of its path (design 35 § 8.5); the \
         second line is the denied file through the mount at fs/one/deep"
    );
    println!("adversary4 exec gate namespace {NAME}: executed");
}

/// **A deny entry written through a nested mount denies nothing.** `fs/one` is listed and the
/// deny entry is `fs/one/deep/tool`, the very path the run executes. Its place is that of
/// `fs/tree/bin/tool`, which does not lie in the place of `fs/one`, so step 2 writes one rule on
/// `fs/one` and the denied path executes.
#[test]
fn adversary4_a_deny_entry_beneath_a_mount_nested_in_a_listed_directory_is_enforced() {
    const NAME: &str = "deny entry beneath a nested mount";
    let Some(stdout) = run(
        NAME,
        BIN_BENEATH_ONE,
        &["fs/one"],
        &["fs/one/deep/tool"],
        "fs/one/deep/tool 2>/dev/null; echo $?; fs/one/deep/other; echo $?; fs/one/tool; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        stdout, "126\n0\n0\n",
        "the first line is the deny entry's own path: a denied path wins over a listed \
         directory that holds it (design 35 D10)"
    );
    println!("adversary4 exec gate namespace {NAME}: executed");
}

/// The same with the nested mount on another file system: a tmpfs at `fs/one/deep` holding a
/// program, the deny entry naming that program by the only path it has.
#[test]
fn adversary4_a_deny_entry_on_another_file_system_mounted_in_a_listed_directory_is_enforced() {
    const NAME: &str = "deny entry on a nested tmpfs";
    let Some(stdout) = run(
        NAME,
        "$mkdir fs/one/deep && $mount -t tmpfs tmpfs fs/one/deep \
         && $cp fs/one/tool fs/one/deep/tool && $cp fs/one/tool fs/one/deep/other",
        &["fs/one"],
        &["fs/one/deep/tool"],
        "fs/one/deep/tool 2>/dev/null; echo $?; fs/one/deep/other; echo $?; fs/one/tool; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        stdout, "126\n0\n0\n",
        "the first line is the deny entry's own path, on a tmpfs mounted beneath the listed \
         directory"
    );
    println!("adversary4 exec gate namespace {NAME}: executed");
}

/// **The reverse: a listed directory that is a mount inside a denied directory runs.** Design
/// 35 D10: a denied path wins "over an entry that names the same path" and step 1 "covers a
/// listed directory inside a denied one" (K8). `fs/one` is denied and `fs/one/deep` listed; in
/// the view the second lies in the first. `fs/one/deep` is a mount of `fs/tree/bin`, whose
/// place does not lie in the place of `fs/one`, so it gets its rule.
#[test]
fn adversary4_a_listed_mount_inside_a_denied_directory_is_denied() {
    const NAME: &str = "listed mount inside a denied directory";
    let Some(stdout) = run(
        NAME,
        BIN_BENEATH_ONE,
        &["fs/one/deep"],
        &["fs/one"],
        "fs/one/deep/other 2>/dev/null; echo $?; fs/one/tool 2>/dev/null; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        stdout, "126\n126\n",
        "the first line is a program of the listed directory, which the view shows inside the \
         denied one"
    );
    println!("adversary4 exec gate namespace {NAME}: executed");
}

/// The control of the cases above: the same tree and lists without any nested mount. The
/// deny entry holds, so what the cases above show is the mount and nothing else.
#[test]
fn adversary4_control_the_same_lists_without_a_nested_mount_hold() {
    const NAME: &str = "control without a nested mount";
    let Some(stdout) = run(
        NAME,
        "$mkdir fs/one/deep && $cp fs/one/tool fs/one/deep/tool && $cp fs/one/tool fs/one/deep/other",
        &["fs/one"],
        &["fs/one/deep/tool"],
        "fs/one/deep/tool 2>/dev/null; echo $?; fs/one/deep/other; echo $?; fs/one/tool; echo $?",
    ) else {
        return;
    };
    assert_eq!(stdout, "126\n0\n0\n");
    println!("adversary4 exec gate namespace {NAME}: executed");
}
