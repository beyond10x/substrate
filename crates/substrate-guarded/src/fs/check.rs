//! The keeper's `fs check` (design 34 § 6.2): three proofs, made in a scratch directory created
//! beneath the pinned root, each answered on its own so the driver publishes each fact from its
//! own proof. The scratch directory and everything in it are removed before the answer is given;
//! a check that cannot remove them answers nothing proved.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::Read as _;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use substrate_wire::{FileMode, FileReadQuery};
use ulid::Ulid;

use super::{
    DESTROY_BATCH_ITEMS, FILE_READ_FLAGS, GUARDED_RESOLVE, GuardedFilesystem, fstat, fstatat,
    openat2_resolved, raw_directory_entries, remove_children_batch, sync_fd, unlink_entry,
};
use crate::DriverError;

/// The scratch directory's name before its ULID. A served write's temporary is `.substrate-…`
/// too, which is why the replacement proof looks for that prefix inside the scratch directory.
const SCRATCH_PREFIX: &str = ".substrate-check-";
/// The root name the check's served calls use. On a single-workspace root every legal root name
/// names the pinned root itself.
const ROOT_NAME: &str = "check";
/// The file the replacement proof writes and then replaces.
const TARGET: &CStr = c"target";
const BEFORE: &[u8] = b"before\n";
const AFTER: &[u8] = b"after\n";
/// The links the escape proof creates, by name and target: one leaves the scratch directory by an
/// absolute target, one by climbing, and one stays inside it, which `RESOLVE_NO_SYMLINKS` refuses
/// all the same.
const LINKS: [(&CStr, &CStr); 3] = [(c"absolute", c"/"), (c"climb", c".."), (c"inner", c".")];

/// What the keeper's check proved, one answer per proof of design 34 § 6.2.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuardedIoProofs {
    /// `openat2` with `RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS|RESOLVE_NO_MAGICLINKS|RESOLVE_NO_XDEV`
    /// opened the scratch directory beneath the root as the directory it is, and refused `..` and
    /// `/` from it with `EXDEV`.
    pub openat2_beneath: bool,
    /// Three links (an absolute target, a climb out, one inside) were created, and each was
    /// refused by `openat2` under those flags and by the served read of a path through it
    /// (`workspace.path-escape`).
    pub symlink_escape_refused: bool,
    /// The served write replaced an existing file by renaming a new one over it: the name holds
    /// the new bytes in a new inode, a descriptor held on the old file still reads every old byte,
    /// and no temporary is left beside it.
    pub atomic_replace: bool,
}

/// How a proof reaches the kernel. The served check always uses [`GUARDED_RESOLVE`] and the
/// served write; the tests replace one of them at a time to show each proof failing on its own.
struct Mechanism<'a> {
    resolve: u64,
    replace: Replace<'a>,
}

/// Writes `content` at a root-relative path, replacing what is there.
type Replace<'a> = &'a dyn Fn(&GuardedFilesystem, &str, &[u8]) -> Result<(), DriverError>;

impl GuardedFilesystem {
    /// Runs the keeper's three proofs (design 34 § 6.2) in a scratch directory beneath the root and
    /// removes it again. Only a single-workspace root with `openat2` is checked; any other answers
    /// nothing proved and creates nothing.
    pub fn prove_guarded_io(&self) -> GuardedIoProofs {
        prove(
            self,
            &Mechanism {
                resolve: GUARDED_RESOLVE,
                replace: &served_replace,
            },
        )
    }
}

fn served_replace(
    filesystem: &GuardedFilesystem,
    path: &str,
    content: &[u8],
) -> Result<(), DriverError> {
    filesystem
        .write_atomic(ROOT_NAME, ROOT_NAME, path, content)
        .map(drop)
}

fn prove(filesystem: &GuardedFilesystem, mechanism: &Mechanism<'_>) -> GuardedIoProofs {
    if !filesystem.single || !filesystem.openat2 {
        return GuardedIoProofs::default();
    }
    let Some(scratch) = Scratch::create(filesystem.root.as_raw_fd()) else {
        return GuardedIoProofs::default();
    };
    let proofs = GuardedIoProofs {
        openat2_beneath: openat2_beneath(&scratch, mechanism.resolve),
        symlink_escape_refused: symlink_escape_refused(filesystem, &scratch, mechanism.resolve),
        atomic_replace: atomic_replace(filesystem, &scratch, mechanism.replace),
    };
    if scratch.remove() {
        proofs
    } else {
        GuardedIoProofs::default()
    }
}

/// The check's scratch directory, a direct child of the root, removed on [`Scratch::remove`] or,
/// failing that, on drop.
struct Scratch {
    root: RawFd,
    name: CString,
    directory: Option<OwnedFd>,
    removed: bool,
}

impl Scratch {
    fn create(root: RawFd) -> Option<Self> {
        let name = CString::new(format!("{SCRATCH_PREFIX}{}", Ulid::generate()))
            .expect("a ULID has no NUL");
        // SAFETY: root is the live pinned root descriptor and name is NUL-terminated.
        if unsafe { libc::mkdirat(root, name.as_ptr(), 0o700) } != 0 {
            return None;
        }
        let mut scratch = Self {
            root,
            name,
            directory: None,
            removed: false,
        };
        scratch.directory = openat2_resolved(
            root,
            &scratch.name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0,
            GUARDED_RESOLVE,
        )
        .ok();
        // An unopened scratch directory is empty; dropping it removes it.
        scratch.directory.is_some().then_some(scratch)
    }

    fn descriptor(&self) -> Option<RawFd> {
        self.directory.as_ref().map(AsRawFd::as_raw_fd)
    }

    /// `name` beneath the scratch directory, as a path relative to the root.
    fn path(&self, name: &CStr) -> String {
        format!(
            "{}/{}",
            self.name.to_str().expect("the scratch name is ASCII"),
            name.to_str().expect("every check name is ASCII")
        )
    }

    fn remove(mut self) -> bool {
        self.removed = true;
        self.clear()
    }

    fn clear(&mut self) -> bool {
        let emptied = self.directory.take().is_none_or(|directory| {
            remove_children_batch(directory.as_raw_fd(), DESTROY_BATCH_ITEMS)
                .is_ok_and(|batch| batch.complete)
        });
        emptied
            && unlink_entry(self.root, &self.name, libc::AT_REMOVEDIR).is_ok()
            && sync_fd(self.root).is_ok()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.removed {
            self.clear();
        }
    }
}

fn errno(opened: std::io::Result<OwnedFd>) -> Option<i32> {
    opened.err().and_then(|error| error.raw_os_error())
}

fn openat2_beneath(scratch: &Scratch, resolve: u64) -> bool {
    let Some(directory) = scratch.descriptor() else {
        return false;
    };
    let opened = openat2_resolved(
        scratch.root,
        &scratch.name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0,
        resolve,
    );
    let same = opened
        .ok()
        .and_then(|fd| fstat(fd.as_raw_fd()).ok())
        .zip(fstat(directory).ok())
        .is_some_and(|(opened, held)| opened.st_dev == held.st_dev && opened.st_ino == held.st_ino);
    same && [c"..", c"/"].into_iter().all(|path| {
        errno(openat2_resolved(
            directory,
            path,
            libc::O_PATH | libc::O_CLOEXEC,
            0,
            resolve,
        )) == Some(libc::EXDEV)
    })
}

fn symlink_escape_refused(filesystem: &GuardedFilesystem, scratch: &Scratch, resolve: u64) -> bool {
    let Some(directory) = scratch.descriptor() else {
        return false;
    };
    let query = FileReadQuery {
        mode: FileMode::File,
        offset: Some(0),
        limit_bytes: Some(1),
        cursor: None,
        limit_items: None,
    };
    LINKS.into_iter().all(|(name, target)| {
        // SAFETY: directory is the live scratch descriptor and both strings are NUL-terminated.
        let linked = unsafe { libc::symlinkat(target.as_ptr(), directory, name.as_ptr()) } == 0;
        let opened = errno(openat2_resolved(
            directory,
            name,
            libc::O_PATH | libc::O_CLOEXEC,
            0,
            resolve,
        ));
        let through = format!("{}/{}", scratch.path(name), TARGET.to_str().expect("ASCII"));
        let served = filesystem.read_digested(ROOT_NAME, ROOT_NAME, &through, &query);
        linked
            && matches!(opened, Some(libc::ELOOP | libc::EXDEV))
            && served.is_err_and(|error| error.code == "workspace.path-escape")
    })
}

fn atomic_replace(filesystem: &GuardedFilesystem, scratch: &Scratch, replace: Replace<'_>) -> bool {
    let Some(directory) = scratch.descriptor() else {
        return false;
    };
    let path = scratch.path(TARGET);
    if replace(filesystem, &path, BEFORE).is_err() {
        return false;
    }
    let Ok(held) = openat2_resolved(directory, TARGET, FILE_READ_FLAGS, 0, GUARDED_RESOLVE) else {
        return false;
    };
    let Ok(before) = fstat(held.as_raw_fd()) else {
        return false;
    };
    if replace(filesystem, &path, AFTER).is_err() {
        return false;
    }
    let Ok(after) = fstatat(directory, TARGET) else {
        return false;
    };
    let renamed = after.st_mode & libc::S_IFMT == libc::S_IFREG
        && (after.st_dev, after.st_ino) != (before.st_dev, before.st_ino);
    let current = openat2_resolved(directory, TARGET, FILE_READ_FLAGS, 0, GUARDED_RESOLVE).ok();
    let no_temporary = raw_directory_entries(directory).is_ok_and(|entries| {
        entries
            .iter()
            .all(|(name, _, _)| !name.to_bytes().starts_with(b".substrate-"))
    });
    renamed
        && read_all(held).as_deref() == Some(BEFORE)
        && current.and_then(read_all).as_deref() == Some(AFTER)
        && no_temporary
}

/// Every byte of a file the check wrote, bounded by the larger of the two contents.
fn read_all(fd: OwnedFd) -> Option<Vec<u8>> {
    let bound = u64::try_from(BEFORE.len().max(AFTER.len())).expect("usize fits u64") + 1;
    let mut bytes = Vec::new();
    File::from(fd).take(bound).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use super::{
        GUARDED_RESOLVE, GuardedFilesystem, GuardedIoProofs, Mechanism, prove, served_replace,
    };
    use crate::DriverError;

    const RESOLVE_NO_SYMLINKS: u64 = 0x04;
    const RESOLVE_BENEATH: u64 = 0x08;

    fn single(root: &Path) -> Option<GuardedFilesystem> {
        let filesystem =
            GuardedFilesystem::open_single(root, 1024, 1024, 100).expect("guarded filesystem");
        if filesystem.openat2_available() {
            Some(filesystem)
        } else {
            eprintln!(
                "openat2 is unavailable on this kernel; the check's proofs are not exercised"
            );
            None
        }
    }

    fn assert_left_empty(root: &Path) {
        let left: Vec<_> = std::fs::read_dir(root)
            .expect("the root lists")
            .map(|entry| entry.expect("an entry").file_name())
            .collect();
        assert!(left.is_empty(), "the check left {left:?} behind");
    }

    fn with_resolve(filesystem: &GuardedFilesystem, resolve: u64) -> GuardedIoProofs {
        prove(
            filesystem,
            &Mechanism {
                resolve,
                replace: &served_replace,
            },
        )
    }

    #[test]
    fn the_served_check_proves_all_three_and_leaves_nothing_behind() {
        let root = tempdir().expect("tempdir");
        let Some(filesystem) = single(root.path()) else {
            return;
        };
        assert_eq!(
            filesystem.prove_guarded_io(),
            GuardedIoProofs {
                openat2_beneath: true,
                symlink_escape_refused: true,
                atomic_replace: true,
            }
        );
        assert_left_empty(root.path());
    }

    #[test]
    fn resolution_that_is_not_held_beneath_fails_only_the_openat2_proof() {
        let root = tempdir().expect("tempdir");
        let Some(filesystem) = single(root.path()) else {
            return;
        };
        assert_eq!(
            with_resolve(&filesystem, GUARDED_RESOLVE & !RESOLVE_BENEATH),
            GuardedIoProofs {
                openat2_beneath: false,
                symlink_escape_refused: true,
                atomic_replace: true,
            }
        );
        assert_left_empty(root.path());
    }

    #[test]
    fn resolution_that_follows_a_symlink_fails_only_the_escape_proof() {
        let root = tempdir().expect("tempdir");
        let Some(filesystem) = single(root.path()) else {
            return;
        };
        assert_eq!(
            with_resolve(&filesystem, GUARDED_RESOLVE & !RESOLVE_NO_SYMLINKS),
            GuardedIoProofs {
                openat2_beneath: true,
                symlink_escape_refused: false,
                atomic_replace: true,
            }
        );
        assert_left_empty(root.path());
    }

    #[test]
    fn a_write_in_place_fails_only_the_replacement_proof() {
        let root = tempdir().expect("tempdir");
        let Some(filesystem) = single(root.path()) else {
            return;
        };
        let directory = root.path().to_path_buf();
        let in_place = move |_: &GuardedFilesystem, path: &str, content: &[u8]| {
            std::fs::write(directory.join(path), content).map_err(|_| DriverError::not_found())
        };
        let proofs = prove(
            &filesystem,
            &Mechanism {
                resolve: GUARDED_RESOLVE,
                replace: &in_place,
            },
        );
        assert_eq!(
            proofs,
            GuardedIoProofs {
                openat2_beneath: true,
                symlink_escape_refused: true,
                atomic_replace: false,
            }
        );
        assert_left_empty(root.path());
    }

    #[test]
    fn a_root_of_many_workspaces_is_not_checked_and_nothing_is_created() {
        let root = tempdir().expect("tempdir");
        let filesystem =
            GuardedFilesystem::open(root.path(), 1024, 1024, 100).expect("guarded filesystem");
        assert_eq!(filesystem.prove_guarded_io(), GuardedIoProofs::default());
        assert_left_empty(root.path());
    }
}
