//! Security review of `story:linux-executable-allow-list` (design 35): conformance checks of the
//! shipped `substrate-exec-gate` against what the design says of it.
//!
//! - **It fails closed** (§ 6, *When the helper fails*): "The helper executes the program only
//!   after `landlock_restrict_self` returned 0. Any earlier failure … ends the run before the
//!   program starts". Each of the three Landlock system calls is made to fail here, by a seccomp
//!   filter installed on the helper before it starts, and the program must not run.
//! - **The run cannot remove the ruleset** (D4): "Every process of the run descends from the
//!   helper and inherits the domain". The helper's own start is part of that claim: nothing the
//!   run's environment names may run in the helper's process before the ruleset exists.
//! - **Its arguments are positional** (D24): "Every operand is its own argument … so no operand
//!   reads as an option."
//!
//! Each case runs the shipped binary once per condition and reads what the program itself did.

#![cfg(target_os = "linux")]

use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use substrate_exec_gate::{Class, Lists, Located, REFUSED};

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

/// A base tool as a `follow` path beneath `/usr` or `/bin`.
fn base(path: &Path) -> Located {
    let text = path.to_str().expect("UTF-8");
    let (root, rel) = match text.strip_prefix("/usr/") {
        Some(rel) => ("/usr", rel),
        None => ("/bin", text.strip_prefix("/bin/").expect("a base tool")),
    };
    Located::new(Class::Follow, root, rel).expect("a base path")
}

fn loader() -> Option<String> {
    substrate_exec_gate::elf_interpreter(&std::fs::read(tool("env")).expect("env"))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A scratch root with `allowed/true` and `outside/true`.
fn scratch() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    for name in ["allowed", "outside"] {
        std::fs::create_dir(root.join(name)).expect(name);
        std::fs::copy(tool("true"), root.join(name).join("true")).expect("copy true");
    }
    (directory, root)
}

/// The gate about to run `/bin/sh -c script` in `root`, held to the shell and `allow`.
fn shell(root: &Path, allow: Vec<Located>, script: &str) -> Command {
    let sh = tool("sh");
    let lists = Lists {
        program: base(&sh),
        loader: loader(),
        allow,
        writable: Vec::new(),
    };
    let mut command = gate();
    command
        .args(lists.options())
        .arg(&sh)
        .args(["-c", script])
        .current_dir(root)
        .env_clear();
    command
}

fn beneath(root: &Path, rel: &str) -> Located {
    Located::new(Class::Beneath, root.to_str().expect("UTF-8"), rel).expect("a beneath path")
}

/// Whether this kernel applies the ruleset at all: the control every case needs.
fn landlock(name: &str, root: &Path) -> bool {
    let output = shell(root, Vec::new(), "outside/true 2>/dev/null; echo $?")
        .output()
        .expect("the gate starts");
    if output.status.code() == Some(REFUSED) {
        println!(
            "security exec gate {name}: ABSENT ({})",
            text(&output.stderr).trim()
        );
        return false;
    }
    assert_eq!(text(&output.stdout), "126\n", "the control: {output:?}");
    true
}

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15;
const BPF_RET_K: u16 = 0x06;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;

/// A seccomp program under which system call `number` fails with `errno` and every other call
/// is allowed: what a kernel without that call, or with Landlock disabled, answers.
fn failing(number: libc::c_long, errno: i32) -> Vec<libc::sock_filter> {
    let number = u32::try_from(number).expect("a system call number");
    let errno = u32::try_from(errno).expect("an errno");
    vec![
        // The system call number is the first member of `struct seccomp_data`.
        libc::sock_filter {
            code: BPF_LD_W_ABS,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: BPF_JMP_JEQ_K,
            jt: 0,
            jf: 1,
            k: number,
        },
        libc::sock_filter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ERRNO | errno,
        },
        libc::sock_filter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ALLOW,
        },
    ]
}

/// Installs `program` on the child between its fork and its exec, so the helper starts under it.
fn under(command: &mut Command, program: Vec<libc::sock_filter>) {
    // SAFETY: the closure runs after the fork and makes two `prctl` calls on memory built before
    // it; both are async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            let filter = libc::sock_fprog {
                len: u16::try_from(program.len()).expect("a short program"),
                filter: program.as_ptr().cast_mut(),
            };
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                || libc::prctl(
                    libc::PR_SET_SECCOMP,
                    libc::SECCOMP_MODE_FILTER,
                    &raw const filter,
                ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// **Fail closed, at each of the three calls** (design 35 § 6, D8; `AGENTS.md` invariant 3). A
/// kernel without Landlock answers `ENOSYS`, one with Landlock disabled at boot `EOPNOTSUPP`, an
/// ABI 1 kernel `EINVAL` for the refer right. Whichever call fails, and with whichever of them,
/// the helper exits 125 with one diagnostic line and the program does not start.
#[test]
fn security_a_kernel_that_refuses_a_landlock_call_ends_the_run_125_and_executes_nothing() {
    let (_scratch, root) = scratch();
    if !landlock("landlock refused", &root) {
        return;
    }
    let calls = [
        ("landlock_create_ruleset", libc::SYS_landlock_create_ruleset),
        ("landlock_add_rule", libc::SYS_landlock_add_rule),
        ("landlock_restrict_self", libc::SYS_landlock_restrict_self),
    ];
    let mut executed = 0;
    for (name, number) in calls {
        for errno in [libc::ENOSYS, libc::EOPNOTSUPP, libc::EINVAL, libc::EPERM] {
            let marker = root.join(format!("ran-{name}-{errno}"));
            let script = format!(": > {}; allowed/true; outside/true", marker.display());
            let mut command = shell(&root, vec![beneath(&root, "allowed")], &script);
            under(&mut command, failing(number, errno));
            let output = command.output().expect("the gate starts");
            let diagnostic = text(&output.stderr);
            assert_eq!(
                output.status.code(),
                Some(REFUSED),
                "{name} failing with {errno}: {output:?}"
            );
            assert!(
                diagnostic.starts_with("substrate-exec-gate: ")
                    && diagnostic.trim_end().lines().count() == 1,
                "{name} failing with {errno}: one diagnostic line, got {diagnostic:?}"
            );
            assert!(
                !marker.exists() && output.stdout.is_empty(),
                "{name} failing with {errno}: the program ran without the ruleset"
            );
            executed += 1;
        }
    }
    // The same harness with a filter that fails nothing the helper needs: the program runs, so
    // the refusals above are the helper's and not the harness's.
    let marker = root.join("ran-control");
    let script = format!(": > {}", marker.display());
    let mut command = shell(&root, Vec::new(), &script);
    under(
        &mut command,
        failing(libc::SYS_landlock_create_ruleset + 4096, libc::ENOSYS),
    );
    let output = command.output().expect("the gate starts");
    assert!(
        output.status.success() && marker.exists(),
        "the control under a filter that fails nothing: {output:?}"
    );
    println!("security exec gate landlock refused: executed, {executed} refusals");
}

/// A shared object whose constructor, in a process named `substrate-exec-gate`, executes
/// whatever follows `--` in that process's arguments: the run's program, directly.
const PRELOAD: &str = r#"
#define _GNU_SOURCE
#include <string.h>
#include <unistd.h>

static void __attribute__((constructor)) start(int argc, char **argv, char **envp) {
    if (argc < 1 || strstr(argv[0], "substrate-exec-gate") == NULL) {
        return;
    }
    for (int i = 1; i + 1 < argc; i++) {
        if (strcmp(argv[i], "--") == 0) {
            execve(argv[i + 1], argv + i + 1, envp);
            return;
        }
    }
}
"#;

/// Builds [`PRELOAD`] as `directory/pre.so` with the host's C compiler, or says why not.
fn preload(directory: &Path) -> Result<PathBuf, String> {
    let source = directory.join("pre.c");
    let object = directory.join("pre.so");
    std::fs::write(&source, PRELOAD).map_err(|error| error.to_string())?;
    let built = Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&object)
        .arg(&source)
        .output()
        .map_err(|error| format!("no C compiler: {error}"))?;
    if built.status.success() {
        Ok(object)
    } else {
        Err(format!("cc failed: {}", text(&built.stderr).trim()))
    }
}

/// **The run's environment does not get code into the helper before the ruleset exists**
/// (design 35 D4: "Every process of the run descends from the helper and inherits the domain …
/// The run cannot remove it"; § 6: "Nothing runs unrestricted").
///
/// The driver's environment rule admits any upper-case name that names no credential,
/// `LD_PRELOAD` among them. Since design 35 D28 the driver hands the run's variables to the
/// helper as `--env` operands and sets none of them for bubblewrap, so the helper's own process
/// never has them. The helper is started here as the driver starts it, with `LD_PRELOAD` naming
/// an object in the run's directory among the operands, and the list must hold: the program
/// outside it ends with status 126.
///
/// The second half is the helper's own rule: started with a variable of its own it executes
/// nothing, so a caller that leaks the run's variables into the helper's environment is refused
/// at every start. This case first put the object itself there, where it was red. That form
/// cannot be made green in the helper: the loader of a dynamically linked helper runs the
/// object's constructor before the helper's first instruction. D28 removes the variable from
/// that environment; the helper's refusal is what notices a caller that does not.
#[test]
fn security_a_preload_named_in_the_environment_does_not_start_the_program_without_the_ruleset() {
    const NAME: &str = "preload";
    let (_scratch, root) = scratch();
    if !landlock(NAME, &root) {
        return;
    }
    let object = match preload(&root) {
        Ok(object) => object,
        Err(reason) => {
            println!("security exec gate {NAME}: ABSENT ({reason})");
            return;
        }
    };
    let script = "allowed/true; echo allowed:$?; outside/true 2>/dev/null; echo outside:$?";
    let sh = tool("sh");
    let lists = Lists {
        program: base(&sh),
        loader: loader(),
        allow: vec![beneath(&root, "allowed")],
        writable: Vec::new(),
    };
    let output = gate()
        .args(lists.options_with(&[(
            "LD_PRELOAD".to_owned(),
            object.to_str().expect("UTF-8").to_owned(),
        )]))
        .arg(&sh)
        .args(["-c", script])
        .current_dir(&root)
        .env_clear()
        .output()
        .expect("the gate starts");
    assert_eq!(
        text(&output.stdout),
        "allowed:0\noutside:126\n",
        "with LD_PRELOAD={} among the run's variables the program ran without the ruleset \
         (stderr: {:?})",
        object.display(),
        text(&output.stderr)
    );

    // In the helper's own environment a variable of the run is refused: nothing is executed.
    // The value names no object a loader could act on, so what answers is the helper.
    let marker = root.join("ran-with-its-own-variable");
    let output = shell(
        &root,
        vec![beneath(&root, "allowed")],
        &format!(": > {}; {script}", marker.display()),
    )
    .env("LD_PRELOAD", root.join("no-object"))
    .output()
    .expect("the gate starts");
    assert_eq!(output.status.code(), Some(REFUSED), "{output:?}");
    assert!(
        !marker.exists() && output.stdout.is_empty(),
        "started with LD_PRELOAD in its own environment the helper ran the program: {output:?}"
    );
    assert!(
        text(&output.stderr).contains("was started with the variable \"LD_PRELOAD\""),
        "{output:?}"
    );
    println!("security exec gate {NAME}: executed");
}

/// **An operand is never read as an option** (design 35 D24): an entry whose name is `--`,
/// `--version`, `--allow` or `-x` is the `<rel>` of its `--allow`, the list holds, and the
/// program's own arguments are the program's.
#[test]
fn security_an_entry_named_like_an_option_is_an_operand() {
    const NAME: &str = "option-like operands";
    let (_scratch, root) = scratch();
    if !landlock(NAME, &root) {
        return;
    }
    for name in ["--", "--version", "--allow", "-x", "--writable"] {
        std::fs::create_dir(root.join(name)).expect(name);
        std::fs::copy(tool("true"), root.join(name).join("true")).expect("copy true");
        let script = format!(
            "./{name}/true; echo listed:$?; outside/true 2>/dev/null; echo outside:$?; echo \"$0\""
        );
        let output = shell(&root, vec![beneath(&root, name)], &script)
            .args(["--allow", "follow", "/usr", "bin"])
            .output()
            .expect("the gate starts");
        assert_eq!(
            text(&output.stdout),
            "listed:0\noutside:126\n--allow\n",
            "an entry named {name:?}: {output:?}"
        );
    }
    println!("security exec gate {NAME}: executed");
}

/// **A long list is enforced or refused, never dropped.** Each entry is an open descriptor
/// until the ruleset is applied. With more entries than the process may hold descriptors the
/// helper exits 125 and executes nothing; within the limit every entry is a rule.
#[test]
fn security_a_list_longer_than_the_descriptor_limit_executes_nothing() {
    const NAME: &str = "long list";
    let (_scratch, root) = scratch();
    if !landlock(NAME, &root) {
        return;
    }
    let entries = |count: usize| -> Vec<Located> {
        (0..count)
            .map(|index| {
                let name = format!("listed-{index}");
                std::fs::create_dir_all(root.join(&name)).expect("a listed directory");
                beneath(&root, &name)
            })
            .collect()
    };
    let marker = root.join("ran-long");
    let script = format!(
        ": > {}; allowed/true; echo allowed:$?; outside/true 2>/dev/null; echo outside:$?",
        marker.display()
    );

    let mut over = shell(&root, entries(96), &script);
    // SAFETY: `setrlimit` between the fork and the exec, on a value built before the fork.
    unsafe {
        over.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 64,
                rlim_max: 64,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = over.output().expect("the gate starts");
    assert_eq!(output.status.code(), Some(REFUSED), "{output:?}");
    assert!(
        !marker.exists() && output.stdout.is_empty(),
        "96 entries under a limit of 64 descriptors: the program ran: {output:?}"
    );

    let mut list = entries(96);
    list.push(beneath(&root, "allowed"));
    let output = shell(&root, list, &script)
        .output()
        .expect("the gate starts");
    assert_eq!(
        text(&output.stdout),
        "allowed:0\noutside:126\n",
        "97 entries within the limit: {output:?}"
    );
    println!("security exec gate {NAME}: executed");
}

/// **The program starts with `no_new_privs` set**, also where the helper is started outside
/// bubblewrap, which sets it itself: the helper does not rest on its caller for it.
#[test]
fn security_the_program_starts_with_no_new_privs() {
    const NAME: &str = "no_new_privs";
    let (_scratch, root) = scratch();
    if !landlock(NAME, &root) {
        return;
    }
    let script = "while read -r key value; do \
                  [ \"$key\" = NoNewPrivs: ] && echo \"no_new_privs:$value\"; \
                  done < /proc/self/status";
    let output = shell(&root, Vec::new(), script)
        .output()
        .expect("the gate starts");
    assert_eq!(text(&output.stdout), "no_new_privs:1\n", "{output:?}");
    println!("security exec gate {NAME}: executed");
}
