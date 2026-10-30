//! Process hardening at start (design 31 § 5, point 1): no core file, and no same-uid `ptrace`
//! or `/proc/<pid>/mem` read of the credential the runner holds for one request. And the
//! descriptor rule for every child it starts (§ 5, point 4; § 4): the token command and the
//! restart hook hold descriptors 0 to 2 and the pipes the runner passes as those, nothing else.

use nix::sys::resource::{Resource, setrlimit};

/// Sets `PR_SET_DUMPABLE` to 0 and `RLIMIT_CORE` to 0.
pub fn harden() -> nix::Result<()> {
    nix::sys::prctl::set_dumpable(false)?;
    setrlimit(Resource::RLIMIT_CORE, 0, 0)
}

/// Makes `command`'s child close every descriptor above 2 at `exec`, whoever opened it: one this
/// runner's own parent left inheritable (a supervisor, `flock(1)`) as much as one the runner
/// opened. A kernel without `close_range(2)` (before Linux 5.11) refuses the spawn, so no child
/// starts with a descriptor it should not hold.
pub fn standard_streams_only(command: &mut tokio::process::Command) {
    // The closure runs in the forked child after its standard streams are in place and before
    // `exec`. Marking instead of closing keeps the standard library's own exec-error pipe, which
    // is close-on-exec already, working until the `exec`.
    let seal = || {
        // SAFETY: one raw `close_range(2)` system call on descriptors of this (child) process; it
        // takes no pointer, allocates nothing and is async-signal-safe.
        let marked = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                3_u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            )
        };
        if marked == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    };
    // SAFETY: `seal` is async-signal-safe (a single system call and an `errno` read) and touches
    // no state shared with the parent, which is what `pre_exec` requires between fork and exec.
    unsafe {
        command.pre_exec(seal);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hardening_leaves_the_process_undumpable_without_a_core_limit() {
        super::harden().expect("hardening");
        assert!(!nix::sys::prctl::get_dumpable().expect("dumpable flag"));
        let (soft, hard) = nix::sys::resource::getrlimit(nix::sys::resource::Resource::RLIMIT_CORE)
            .expect("core limit");
        assert_eq!((soft, hard), (0, 0));
    }

    /// A descriptor this process holds without close-on-exec never reaches a sealed child.
    #[tokio::test]
    async fn a_sealed_child_holds_only_the_standard_streams() {
        let inherited = std::fs::File::open("/proc/self/stat").expect("a descriptor");
        nix::fcntl::fcntl(
            &inherited,
            nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()),
        )
        .expect("clear close-on-exec");
        let mut command = tokio::process::Command::new("/bin/sh");
        command
            // `; true` keeps the shell from exec-ing `ls`, which would list its own directory fd.
            .args(["-c", "ls /proc/$$/fd; true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        super::standard_streams_only(&mut command);
        let output = command.output().await.expect("the sealed child runs");
        drop(inherited);
        let listed = String::from_utf8(output.stdout).expect("listing");
        assert_eq!(
            listed.split_whitespace().collect::<Vec<_>>(),
            ["0", "1", "2"]
        );
    }
}
