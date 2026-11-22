//! Security review of `story:executable-deny-list`: the helper's expansion (design 35 § 8.2,
//! D11) against what the kernel grants.
//!
//! A Landlock rule on a directory grants every file the kernel reaches by walking up from the
//! file to that directory, and the walk goes up **through mount points**: a rule on `A` covers a
//! file in a mount at `A/held`, whatever that mount shows. D11 decides "a denied path lies in
//! `A`" by place (D25), and the place of a file in the mount at `A/held` is in the directory the
//! mount shows, not in `A`. These cases ask whether a deny entry holds where the two differ:
//!
//! - a program denied in a mount that lies beneath a listed directory;
//! - a listed mount that lies beneath a denied directory;
//!
//! and one thing the committed cases do not run:
//!
//! - the walk's stop at 65536 rules ends with status 125 and executes nothing.
//!
//! The mount cases make their mounts in a user and mount namespace of their own, as
//! `tests/namespace_gate.rs` does, and print `ABSENT` with the reason, and are not passed,
//! where this host gives a user namespace no mount, has no Landlock, or keeps the scratch
//! directory on a file-system type outside the list.

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

fn tool(name: &str) -> PathBuf {
    find(name).unwrap_or_else(|| panic!("no {name} on this host"))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A base tool as a `follow` path of the helper.
fn base(path: &Path) -> Located {
    let text = path.to_str().expect("UTF-8");
    let (root, rel) = text
        .strip_prefix("/usr/")
        .map(|rel| ("/usr", rel))
        .or_else(|| text.strip_prefix("/bin/").map(|rel| ("/bin", rel)))
        .expect("a base tool");
    Located::new(Class::Follow, root, rel).expect("a base path")
}

/// The lists of one run: the shell as the program, `allow` and `deny` beneath `root`.
fn lists(root: &Path, allow: &[&str], deny: &[&str]) -> Lists {
    let located = |entries: &[&str]| {
        entries
            .iter()
            .map(|rel| {
                Located::new(Class::Follow, root.to_str().expect("UTF-8"), rel)
                    .expect("a follow path")
            })
            .collect()
    };
    Lists {
        program: base(&tool("sh")),
        loader: substrate_exec_gate::elf_interpreter(&std::fs::read(tool("env")).expect("env")),
        allow: located(allow),
        deny: located(deny),
        writable: Vec::new(),
    }
}

/// What the mount beneath the listed directory shows.
#[derive(Clone, Copy, Debug)]
enum Held {
    /// Another directory of the scratch directory's own file system, bound there.
    Bind,
    /// A file system of its own, with another device.
    Tmpfs,
}

/// One run's scratch root: `tree/bin/{tool,other}` and `one/{tool,other}`, each a copy of
/// `true`, and an empty `tree/held`. The stage mounts over `tree/held`: `one` bound there, or a
/// tmpfs with copies of the two programs. So `tree/held/tool` is a path beneath `tree` whose
/// place is not in the place of `tree`.
struct Scratch {
    _directory: tempfile::TempDir,
    root: PathBuf,
    stage: String,
}

impl Scratch {
    fn new(held: Held) -> Result<Self, String> {
        let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
        let root = directory.path().canonicalize().expect("canonical scratch");
        for directory in ["tree/bin", "tree/held", "one"] {
            std::fs::create_dir_all(root.join(directory)).expect(directory);
        }
        for name in ["tree/bin/tool", "tree/bin/other", "one/tool", "one/other"] {
            std::fs::copy(tool("true"), root.join(name)).expect("copy true");
        }
        let needed = |name: &str| {
            find(name)
                .map(|path| path.to_str().expect("UTF-8").to_owned())
                .ok_or_else(|| format!("no {name} on this host"))
        };
        let (mount, cp) = (needed("mount")?, needed("cp")?);
        let stage = match held {
            Held::Bind => format!("{mount} --bind one tree/held"),
            Held::Tmpfs => {
                format!("{mount} -t tmpfs tmpfs tree/held && {cp} one/tool one/other tree/held/")
            }
        };
        Ok(Self {
            _directory: directory,
            root,
            stage,
        })
    }

    /// Makes the namespace and the mount, then starts the helper with `lists` and
    /// `sh -c script` in this root, with an empty environment.
    fn run(&self, lists: &Lists, script: &str) -> Output {
        let stage = &self.stage;
        Command::new(tool("unshare"))
            .args(["--user", "--map-root-user", "--mount", "--"])
            .arg(tool("sh"))
            .arg("-c")
            .arg(format!(
                "{{ {stage}; }} >&2 || exit {STAGE_FAILED}; exec \"$@\""
            ))
            .arg("sh")
            .arg(tool("env"))
            .arg("-i")
            .arg(env!("CARGO_BIN_EXE_substrate-exec-gate"))
            .args(lists.options())
            .arg(tool("sh"))
            .args(["-c", script])
            .current_dir(&self.root)
            .env_clear()
            .output()
            .expect("unshare starts")
    }
}

/// The standard output of one run on a fresh root, or `None` after printing the case as
/// absent: no namespace, a stage that failed, no Landlock, or a place this host cannot
/// establish.
fn observed(name: &str, held: Held, allow: &[&str], deny: &[&str], script: &str) -> Option<String> {
    let absent = |reason: &str| {
        println!("security4 exec gate {name}: ABSENT ({reason})");
        None
    };
    let scratch = match Scratch::new(held) {
        Ok(scratch) => scratch,
        Err(reason) => return absent(&reason),
    };
    let output = scratch.run(&lists(&scratch.root, allow, deny), script);
    let diagnostic = text(&output.stderr);
    match output.status.code() {
        Some(STAGE_FAILED) => absent(&format!("the stage failed: {}", diagnostic.trim())),
        // `unshare` itself ended: this host gives this user no user and mount namespace.
        Some(1) if diagnostic.trim_start().starts_with("unshare:") => absent(&format!(
            "no user and mount namespace: {}",
            diagnostic.trim()
        )),
        Some(REFUSED)
            if diagnostic.contains("Landlock is unavailable")
                || diagnostic.contains("cannot establish the place") =>
        {
            absent(diagnostic.trim())
        }
        Some(REFUSED) => panic!("{held:?}: the helper refused: {diagnostic}"),
        _ => Some(text(&output.stdout)),
    }
}

/// **A program denied in a mount beneath a listed directory is not executed** (design 35 D10:
/// "A denied path wins. It wins over a listed directory that holds it"; § 8.5: "A deny entry
/// denies a path, and every mount of that path").
///
/// `tree` is listed. A mount lies at `tree/held`, and `tree/held/tool` is denied. The control
/// runs first, without the deny entry: all three programs run, so the mount itself refuses
/// nothing. With the entry, the denied program ends with status 126 and both neighbours run.
/// The mount is once a second directory of the same file system and once a file system of its
/// own; a host that mounts a tool directory beneath `/usr`, or beneath a directory an operator
/// declares as a read-only root, has this shape in every view, because the driver binds both
/// recursively.
#[test]
fn security4_a_program_denied_in_a_mount_beneath_a_listed_directory_is_not_executed() {
    const NAME: &str = "deny in a mount beneath a listed directory";
    const SCRIPT: &str = "tree/held/tool 2>/dev/null; echo denied:$?; tree/held/other; \
                          echo beside:$?; tree/bin/tool; echo listed:$?";
    let mut seen = Vec::new();
    for held in [Held::Bind, Held::Tmpfs] {
        let Some(control) = observed(NAME, held, &["tree"], &[], SCRIPT) else {
            return;
        };
        assert_eq!(
            control, "denied:0\nbeside:0\nlisted:0\n",
            "{held:?}: the control, without the deny entry"
        );
        let Some(stdout) = observed(NAME, held, &["tree"], &["tree/held/tool"], SCRIPT) else {
            return;
        };
        seen.push((held, stdout));
    }
    for (held, stdout) in &seen {
        assert_eq!(
            stdout, "denied:126\nbeside:0\nlisted:0\n",
            "{held:?}: `tree` is listed, `tree/held/tool` is denied and lies in a mount at \
             `tree/held`: the helper wrote one rule on `tree`, the kernel walks up from the \
             denied file through the mount point to `tree`, and the denied program ran (all \
             runs: {seen:?})"
        );
    }
    println!("security4 exec gate {NAME}: executed");
}

/// **A listed mount beneath a denied directory is not executed** (design 35 D11 step 1: "`A`
/// lies in a denied path: no rule", and N5: "A directory can be denied").
///
/// `tree` is denied, and the mount at `tree/held` is listed. Everything beneath a denied
/// directory is denied, so `tree/held/tool` ends with status 126. The control lists the mount
/// without the deny entry, and the program runs.
#[test]
fn security4_a_listed_mount_beneath_a_denied_directory_is_not_executed() {
    const NAME: &str = "listed mount beneath a denied directory";
    const SCRIPT: &str = "tree/held/tool 2>/dev/null; echo held:$?";
    let mut seen = Vec::new();
    for held in [Held::Bind, Held::Tmpfs] {
        let Some(control) = observed(NAME, held, &["tree/held"], &[], SCRIPT) else {
            return;
        };
        assert_eq!(control, "held:0\n", "{held:?}: the control");
        let Some(stdout) = observed(NAME, held, &["tree/held"], &["tree"], SCRIPT) else {
            return;
        };
        seen.push((held, stdout));
    }
    for (held, stdout) in &seen {
        assert_eq!(
            stdout, "held:126\n",
            "{held:?}: `tree` is denied and the mount at `tree/held` is listed: the place of \
             the mount is not in the place of `tree`, the helper wrote a rule on it, and a \
             program beneath the denied directory ran (all runs: {seen:?})"
        );
    }
    println!("security4 exec gate {NAME}: executed");
}

/// **The walk stops at 65536 rules, with status 125, and nothing runs** (design 35 § 8.2:
/// "The walk is bounded: the helper stops at 65536 rules and exits 125").
///
/// A listed directory holds a denied file and more other entries than the bound, so the
/// expansion needs more rules than the helper writes. The control is the same tree with a
/// few entries, where the denied file is refused and its neighbour runs. No namespace is
/// needed.
#[test]
fn security4_the_walk_stops_at_its_rule_bound_and_executes_nothing() {
    const NAME: &str = "rule bound";
    const BOUND: usize = 65_536;
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    let run = |entries: usize| {
        let listed = root.join(format!("listed-{entries}"));
        std::fs::create_dir(&listed).expect("listed");
        for name in ["tool", "other"] {
            std::fs::copy(tool("true"), listed.join(name)).expect("copy true");
        }
        for index in 0..entries {
            std::fs::write(listed.join(format!("entry-{index}")), b"").expect("an entry");
        }
        let rel = format!("listed-{entries}");
        Command::new(env!("CARGO_BIN_EXE_substrate-exec-gate"))
            .current_dir(&root)
            .env_clear()
            .args(lists(&root, &[&rel], &[&format!("{rel}/tool")]).options())
            .arg(tool("sh"))
            .args([
                "-c",
                &format!("echo ran; {rel}/tool 2>/dev/null; echo $?; {rel}/other; echo $?"),
            ])
            .output()
            .expect("the gate starts")
    };
    let control = run(8);
    let diagnostic = text(&control.stderr);
    if control.status.code() == Some(REFUSED)
        && (diagnostic.contains("Landlock is unavailable")
            || diagnostic.contains("cannot establish the place"))
    {
        println!("security4 exec gate {NAME}: ABSENT ({})", diagnostic.trim());
        return;
    }
    assert_eq!(
        (control.status.code(), text(&control.stdout).as_str()),
        (Some(0), "ran\n126\n0\n"),
        "the control: {diagnostic}"
    );
    let over = run(BOUND);
    let diagnostic = text(&over.stderr);
    assert_eq!(
        (over.status.code(), text(&over.stdout).as_str()),
        (Some(REFUSED), ""),
        "a directory with more entries than the bound: {diagnostic}"
    );
    assert!(
        diagnostic.contains("more than 65536 rules"),
        "the line names the bound: {diagnostic}"
    );
    println!("security4 exec gate {NAME}: executed");
}
