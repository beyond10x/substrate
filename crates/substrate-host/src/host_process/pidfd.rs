//! Process descriptors: a signal that cannot reach a recycled pid.
//!
//! A pid read from `cgroup.procs` names whichever process holds that number when the signal is
//! sent, and an exited member's number can be reused in between. A pidfd names one process for its
//! whole life: [`send`] to an exited one answers `ESRCH` and reaches nobody else. The driver also
//! holds one on its own main process, which keeps that process reachable after it left its cgroup.

use std::os::fd::{AsRawFd as _, BorrowedFd, FromRawFd as _, OwnedFd};

/// Opens a descriptor on `pid`. `None` when no process holds that number now.
pub(super) fn open(pid: u32) -> std::io::Result<Option<OwnedFd>> {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return Ok(None);
    };
    if pid <= 0 {
        return Ok(None);
    }
    // SAFETY: `pidfd_open` takes a pid and a flags word and returns a new descriptor or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0_u32) };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let fd = i32::try_from(fd).map_err(|_| std::io::Error::from_raw_os_error(libc::EBADF))?;
    // SAFETY: `pidfd_open` returned a new close-on-exec descriptor owned by this scope.
    Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Sends `signal` to the process `fd` names. `Ok(false)` when that process has exited; it never
/// reaches another process. Signal `0` probes without sending.
pub(super) fn send(fd: BorrowedFd<'_>, signal: i32) -> std::io::Result<bool> {
    // SAFETY: the descriptor is a live pidfd; a null siginfo and zero flags are the documented
    // form of a plain `kill`.
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0_u32,
        )
    };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error)
    }
}

/// Whether the process `fd` names has exited, reaped or not. A pidfd polls readable once its
/// process has exited.
pub(super) fn exited(fd: BorrowedFd<'_>) -> bool {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one initialized pollfd and a zero timeout.
    let ready = unsafe { libc::poll(&raw mut poll, 1, 0) };
    ready > 0 && poll.revents != 0
}
