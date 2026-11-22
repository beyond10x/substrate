//! Adversary round 2 on `story:executable-deny-list` (design 35 § 8.2, § 9, D30), commit
//! `4653b2f9c`: the shipped helper alone, with the mounts made in a user and mount namespace
//! of its own, as `tests/namespace_gate.rs` makes them.
//!
//! - the two shapes of the deny probe's extra bind (§ 9, ninth revision): beneath a second
//!   listed directory, which the probe stages, and beneath the directory the probe expands
//!   in any case, which the design wrote;
//! - a mount of a file-system type outside the list among the entries of an expanded
//!   directory, one directory deeper, and above a mount of a listed type;
//! - a denied root of a file system with a part of it bound beneath a listed directory;
//! - a deny entry beneath a file.
//!
//! Where the namespace or Landlock is missing a case prints `ABSENT` and is not passed.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// What one start of the helper came to.
struct Seen {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// One run. `files` are copies of `true` beneath a scratch root on the build's file system,
/// each directory made first; then `stage` (shell text, run as the namespace's root in the
/// scratch root with `$mount`, `$mkdir` and `$cp` set); then the helper with `allow` and
/// `deny` beneath the root, as `follow` paths, and `sh -c script`. `None` after printing the
/// case as absent.
fn run(
    name: &str,
    files: &[&str],
    stage: &str,
    allow: &[&str],
    deny: &[&str],
    script: &str,
) -> Option<Seen> {
    let absent = |reason: &str| {
        println!("adversary5 exec gate denied reach {name}: ABSENT ({reason})");
        None
    };
    let (Some(unshare), Some(mount), Some(mkdir), Some(cp)) =
        (find("unshare"), find("mount"), find("mkdir"), find("cp"))
    else {
        return absent("no unshare, mount, mkdir or cp on this host");
    };
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    for file in files {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
        std::fs::copy(tool("true"), &path).expect("copy true");
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
    let output = Command::new(unshare)
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
    let stderr = text(&output.stderr);
    match output.status.code() {
        Some(STAGE_FAILED) => return absent(&format!("the stage failed: {}", stderr.trim())),
        Some(1) if stderr.trim_start().starts_with("unshare:") => {
            return absent(&format!("no user and mount namespace: {}", stderr.trim()));
        }
        // The scratch root itself has no place on this host, or the kernel has no Landlock:
        // nothing about the case. A place the case staged is named by its own path and is
        // the helper's answer.
        Some(REFUSED)
            if stderr.contains("Landlock is unavailable")
                || (stderr.contains("file-system-unlisted") && !stderr.contains("/vol")) =>
        {
            return absent(stderr.trim());
        }
        _ => {}
    }
    let seen = Seen {
        status: output.status.code(),
        stdout: text(&output.stdout),
        stderr: stderr.trim().to_owned(),
    };
    println!(
        "adversary5 exec gate denied reach {name}: status {:?}, stdout {:?}, stderr {:?}",
        seen.status, seen.stdout, seen.stderr
    );
    Some(seen)
}

/// The probe's files: `allowed/{true,sub/true}`, `apart/keep`, `outside/held/{tool,other}`.
const PROBE_FILES: [&str; 5] = [
    "allowed/true",
    "allowed/sub/true",
    "apart/keep",
    "outside/held/tool",
    "outside/held/other",
];

/// **The deny probe's extra bind, as shipped** (`PROBE_HELD_MOUNT`, design 35 § 9): a
/// directory no entry lists, bound beneath a second listed directory that holds no deny
/// entry of its own, and the deny entry written through that mount. One rule on `apart`
/// would run the denied file. The helper must expand `apart`.
#[test]
fn adversary5_a_bind_beneath_a_second_listed_directory_holds_its_deny_entry() {
    const NAME: &str = "probe shape, second listed directory";
    let Some(seen) = run(
        NAME,
        &PROBE_FILES,
        "$mkdir apart/held && $mount --bind outside/held apart/held",
        &["allowed", "apart"],
        &["allowed/sub/true", "apart/held/tool"],
        "allowed/true; echo $?; allowed/sub/true 2>/dev/null; echo $?; \
         apart/held/tool 2>/dev/null; echo $?; apart/held/other; echo $?; apart/keep; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        (seen.status, seen.stdout.as_str()),
        (Some(0), "0\n126\n126\n0\n0\n"),
        "{}",
        seen.stderr
    );
    println!("adversary5 exec gate denied reach {NAME}: executed");
}

/// **The same bind where the design's § 9 wrote it**, beneath `allowed`, the directory the
/// probe's other deny entry expands in any case. It holds on the shipped helper too. Which of
/// the two shapes tells a helper that compares places alone from one that follows the mounts
/// is what the mutation run of this round measured; this case and the one above are its two
/// rows.
#[test]
fn adversary5_a_bind_beneath_the_expanded_listed_directory_holds_its_deny_entry() {
    const NAME: &str = "probe shape, the expanded directory";
    let Some(seen) = run(
        NAME,
        &PROBE_FILES,
        "$mkdir allowed/held && $mount --bind outside/held allowed/held",
        &["allowed"],
        &["allowed/sub/true", "allowed/held/tool"],
        "allowed/true; echo $?; allowed/sub/true 2>/dev/null; echo $?; \
         allowed/held/tool 2>/dev/null; echo $?; allowed/held/other; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        (seen.status, seen.stdout.as_str()),
        (Some(0), "0\n126\n126\n0\n"),
        "{}",
        seen.stderr
    );
    println!("adversary5 exec gate denied reach {NAME}: executed");
}

/// **A mount of a type outside the list among the entries of an expanded directory: the
/// helper alone runs nothing.** `k` is listed and holds `tool`, `other` and a ramfs at
/// `k/vol`; the deny entry is `k/tool`. The helper expands `k`, cannot place `k/vol` and ends
/// the start with status 125 and the cause `file-system-unlisted`, naming the mount point.
///
/// This case first required the start to run, by design 35 D30's sentence "a mount of a type
/// outside the list is not compared and, but for the last row, not refused", and was red at
/// `4653b2f9c`. The decision of the round after (D26: fail closed, a named refusal before
/// dispatch where the driver can know) is that the driver refuses such a start
/// `exec.sandbox-place-unserved` and the helper alone still ends it. The expectation was
/// turned to that: a rule on the mount point is not safe to write without its place, because
/// a mount of a listed type can sit beneath it and hold a denied file, which is the next
/// case of this file.
///
/// The control is the same mount one directory deeper, `far/in/vol`, with `far/tool`
/// denied: there the helper asks what `far/in` reaches, skips the mount and writes one rule,
/// and the start runs. The two answers differ by where the deny entry is, not by the mount.
#[test]
fn adversary5_an_unlisted_mount_among_the_entries_of_an_expanded_directory_runs_nothing() {
    const NAME: &str = "unlisted mount among the entries";
    let Some(deeper) = run(
        NAME,
        &["far/tool", "far/other"],
        "$mkdir -p far/in/vol && $mount -t ramfs ramfs far/in/vol",
        &["far"],
        &["far/tool"],
        "far/tool 2>/dev/null; echo $?; far/other; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        (deeper.status, deeper.stdout.as_str()),
        (Some(0), "126\n0\n"),
        "the control, the mount one directory deeper: {}",
        deeper.stderr
    );
    let Some(seen) = run(
        NAME,
        &["k/tool", "k/other"],
        "$mkdir k/vol && $mount -t ramfs ramfs k/vol",
        &["k"],
        &["k/tool"],
        "k/tool 2>/dev/null; echo $?; k/other; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        (seen.status, seen.stdout.as_str()),
        (Some(REFUSED), ""),
        "a ramfs beside the deny entry, in the directory the helper expands: the helper alone \
         ends the start and runs nothing: {}",
        seen.stderr
    );
    assert!(
        seen.stderr.contains("file-system-unlisted") && seen.stderr.contains("/k/vol"),
        "the helper names the mount point and the cause: {}",
        seen.stderr
    );
    println!("adversary5 exec gate denied reach {NAME}: executed");
}

/// **A mount of a listed type above which a mount of another type sits.** A ramfs at
/// `n/vol`, and `outside/held` bound at `n/vol/inner`; `n` is listed and the deny entry is
/// `outside/held/tool`, the file's own name. The denied file must not run as
/// `n/vol/inner/tool`: the bind is mounted beneath `n` through the ramfs, and its root is
/// a place `n` reaches. Ending the start (125) fails closed and is accepted here; the case
/// above holds what the design says of the ramfs itself.
#[test]
fn adversary5_a_denied_file_in_a_listed_mount_above_an_unlisted_one_never_runs() {
    const NAME: &str = "listed mount above an unlisted one";
    let Some(seen) = run(
        NAME,
        &["n/keep", "outside/held/tool", "outside/held/other"],
        "$mkdir n/vol && $mount -t ramfs ramfs n/vol && $mkdir n/vol/inner \
         && $mount --bind outside/held n/vol/inner",
        &["n"],
        &["outside/held/tool"],
        "n/vol/inner/tool 2>/dev/null; echo $?; n/keep; echo $?",
    ) else {
        return;
    };
    assert!(
        (seen.status, seen.stdout.as_str()) == (Some(0), "126\n0\n")
            || (seen.status == Some(REFUSED) && seen.stdout.is_empty()),
        "the denied file ran through the mount above the ramfs: status {:?}, {:?}, {}",
        seen.status,
        seen.stdout,
        seen.stderr
    );
    println!(
        "adversary5 exec gate denied reach {NAME}: executed, status {:?}",
        seen.status
    );
}

/// **A denied root of a file system, and a part of it bound beneath a listed directory**
/// (D23, "every mount of that path"; D30 rule 2). A tmpfs at `whole/t` holds `prog` and
/// `sub/prog`; `whole/t/sub` is bound at `u/s` and the whole tmpfs a second time at `v/w`.
/// `u` and `v` are listed and `whole/t` is denied. No file of the tmpfs runs under any of
/// its paths, and the listed neighbours run.
#[test]
fn adversary5_a_denied_root_of_a_file_system_is_denied_under_every_bind_of_its_parts() {
    const NAME: &str = "denied root of a file system";
    let Some(seen) = run(
        NAME,
        &["u/other", "v/other", "outside/prog"],
        "$mkdir -p whole/t u/s v/w && $mount -t tmpfs tmpfs whole/t && $mkdir whole/t/sub \
         && $cp outside/prog whole/t/prog && $cp outside/prog whole/t/sub/prog \
         && $mount --bind whole/t/sub u/s && $mount --bind whole/t v/w",
        &["u", "v", "whole"],
        &["whole/t"],
        "u/s/prog 2>/dev/null; echo $?; v/w/prog 2>/dev/null; echo $?; \
         v/w/sub/prog 2>/dev/null; echo $?; whole/t/prog 2>/dev/null; echo $?; \
         u/other; echo $?; v/other; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        (seen.status, seen.stdout.as_str()),
        (Some(0), "126\n126\n126\n126\n0\n0\n"),
        "{}",
        seen.stderr
    );
    println!("adversary5 exec gate denied reach {NAME}: executed");
}

/// **A deny entry beneath a file** names nothing that can exist: `k/tool/x`, with `k/tool`
/// a file. It denies nothing (design 35 § 8.1, "a deny entry that no allowed path holds
/// changes nothing and is not an error"; § 8.2, an entry may be absent at the start), and
/// it does not take the file above it for the entry: `k/tool` and `k/other` run.
#[test]
fn adversary5_a_deny_entry_beneath_a_file_denies_nothing_and_ends_no_start() {
    const NAME: &str = "deny entry beneath a file";
    let Some(seen) = run(
        NAME,
        &["k/tool", "k/other"],
        "true",
        &["k"],
        &["k/tool/x"],
        "k/tool; echo $?; k/other; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        (seen.status, seen.stdout.as_str()),
        (Some(0), "0\n0\n"),
        "{}",
        seen.stderr
    );
    println!("adversary5 exec gate denied reach {NAME}: executed");
}
