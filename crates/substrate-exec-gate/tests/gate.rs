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
            "--deny, which this helper does not implement",
            vec![
                "--program",
                class,
                program.root(),
                program.rel(),
                "--deny",
                "follow",
                "/usr",
                "bin/id",
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
#[test]
fn a_link_and_a_rename_between_two_unlisted_directories_still_work() {
    const NAME: &str = "refer";
    let (_directory, root) = scratch();
    let tools = base(tool("ln").parent().expect("a directory"));
    let output = shell(
        &root,
        vec![tools],
        "ln outside/true elsewhere/linked && mv elsewhere/linked outside/moved && \
         mv outside/moved outside/renamed && echo done",
    );
    if absent(NAME, &output) {
        return;
    }
    assert_eq!(text(&output.stdout), "done\n", "{}", text(&output.stderr));
    assert!(root.join("outside/renamed").is_file());
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
