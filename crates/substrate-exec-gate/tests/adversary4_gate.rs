//! Adversary pass on `story:executable-deny-list` (design 35 § 8.2, D11), commit `aafb9823a`:
//! the helper's expansion at its edges, with no namespace and no driver around it.
//!
//! Each case starts the shipped `substrate-exec-gate` once or twice. On a kernel without
//! Landlock, or with the scratch directory on a file system outside the list, a case prints
//! `ABSENT` and is not passed.

#![cfg(target_os = "linux")]

use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use substrate_exec_gate::{Class, Lists, Located, REFUSED};

fn tool(name: &str) -> PathBuf {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {name} in /usr/bin or /bin"))
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

fn follow(root: &Path, rel: &str) -> Located {
    Located::new(Class::Follow, root.to_str().expect("UTF-8"), rel).expect("a follow path")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn loader() -> Option<String> {
    substrate_exec_gate::elf_interpreter(&std::fs::read(tool("env")).expect("env"))
}

/// A scratch root beneath the build's own directory, and the lists of a run of the shell that
/// allows `allow` and denies `deny` beneath it.
fn scratch() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    (directory, root)
}

fn lists(root: &Path, allow: &[&str], deny: &[&str]) -> Lists {
    Lists {
        program: base(&tool("sh")),
        loader: loader(),
        allow: allow.iter().map(|rel| follow(root, rel)).collect(),
        deny: deny.iter().map(|rel| follow(root, rel)).collect(),
        writable: Vec::new(),
    }
}

fn run(root: &Path, lists: &Lists, script: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_substrate-exec-gate"))
        .current_dir(root)
        .env_clear()
        .args(lists.options())
        .arg(tool("sh"))
        .args(["-c", script])
        .output()
        .expect("the gate starts")
}

/// Whether the helper refused for a reason of this host, not of the case; prints it as absent.
/// A place that cannot be established counts unless `placed`: the case itself asks for one.
fn absent_unless(name: &str, output: &Output, placed: bool) -> bool {
    let diagnostic = text(&output.stderr);
    if output.status.code() == Some(REFUSED)
        && (diagnostic.contains("Landlock is unavailable")
            || (!placed && diagnostic.contains("cannot establish the place")))
    {
        println!(
            "adversary4 exec gate {name}: ABSENT ({})",
            diagnostic.trim()
        );
        return true;
    }
    false
}

fn absent(name: &str, output: &Output) -> bool {
    absent_unless(name, output, false)
}

/// **The stop of the walk, at its exact count** (design 35 § 8.2: "the helper stops at 65536
/// rules and exits 125"). One listed directory holds one deny entry and enough other files
/// that the lists need exactly 65536 execute rules: the program's, the loader's where the host
/// has one, and one per remaining entry. That run executes its program. With one file more the
/// lists need 65537, and the helper executes nothing.
#[test]
fn adversary4_the_walk_runs_at_65536_rules_and_stops_at_65537() {
    const NAME: &str = "rule stop";
    const MAX_RULES: usize = 65_536;
    let (_directory, root) = scratch();
    let many = root.join("many");
    std::fs::create_dir(&many).expect("many");
    let lists = lists(&root, &["many"], &["many/denied"]);
    // The program, the loader, and every entry of `many` but the denied one.
    let fixed = 1 + usize::from(lists.loader.is_some());
    std::fs::write(many.join("denied"), b"").expect("the denied entry");
    for index in 0..MAX_RULES - fixed {
        std::fs::write(many.join(format!("f{index}")), b"").expect("an entry");
    }

    let at_the_bound = run(&root, &lists, "echo ran");
    if absent(NAME, &at_the_bound) {
        return;
    }
    assert_eq!(
        (
            at_the_bound.status.code(),
            text(&at_the_bound.stdout).as_str()
        ),
        (Some(0), "ran\n"),
        "65536 rules are within the bound: {}",
        text(&at_the_bound.stderr)
    );

    std::fs::write(many.join("one-more"), b"").expect("one more entry");
    let past_the_bound = run(&root, &lists, "echo ran");
    let diagnostic = text(&past_the_bound.stderr);
    assert_eq!(
        (
            past_the_bound.status.code(),
            text(&past_the_bound.stdout).as_str()
        ),
        (Some(REFUSED), ""),
        "65537 rules: nothing is executed: {diagnostic}"
    );
    assert!(
        diagnostic.contains("more than 65536 rules"),
        "the refusal names the stop: {diagnostic}"
    );
    println!("adversary4 exec gate {NAME}: executed");
}

/// **Entries of an expanded directory that are no regular file**: a FIFO, a socket, an empty
/// directory, a directory the helper cannot read, a dangling link and a link to the denied
/// file. None may stop the walk, none may make the denied file executable, and the listed
/// neighbour runs.
#[test]
fn adversary4_special_entries_of_an_expanded_directory_do_not_stop_or_open_the_walk() {
    const NAME: &str = "special entries";
    let (_directory, root) = scratch();
    let listed = root.join("listed");
    std::fs::create_dir(&listed).expect("listed");
    for name in ["tool", "other"] {
        std::fs::copy(tool("true"), listed.join(name)).expect("copy true");
    }
    let fifo = std::ffi::CString::new(listed.join("fifo").as_os_str().as_bytes()).expect("a path");
    // SAFETY: `fifo` is a NUL-terminated path that outlives the call.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0, "mkfifo");
    // Through a descriptor of the directory: the scratch path is longer than a socket address.
    let directory = std::fs::File::open(&listed).expect("listed");
    let _socket = std::os::unix::net::UnixListener::bind(format!(
        "/proc/self/fd/{}/socket",
        std::os::fd::AsRawFd::as_raw_fd(&directory)
    ))
    .expect("a socket");
    std::fs::create_dir(listed.join("empty")).expect("empty");
    let closed = listed.join("closed");
    std::fs::create_dir(&closed).expect("closed");
    std::fs::copy(tool("true"), closed.join("tool")).expect("copy true");
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o111)).expect("chmod");
    std::os::unix::fs::symlink("nowhere", listed.join("dangling")).expect("a link");
    std::os::unix::fs::symlink("tool", listed.join("alias")).expect("a link");

    let output = run(
        &root,
        &lists(&root, &["listed"], &["listed/tool"]),
        "listed/tool 2>/dev/null; echo $?; listed/alias 2>/dev/null; echo $?; \
         listed/other; echo $?; listed/closed/tool; echo $?",
    );
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    if absent(NAME, &output) {
        return;
    }
    assert_eq!(
        (output.status.code(), text(&output.stdout).as_str()),
        (Some(0), "126\n126\n0\n0\n"),
        "{}",
        text(&output.stderr)
    );
    println!("adversary4 exec gate {NAME}: executed");
}

/// **An expanded directory the helper cannot read** holds the deny entry. The walk cannot list
/// it, so the helper executes nothing: status 125 and no output.
#[test]
fn adversary4_an_expanded_directory_that_cannot_be_read_runs_nothing() {
    const NAME: &str = "unreadable expanded directory";
    // SAFETY: `geteuid` reads this process's identity.
    if unsafe { libc::geteuid() } == 0 {
        println!("adversary4 exec gate {NAME}: ABSENT (this process reads every directory)");
        return;
    }
    let (_directory, root) = scratch();
    let listed = root.join("listed");
    std::fs::create_dir(&listed).expect("listed");
    for name in ["tool", "other"] {
        std::fs::copy(tool("true"), listed.join(name)).expect("copy true");
    }
    std::fs::set_permissions(&listed, std::fs::Permissions::from_mode(0o311)).expect("chmod");
    let output = run(
        &root,
        &lists(&root, &["listed"], &["listed/tool"]),
        "echo ran; listed/tool; echo $?",
    );
    std::fs::set_permissions(&listed, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    if absent(NAME, &output) {
        return;
    }
    assert_eq!(
        (output.status.code(), text(&output.stdout).as_str()),
        (Some(REFUSED), ""),
        "{}",
        text(&output.stderr)
    );
    println!("adversary4 exec gate {NAME}: executed");
}

/// **A name that is not UTF-8 in an expanded directory.** An earlier run can leave one in a
/// directory it can write. The helper takes the name as bytes: the start runs, the denied
/// file stays denied under its own name and under the name that is not text, which is a hard
/// link to it, and the listed neighbour runs. It never runs the denied file, and it no longer
/// ends the start with status 125 (design 35 § 8.2, ninth revision).
#[test]
fn adversary4_a_name_that_is_not_utf8_in_an_expanded_directory_keeps_the_deny_entry() {
    const NAME: &str = "non-UTF-8 name";
    let (_directory, root) = scratch();
    let listed = root.join("listed");
    std::fs::create_dir(&listed).expect("listed");
    for name in ["tool", "other"] {
        std::fs::copy(tool("true"), listed.join(name)).expect("copy true");
    }
    let odd = listed.join(std::ffi::OsStr::from_bytes(b"odd\xff"));
    if std::fs::hard_link(listed.join("tool"), &odd).is_err() {
        println!("adversary4 exec gate {NAME}: ABSENT (this file system takes no such name)");
        return;
    }
    let output = run(
        &root,
        &lists(&root, &["listed"], &["listed/tool"]),
        "listed/tool 2>/dev/null; echo $?; listed/odd* 2>/dev/null; echo $?; listed/other; echo $?",
    );
    if text(&output.stderr).contains("file-system-unlisted") || absent_unless(NAME, &output, true) {
        println!("adversary4 exec gate {NAME}: ABSENT (this host's file system or kernel)");
        return;
    }
    let (code, stdout) = (output.status.code(), text(&output.stdout));
    assert_eq!(
        (code, stdout.as_str()),
        (Some(0), "126\n126\n0\n"),
        "stderr {}",
        text(&output.stderr)
    );
    println!(
        "adversary4 exec gate {NAME}: executed, status {code:?}: {}",
        text(&output.stderr).trim()
    );
}
