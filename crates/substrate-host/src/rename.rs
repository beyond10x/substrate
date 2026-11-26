//! `renameat2(2)` with `RENAME_NOREPLACE` on every Linux C library the driver is built for.
//!
//! nix wraps `renameat2` only for glibc. A glibc build keeps that wrapper; a musl build makes the
//! same system call itself, with the same arguments and the same errors.

use std::path::Path;

use nix::errno::Errno;

/// Renames `source` to `target`, both relative to the working directory, and refuses with
/// `EEXIST` when `target` exists.
#[cfg(target_env = "gnu")]
pub(crate) fn rename_noreplace(source: &Path, target: &Path) -> Result<(), Errno> {
    nix::fcntl::renameat2(
        nix::fcntl::AT_FDCWD,
        source,
        nix::fcntl::AT_FDCWD,
        target,
        nix::fcntl::RenameFlags::RENAME_NOREPLACE,
    )
}

#[cfg(not(target_env = "gnu"))]
pub(crate) use rename_noreplace_by_syscall as rename_noreplace;

/// [`rename_noreplace`] as the system call. A glibc build compiles it only for its tests.
#[cfg(any(test, not(target_env = "gnu")))]
pub(crate) fn rename_noreplace_by_syscall(source: &Path, target: &Path) -> Result<(), Errno> {
    use std::os::unix::ffi::OsStrExt as _;

    let source =
        std::ffi::CString::new(source.as_os_str().as_bytes()).map_err(|_| Errno::EINVAL)?;
    let target =
        std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(|_| Errno::EINVAL)?;
    // SAFETY: both pointers name NUL-terminated strings that outlive the call, and the kernel
    // reads nothing else.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    Errno::result(result).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Rename = fn(&Path, &Path) -> Result<(), Errno>;

    fn both() -> [(&'static str, Rename); 2] {
        [
            ("rename_noreplace", rename_noreplace),
            ("rename_noreplace_by_syscall", rename_noreplace_by_syscall),
        ]
    }

    #[test]
    fn a_rename_to_an_absent_name_moves_the_entry() {
        for (name, rename) in both() {
            let root = tempfile::tempdir().expect("temporary directory");
            let source = root.path().join("source");
            let target = root.path().join("target");
            std::fs::create_dir(&source).expect("source");
            std::fs::write(source.join("file"), "bytes").expect("file");
            rename(&source, &target).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(!source.exists(), "{name}: the source name is still there");
            assert_eq!(
                std::fs::read_to_string(target.join("file")).expect("moved file"),
                "bytes",
                "{name}"
            );
        }
    }

    #[test]
    fn a_rename_onto_an_existing_name_is_eexist_and_changes_nothing() {
        for (name, rename) in both() {
            let root = tempfile::tempdir().expect("temporary directory");
            let source = root.path().join("source");
            let target = root.path().join("target");
            std::fs::create_dir(&source).expect("source");
            std::fs::create_dir(&target).expect("target");
            std::fs::write(target.join("kept"), "kept").expect("kept file");
            assert_eq!(rename(&source, &target), Err(Errno::EEXIST), "{name}");
            assert!(source.is_dir(), "{name}: the source moved");
            assert_eq!(
                std::fs::read_to_string(target.join("kept")).expect("kept file"),
                "kept",
                "{name}"
            );
        }
    }

    #[test]
    fn a_path_holding_a_nul_byte_is_einval() {
        let root = tempfile::tempdir().expect("temporary directory");
        let source = root.path().join("source");
        std::fs::create_dir(&source).expect("source");
        let target = root.path().join("tar\0get");
        for (name, rename) in both() {
            assert_eq!(rename(&source, &target), Err(Errno::EINVAL), "{name}");
            assert!(source.is_dir(), "{name}: the source moved");
        }
    }
}
