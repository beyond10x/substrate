//! The place rule of design 35 (D25, D26) on mounts a test has to make: a host bind mount, a
//! tmpfs and an overlay (§ 16, *D25, a host bind mount* and *D26, the file systems*), and the
//! limit an overlay beside its lower directory has (§ 8.5, L1).
//!
//! No case has a privilege. Each case makes its mounts once, in a user and mount namespace of
//! its own, `unshare --user --map-root-user --mount`, and starts the shipped
//! `substrate-exec-gate` there once per list, each time with an empty environment, so the
//! helper reads that namespace's mount table as it reads a view's. The cases drive the helper
//! and not the driver: a driver opened inside such a namespace was not probed (design 35
//! § 18).
//!
//! Where the namespace cannot be created, a mount is refused there, the kernel has no Landlock
//! or the scratch directory is on a file-system type outside the list, a case prints
//! `ABSENT` and the reason and is **not passed**. An overlay that cannot be mounted has its
//! own reason, `no overlay mount in a user namespace on this host`, and every other command
//! of a stage `the stage failed`. `scripts/delegated-lane.sh` requires each case by name: its
//! `executed` line where a user namespace is granted a mount, and its own `ABSENT` line where
//! it is not, as on a hosted runner, whose `AppArmor` policy grants a user namespace none.
//! There these cases are workstation evidence.

#![cfg(target_os = "linux")]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use substrate_exec_gate::{Class, Lists, Located, REFUSED};

/// What a stage exits with when one of its own commands failed, before the helper starts.
const STAGE_FAILED: i32 = 97;

/// What a stage exits with when the overlay itself could not be mounted: the one thing an
/// overlay case needs beyond what the other cases need.
const OVERLAY_REFUSED: i32 = 96;

/// One argument as a word of shell text.
fn quoted(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', "'\\''"))
}

/// What one start of the helper came to inside a case's namespace.
struct Seen {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

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
                "{mkdir} fs upper work && {{ {mount} -t overlay overlay \
                 -o lowerdir={at}/src,upperdir={at}/upper,workdir={at}/work fs \
                 || exit {OVERLAY_REFUSED}; }}"
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

    /// Makes the namespace and the mounts once, then starts the helper once per row, with
    /// the row's lists and `sh -c script`, in this root and with an empty environment. One
    /// stage serves every row: the mounts are the same for each, and a stage per row was
    /// most of what a case cost. The standard streams and the exit status of each start are
    /// kept beneath `rows/`. Returns the exit status and the diagnostics of the namespace's
    /// own shell, and what each start came to.
    fn run(&self, rows: &[(Lists, &str)]) -> (Option<i32>, String, Vec<Seen>) {
        let kept = self.root.join("rows");
        std::fs::create_dir(&kept).expect("the directory of the rows");
        let word = |path: PathBuf| quoted(path.to_str().expect("UTF-8"));
        let (env, shell) = (word(tool("env")), word(tool("sh")));
        let helper = quoted(env!("CARGO_BIN_EXE_substrate-exec-gate"));
        let mut script = format!("{{ {}; }} >&2 || exit {STAGE_FAILED}\n", self.stage);
        for (index, (lists, row)) in rows.iter().enumerate() {
            let options: Vec<String> = lists
                .options()
                .iter()
                .map(|option| quoted(option))
                .collect();
            let _ = writeln!(
                script,
                "{env} -i {helper} {} {shell} -c {} >rows/{index}.out 2>rows/{index}.err\n\
                 echo $? >rows/{index}.status",
                options.join(" "),
                quoted(row)
            );
        }
        let output = Command::new(tool("unshare"))
            .args(["--user", "--map-root-user", "--mount", "--"])
            .arg(tool("sh"))
            .arg("-c")
            .arg(script)
            .current_dir(&self.root)
            .env_clear()
            .output()
            .expect("unshare starts");
        let read = |name: String| std::fs::read_to_string(kept.join(name)).unwrap_or_default();
        let seen = (0..rows.len())
            .map(|index| Seen {
                status: read(format!("{index}.status")).trim().parse().ok(),
                stdout: read(format!("{index}.out")),
                stderr: read(format!("{index}.err")),
            })
            .collect();
        (output.status.code(), text(&output.stderr), seen)
    }
}

/// The entries of one run beneath its root: the allow-list, the deny-list, the writable mounts.
type Entries<'a> = (&'a [&'a str], &'a [&'a str], &'a [&'a str]);

/// Every row on one fresh root of `kind`, in one namespace: what each start came to, or
/// `None` after printing the case as absent. A case is absent where this host has no tool
/// the stage needs, the namespace cannot be created, the overlay cannot be mounted, another
/// command of the stage failed, the kernel has no Landlock, or a place could not be
/// established on this host; each has its own reason.
fn run(name: &str, kind: Kind, rows: &[(Entries<'_>, &str)]) -> Option<Vec<Seen>> {
    let absent = |reason: &str| {
        println!("exec gate namespace {name}: ABSENT ({reason})");
        None
    };
    if find("unshare").is_none() {
        return absent("no unshare on this host");
    }
    let scratch = match Scratch::new(kind) {
        Ok(scratch) => scratch,
        Err(reason) => return absent(&reason),
    };
    let listed: Vec<(Lists, &str)> = rows
        .iter()
        .map(|((allow, deny, writable), script)| (scratch.lists(allow, deny, writable), *script))
        .collect();
    let (status, diagnostic, seen) = scratch.run(&listed);
    let diagnostic = diagnostic.trim();
    match status {
        Some(OVERLAY_REFUSED) => {
            return absent(&format!(
                "no overlay mount in a user namespace on this host: {diagnostic}"
            ));
        }
        Some(STAGE_FAILED) => return absent(&format!("the stage failed: {diagnostic}")),
        Some(1) if diagnostic.starts_with("unshare:") => {
            return absent(&format!(
                "unshare cannot create the namespace: {diagnostic}"
            ));
        }
        _ => {}
    }
    if let Some(unplaced) = seen.iter().find(|seen| {
        seen.status == Some(REFUSED)
            && (seen.stderr.contains("Landlock is unavailable")
                || seen.stderr.contains("cannot establish the place"))
    }) {
        return absent(unplaced.stderr.trim());
    }
    Some(seen)
}

/// The lists of design 35 § 3.9 that end in a run (F1 to F6, and the control of F7), and one
/// of D30: what each states, its entries, its script and what the script prints.
fn lists_that_run() -> [(&'static str, Entries<'static>, String, &'static str); 9] {
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
            // The walk of D11 meets `fs/x` as an entry of the expanded `fs`. It is a mount
            // point, and its place is the one its own row gives it, which is the denied
            // directory: with the directory's place and its name it would be `fs/x`, which
            // nothing denies, and the programs in it would get a rule each.
            "D30, a denied directory that is a mount point among the entries of a listed one",
            (&["fs"], &["fs/x"], &[]),
            "fs/x/tool 2>/dev/null; echo $?; fs/tree/bin/other 2>/dev/null; echo $?; \
             fs/one/tool; echo $?"
                .to_owned(),
            "126\n126\n0\n",
        ),
        (
            "F7, the control: a followed link whose target no writable mount holds",
            (&["fs/one/into"], &[], &[]),
            "fs/one/into/other; echo $?".to_owned(),
            "0\n",
        ),
    ]
}

/// The lists of design 35 § 3.9 (F1 to F7) on one file system, every list in the one
/// namespace of the case. Returns whether the case ran.
fn the_lists_hold(name: &str, kind: Kind) -> bool {
    let lists = lists_that_run();
    let mut rows: Vec<(Entries<'_>, &str)> = lists
        .iter()
        .map(|(_, entries, script, _)| (*entries, script.as_str()))
        .collect();
    // F7: the same link with its target's directory as a mount the run can write is refused,
    // and nothing runs.
    rows.push(((&["fs/one/into"], &[], &["fs/tree"]), "echo ran"));
    let Some(seen) = run(name, kind, &rows) else {
        return false;
    };
    for ((what, _, _, expected), seen) in lists.iter().zip(&seen) {
        assert_eq!(seen.stdout, *expected, "{kind:?}: {what}: {}", seen.stderr);
    }
    let refused = seen.last().expect("the row of F7");
    assert_eq!(
        (refused.status, refused.stdout.as_str()),
        (Some(REFUSED), ""),
        "{kind:?}: F7: {}",
        refused.stderr
    );
    assert!(
        refused.stderr.contains("in the writable mount"),
        "{kind:?}: F7: the line of D24: {}",
        refused.stderr
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
    let Some(seen) = run(
        NAME,
        Kind::Overlay,
        &[(
            (&["fs", "src"], &["fs/tree/bin/tool"], &[]),
            "fs/tree/bin/tool 2>/dev/null; echo $?; src/tree/bin/tool; echo $?; \
             fs/tree/bin/other; echo $?",
        )],
    ) else {
        return;
    };
    assert_eq!(
        seen[0].stdout, "126\n0\n0\n",
        "denied in the overlay, run from its lower directory, and the neighbour: the limit \
         holds as stated: {}",
        seen[0].stderr
    );
    println!(
        "exec gate namespace {NAME}: executed, the denied program runs from the lower directory"
    );
}
