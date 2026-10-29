//! The `keeper` role: PID 1 of a workspace's keeper Pod (design 34 § 7.2).
//!
//! The keeper holds the workspace volume mounted while the driver runs `fs` calls in it through
//! `pods/exec`. As PID 1 it inherits every orphan, so it reaps children, and it exits on `TERM`
//! (or `INT`) so a Pod deletion completes inside its grace period.

/// Reaps every child until `TERM` or `INT` arrives, then returns.
pub fn keep() {
    // SAFETY: the set is initialised by sigemptyset before use, and blocking these three signals
    // in this single-threaded process routes them all to sigwait below.
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        libc::sigaddset(&raw mut set, libc::SIGTERM);
        libc::sigaddset(&raw mut set, libc::SIGINT);
        libc::sigaddset(&raw mut set, libc::SIGCHLD);
        libc::sigprocmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut());
        set
    };
    loop {
        let mut signal: libc::c_int = 0;
        // SAFETY: set is a valid, initialised signal set and signal a valid out pointer.
        if unsafe { libc::sigwait(&raw const set, &raw mut signal) } != 0 {
            continue;
        }
        reap();
        if signal == libc::SIGTERM || signal == libc::SIGINT {
            return;
        }
    }
}

/// Reaps every child that has ended, without waiting for one that has not.
fn reap() {
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: waitpid with WNOHANG and a valid status pointer never blocks.
        let pid = unsafe { libc::waitpid(-1, &raw mut status, libc::WNOHANG) };
        if pid <= 0 {
            return;
        }
    }
}
