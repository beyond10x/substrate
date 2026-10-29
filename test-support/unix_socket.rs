//! Unix socket paths for tests, checked against the platform's `sun_path` before anything binds.
//!
//! `bind(2)` takes a path of at most `sun_path` bytes including its NUL: 108 on Linux, 104 on
//! macOS and the BSDs. A test's temporary directory is beneath `TMPDIR`, which may be any length,
//! so a socket placed under it can fail to bind by a few bytes depending on the random name the
//! directory got (`story:test-socket-paths-fit-sun-len`). Every test that binds a Unix socket, or
//! starts a daemon that binds one, takes its path from here:
//!
//! - [`tempdir`] for a directory the test may put anywhere: beneath `TMPDIR` when the socket fits
//!   there, otherwise beneath a short base this module owns;
//! - [`socket_path`] for one socket beside a test's other files: the preferred path when it fits,
//!   otherwise a short owned directory;
//! - [`bind_in`] for a socket that has to be an entry of one particular directory;
//! - [`assert_fits`] where neither can move it.
//!
//! What cannot be made to fit fails naming the path, its length and the limit.
//!
//! This one file is included with `#[path]` by every crate's tests that need it; it is test
//! support, never part of a production path.
#![allow(
    dead_code,
    reason = "each including test binary uses the part it needs and nothing else"
)]

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// The bytes of `sun_path`, the terminating NUL included.
pub const SUN_PATH_BYTES: usize = if cfg!(any(
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

/// `Ok` when `path` and its NUL fit `sun_path`; otherwise the message naming its length and the
/// limit.
///
/// # Errors
///
/// The path is too long to bind.
pub fn check(path: &Path) -> Result<(), String> {
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

/// Fails the test, naming the length and the limit, unless `path` can be bound.
#[track_caller]
pub fn assert_fits(path: &Path) {
    if let Err(message) = check(path) {
        panic!("{message}");
    }
}

/// An owned temporary directory beneath which `relative` fits as a socket path.
///
/// `TMPDIR` first, so a test keeps its usual place whenever that works; then a short base.
#[track_caller]
pub fn tempdir(relative: &str) -> TempDir {
    tempdir_under(&std::env::temp_dir(), relative)
}

/// [`tempdir`] with `tmpdir` standing for `TMPDIR`.
#[track_caller]
pub fn tempdir_under(tmpdir: &Path, relative: &str) -> TempDir {
    let mut refused = Vec::new();
    for base in std::iter::once(tmpdir.to_owned()).chain(short_bases()) {
        // The random name has a fixed length, so the directory as created is what is measured.
        let directory = match tempfile::Builder::new().prefix("s").tempdir_in(&base) {
            Ok(directory) => directory,
            Err(error) => {
                refused.push(format!("{}: {error}", base.display()));
                continue;
            }
        };
        match check(&directory.path().join(relative)) {
            Ok(()) => return directory,
            Err(message) => refused.push(message),
        }
    }
    panic!(
        "no directory holds the Unix socket {relative}: {}",
        refused.join("; ")
    );
}

/// The bases tried after `TMPDIR`: the per-user runtime directory, then `/tmp`.
fn short_bases() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
        && runtime.is_absolute()
        && runtime.is_dir()
    {
        bases.push(runtime);
    }
    bases.push(PathBuf::from("/tmp"));
    bases
}

/// A socket path that fits, and the short directory holding it when the preferred one did not.
#[derive(Debug)]
pub struct SocketPath {
    path: PathBuf,
    _owned: Option<TempDir>,
}

impl SocketPath {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for SocketPath {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for SocketPath {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

/// `preferred/name` when it fits; otherwise `name` in a short directory this value owns until it
/// is dropped.
#[track_caller]
pub fn socket_path(preferred: &Path, name: &str) -> SocketPath {
    let path = preferred.join(name);
    if check(&path).is_ok() {
        return SocketPath { path, _owned: None };
    }
    let owned = tempdir(name);
    SocketPath {
        path: owned.path().join(name),
        _owned: Some(owned),
    }
}

/// Binds a socket that must be the entry `name` of `directory`, which a short directory cannot
/// stand in for.
///
/// When `directory/name` is too long, Linux binds it through `/proc/self/fd/<fd>/name`, where
/// `<fd>` holds `directory` open: the kernel resolves that link to the directory itself, and the
/// entry is created there. Elsewhere it fails naming the length and the limit.
#[track_caller]
pub fn bind_in(directory: &Path, name: &str) -> UnixListener {
    let path = directory.join(name);
    match check(&path) {
        Ok(()) => UnixListener::bind(&path)
            .unwrap_or_else(|error| panic!("bind {}: {error}", path.display())),
        Err(too_long) => bind_beneath(directory, name, &too_long),
    }
}

#[cfg(target_os = "linux")]
#[track_caller]
fn bind_beneath(directory: &Path, name: &str, too_long: &str) -> UnixListener {
    use std::os::fd::AsRawFd as _;

    let held = std::fs::File::open(directory)
        .unwrap_or_else(|error| panic!("{too_long}; open {}: {error}", directory.display()));
    let short = PathBuf::from(format!("/proc/self/fd/{}/{name}", held.as_raw_fd()));
    if let Err(message) = check(&short) {
        panic!("{too_long}; {message}");
    }
    let listener = UnixListener::bind(&short)
        .unwrap_or_else(|error| panic!("{too_long}; bind {}: {error}", short.display()));
    drop(held);
    listener
}

#[cfg(not(target_os = "linux"))]
#[track_caller]
fn bind_beneath(_directory: &Path, _name: &str, too_long: &str) -> UnixListener {
    panic!("{too_long}");
}
