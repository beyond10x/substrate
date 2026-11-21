//! Adversary pass 1 on `story:linux-executable-allow-list` (design 35), commit `4b5208922`: the
//! guards of the helper that the shipped suite runs on no host with Landlock.
//!
//! `tests/gate.rs` reaches "Landlock is unavailable" only on a kernel without Landlock, where it
//! prints `ABSENT`. Here a seccomp filter makes one Landlock system call fail on any kernel, so
//! "the helper failed and the program did not start" is asserted where the lane runs.

#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use substrate_exec_gate::{Class, Lists, Located, REFUSED};

fn tool(name: &str) -> PathBuf {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {name} in /usr/bin or /bin"))
}

fn instruction(code: u32, jump_false: u8, value: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: u16::try_from(code).expect("a BPF code"),
        jt: 0,
        jf: jump_false,
        k: value,
    }
}

/// Makes `syscall` fail with `errno` in the process `command` starts, and in nothing else.
fn failing(command: &mut Command, syscall: libc::c_long, errno: i32) {
    let filter = [
        // The system call number is the first member of `struct seccomp_data`.
        instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0),
        instruction(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            1,
            u32::try_from(syscall).expect("a system call number"),
        ),
        instruction(
            libc::BPF_RET | libc::BPF_K,
            0,
            libc::SECCOMP_RET_ERRNO | u32::try_from(errno).expect("an errno"),
        ),
        instruction(libc::BPF_RET | libc::BPF_K, 0, libc::SECCOMP_RET_ALLOW),
    ];
    // SAFETY: the closure runs after the fork and makes two system calls on memory built before
    // it; it allocates nothing.
    unsafe {
        command.pre_exec(move || {
            let program = libc::sock_fprog {
                len: 4,
                filter: filter.as_ptr().cast_mut(),
            };
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                || libc::syscall(
                    libc::SYS_seccomp,
                    libc::SECCOMP_SET_MODE_FILTER,
                    0,
                    &raw const program,
                ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// **A helper that could not restrict itself starts nothing** (design 35 § 6, *When the helper
/// fails*; D8): "The helper executes the program only after `landlock_restrict_self` returned
/// 0", and a kernel without Landlock, or one that refuses a rule or the ruleset, ends the run
/// with status 125 and one diagnostic line.
///
/// Each Landlock system call is made to fail once, on this kernel, and the program must not
/// have run. The control runs the same command without the filter and the program runs.
#[test]
fn adversary_a_landlock_call_that_fails_starts_no_program() {
    let directory = tempfile::tempdir().expect("scratch");
    let root = directory.path().canonicalize().expect("canonical scratch");
    let sh = tool("sh");
    let sh_text = sh.to_str().expect("UTF-8");
    let (mount, rel) = sh_text
        .strip_prefix("/usr/")
        .map_or(("/bin", sh_text.trim_start_matches("/bin/")), |rel| {
            ("/usr", rel)
        });
    let lists = Lists {
        program: Located::new(Class::Follow, mount, rel).expect("the shell's path"),
        loader: substrate_exec_gate::elf_interpreter(&std::fs::read(tool("env")).expect("env")),
        allow: Vec::new(),
        writable: Vec::new(),
    };
    let gate = |marker: &Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_substrate-exec-gate"));
        command
            .args(lists.options())
            .arg(&sh)
            .args(["-c", &format!("echo ran; : > '{}'", marker.display())])
            .current_dir(&root)
            .env_clear();
        command
    };

    let control = root.join("control");
    let output = gate(&control).output().expect("the gate starts");
    if output.status.code() == Some(REFUSED) {
        println!(
            "adversary landlock failures: ABSENT ({})",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return;
    }
    assert!(output.status.success() && control.exists(), "the control");

    for (name, syscall, errno) in [
        (
            "landlock_create_ruleset",
            libc::SYS_landlock_create_ruleset,
            libc::ENOSYS,
        ),
        (
            "landlock_create_ruleset",
            libc::SYS_landlock_create_ruleset,
            libc::EOPNOTSUPP,
        ),
        (
            "landlock_add_rule",
            libc::SYS_landlock_add_rule,
            libc::EINVAL,
        ),
        (
            "landlock_restrict_self",
            libc::SYS_landlock_restrict_self,
            libc::EPERM,
        ),
    ] {
        let marker = root.join(format!("ran-{name}-{errno}"));
        let mut command = gate(&marker);
        failing(&mut command, syscall, errno);
        let output = command.output().expect("the gate starts");
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            !marker.exists() && output.stdout.is_empty(),
            "{name} failed with errno {errno} and the program ran: {diagnostic}"
        );
        assert_eq!(
            output.status.code(),
            Some(REFUSED),
            "{name} errno {errno}: {diagnostic}"
        );
        assert!(
            diagnostic.starts_with("substrate-exec-gate: ")
                && diagnostic.trim_end().lines().count() == 1,
            "{name} errno {errno}: one diagnostic line, got {diagnostic:?}"
        );
    }
    println!("adversary landlock failures: executed");
}

/// A 64-bit little-endian ELF image whose one `PT_INTERP` header names `interpreter`.
fn elf64(interpreter: &str) -> Vec<u8> {
    let text = format!("{interpreter}\0");
    let mut image = vec![0_u8; 64];
    image[..4].copy_from_slice(b"\x7fELF");
    image[4] = 2;
    image[5] = 1;
    image[32..40].copy_from_slice(&64_u64.to_le_bytes());
    image[54..56].copy_from_slice(&56_u16.to_le_bytes());
    image[56..58].copy_from_slice(&1_u16.to_le_bytes());
    let mut header = vec![0_u8; 56];
    header[..4].copy_from_slice(&3_u32.to_le_bytes());
    header[8..16].copy_from_slice(&120_u64.to_le_bytes());
    header[32..40].copy_from_slice(&(text.len() as u64).to_le_bytes());
    image.extend(header);
    image.extend(text.bytes());
    image
}

/// **The loader is one file, never the root of the view.** The helper grants the execute right
/// to what `--loader` names, and a rule on `/` reaches every file of the view: a loader of `/`
/// would turn the list off. Two places refuse it: the argument grammar, and the reader of
/// `PT_INTERP` the driver takes the loader from (D7).
#[test]
fn adversary_the_root_is_never_a_loader() {
    assert_eq!(
        substrate_exec_gate::elf_interpreter(&elf64("/lib64/ld-linux-x86-64.so.2")).as_deref(),
        Some("/lib64/ld-linux-x86-64.so.2"),
        "the control: the image is one the reader takes a loader from"
    );
    assert_eq!(substrate_exec_gate::elf_interpreter(&elf64("/")), None);

    let arguments = |loader: &str| -> Vec<OsString> {
        [
            "--program",
            "follow",
            "/usr",
            "bin/sh",
            "--loader",
            loader,
            "--",
            "/usr/bin/sh",
        ]
        .iter()
        .map(OsString::from)
        .collect()
    };
    substrate_exec_gate::parse(arguments("/lib64/ld-linux-x86-64.so.2")).expect("the control");
    let refused = substrate_exec_gate::parse(arguments("/")).expect_err("a loader of /");
    assert!(refused.contains("loader"), "{refused}");
}
