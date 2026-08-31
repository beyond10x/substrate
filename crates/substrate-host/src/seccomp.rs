use std::fs::File;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::os::fd::{AsRawFd as _, RawFd};

use crate::DriverError;

const ALLOW: u32 = 0x7fff_0000;
const ERRNO: u32 = 0x0005_0000;
const LD_W_ABS: u16 = 0x20;
const JMP_JEQ_K: u16 = 0x15;
const RET_K: u16 = 0x06;

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("the confinement seccomp profile has no audited architecture for this target");

fn statement(code: u16, value: u32) -> libc::sock_filter {
    libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k: value,
    }
}

fn jump(value: u32, yes: u8, no: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: JMP_JEQ_K,
        jt: yes,
        jf: no,
        k: value,
    }
}

pub(crate) fn profile() -> Result<File, DriverError> {
    let deny = ERRNO | u32::try_from(libc::EACCES).unwrap_or(13);
    let filters = [
        statement(LD_W_ABS, 4),
        jump(AUDIT_ARCH, 1, 0),
        statement(RET_K, deny),
        statement(LD_W_ABS, 0),
        jump(
            u32::try_from(libc::SYS_io_uring_setup).unwrap_or(u32::MAX),
            0,
            1,
        ),
        statement(RET_K, deny),
        jump(u32::try_from(libc::SYS_socket).unwrap_or(u32::MAX), 0, 3),
        statement(LD_W_ABS, 16),
        jump(u32::try_from(libc::AF_UNIX).unwrap_or(1), 0, 1),
        statement(RET_K, deny),
        statement(RET_K, ALLOW),
    ];
    let mut file = tempfile::tempfile().map_err(|error| failed(&error))?;
    // SAFETY: `filters` is a contiguous array of the kernel ABI type for this slice's lifetime.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            filters.as_ptr().cast::<u8>(),
            std::mem::size_of_val(&filters),
        )
    };
    file.write_all(bytes)
        .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
        .map_err(|error| failed(&error))?;
    clear_cloexec(file.as_raw_fd())?;
    Ok(file)
}

fn failed(error: &std::io::Error) -> DriverError {
    DriverError::failed(
        "exec.sandbox-unavailable",
        format!("seccomp profile: {error}"),
    )
}

fn clear_cloexec(fd: RawFd) -> Result<(), DriverError> {
    // SAFETY: fcntl reads and updates flags on an owned descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } != 0 {
        return Err(failed(&std::io::Error::last_os_error()));
    }
    Ok(())
}
