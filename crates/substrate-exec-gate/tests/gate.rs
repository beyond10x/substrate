//! The shipped `substrate-exec-gate`, started as the driver starts it (design 35 D4, D6, D24).
//!
//! Each case runs the binary once. The cases that need the kernel to apply a ruleset print
//! `ABSENT` and their reason on a kernel without Landlock, and are then not passed: the helper
//! itself exits 125 there, which the first case of that kind asserts.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use substrate_exec_gate::{Class, Lists, Located, PROTOCOL, REFUSED};

fn gate() -> Command {
    Command::new(env!("CARGO_BIN_EXE_substrate-exec-gate"))
}

/// The host's copy of a base tool, where this distribution keeps it.
fn tool(name: &str) -> PathBuf {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {name} in /usr/bin or /bin"))
}

/// A base tool as a `follow` path: its directory is a mount of the driver's view.
fn base(path: &Path) -> Located {
    let text = path.to_str().expect("UTF-8");
    let (root, rel) = if let Some(rel) = text.strip_prefix("/usr/") {
        ("/usr", rel)
    } else if text == "/bin" {
        ("/bin", ".")
    } else {
        ("/bin", text.strip_prefix("/bin/").expect("a base tool"))
    };
    Located::new(Class::Follow, root, rel).expect("a base path")
}

/// The loader the host's base programs name, as the driver reads it at open (D7).
fn loader() -> Option<String> {
    substrate_exec_gate::elf_interpreter(&std::fs::read(tool("env")).expect("env"))
}

/// A scratch root with `allowed/true`, `outside/true` and an empty `elsewhere`.
fn scratch() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    for name in ["allowed", "outside", "elsewhere"] {
        std::fs::create_dir(root.join(name)).expect(name);
    }
    for name in ["allowed", "outside"] {
        std::fs::copy(tool("true"), root.join(name).join("true")).expect("copy true");
    }
    (directory, root)
}

/// Runs `/bin/sh -c script` in `root` through the gate, with `allow` beside the shell itself.
fn shell(root: &Path, allow: Vec<Located>, script: &str) -> Output {
    shell_in_view(root, allow, &[], script)
}

/// [`shell`], with `writable` as the mounts of the view the run can write.
fn shell_in_view(root: &Path, allow: Vec<Located>, writable: &[&Path], script: &str) -> Output {
    let sh = tool("sh");
    let lists = Lists {
        deny: Vec::new(),
        program: base(&sh),
        loader: loader(),
        allow,
        writable: writable
            .iter()
            .map(|path| path.to_str().expect("UTF-8").to_owned())
            .collect(),
    };
    gate()
        .args(lists.options())
        .arg(&sh)
        .args(["-c", script])
        .current_dir(root)
        .env_clear()
        .output()
        .expect("the gate starts")
}

fn beneath(root: &Path, rel: &str) -> Located {
    Located::new(Class::Beneath, root.to_str().expect("UTF-8"), rel).expect("a beneath path")
}

fn follow(root: &Path, rel: &str) -> Located {
    Located::new(Class::Follow, root.to_str().expect("UTF-8"), rel).expect("a follow path")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Whether `output` is the helper refusing for want of Landlock; prints the case as absent then.
fn absent(name: &str, output: &Output) -> bool {
    let diagnostic = text(&output.stderr);
    if output.status.code() == Some(REFUSED) && diagnostic.contains("Landlock is unavailable") {
        println!("exec gate {name}: ABSENT ({})", diagnostic.trim());
        return true;
    }
    false
}

/// A refusal: status 125, one diagnostic line with the helper's prefix, and nothing on stdout.
fn assert_refused(output: &Output, what: &str) {
    let diagnostic = text(&output.stderr);
    assert_eq!(output.status.code(), Some(REFUSED), "{what}: {diagnostic}");
    assert!(
        diagnostic.starts_with("substrate-exec-gate: ")
            && diagnostic.trim_end().lines().count() == 1,
        "{what}: one diagnostic line, got {diagnostic:?}"
    );
    assert!(output.stdout.is_empty(), "{what}: the program ran");
}

#[test]
fn version_prints_the_protocol_line() {
    let output = gate().arg("--version").output().expect("the gate starts");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), format!("{PROTOCOL}\n"));
}

/// D24: an argument list the grammar does not admit ends with status 125, one diagnostic line,
/// and the program was not executed.
#[test]
fn each_malformed_argument_list_exits_125_and_executes_nothing() {
    let (_directory, root) = scratch();
    let sh = tool("sh");
    let program = base(&sh);
    let marker = root.join("ran");
    let script = format!("echo ran; : > '{}'", marker.display());
    let run = |options: &[&str]| {
        gate()
            .args(options)
            .arg("--")
            .arg(&sh)
            .args(["-c", &script])
            .output()
            .expect("the gate starts")
    };
    let class = program.class().word();
    let cases: [(&str, Vec<&str>); 8] = [
        ("no --program", vec![]),
        (
            "an unknown option",
            vec!["--program", class, program.root(), program.rel(), "--bogus"],
        ),
        (
            "--deny without its operands",
            vec![
                "--program",
                class,
                program.root(),
                program.rel(),
                "--deny",
                "follow",
                "/usr",
            ],
        ),
        (
            "--alias, which this helper does not implement",
            vec![
                "--program",
                class,
                program.root(),
                program.rel(),
                "--alias",
                "/usr/bin",
                "/bin",
            ],
        ),
        (
            "a missing operand",
            vec!["--program", class, program.root()],
        ),
        (
            "a rel that is not normal",
            vec![
                "--program",
                class,
                program.root(),
                program.rel(),
                "--allow",
                "follow",
                "/usr",
                "bin/../lib",
            ],
        ),
        (
            "a second --program",
            vec![
                "--program",
                class,
                program.root(),
                program.rel(),
                "--program",
                class,
                program.root(),
                program.rel(),
            ],
        ),
        (
            "a program that differs from its option",
            vec!["--program", "follow", "/usr", "bin/definitely-not-sh"],
        ),
    ];
    for (what, options) in cases {
        assert_refused(&run(&options), what);
        assert!(!marker.exists(), "{what}: the program ran");
    }
    let output = gate()
        .args(["--version", "extra"])
        .output()
        .expect("starts");
    assert_refused(&output, "--version with another argument");
}

/// The acceptance, at the helper: with one directory listed, a program beneath it executes and a
/// program outside it fails with `EACCES`, which the shell reports as status 126.
#[test]
fn a_listed_directory_executes_and_a_program_outside_it_is_refused() {
    const NAME: &str = "listed directory";
    let (_directory, root) = scratch();
    let output = shell(
        &root,
        vec![beneath(&root, "allowed")],
        "allowed/true; echo $?; outside/true 2>/dev/null; echo $?; /usr/bin/env true 2>/dev/null; echo $?",
    );
    if absent(NAME, &output) {
        return;
    }
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "0\n126\n126\n",
        "the listed program ran; the unlisted one and a base tool were refused"
    );
    println!("exec gate {NAME}: executed");
}

/// D2: an entry that names one file admits that file and not its sibling.
#[test]
fn a_file_entry_admits_that_file_only() {
    const NAME: &str = "file entry";
    let (_directory, root) = scratch();
    std::fs::copy(tool("true"), root.join("allowed/other")).expect("sibling");
    let output = shell(
        &root,
        vec![beneath(&root, "allowed/true")],
        "allowed/true; echo $?; allowed/other 2>/dev/null; echo $?",
    );
    if absent(NAME, &output) {
        return;
    }
    assert_eq!(text(&output.stdout), "0\n126\n", "{}", text(&output.stderr));
    println!("exec gate {NAME}: executed");
}

/// D6: refer is granted on `/`, so a hard link and a rename between two unlisted directories
/// still work under the ruleset.
///
/// The two tools are listed and started by their own paths, each a `follow` entry, so the rule
/// is on the file the name resolves to. Listing the directory of the name is not the same
/// thing, and searching the shell's default path is not either: on Ubuntu 26.04 `/usr/bin/ln`
/// is a symbolic link to `/usr/lib/cargo/bin/coreutils/ln`, and a rule on `/usr/bin` does not
/// reach it (the limit of the case below). This case listed `/usr/bin` and called `ln` by name
/// until it met that layout.
#[test]
fn a_link_and_a_rename_between_two_unlisted_directories_still_work() {
    const NAME: &str = "refer";
    let (_directory, root) = scratch();
    let (ln, mv) = (tool("ln"), tool("mv"));
    let output = shell(
        &root,
        vec![base(&ln), base(&mv)],
        &format!(
            "{ln} outside/true elsewhere/linked && {mv} elsewhere/linked outside/moved && \
             {mv} outside/moved outside/renamed && echo done",
            ln = ln.display(),
            mv = mv.display()
        ),
    );
    if absent(NAME, &output) {
        return;
    }
    assert_eq!(text(&output.stdout), "done\n", "{}", text(&output.stderr));
    assert!(root.join("outside/renamed").is_file());
    println!("exec gate {NAME}: executed");
}

/// The limit behind the case above, held as it is: **a listed directory admits the files beneath
/// it, not what a symbolic link in it points to.** The kernel decides on the file a path
/// resolves to. With `listed` as the entry, `listed/link`, a link to a program in `elsewhere`,
/// is refused; with the link itself as a `follow` entry the rule is on the program it resolves
/// to, which then runs by either name. A distribution whose base tools are links out of
/// `/usr/bin` needs the second form, or the directory the links point into.
#[test]
fn limit_a_link_in_a_listed_directory_that_points_outside_it_is_not_admitted() {
    const NAME: &str = "limit link out of a listed directory";
    let (_directory, root) = scratch();
    std::fs::create_dir(root.join("listed")).expect("listed");
    std::fs::copy(tool("true"), root.join("listed/true")).expect("copy true");
    std::os::unix::fs::symlink("../outside/true", root.join("listed/link")).expect("a link");
    let script = "listed/true; echo $?; listed/link 2>/dev/null; echo $?; \
                  outside/true 2>/dev/null; echo $?";
    let output = shell(&root, vec![follow(&root, "listed")], script);
    if absent(NAME, &output) {
        return;
    }
    assert_eq!(
        text(&output.stdout),
        "0\n126\n126\n",
        "the directory admits its own file and not the target of its link: {}",
        text(&output.stderr)
    );
    let output = shell(
        &root,
        vec![follow(&root, "listed"), follow(&root, "listed/link")],
        script,
    );
    assert_eq!(
        text(&output.stdout),
        "0\n0\n0\n",
        "the link as an entry is a rule on what it resolves to: {}",
        text(&output.stderr)
    );
    println!("exec gate {NAME}: executed");
}

/// D24, `beneath`: a symbolic link in any part of the path is an error, so an entry swapped for a
/// link to `/usr/bin` after the driver's check admits nothing.
#[test]
fn a_beneath_entry_with_a_symbolic_link_is_refused() {
    let (_directory, root) = scratch();
    std::os::unix::fs::symlink("/usr/bin", root.join("bin")).expect("link");
    std::os::unix::fs::symlink("allowed", root.join("alias")).expect("link");
    for rel in ["bin", "alias/true", "alias"] {
        let output = shell(&root, vec![beneath(&root, rel)], "echo ran");
        assert_refused(&output, &format!("beneath entry {rel}"));
    }
}

/// Whether the place of `path` can be established on this host, or why not.
fn place_of(path: &Path) -> Result<(), String> {
    use substrate_exec_gate::place::{MountTable, Opened};
    let table = MountTable::read().map_err(|cause| cause.to_string())?;
    let opened = Opened::of_path(path.to_str().expect("UTF-8")).map_err(|why| why.to_string())?;
    table
        .place_of(&opened)
        .map(drop)
        .map_err(|why| why.to_string())
}

/// A scratch root in the build's own directory. The place of a path is established only on the
/// listed file-system types (D26). `None`, after printing the case as absent, where this
/// directory or the shell is on another type.
fn placed_scratch(name: &str) -> Option<(tempfile::TempDir, PathBuf)> {
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    // The shell is a followed path of every case, so its place is asked as well.
    let placed = place_of(&root).and_then(|()| place_of(&tool("sh")));
    if let Err(why) = placed {
        println!("exec gate {name}: ABSENT ({why})");
        return None;
    }
    for name in ["writable", "read-only"] {
        std::fs::create_dir(root.join(name)).expect(name);
    }
    std::fs::copy(tool("true"), root.join("writable/true")).expect("copy true");
    std::fs::create_dir(root.join("writable/deep")).expect("deep");
    Some((directory, root))
}

/// D24 (K9): a `follow` path that resolves into a mount the run can write is refused, with the
/// line that names the option, the path as written, what it resolved to and the mount. The
/// read-only side cannot hand out a rule on something a run can replace.
#[test]
fn a_follow_entry_that_resolves_into_a_writable_mount_is_refused() {
    const NAME: &str = "follow into a writable mount";
    let Some((_directory, root)) = placed_scratch(NAME) else {
        return;
    };
    let writable = root.join("writable");
    let read_only = root.join("read-only");
    for (rel, target) in [("file", "true"), ("directory", "."), ("deep", "deep")] {
        let target = writable.join(target).canonicalize().expect("the target");
        std::os::unix::fs::symlink(&target, read_only.join(rel)).expect("link");
        let entry = follow(&read_only, rel);
        let expected = format!(
            "substrate-exec-gate: --allow {} resolves to {}, in the writable mount {}\n",
            entry.path(),
            target.display(),
            writable.display()
        );
        let output = shell_in_view(&root, vec![entry], &[&writable], "echo ran");
        assert_refused(&output, &format!("follow entry {rel}"));
        assert_eq!(text(&output.stderr), expected, "the line of D24");
    }
    // The control: the same view admits a followed entry that stays outside the writable mount.
    std::fs::copy(tool("true"), read_only.join("tool")).expect("copy true");
    let output = shell_in_view(
        &root,
        vec![follow(&read_only, "tool")],
        &[&writable],
        "read-only/tool && echo ran",
    );
    if absent(NAME, &output) {
        return;
    }
    assert_eq!(text(&output.stdout), "ran\n", "{}", text(&output.stderr));
    println!("exec gate {NAME}: executed");
}

/// D26, fail closed: a followed path whose place cannot be established is refused with the
/// cause, never decided by its text. `/proc` is on a file-system type outside the list (U1).
#[test]
fn a_follow_entry_whose_place_cannot_be_established_is_refused() {
    let Some((_directory, root)) = placed_scratch("unestablished place") else {
        return;
    };
    let proc = Located::new(Class::Follow, "/proc", "version").expect("a follow path");
    let output = shell_in_view(&root, vec![proc], &[&root.join("writable")], "echo ran");
    assert_refused(&output, "a followed path on procfs");
    assert_eq!(
        text(&output.stderr),
        "substrate-exec-gate: cannot establish the place of /proc/version: \
         file-system-unlisted proc\n"
    );
    // A `--writable` root whose place cannot be established refuses too: nothing is compared
    // with a mount the helper could not place.
    let output = shell_in_view(
        &root,
        vec![follow(&root, "read-only")],
        &[Path::new("/proc")],
        "echo ran",
    );
    assert_refused(&output, "a writable root on procfs");
    assert_eq!(
        text(&output.stderr),
        "substrate-exec-gate: cannot establish the place of /proc: file-system-unlisted proc\n"
    );
    println!("exec gate unestablished place: executed");
}

/// D24: a `beneath` path whose `<rel>` leaves its root is refused, as a link in it is (K10).
#[test]
fn a_beneath_root_that_is_a_link_is_refused() {
    let (_directory, root) = scratch();
    std::os::unix::fs::symlink(root.join("allowed"), root.join("root-link")).expect("link");
    for rel in [".", "true"] {
        let output = shell(
            &root,
            vec![beneath(&root.join("root-link"), rel)],
            "echo ran",
        );
        assert_refused(&output, &format!("a root that is a link, {rel}"));
    }
}

/// D24: an `--allow` path that does not open is an error, not an entry left out.
#[test]
fn an_entry_that_does_not_open_is_refused() {
    let (_directory, root) = scratch();
    for entry in [beneath(&root, "absent"), follow(&root, "absent")] {
        assert_refused(&shell(&root, vec![entry], "echo ran"), "an absent entry");
    }
}

/// When the program itself cannot be executed the status is 126, as `env` answers there.
#[test]
fn a_program_that_cannot_be_executed_exits_126() {
    const NAME: &str = "program exec failure";
    let (_directory, root) = scratch();
    let lists = Lists {
        deny: Vec::new(),
        program: beneath(&root, "allowed"),
        loader: loader(),
        allow: Vec::new(),
        writable: Vec::new(),
    };
    let output = gate()
        .args(lists.options())
        .arg(root.join("allowed"))
        .env_clear()
        .output()
        .expect("the gate starts");
    if absent(NAME, &output) {
        return;
    }
    assert_eq!(output.status.code(), Some(126), "{}", text(&output.stderr));
    assert!(text(&output.stderr).starts_with("substrate-exec-gate: "));
    println!("exec gate {NAME}: executed");
}

/// A16: the run's program holds no descriptor of the helper's: every one it opened is
/// close-on-exec, the ruleset's included.
#[test]
fn the_program_inherits_no_descriptor_of_the_helper() {
    const NAME: &str = "descriptors";
    const SCRIPT: &str = "for f in /proc/$$/fd/*; do echo \"${f##*/}\"; done; echo end";
    let (_directory, root) = scratch();
    let output = shell(&root, vec![beneath(&root, "allowed")], SCRIPT);
    if absent(NAME, &output) {
        return;
    }
    // The control: the same shell started without the helper, from this same process.
    let control = Command::new(tool("sh"))
        .args(["-c", SCRIPT])
        .current_dir(&root)
        .env_clear()
        .output()
        .expect("the control starts");
    assert!(text(&control.stdout).ends_with("end\n"), "the control ran");
    assert_eq!(
        text(&output.stdout),
        text(&control.stdout),
        "{}",
        text(&output.stderr)
    );
    println!("exec gate {NAME}: executed");
}

/// The gate about to run `program` in `root`, held to it alone, with `environment` as `--env`
/// operands and an empty environment of its own.
fn gate_with(root: &Path, program: &Path, environment: &[(&str, &str)]) -> Command {
    let lists = Lists {
        deny: Vec::new(),
        program: base(program),
        loader: loader(),
        allow: Vec::new(),
        writable: Vec::new(),
    };
    let environment: Vec<(String, String)> = environment
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    let mut command = gate();
    command
        .args(lists.options_with(&environment))
        .arg(program)
        .current_dir(root)
        .env_clear();
    command
}

/// D28: the program's environment is the `--env` operands and nothing else. Nothing of the
/// helper's own environment reaches it, `PWD` included, which bubblewrap sets for the helper;
/// a value is passed byte for byte, and without an operand the environment is empty.
#[test]
fn the_program_starts_with_the_env_operands_and_nothing_of_the_helpers_own() {
    const NAME: &str = "environment";
    let (_directory, root) = scratch();
    let env = tool("env");
    let output = gate_with(
        &root,
        &env,
        &[
            ("B", "two words --env C d"),
            ("A", "1"),
            ("LD_LIBRARY_PATH", ""),
        ],
    )
    .env("PWD", "/where/bubblewrap/changed/to")
    .output()
    .expect("the gate starts");
    if absent(NAME, &output) {
        return;
    }
    assert!(output.status.success(), "{}", text(&output.stderr));
    let printed = text(&output.stdout);
    let mut variables: Vec<&str> = printed.lines().collect();
    variables.sort_unstable();
    assert_eq!(
        variables,
        ["A=1", "B=two words --env C d", "LD_LIBRARY_PATH="],
        "the program's environment is the operands"
    );

    let empty = gate_with(&root, &env, &[])
        .env("PWD", "/where/bubblewrap/changed/to")
        .output()
        .expect("the gate starts");
    assert!(empty.status.success(), "{}", text(&empty.stderr));
    assert!(
        empty.stdout.is_empty(),
        "without an operand the program has no variable: {}",
        text(&empty.stdout)
    );
    println!("exec gate {NAME}: executed");
}

/// D28: a helper started with a variable of its own other than `PWD` executes nothing. A
/// variable of the run in the helper's environment is one that reached a process before the
/// ruleset, so the caller that put it there is refused, at every start.
#[test]
fn a_helper_started_with_a_variable_of_its_own_executes_nothing() {
    let (_directory, root) = scratch();
    let sh = tool("sh");
    for (name, value) in [
        ("PATH", "/usr/bin:/bin"),
        ("LD_LIBRARY_PATH", "/workspace/lib"),
        ("LANG", "C"),
    ] {
        let marker = root.join(format!("ran-{name}"));
        let output = gate_with(&root, &sh, &[(name, value)])
            .args(["-c", &format!(": > '{}'; echo ran", marker.display())])
            .env(name, value)
            .output()
            .expect("the gate starts");
        assert_refused(&output, name);
        assert!(!marker.exists(), "{name}: the program ran");
        let diagnostic = text(&output.stderr);
        assert!(
            diagnostic.contains(&format!("was started with the variable {name:?}")),
            "{diagnostic}"
        );
    }
    // The control: the same start with the variable as an operand only runs the program, on a
    // kernel that applies the ruleset.
    let marker = root.join("ran-control");
    let output = gate_with(&root, &sh, &[("PATH", "/usr/bin:/bin")])
        .args(["-c", &format!(": > '{}'; echo ran", marker.display())])
        .output()
        .expect("the gate starts");
    if absent("own environment", &output) {
        return;
    }
    assert_eq!(text(&output.stdout), "ran\n", "{}", text(&output.stderr));
    assert!(marker.exists());
    println!("exec gate own environment: executed");
}

/// A shared object whose constructor executes the file `GATE_PRELOAD_TARGET` names, once, in
/// whatever process loads it first: in a process that is held to no list this replaces it.
const HOSTILE_PRELOAD: &str = r#"
#include <stdlib.h>
#include <unistd.h>

extern char **environ;

static void __attribute__((constructor)) start(void) {
    char *target = getenv("GATE_PRELOAD_TARGET");
    if (target != NULL) {
        char *argv[] = { target, NULL };
        unsetenv("GATE_PRELOAD_TARGET");
        execve(target, argv, environ);
    }
}
"#;

/// D28, with an object that does what the rule is about: `LD_PRELOAD` handed to the helper as
/// an operand names an object that executes a program outside the list wherever it is loaded.
/// It is loaded only in the program, after the ruleset, where that execution is refused; the
/// helper's own process never has the variable. So the list holds: the shell runs its script,
/// the listed program runs, and the one outside the list ends with status 126.
#[test]
fn a_preload_among_the_env_operands_is_loaded_only_under_the_ruleset() {
    const NAME: &str = "preload operand";
    let (_directory, root) = scratch();
    let sh = tool("sh");
    let control = gate_with(&root, &sh, &[])
        .args(["-c", "outside/true 2>/dev/null; echo $?"])
        .output()
        .expect("the gate starts");
    if absent(NAME, &control) {
        return;
    }
    let source = root.join("pre.c");
    let object = root.join("pre.so");
    std::fs::write(&source, HOSTILE_PRELOAD).expect("the source");
    let built = Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&object)
        .arg(&source)
        .output();
    match built {
        Ok(output) if output.status.success() => {}
        other => {
            println!("exec gate {NAME}: ABSENT (no shared object could be built: {other:?})");
            return;
        }
    }
    let outside = root.join("outside/true");
    let lists = Lists {
        deny: Vec::new(),
        program: base(&sh),
        loader: loader(),
        allow: vec![beneath(&root, "allowed")],
        writable: Vec::new(),
    };
    let environment = [
        (
            "LD_PRELOAD".to_owned(),
            object.to_str().expect("UTF-8").to_owned(),
        ),
        (
            "GATE_PRELOAD_TARGET".to_owned(),
            outside.to_str().expect("UTF-8").to_owned(),
        ),
    ];
    let script = "echo started; allowed/true; echo allowed:$?; outside/true 2>/dev/null; \
                  echo outside:$?";
    let run = |options: Vec<String>| {
        gate()
            .args(options)
            .arg(&sh)
            .args(["-c", script])
            .current_dir(&root)
            .env_clear()
            .output()
            .expect("the gate starts")
    };
    let output = run(lists.options_with(&environment));
    assert_eq!(
        text(&output.stdout),
        "started\nallowed:0\noutside:126\n",
        "the object executed a program outside the list: {}",
        text(&output.stderr)
    );
    // The object is hostile where no list holds: the same start with the target listed lets
    // its constructor replace the shell, and the script never starts.
    let unheld = Lists {
        deny: Vec::new(),
        allow: vec![beneath(&root, "allowed"), beneath(&root, "outside")],
        ..lists
    };
    let output = run(unheld.options_with(&environment));
    assert!(
        output.status.success() && output.stdout.is_empty(),
        "the control: with its target listed the object replaces the shell: {output:?}"
    );
    println!("exec gate {NAME}: executed");
}

/// A scratch tree for the deny cases: `listed/tool` and `listed/other`, copies of `true`, and
/// the lists that name `listed` with the shell as the program.
fn deny_scratch() -> (tempfile::TempDir, PathBuf, Lists) {
    let (directory, root) = scratch();
    std::fs::create_dir_all(root.join("listed")).expect("listed");
    for name in ["tool", "other"] {
        std::fs::copy(tool("true"), root.join("listed").join(name)).expect("copy true");
    }
    let lists = Lists {
        deny: Vec::new(),
        program: base(&tool("sh")),
        loader: loader(),
        allow: vec![follow(&root, "listed")],
        writable: Vec::new(),
    };
    (directory, root, lists)
}

/// Lists each base tool of `names` by its own path, which puts the rule on the file the name
/// resolves to, and returns the paths a script calls them by.
fn with_tools(lists: &mut Lists, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|name| {
            let path = tool(name);
            lists.allow.push(base(&path));
            path.to_str().expect("UTF-8").to_owned()
        })
        .collect()
}

/// Whether the helper refused for want of Landlock or of a place on this host's file systems;
/// prints the case as absent then.
fn deny_absent(name: &str, output: &Output) -> bool {
    let diagnostic = text(&output.stderr);
    if output.status.code() == Some(REFUSED)
        && (diagnostic.contains("Landlock is unavailable")
            || diagnostic.contains("cannot establish the place"))
    {
        println!("exec gate {name}: ABSENT ({})", diagnostic.trim());
        return true;
    }
    false
}

/// Runs `script` under `lists` with the shell, in `root`: its standard output, or `None` after
/// printing the case as absent.
fn deny_run(name: &str, root: &Path, lists: &Lists, script: &str) -> Option<String> {
    let output = gate()
        .current_dir(root)
        .env_clear()
        .args(lists.options())
        .arg(tool("sh"))
        .args(["-c", script])
        .output()
        .expect("the gate starts");
    if deny_absent(name, &output) {
        return None;
    }
    assert!(
        output.status.code() != Some(REFUSED),
        "the helper refused: {}",
        text(&output.stderr)
    );
    Some(text(&output.stdout))
}

/// D11: a denied file beneath a listed directory is not executed and its neighbour is. The
/// directory is expanded, so the denied file stays readable, a symbolic link to it is denied
/// wherever the link lies (N1, N7), and another name of it in the same directory is denied by
/// device and inode (N2). A file created in the expanded directory during the run is not
/// executable (N2), and a deny entry that does not exist at the start denies the file once it
/// is created (N6).
#[test]
fn a_denied_file_in_a_listed_directory_is_not_executed_under_any_name_there() {
    const NAME: &str = "deny listed file";
    let (_directory, root, mut lists) = deny_scratch();
    std::fs::hard_link(root.join("listed/tool"), root.join("listed/again")).expect("a hard link");
    std::os::unix::fs::symlink("tool", root.join("listed/link")).expect("a link");
    std::os::unix::fs::symlink("../listed/tool", root.join("elsewhere/link")).expect("a link");
    lists.allow.push(follow(&root, "elsewhere"));
    lists.deny = vec![follow(&root, "listed/tool"), follow(&root, "listed/later")];
    let tools = with_tools(&mut lists, &["cat", "chmod"]);
    let (cat, chmod) = (&tools[0], &tools[1]);
    let Some(stdout) = deny_run(
        NAME,
        &root,
        &lists,
        &format!(
            "listed/other; echo $?; listed/tool 2>/dev/null; echo $?; \
             listed/again 2>/dev/null; echo $?; listed/link 2>/dev/null; echo $?; \
             elsewhere/link 2>/dev/null; echo $?; [ -r listed/tool ]; echo $?; \
             {cat} listed/other > listed/new; {chmod} +x listed/new; \
             listed/new 2>/dev/null; echo $?; \
             {cat} listed/other > listed/later; {chmod} +x listed/later; \
             listed/later 2>/dev/null; echo $?"
        ),
    ) else {
        return;
    };
    assert_eq!(
        stdout, "0\n126\n126\n126\n126\n0\n126\n126\n",
        "the neighbour, the denied file, its hard link, a link to it beside it and one in \
         another listed directory, that it is readable, a file created in the expanded \
         directory, and a deny entry created during the run"
    );
    println!("exec gate {NAME}: executed");
}

/// D11 step 1 (K8) and N5: a denied directory denies everything beneath it, a listed directory
/// inside a denied one gets no rule, and a deny entry that no listed path holds changes
/// nothing.
#[test]
fn a_denied_directory_denies_what_lies_in_it_and_a_listed_directory_inside_it() {
    const NAME: &str = "deny directory";
    let (_directory, root, mut lists) = deny_scratch();
    std::fs::create_dir_all(root.join("listed/inner")).expect("inner");
    std::fs::copy(tool("true"), root.join("listed/inner/tool")).expect("copy true");
    lists.deny = vec![follow(&root, "listed/inner"), follow(&root, "outside")];
    let Some(stdout) = deny_run(
        NAME,
        &root,
        &lists,
        "listed/tool; echo $?; listed/inner/tool 2>/dev/null; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        stdout, "0\n126\n",
        "a denied directory beneath a listed one"
    );
    // K8: the listed directory lies inside the denied one.
    lists.allow = vec![follow(&root, "listed/inner")];
    lists.deny = vec![follow(&root, "listed")];
    assert_eq!(
        deny_run(
            NAME,
            &root,
            &lists,
            "listed/inner/tool 2>/dev/null; echo $?"
        )
        .as_deref(),
        Some("126\n"),
        "a listed directory inside a denied one"
    );
    println!("exec gate {NAME}: executed");
}

/// D23: the program and the loader go through the deny check. A program that is a link to a
/// denied program gets no rule, and the helper's own execution of it fails with status 126
/// (B2). A denied loader starts no dynamically linked program (B3).
#[test]
fn a_denied_program_and_a_denied_loader_get_no_rule() {
    const NAME: &str = "deny program and loader";
    let (_directory, root, mut lists) = deny_scratch();
    std::os::unix::fs::symlink("tool", root.join("listed/sh")).expect("a link");
    lists.program = follow(&root, "listed/sh");
    lists.deny = vec![follow(&root, "listed/tool")];
    let output = gate()
        .current_dir(&root)
        .env_clear()
        .args(lists.options())
        .arg(root.join("listed/sh"))
        .output()
        .expect("the gate starts");
    if deny_absent(NAME, &output) {
        return;
    }
    assert_eq!(
        output.status.code(),
        Some(126),
        "a program that is a link to a denied program: {}",
        text(&output.stderr)
    );
    let Some(loader) = loader() else {
        println!("exec gate {NAME}: executed (the base programs name no loader here)");
        return;
    };
    let (_directory, root, mut lists) = deny_scratch();
    let (loader_root, loader_rel) = loader.rsplit_once('/').expect("an absolute loader");
    lists.deny = vec![Located::new(Class::Follow, loader_root, loader_rel).expect("the loader")];
    let output = gate()
        .current_dir(&root)
        .env_clear()
        .args(lists.options())
        .arg(tool("sh"))
        .args(["-c", "listed/tool"])
        .output()
        .expect("the gate starts");
    assert_eq!(
        output.status.code(),
        Some(126),
        "a dynamically linked program under a denied loader: {}",
        text(&output.stderr)
    );
    println!("exec gate {NAME}: executed");
}

/// The limit of design 35 § 8.5 (N4): a deny entry denies a path and every mount of it, not
/// every hard link to the file. A hard link that existed before the start elsewhere beneath a
/// listed path, in a directory the deny-list does not expand, runs.
#[test]
fn limit_a_hard_link_that_existed_in_another_listed_directory_runs() {
    const NAME: &str = "deny limit hard link elsewhere";
    let (_directory, root, mut lists) = deny_scratch();
    std::fs::hard_link(root.join("listed/tool"), root.join("elsewhere/same")).expect("a hard link");
    lists.allow.push(follow(&root, "elsewhere"));
    lists.deny = vec![follow(&root, "listed/tool")];
    let Some(stdout) = deny_run(
        NAME,
        &root,
        &lists,
        "listed/tool 2>/dev/null; echo $?; elsewhere/same; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        stdout, "126\n0\n",
        "the limit holds as stated: the denied file runs under its other name"
    );
    println!("exec gate {NAME}: executed");
}

/// Two limits of design 35 § 8 (B9, § 8.4). A program overwritten in place in an expanded
/// directory keeps its rule; one removed and written again is a new file without a rule and
/// no longer runs. And a copy of a denied program in a listed directory the deny-list does not
/// expand runs: a deny-list means something only beside an allow-list that admits no directory
/// the run can write.
#[test]
fn limit_a_replaced_program_loses_the_right_and_a_copy_elsewhere_runs() {
    const NAME: &str = "deny limit replaced and copied";
    let (_directory, root, mut lists) = deny_scratch();
    lists.allow.push(follow(&root, "elsewhere"));
    lists.deny = vec![follow(&root, "listed/tool")];
    let tools = with_tools(&mut lists, &["cat", "chmod", "rm"]);
    let (cat, chmod, rm) = (&tools[0], &tools[1], &tools[2]);
    let Some(stdout) = deny_run(
        NAME,
        &root,
        &lists,
        &format!(
            "{cat} outside/true > listed/other; listed/other; echo $?; \
             {rm} listed/other; {cat} outside/true > listed/other; {chmod} +x listed/other; \
             listed/other 2>/dev/null; echo $?; \
             {cat} listed/tool > elsewhere/copy; {chmod} +x elsewhere/copy; \
             elsewhere/copy; echo $?"
        ),
    ) else {
        return;
    };
    assert_eq!(
        stdout, "0\n126\n0\n",
        "overwritten in place, removed and written again, and a copy of the denied program"
    );
    println!("exec gate {NAME}: executed");
}

/// D24 for an entry of the deny-list: **a `follow` deny entry is opened as every followed
/// path is.** One that resolves into a mount the run can write is refused with the line of
/// D24, naming `--deny`; so is one that does not exist beneath a link into such a mount,
/// where the link is what was opened. The run could replace what such an entry names, and
/// with it what the entry denies. The control denies a file no writable mount holds.
#[test]
fn a_follow_deny_entry_that_resolves_into_a_writable_mount_is_refused() {
    const NAME: &str = "deny entry into a writable mount";
    let Some((_directory, root)) = placed_scratch(NAME) else {
        return;
    };
    let writable = root.join("writable");
    let read_only = root.join("read-only");
    for name in ["tool", "other"] {
        std::fs::copy(tool("true"), read_only.join(name)).expect("copy true");
    }
    let run = |deny: Located, script: &str| {
        let lists = Lists {
            program: base(&tool("sh")),
            loader: loader(),
            allow: vec![follow(&read_only, "tool"), follow(&read_only, "other")],
            deny: vec![deny],
            writable: vec![writable.to_str().expect("UTF-8").to_owned()],
        };
        gate()
            .args(lists.options())
            .arg(tool("sh"))
            .args(["-c", script])
            .current_dir(&root)
            .env_clear()
            .output()
            .expect("the gate starts")
    };
    let control = run(
        follow(&read_only, "tool"),
        "read-only/tool 2>/dev/null; echo $?; read-only/other; echo $?",
    );
    if absent(NAME, &control) {
        return;
    }
    assert_eq!(
        (control.status.code(), text(&control.stdout).as_str()),
        (Some(0), "126\n0\n"),
        "the control: {}",
        text(&control.stderr)
    );
    for (rel, entry, target) in [
        ("file", "file", "true"),
        ("directory", "directory/absent", "deep"),
    ] {
        let target = writable.join(target).canonicalize().expect("the target");
        std::os::unix::fs::symlink(&target, read_only.join(rel)).expect("link");
        let output = run(follow(&read_only, entry), "echo ran");
        assert_refused(&output, &format!("deny entry {entry}"));
        assert_eq!(
            text(&output.stderr),
            format!(
                "substrate-exec-gate: --deny {} resolves to {}, in the writable mount {}\n",
                read_only.join(rel).display(),
                target.display(),
                writable.display()
            ),
            "the line of D24, for what of {entry} exists"
        );
    }
    println!("exec gate {NAME}: executed");
}

/// Design 35 § 8.2: **a deny entry that does not exist is a name, and denies nothing above
/// it.** `listed/tool/beneath` and `listed/other/beneath` name something beneath a file:
/// the kernel answers `ENOTDIR`, the entry is absent like one whose name is missing, and
/// the file above it, which is what the helper opened, keeps its rule and runs. One entry is
/// a `follow` path and one a `beneath` path. An absent name in a directory is the same: the
/// directory is expanded and its files run.
#[test]
fn a_deny_entry_beneath_a_file_is_absent_and_denies_nothing_above_it() {
    const NAME: &str = "deny entry beneath a file";
    let (_directory, root, mut lists) = deny_scratch();
    lists.deny = vec![
        follow(&root, "listed/tool/beneath"),
        beneath(&root, "listed/other/beneath"),
        follow(&root, "listed/later"),
    ];
    let Some(stdout) = deny_run(
        NAME,
        &root,
        &lists,
        "listed/tool; echo $?; listed/other; echo $?",
    ) else {
        return;
    };
    assert_eq!(
        stdout, "0\n0\n",
        "the file above an absent deny entry is not what the entry denies"
    );
    println!("exec gate {NAME}: executed");
}

/// Design 35 § 8.2, a name that is not UTF-8 in an expanded directory: **the entry is
/// handled as bytes and gets its rule like any other file.** A file of its own under such a
/// name runs; a hard link to the denied file under such a name stays denied, by device and
/// inode; a directory under such a name gets one rule, and a program in it runs. Before, the
/// walk built a path as text for every entry, and one such name ended every start with a
/// deny entry in that directory with status 125 and a cause that was not true of it.
#[test]
fn a_name_that_is_not_utf8_in_an_expanded_directory_gets_its_rule() {
    use std::os::unix::ffi::OsStrExt as _;
    const NAME: &str = "deny beside a name that is not UTF-8";
    let (_directory, root, mut lists) = deny_scratch();
    let listed = root.join("listed");
    let named = |bytes: &[u8]| listed.join(std::ffi::OsStr::from_bytes(bytes));
    let staged = std::fs::copy(tool("true"), named(b"own\xff"))
        .and_then(|_| std::fs::hard_link(listed.join("tool"), named(b"same\xfe")))
        .and_then(|()| std::fs::create_dir(named(b"dir\xfd")))
        .and_then(|()| std::fs::copy(tool("true"), named(b"dir\xfd").join("inner")));
    if let Err(error) = staged {
        println!("exec gate {NAME}: ABSENT (this file system takes no such name: {error})");
        return;
    }
    lists.deny = vec![follow(&root, "listed/tool")];
    let output = gate()
        .current_dir(&root)
        .env_clear()
        .args(lists.options())
        .arg(tool("sh"))
        .args([
            "-c",
            "listed/tool 2>/dev/null; echo $?; listed/own*; echo $?; \
             listed/same* 2>/dev/null; echo $?; listed/dir*/inner; echo $?; listed/other; echo $?",
        ])
        .output()
        .expect("the gate starts");
    // A place that cannot be established is this host's only for a file-system type outside
    // the list: any other cause is the walk's own answer to such a name, and fails the case.
    let diagnostic = text(&output.stderr);
    if absent(NAME, &output) {
        return;
    }
    if diagnostic.contains("file-system-unlisted") {
        println!("exec gate {NAME}: ABSENT ({})", diagnostic.trim());
        return;
    }
    assert_eq!(
        (output.status.code(), text(&output.stdout).as_str()),
        (Some(0), "126\n0\n126\n0\n0\n"),
        "the denied file, a file of its own under a name that is not text, another name of \
         the denied file, a program in a directory with such a name, and the neighbour: \
         {diagnostic}"
    );
    println!("exec gate {NAME}: executed");
}
