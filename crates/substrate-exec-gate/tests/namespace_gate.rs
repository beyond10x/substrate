//! The place rule of design 35 (D25, D26) on mounts a test has to make: a host bind mount, a
//! tmpfs and an overlay (§ 16, *D25, a host bind mount* and *D26, the file systems*), and the
//! limit an overlay beside its lower directory has (§ 8.5, L1).
//!
//! No case has a privilege. Each run makes its mounts in a user and mount namespace of its own,
//! `unshare --user --map-root-user --mount`, and starts the shipped `substrate-exec-gate` there
//! with an empty environment, so the helper reads that namespace's mount table as it reads a
//! view's. The cases drive the helper and not the driver: a driver opened inside such a
//! namespace was not probed (design 35 § 18).
//!
//! Where the namespace cannot be created, a mount is refused there, the kernel has no Landlock
//! or the scratch directory is on a file-system type outside the list, a case prints
//! `ABSENT` and the reason and is **not passed**. That is why these cases are in no required
//! set of `scripts/delegated-lane.sh`: a hosted runner's `AppArmor` policy grants a user
//! namespace no mount.

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

fn follow(root: &Path, rel: &str) -> Located {
    Located::new(Class::Follow, root.to_str().expect("UTF-8"), rel).expect("a follow path")
}

/// The file system a case puts its tree on.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// The scratch directory's own file system.
    Scratch,
    Tmpfs,
    Overlay,
}

/// One run's scratch root. `src` holds the tree: `one/{tool,other}`, `tree/bin/{tool,other,cup,
/// cupok}`, each a copy of `true`, and `one/into`, a symbolic link to `tree/bin`. The stage
/// puts it at `fs` on the case's file system, binds `fs/one` a second time at `fs/two` and
/// `fs/tree/bin` at `fs/x`.
struct Scratch {
    _directory: tempfile::TempDir,
    root: PathBuf,
    stage: String,
}

impl Scratch {
    /// A fresh root for one run, or the reason this host has no tool the stage needs.
    fn new(kind: Kind) -> Result<Self, String> {
        let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
        let root = directory.path().canonicalize().expect("canonical scratch");
        let src = root.join("src");
        for directory in ["one", "tree/bin"] {
            std::fs::create_dir_all(src.join(directory)).expect(directory);
        }
        for name in [
            "one/tool",
            "one/other",
            "tree/bin/tool",
            "tree/bin/other",
            "tree/bin/cup",
            "tree/bin/cupok",
        ] {
            std::fs::copy(tool("true"), src.join(name)).expect("copy true");
        }
        std::os::unix::fs::symlink("../tree/bin", src.join("one/into")).expect("a link");
        let needed = |name: &str| {
            find(name)
                .map(|path| path.to_str().expect("UTF-8").to_owned())
                .ok_or_else(|| format!("no {name} on this host"))
        };
        let (mount, mkdir, cp) = (needed("mount")?, needed("mkdir")?, needed("cp")?);
        let at = root.to_str().expect("UTF-8");
        let tree = match kind {
            Kind::Scratch => format!("{cp} -RP src fs"),
            Kind::Tmpfs => {
                format!("{mkdir} fs && {mount} -t tmpfs tmpfs fs && {cp} -RP src/. fs/")
            }
            Kind::Overlay => format!(
                "{mkdir} fs upper work && {mount} -t overlay overlay \
                 -o lowerdir={at}/src,upperdir={at}/upper,workdir={at}/work fs"
            ),
        };
        let stage = format!(
            "{tree} && {mkdir} fs/two fs/x && {mount} --bind fs/one fs/two \
             && {mount} --bind fs/tree/bin fs/x"
        );
        Ok(Self {
            _directory: directory,
            root,
            stage,
        })
    }

    /// The lists of one run: the shell as the program, `allow` and `deny` beneath this root.
    fn lists(&self, allow: &[&str], deny: &[&str], writable: &[&str]) -> Lists {
        let located =
            |entries: &[&str]| entries.iter().map(|rel| follow(&self.root, rel)).collect();
        Lists {
            program: base(&tool("sh")),
            loader: substrate_exec_gate::elf_interpreter(&std::fs::read(tool("env")).expect("env")),
            allow: located(allow),
            deny: located(deny),
            writable: writable
                .iter()
                .map(|rel| self.root.join(rel).to_str().expect("UTF-8").to_owned())
                .collect(),
        }
    }

    /// Makes the namespace and the mounts, then starts the helper with `lists` and
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

/// Why this host cannot run a case, if it cannot: no `unshare`, or no namespace.
fn unavailable() -> Option<String> {
    let Some(unshare) = find("unshare") else {
        return Some("no unshare on this host".to_owned());
    };
    let probe = Command::new(unshare)
        .args(["--user", "--map-root-user", "--mount", "--"])
        .arg(tool("true"))
        .output()
        .expect("unshare starts");
    (!probe.status.success()).then(|| {
        format!(
            "unshare cannot create the namespace: {}",
            text(&probe.stderr).trim()
        )
    })
}

/// The standard output of one run, or `None` after printing the case as absent: the stage
/// failed, the kernel has no Landlock, or a place could not be established on this host.
fn observed(name: &str, output: &Output) -> Option<String> {
    let diagnostic = text(&output.stderr);
    let reason = match output.status.code() {
        Some(STAGE_FAILED) => Some(format!("the stage failed: {}", diagnostic.trim())),
        Some(REFUSED)
            if diagnostic.contains("Landlock is unavailable")
                || diagnostic.contains("cannot establish the place") =>
        {
            Some(diagnostic.trim().to_owned())
        }
        _ => None,
    };
    if let Some(reason) = reason {
        println!("exec gate namespace {name}: ABSENT ({reason})");
        return None;
    }
    Some(text(&output.stdout))
}

/// The entries of one run beneath its root: the allow-list, the deny-list, the writable mounts.
type Entries<'a> = (&'a [&'a str], &'a [&'a str], &'a [&'a str]);

/// One run on a fresh root of `kind`: its standard output, or `None` after printing the case
/// as absent.
fn run(
    name: &str,
    kind: Kind,
    (allow, deny, writable): Entries<'_>,
    script: &str,
) -> Option<(Output, String)> {
    let scratch = match Scratch::new(kind) {
        Ok(scratch) => scratch,
        Err(reason) => {
            println!("exec gate namespace {name}: ABSENT ({reason})");
            return None;
        }
    };
    let output = scratch.run(&scratch.lists(allow, deny, writable), script);
    let stdout = observed(name, &output)?;
    Some((output, stdout))
}

/// The lists of design 35 § 3.9 that end in a run (F1 to F6, and the control of F7): what each
/// states, its entries, its script and what the script prints.
fn lists_that_run() -> [(&'static str, Entries<'static>, String, &'static str); 8] {
    let both = "fs/tree/bin/tool 2>/dev/null; echo $?; fs/x/tool 2>/dev/null; echo $?; \
                fs/tree/bin/other; echo $?";
    let copied_up = "fs/tree/bin/cup 2>/dev/null; echo $?; fs/x/cup 2>/dev/null; echo $?; \
                     fs/tree/bin/cupok; echo $?; fs/x/cupok; echo $?";
    [
        (
            "F1, one directory at two host paths, the program denied under the second",
            (&["fs/one"], &["fs/two/tool"], &[]),
            "fs/one/tool 2>/dev/null; echo $?; fs/two/tool 2>/dev/null; echo $?; \
             fs/one/other; echo $?; fs/two/other; echo $?"
                .to_owned(),
            "126\n126\n0\n0\n",
        ),
        (
            "F1, the reverse",
            (&["fs/two"], &["fs/one/tool"], &[]),
            "fs/one/tool 2>/dev/null; echo $?; fs/two/tool 2>/dev/null; echo $?; \
             fs/one/other; echo $?; fs/two/other; echo $?"
                .to_owned(),
            "126\n126\n0\n0\n",
        ),
        (
            "F2, a part of a listed directory at a second path, the program denied there",
            (&["fs/tree"], &["fs/x/tool"], &[]),
            both.to_owned(),
            "126\n126\n0\n",
        ),
        (
            "F3, the same program denied under its listed name",
            (&["fs/tree"], &["fs/tree/bin/tool"], &[]),
            both.to_owned(),
            "126\n126\n0\n",
        ),
        (
            "F4 and F5, a file denied through the second path, before and after the run \
             appends to it and to its neighbour",
            (&["fs/tree"], &["fs/x/cup"], &[]),
            format!("{copied_up}; echo >> fs/tree/bin/cup; echo >> fs/x/cupok; {copied_up}"),
            "126\n126\n0\n0\n126\n126\n0\n0\n",
        ),
        (
            "F6, a listed directory inside a denied one",
            (&["fs/tree/bin"], &["fs/tree"], &[]),
            "fs/tree/bin/other 2>/dev/null; echo $?".to_owned(),
            "126\n",
        ),
        (
            "F6, the control without the deny entry",
            (&["fs/tree/bin"], &[], &[]),
            "fs/tree/bin/other; echo $?".to_owned(),
            "0\n",
        ),
        (
            "F7, the control: a followed link whose target no writable mount holds",
            (&["fs/one/into"], &[], &[]),
            "fs/one/into/other; echo $?".to_owned(),
            "0\n",
        ),
    ]
}

/// The lists of design 35 § 3.9 (F1 to F7) on one file system. Returns whether every run ran.
fn the_lists_hold(name: &str, kind: Kind) -> bool {
    if let Some(reason) = unavailable() {
        println!("exec gate namespace {name}: ABSENT ({reason})");
        return false;
    }
    for (what, lists, script, expected) in lists_that_run() {
        let Some((output, stdout)) = run(name, kind, lists, &script) else {
            return false;
        };
        assert_eq!(
            stdout,
            expected,
            "{kind:?}: {what}: {}",
            text(&output.stderr)
        );
    }
    // F7: the same link with its target's directory as a mount the run can write is refused,
    // and nothing runs.
    let Some((output, stdout)) = run(
        name,
        kind,
        (&["fs/one/into"], &[], &["fs/tree"]),
        "echo ran",
    ) else {
        return false;
    };
    let diagnostic = text(&output.stderr);
    assert_eq!(
        (output.status.code(), stdout.as_str()),
        (Some(REFUSED), ""),
        "{kind:?}: F7: {diagnostic}"
    );
    assert!(
        diagnostic.contains("in the writable mount"),
        "{kind:?}: F7: the line of D24: {diagnostic}"
    );
    true
}

/// D25, a host bind mount (K5): one directory bound at a second host path, on the scratch
/// directory's own file system. A program denied under one name is denied under both and its
/// neighbour runs, for a whole directory and for a part of a listed one.
#[test]
fn the_place_rule_holds_on_a_host_bind_mount() {
    const NAME: &str = "host bind mount";
    if the_lists_hold(NAME, Kind::Scratch) {
        println!("exec gate namespace {NAME}: executed");
    }
}

/// D26: the same lists with every listed and denied path on a tmpfs.
#[test]
fn the_place_rule_holds_on_a_tmpfs() {
    const NAME: &str = "tmpfs";
    if the_lists_hold(NAME, Kind::Tmpfs) {
        println!("exec gate namespace {NAME}: executed");
    }
}

/// D26: the same lists on an overlay. `cup` and `cupok` exist only in the lower directory at
/// the start; the run's append copies each up, and the denied one stays denied under both
/// names while the other still runs (F5).
#[test]
fn the_place_rule_holds_on_an_overlay() {
    const NAME: &str = "overlay";
    if the_lists_hold(NAME, Kind::Overlay) {
        println!("exec gate namespace {NAME}: executed");
    }
}

/// The limit of design 35 § 8.5 (L1, D26): **a deny entry does not find the same bytes in a
/// layer directory of an overlay that the view holds beside the overlay.** With the overlay
/// and its lower directory both listed and a program denied in the overlay, the program is
/// denied there and runs from the lower directory: a layer is another file system to the
/// kernel, with another place. The embedder notes say what follows: never list a lower
/// directory of an overlay that is also in the view.
#[test]
fn limit_an_overlay_and_its_lower_directory_both_listed_run_the_denied_program_from_the_lower_one()
{
    const NAME: &str = "limit overlay lower directory";
    if let Some(reason) = unavailable() {
        println!("exec gate namespace {NAME}: ABSENT ({reason})");
        return;
    }
    let Some((output, stdout)) = run(
        NAME,
        Kind::Overlay,
        (&["fs", "src"], &["fs/tree/bin/tool"], &[]),
        "fs/tree/bin/tool 2>/dev/null; echo $?; src/tree/bin/tool; echo $?; \
         fs/tree/bin/other; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        stdout,
        "126\n0\n0\n",
        "denied in the overlay, run from its lower directory, and the neighbour: the limit \
         holds as stated: {}",
        text(&output.stderr)
    );
    println!(
        "exec gate namespace {NAME}: executed, the denied program runs from the lower directory"
    );
}
