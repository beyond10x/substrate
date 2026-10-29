//! A Unix socket bound in a directory private to this process, on a path `bind(2)` accepts.
//!
//! `bind(2)` takes a path of at most `sun_path` bytes including its NUL: 108 on Linux, 104 on
//! macOS and the BSDs. The confinement-floor probe binds a sentinel socket in a fresh directory,
//! and placing that directory beneath `TMPDIR` alone made the floor depend on how long the
//! operator's `TMPDIR` is: a long one left the socket unbindable, the probe false and every exec
//! fact withheld, for a reason unrelated to the host's capabilities
//! (`story:host-probe-socket-fits-sun-len`).
//!
//! So the path is measured before anything binds. Each candidate base gets a fresh directory,
//! mode `0700` and owned by this process's effective uid, and the first in which `name` fits is
//! used: `TMPDIR`, then `XDG_RUNTIME_DIR`. When none fits, the entry is bound in a `TMPDIR`
//! directory through `/proc/self/fd/<fd>/<name>`, where `<fd>` holds that directory open, so the
//! kernel resolves the short path to the directory itself. When that is unavailable too, the
//! refusal names every candidate's path length and the limit. Dropping the [`PrivateSocket`]
//! closes the socket and removes the directory.
//!
//! The socket's *location* is all this decides. What the probe binds into its sandbox and what it
//! measures there are unchanged: the directory is still the one bind-mounted read-only at the
//! sentinel mount.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// The bytes of `sun_path`, the terminating NUL included.
pub(crate) const SUN_PATH_BYTES: usize = if cfg!(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)) {
    104
} else {
    108
};

/// A bound socket and the private directory holding it; dropping it removes both.
#[derive(Debug)]
pub(crate) struct PrivateSocket {
    // Declared first so the socket closes before its directory is removed.
    _listener: UnixListener,
    directory: TempDir,
}

impl PrivateSocket {
    /// The private directory the socket is an entry of, as `name` beneath it.
    pub(crate) fn directory(&self) -> &Path {
        self.directory.path()
    }
}

/// Binds `name` in a fresh private directory beneath the first candidate base it fits in.
///
/// # Errors
///
/// No candidate holds a private directory in which `name` can be bound; the message names each
/// candidate's path length and the platform limit.
pub(crate) fn bind_private(name: &str) -> Result<PrivateSocket, String> {
    bind_private_among(&candidates(), name, true)
}

/// `TMPDIR` first, so the socket stays where the operator put temporary files whenever it fits;
/// then the per-user runtime directory, which is short and private by specification.
fn candidates() -> Vec<PathBuf> {
    let mut bases = vec![std::env::temp_dir()];
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
        && runtime.is_absolute()
        && runtime.is_dir()
    {
        bases.push(runtime);
    }
    bases
}

/// `Ok` when `path` and its NUL fit `sun_path`; otherwise the message naming its length and the
/// limit.
fn check(path: &Path) -> Result<(), String> {
    let length = path.as_os_str().len();
    if length < SUN_PATH_BYTES {
        Ok(())
    } else {
        Err(format!(
            "Unix socket path {} is {length} bytes; with its NUL it exceeds the \
             {SUN_PATH_BYTES}-byte platform limit",
            path.display()
        ))
    }
}

/// A fresh directory beneath `base`, mode `0700` and owned by this process.
fn private_directory(base: &Path) -> Result<TempDir, String> {
    let directory = tempfile::Builder::new()
        .prefix("s")
        .tempdir_in(base)
        .map_err(|error| format!("{}: {error}", base.display()))?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("{}: {error}", directory.path().display()))?;
    let metadata = std::fs::symlink_metadata(directory.path())
        .map_err(|error| format!("{}: {error}", directory.path().display()))?;
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    let owner = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o7777 != 0o700 {
        return Err(format!(
            "{} is not a directory private to uid {owner}",
            directory.path().display()
        ));
    }
    Ok(directory)
}

fn bind_private_among(
    bases: &[PathBuf],
    name: &str,
    descriptor_fallback: bool,
) -> Result<PrivateSocket, String> {
    let mut refused = Vec::new();
    for base in bases {
        // The random name has a fixed length, so the directory as created is what is measured.
        let directory = match private_directory(base) {
            Ok(directory) => directory,
            Err(message) => {
                refused.push(message);
                continue;
            }
        };
        let path = directory.path().join(name);
        if let Err(message) = check(&path) {
            refused.push(message);
            continue;
        }
        match UnixListener::bind(&path) {
            Ok(listener) => {
                return Ok(PrivateSocket {
                    _listener: listener,
                    directory,
                });
            }
            Err(error) => refused.push(format!("bind {}: {error}", path.display())),
        }
    }
    if descriptor_fallback && let Some(base) = bases.first() {
        match bind_beneath(base, name) {
            Ok(socket) => return Ok(socket),
            Err(message) => refused.push(message),
        }
    }
    Err(format!(
        "no private directory holds the Unix socket {name}: {}",
        refused.join("; ")
    ))
}

/// Binds `name` as an entry of a fresh private directory beneath `base` through
/// `/proc/self/fd/<fd>/<name>`, whatever the directory's own path length.
fn bind_beneath(base: &Path, name: &str) -> Result<PrivateSocket, String> {
    use std::os::fd::AsRawFd as _;

    let directory = private_directory(base)?;
    let held = std::fs::File::open(directory.path())
        .map_err(|error| format!("open {}: {error}", directory.path().display()))?;
    let short = PathBuf::from(format!("/proc/self/fd/{}/{name}", held.as_raw_fd()));
    check(&short)?;
    let listener =
        UnixListener::bind(&short).map_err(|error| format!("bind {}: {error}", short.display()))?;
    drop(held);
    if !directory.path().join(name).exists() {
        return Err(format!(
            "bind {} did not create {}",
            short.display(),
            directory.path().join(name).display()
        ));
    }
    Ok(PrivateSocket {
        _listener: listener,
        directory,
    })
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::FileTypeExt as _;

    use super::*;

    /// A directory whose path is exactly `length` bytes, whatever this test's own `TMPDIR` is.
    ///
    /// The scratch root is named through the `/proc/self/fd` link of a descriptor that holds it,
    /// then padded, so the case measures the same lengths under a short `TMPDIR` and a long one.
    struct LongDirectory {
        path: PathBuf,
        /// A short directory beside it, a candidate in which a socket fits.
        short: PathBuf,
        _held: std::fs::File,
        _scratch: TempDir,
    }

    fn long_directory(length: usize) -> LongDirectory {
        let scratch = tempfile::tempdir().expect("a scratch root");
        let held = std::fs::File::open(scratch.path()).expect("hold the scratch root");
        let parent = PathBuf::from(format!("/proc/self/fd/{}", held.as_raw_fd()));
        let path = parent.join("d".repeat(length - parent.as_os_str().len() - 1));
        std::fs::create_dir(&path).expect("create the long directory");
        assert_eq!(path.as_os_str().len(), length);
        let short = parent.join("r");
        std::fs::create_dir(&short).expect("create the short candidate");
        LongDirectory {
            path,
            short,
            _held: held,
            _scratch: scratch,
        }
    }

    /// The story's observation: a `TMPDIR` of 100 bytes leaves no room within `sun_path` for a
    /// random directory and `host.sock`, and the probe's sentinel still binds there, in a
    /// directory private to this process that is gone once the probe drops it.
    #[test]
    fn the_probe_socket_binds_under_a_100_byte_tmpdir() {
        let tmpdir = long_directory(100);
        assert!(
            check(&tmpdir.path.join("s123456").join("host.sock")).is_err(),
            "the case must exercise a TMPDIR the socket does not fit beneath"
        );

        let socket = bind_private_among(std::slice::from_ref(&tmpdir.path), "host.sock", true)
            .expect("the probe socket binds under a 100-byte TMPDIR");
        let directory = socket.directory().to_owned();
        assert_eq!(directory.parent(), Some(tmpdir.path.as_path()));
        let metadata = std::fs::symlink_metadata(&directory).expect("the private directory");
        assert!(metadata.is_dir());
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o700);
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        let entry = std::fs::symlink_metadata(directory.join("host.sock")).expect("the socket");
        assert!(entry.file_type().is_socket());

        drop(socket);
        assert!(
            !directory.exists(),
            "the private directory outlived the probe"
        );
    }

    /// A shorter candidate that fits is preferred to binding through a descriptor.
    #[test]
    fn a_short_candidate_is_preferred_when_tmpdir_does_not_fit() {
        let tmpdir = long_directory(100);

        let socket = bind_private_among(
            &[tmpdir.path.clone(), tmpdir.short.clone()],
            "host.sock",
            true,
        )
        .expect("the probe socket binds in the short candidate");
        assert_eq!(socket.directory().parent(), Some(tmpdir.short.as_path()));
        assert_eq!(
            std::fs::read_dir(&tmpdir.path)
                .expect("the long TMPDIR")
                .count(),
            0,
            "a candidate the socket did not fit in kept a directory"
        );
    }

    /// With no candidate that fits and no descriptor to bind through, the refusal names the path's
    /// length and the platform limit, and leaves nothing behind.
    #[test]
    fn no_candidate_that_fits_names_the_length_and_the_limit() {
        let tmpdir = long_directory(100);

        let error = bind_private_among(std::slice::from_ref(&tmpdir.path), "host.sock", false)
            .expect_err("nothing fits and nothing may stand in");
        assert!(
            error.contains(&format!("{SUN_PATH_BYTES}-byte platform limit")),
            "{error}"
        );
        let measured = format!("{}/s", tmpdir.path.display());
        assert!(error.contains(&measured), "{error}");
        assert!(error.contains(" bytes;"), "{error}");
        assert_eq!(
            std::fs::read_dir(&tmpdir.path)
                .expect("the long TMPDIR")
                .count(),
            0,
            "a refused candidate left a directory behind"
        );
    }
}
