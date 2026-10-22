//! Declared read-only host roots (ADR 0010), bound by descriptor (design 26 § 3).
//!
//! Admission opens every root once, `O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` through
//! `openat2` with `RESOLVE_NO_SYMLINKS`, so the kernel refuses a symlink at any component and a
//! root that is not a directory. Bubblewrap mounts that descriptor (`--ro-bind-fd`), so the
//! directory that is mounted is the object admission inspected, whatever is renamed or swapped for
//! a symlink at the declared path afterwards, and it closes the descriptor before the program
//! starts. The workspace (ADR 0023) and single files (ADR 0033) are bound the same way.

use std::ffi::CString;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};

use substrate_wire::ReadOnlyRoot;

use crate::DriverError;

const ADDRESS: &str = "read_only_roots";
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

/// A declared root, opened and checked, pinned for the bind.
#[derive(Debug)]
pub(crate) struct PinnedRoot {
    descriptor: OwnedFd,
    mount: String,
}

impl PinnedRoot {
    pub(crate) fn raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }

    /// The bubblewrap bind: the pinned directory, never the declared path, which bubblewrap would
    /// resolve again when it mounts.
    pub(crate) fn bind_args(&self) -> [String; 3] {
        [
            "--ro-bind-fd".to_owned(),
            self.descriptor.as_raw_fd().to_string(),
            self.mount.clone(),
        ]
    }
}

/// Opens, checks and pins every declared root, in declaration order. The lexical rules are the
/// caller's, already applied.
pub(crate) fn pin(roots: &[ReadOnlyRoot]) -> Result<Vec<PinnedRoot>, DriverError> {
    roots
        .iter()
        .map(|root| {
            Ok(PinnedRoot {
                descriptor: pin_one(&root.host_path)?,
                mount: root.mount.clone(),
            })
        })
        .collect()
}

fn pin_one(host_path: &str) -> Result<OwnedFd, DriverError> {
    let path = CString::new(host_path).map_err(|_| absent())?;
    let opened = open(
        &path,
        libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    )
    .map_err(|error| classify(&path, &error))?;
    // SAFETY: an all-zero `stat` is a valid out-parameter for `fstat`.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: a live descriptor and a writable `stat`.
    if unsafe { libc::fstat(opened.as_raw_fd(), &raw mut status) } != 0 {
        return Err(absent());
    }
    if status.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(not_a_directory());
    }
    // Above every descriptor a secret slot may be placed at, so placing one can never replace a
    // pinned root.
    crate::process::above_secret_slots(&opened).ok_or_else(|| {
        DriverError::failed(
            "exec.spawn-failed",
            "A declared read-only root could not be pinned above the secret-slot range.",
        )
    })
}

fn open(path: &CString, flags: libc::c_int) -> std::io::Result<OwnedFd> {
    let how = OpenHow {
        flags: u64::try_from(flags).expect("open flags are non-negative"),
        mode: 0,
        resolve: RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
    };
    // SAFETY: the arguments point to an initialized `OpenHow` and a NUL-terminated path.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            path.as_ptr(),
            &raw const how,
            size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `openat2` returned a new descriptor owned by this scope.
    Ok(unsafe { OwnedFd::from_raw_fd(i32::try_from(fd).expect("descriptor fits i32")) })
}

fn classify(path: &CString, error: &std::io::Error) -> DriverError {
    match error.raw_os_error() {
        // A symlink at a component above the last, refused by the kernel during resolution.
        Some(libc::ELOOP) => not_a_directory(),
        // The last component is not a directory (a file, or a symlink, which `O_NOFOLLOW` opens
        // as itself), or a component above it is not one, so the root is not there. Asked again
        // without `O_DIRECTORY`, only the first opens or meets a symlink.
        Some(libc::ENOTDIR) => {
            match open(path, libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC) {
                Ok(_) => not_a_directory(),
                Err(again) if again.raw_os_error() == Some(libc::ELOOP) => not_a_directory(),
                Err(_) => absent(),
            }
        }
        // The kernel the workspace pin needs too; this host serves no confined exec without it.
        Some(libc::ENOSYS) => DriverError::unserved(
            "workspace.openat2-unavailable",
            "The kernel cannot resolve a declared read-only root without following symlinks.",
            "workspace.openat2-beneath",
        ),
        // A directory on the way the daemon may not search: the daemon cannot observe the root,
        // so for this host it is not present, said apart in the message, as for a single file.
        Some(libc::EACCES) => DriverError::refused(
            "exec.read-only-root-absent",
            "A declared read-only root is not reachable: a directory on its path is not \
             searchable by this host.",
            ADDRESS,
        ),
        // ENOENT, and ENAMETOOLONG: the wire crate bounds the count of roots, not their length,
        // and a path the kernel will not resolve names nothing present on this host.
        _ => absent(),
    }
}

fn absent() -> DriverError {
    DriverError::refused(
        "exec.read-only-root-absent",
        "A declared read-only root is not present on this host.",
        ADDRESS,
    )
}

fn not_a_directory() -> DriverError {
    DriverError::refused(
        "exec.read-only-root-not-a-directory",
        "A declared read-only root is not a directory, or is reached through a symlink.",
        ADDRESS,
    )
}
