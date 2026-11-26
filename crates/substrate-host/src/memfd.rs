//! `memfd_create(2)` as the system call, on every Linux C library the driver is built for.
//!
//! glibc exports `memfd_create` from 2.27 on, so a binary that calls the C library's wrapper does
//! not link against an older glibc. The system call takes the same arguments and gives the same
//! errors on every C library.

use std::ffi::CStr;
use std::io;
use std::os::fd::{FromRawFd as _, OwnedFd};

/// Creates anonymous memory named `name` with `flags` (`MFD_*`) and returns its descriptor.
pub(crate) fn memfd_create(name: &CStr, flags: libc::c_uint) -> io::Result<OwnedFd> {
    // SAFETY: `name` is a NUL-terminated string that outlives the call, and the kernel reads
    // nothing else.
    let result = unsafe { libc::syscall(libc::SYS_memfd_create, name.as_ptr(), flags) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    let raw =
        libc::c_int::try_from(result).map_err(|_| io::Error::from_raw_os_error(libc::EBADF))?;
    // SAFETY: the kernel returned a fresh descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Seek as _, Write as _};
    use std::os::fd::AsRawFd as _;

    use super::*;

    #[test]
    fn the_memory_is_named_sealable_and_closed_on_exec() {
        let memory = memfd_create(
            c"substrate-memfd-test",
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
        .expect("memfd_create");
        let link = std::fs::read_link(format!("/proc/self/fd/{}", memory.as_raw_fd()))
            .expect("descriptor link");
        assert_eq!(
            link.display().to_string(),
            "/memfd:substrate-memfd-test (deleted)"
        );
        // SAFETY: `fcntl` reads the flags of a live descriptor.
        let flags = unsafe { libc::fcntl(memory.as_raw_fd(), libc::F_GETFD) };
        assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
        let mut file = std::fs::File::from(memory);
        file.write_all(b"bytes").expect("write");
        // SAFETY: `fcntl` adds seals to a live memfd.
        let sealed =
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_WRITE) };
        assert_eq!(sealed, 0, "{}", io::Error::last_os_error());
        assert!(
            file.write_all(b"more").is_err(),
            "a sealed memfd took a write"
        );
        file.rewind().expect("rewind");
        let mut read = String::new();
        file.read_to_string(&mut read).expect("read");
        assert_eq!(read, "bytes");
    }

    #[test]
    fn without_allow_sealing_no_seal_is_taken() {
        let memory = memfd_create(c"unsealable", libc::MFD_CLOEXEC).expect("memfd_create");
        // SAFETY: `fcntl` adds seals to a live memfd.
        let sealed =
            unsafe { libc::fcntl(memory.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_WRITE) };
        assert_eq!(sealed, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
    }

    #[test]
    fn an_unknown_flag_is_einval() {
        let error = memfd_create(c"flags", 0x8000_0000).expect_err("an unknown flag");
        assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
    }

    /// The crate calls neither C library wrapper that a glibc older than 2.27 (`memfd_create`)
    /// or 2.28 (`renameat2`) lacks: `memfd_create` only here, `renameat2` only through
    /// `crate::rename`.
    #[test]
    fn no_source_calls_a_wrapper_an_older_glibc_lacks() {
        let wrappers = [
            (["libc::", "memfd_create("].concat(), "memfd.rs"),
            (["fcntl::", "renameat2("].concat(), "rename.rs"),
            (["libc::", "renameat2("].concat(), "rename.rs"),
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut pending = vec![root.clone()];
        let mut found = Vec::new();
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).expect("read directory") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("source");
                for (needle, home) in &wrappers {
                    if text.contains(needle.as_str()) && path != root.join(home) {
                        found.push(format!("{}: {needle}", path.display()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "{found:#?}");
    }
}
