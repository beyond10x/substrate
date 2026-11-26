//! `renameat2(2)` with `RENAME_NOREPLACE` on every Linux C library the driver is built for.
//!
//! The driver makes the system call itself: nix wraps `renameat2` only for glibc, and glibc
//! exports it from 2.28 on, so neither a musl build nor a binary linked against an older glibc
//! has the wrapper. On glibc the tests hold the system call to nix's wrapper, which answers with
//! the same errors.

use std::path::Path;

use nix::errno::Errno;

/// [`rename_noreplace`] through nix's wrapper of the C library's `renameat2`, which the tests
/// compare it with.
#[cfg(all(test, target_env = "gnu"))]
fn rename_noreplace_by_nix(source: &Path, target: &Path) -> Result<(), Errno> {
    nix::fcntl::renameat2(
        nix::fcntl::AT_FDCWD,
        source,
        nix::fcntl::AT_FDCWD,
        target,
        nix::fcntl::RenameFlags::RENAME_NOREPLACE,
    )
}

/// Renames `source` to `target`, both relative to the working directory, and refuses with
/// `EEXIST` when `target` exists.
pub(crate) fn rename_noreplace(source: &Path, target: &Path) -> Result<(), Errno> {
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

    /// The system call, and on glibc nix's wrapper beside it.
    fn both() -> Vec<(&'static str, Rename)> {
        let mut forms: Vec<(&'static str, Rename)> = vec![("rename_noreplace", rename_noreplace)];
        #[cfg(target_env = "gnu")]
        forms.push(("rename_noreplace_by_nix", rename_noreplace_by_nix));
        forms
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

    /// Every entry beneath `root`, by relative path, with its kind, a file's bytes and a link's
    /// target.
    fn tree(root: &Path) -> Vec<(String, String)> {
        let mut entries = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).expect("read directory") {
                let path = entry.expect("entry").path();
                let relative = path
                    .strip_prefix(root)
                    .expect("beneath the root")
                    .display()
                    .to_string();
                let metadata = std::fs::symlink_metadata(&path).expect("metadata");
                let kind = if metadata.file_type().is_symlink() {
                    format!(
                        "link {}",
                        std::fs::read_link(&path).expect("link").display()
                    )
                } else if metadata.is_dir() {
                    pending.push(path);
                    "directory".to_owned()
                } else {
                    format!(
                        "file {}",
                        String::from_utf8_lossy(&std::fs::read(&path).expect("file"))
                    )
                };
                entries.push((relative, kind));
            }
        }
        entries.sort();
        entries
    }

    type Setup = fn(&Path) -> (std::path::PathBuf, std::path::PathBuf);

    /// Each case by its name, the setup that makes its tree and names source and target, and
    /// the answer `renameat2(2)` documents for it.
    fn cases() -> [(&'static str, Setup, Result<(), Errno>); 12] {
        [
            (
                "an absent source",
                |root| (root.join("absent"), root.join("target")),
                Err(Errno::ENOENT),
            ),
            (
                "a file onto a file",
                |root| {
                    std::fs::write(root.join("a"), "a").expect("a");
                    std::fs::write(root.join("b"), "b").expect("b");
                    (root.join("a"), root.join("b"))
                },
                Err(Errno::EEXIST),
            ),
            (
                "a file onto a directory",
                |root| {
                    std::fs::write(root.join("a"), "a").expect("a");
                    std::fs::create_dir(root.join("d")).expect("d");
                    (root.join("a"), root.join("d"))
                },
                Err(Errno::EEXIST),
            ),
            (
                "an entry onto its own name",
                |root| {
                    std::fs::write(root.join("a"), "a").expect("a");
                    (root.join("a"), root.join("a"))
                },
                Err(Errno::EEXIST),
            ),
            (
                "a target whose parent is absent",
                |root| {
                    std::fs::write(root.join("a"), "a").expect("a");
                    (root.join("a"), root.join("absent/target"))
                },
                Err(Errno::ENOENT),
            ),
            (
                "a directory into itself",
                |root| {
                    std::fs::create_dir_all(root.join("d/sub")).expect("d/sub");
                    (root.join("d"), root.join("d/sub/moved"))
                },
                Err(Errno::EINVAL),
            ),
            (
                "an empty source",
                |root| (std::path::PathBuf::new(), root.join("target")),
                Err(Errno::ENOENT),
            ),
            (
                "a file named as a directory",
                |root| {
                    std::fs::write(root.join("a"), "a").expect("a");
                    (root.join("a/"), root.join("target"))
                },
                Err(Errno::ENOTDIR),
            ),
            (
                "a path through a file",
                |root| {
                    std::fs::write(root.join("a"), "a").expect("a");
                    (root.join("a/below"), root.join("target"))
                },
                Err(Errno::ENOTDIR),
            ),
            (
                "a symbolic link moves itself, not what it names",
                |root| {
                    std::fs::create_dir(root.join("d")).expect("d");
                    std::os::unix::fs::symlink("d", root.join("link")).expect("link");
                    (root.join("link"), root.join("moved"))
                },
                Ok(()),
            ),
            (
                "a dangling symbolic link as the target",
                |root| {
                    std::fs::write(root.join("a"), "a").expect("a");
                    std::os::unix::fs::symlink("absent", root.join("dangling")).expect("link");
                    (root.join("a"), root.join("dangling"))
                },
                Err(Errno::EEXIST),
            ),
            (
                "a target name longer than a name may be",
                |root| {
                    std::fs::write(root.join("a"), "a").expect("a");
                    (root.join("a"), root.join("x".repeat(256)))
                },
                Err(Errno::ENAMETOOLONG),
            ),
        ]
    }

    /// The two forms answer each case alike, as `renameat2(2)` documents it, and leave the
    /// same tree behind.
    #[test]
    fn both_forms_answer_every_case_alike_and_leave_the_same_tree() {
        for (case, setup, expected) in cases() {
            let mut answers = Vec::new();
            for (name, rename) in both() {
                let root = tempfile::tempdir().expect("temporary directory");
                let (source, target) = setup(root.path());
                let answer = rename(&source, &target);
                assert_eq!(answer, expected, "{case}: {name}");
                answers.push((answer, tree(root.path())));
            }
            for pair in answers.windows(2) {
                assert_eq!(pair[0], pair[1], "{case}: the forms differ");
            }
        }
    }
}
