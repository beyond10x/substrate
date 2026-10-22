//! The working directory a confined start names (design 26 § 1).
//!
//! Three halves. The lexical half is the wire crate's and needs no host: an absolute, normal path
//! inside a mount this start places. The fact half refuses a well-formed directory on a host that
//! has not proved it can apply one. The filesystem half asks, before anything runs, whether the
//! directory exists in the object the placing mount binds, reached component by component beneath
//! that object with no symlink followed; the mounts a start may place never overlap
//! (`validate_read_only_roots`, `validate_read_only_files`), so exactly one of them contains the
//! path. A declared root is asked through the descriptor admission pinned and bubblewrap binds,
//! never through its declared path again. Bubblewrap's own `--chdir` follows symlinks and runs after the dispatch barrier, so it can
//! be neither the symlink check nor the pre-dispatch one; it is the backstop for a directory removed
//! between the check and the start, recognised by its exact diagnostic in [`is_chdir_failure`].

use std::ffi::CString;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::process::Command;

use substrate_wire::{
    EXEC_SCRATCH_MOUNT, EXEC_WORKDIR_INVALID, EXEC_WORKDIR_MISSING, EXEC_WORKDIR_UNSERVED,
    EXEC_WORKSPACE_MOUNT, EXECUTION_CAPSULE_MOUNT, ExecStartInput, exec_start_refusal,
    exec_start_refusal_for, validate_workdir,
};

use crate::DriverError;
use crate::read_only_roots::PinnedRoot;

const ADDRESS: &str = "workdir";

/// The diagnostic bubblewrap writes, alone on standard error, when `--chdir` fails.
const CHDIR_DIAGNOSTIC: &str = "bwrap: Can't chdir to ";

/// The two `strerror` texts a `chdir` to an absent directory or a non-directory produces. Any other
/// reason is not this refusal and stays the program's own exit.
const CHDIR_MISSING_REASONS: [&str; 2] = ["No such file or directory", "Not a directory"];

/// The directory bubblewrap starts the program in: the requested one, or the workspace.
pub(crate) fn chdir_target(input: &ExecStartInput) -> &str {
    input.workdir.as_deref().unwrap_or(EXEC_WORKSPACE_MOUNT)
}

/// The lexical half, then the fact. A start that names no directory needs neither.
pub(crate) fn admit(input: &ExecStartInput, published: Option<bool>) -> Result<(), DriverError> {
    if input.workdir.is_none() {
        return Ok(());
    }
    if let Err(error) = validate_workdir(input) {
        let code = exec_start_refusal_for(&error).map_or(EXEC_WORKDIR_INVALID, |r| r.code);
        return Err(DriverError::refused(code, message(code), ADDRESS));
    }
    if published != Some(true) {
        return Err(DriverError::unserved(
            EXEC_WORKDIR_UNSERVED,
            message(EXEC_WORKDIR_UNSERVED),
            ADDRESS,
        ));
    }
    Ok(())
}

/// Whether the admitted directory exists, as a directory reached without a symlink, in the object
/// its mount binds: beneath the pinned `workspace`, the materialized `capsule`, or the directory
/// admission pinned for a declared root (`roots`). Fresh scratch is empty, so only its mount point
/// exists.
///
/// # Errors
///
/// `exec.workdir-missing` for an absent component, a symlink at any component or a final
/// component that is not a directory; a guarded-I/O failure as it is.
pub(crate) fn check_present(
    input: &ExecStartInput,
    workspace: &OwnedFd,
    capsule: Option<&Path>,
    roots: &[PinnedRoot],
) -> Result<(), DriverError> {
    let Some(workdir) = input.workdir.as_deref() else {
        return Ok(());
    };
    let beneath = |mount: &str| -> Option<String> {
        if workdir == mount {
            return Some(String::new());
        }
        workdir
            .strip_prefix(mount)
            .and_then(|rest| rest.strip_prefix('/'))
            .map(str::to_owned)
    };
    if let Some(rest) = beneath(EXEC_WORKSPACE_MOUNT) {
        return walk(workspace, &rest);
    }
    if let (Some(directory), Some(rest)) = (capsule, beneath(EXECUTION_CAPSULE_MOUNT)) {
        return walk(&open_host_directory(directory)?, &rest);
    }
    if let (Some(_), Some(rest)) = (input.scratch.as_ref(), beneath(EXEC_SCRATCH_MOUNT)) {
        return if rest.is_empty() {
            Ok(())
        } else {
            Err(missing())
        };
    }
    // The directory admission pinned is the one bubblewrap mounts (design 26 § 3), so that is the
    // one asked, never the declared path, which may name another directory by now.
    for root in roots {
        if let Some(rest) = beneath(root.mount()) {
            return walk(root.descriptor(), &rest);
        }
    }
    // `admit` refused a directory no placed mount contains, so this is a start that bypassed it.
    Err(missing())
}

/// Whether `output` is exactly bubblewrap's `--chdir` diagnostic for `workdir` and an absent
/// directory or a non-directory: one line, nothing before or after it.
///
/// Only the confined program could write the same bytes, and only about its own run: it would be
/// reporting that it did not start, which the caller asked for nothing else about.
pub(crate) fn is_chdir_failure(output: &[u8], workdir: &str) -> bool {
    let Ok(text) = std::str::from_utf8(output) else {
        return false;
    };
    let Some(line) = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
    else {
        return false;
    };
    line.strip_prefix(CHDIR_DIAGNOSTIC)
        .and_then(|rest| rest.strip_prefix(workdir))
        .and_then(|rest| rest.strip_prefix(": "))
        .is_some_and(|reason| CHDIR_MISSING_REASONS.contains(&reason))
}

/// The refusal message a design 26 code carries.
fn message(code: &'static str) -> &'static str {
    exec_start_refusal(code).map_or("The working directory is refused.", |r| r.message)
}

fn missing() -> DriverError {
    DriverError::refused(EXEC_WORKDIR_MISSING, message(EXEC_WORKDIR_MISSING), ADDRESS)
}

/// The directory a host path names, followed as bubblewrap follows a bind source. A source that
/// cannot be opened as a directory holds no working directory either.
fn open_host_directory(path: &Path) -> Result<OwnedFd, DriverError> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| missing())?;
    // SAFETY: the C string is terminated and the successful descriptor is immediately owned.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(missing());
    }
    // SAFETY: `open` returned a new descriptor owned by this scope.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Walks `relative` beneath `directory` one component at a time through guarded `openat2`, and
/// refuses every symlink, the final one included, and a final component that is not a directory.
fn walk(directory: &OwnedFd, relative: &str) -> Result<(), DriverError> {
    if relative.is_empty() {
        return Ok(());
    }
    let mut current: Option<OwnedFd> = None;
    for component in relative.split('/') {
        let parent = current.as_ref().unwrap_or(directory);
        let next =
            crate::fs::open_component_beneath(parent, component).map_err(|error| {
                match error.code {
                    "resource.not-found" | "workspace.path-escape" => missing(),
                    _ => error,
                }
            })?;
        // SAFETY: `fstat` fills the zeroed struct for a live descriptor.
        let mut status: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: as above; `next` is live and `status` is a valid out pointer.
        if unsafe { libc::fstat(next.as_raw_fd(), &raw mut status) } != 0 {
            return Err(missing());
        }
        // `O_PATH | O_NOFOLLOW` opens a final symlink itself rather than refusing it.
        if status.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Err(missing());
        }
        current = Some(next);
    }
    Ok(())
}

/// Proves the mechanism in a throwaway sandbox before the fact is published: a program started
/// with `--chdir` in a directory beneath the workspace mount runs there, and a `--chdir` to an
/// absent one ends bubblewrap with exactly the diagnostic [`is_chdir_failure`] recognises and
/// starts nothing.
pub(crate) fn mechanism_is_provable(bubblewrap: &Path) -> bool {
    let Ok(directory) = tempfile::tempdir() else {
        return false;
    };
    if std::fs::create_dir(directory.path().join("sub")).is_err() {
        return false;
    }
    let sandbox = |workdir: &str, argv: &[&str]| {
        Command::new(bubblewrap)
            .env_clear()
            .args(crate::process::USER_NAMESPACE_ARGV)
            .args([
                "--unshare-ipc",
                "--unshare-pid",
                "--unshare-net",
                "--unshare-uts",
                "--new-session",
                "--die-with-parent",
                "--clearenv",
                "--ro-bind",
                "/usr",
                "/usr",
                "--ro-bind-try",
                "/bin",
                "/bin",
                "--ro-bind-try",
                "/lib",
                "/lib",
                "--ro-bind-try",
                "/lib64",
                "/lib64",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--bind",
            ])
            .arg(directory.path())
            .args([EXEC_WORKSPACE_MOUNT, "--chdir", workdir, "--"])
            .args(argv)
            .output()
    };
    let applied = sandbox(
        "/workspace/sub",
        &["/bin/sh", "-c", "test \"$(pwd -P)\" = /workspace/sub"],
    )
    .is_ok_and(|output| output.status.success());
    let refused =
        sandbox("/workspace/absent", &["/bin/sh", "-c", "echo started"]).is_ok_and(|output| {
            output.status.code() == Some(1)
                && output.stdout.is_empty()
                && is_chdir_failure(&output.stderr, "/workspace/absent")
        });
    applied && refused
}

#[cfg(test)]
#[path = "workdir_tests.rs"]
mod tests;
