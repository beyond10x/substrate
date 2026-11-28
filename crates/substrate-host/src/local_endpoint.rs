//! The operator's local endpoint declaration (ADR 0054): an aperture whose destination is a
//! directory of Unix sockets on the host.
//!
//! The directory is opened once, when the declaration is made, and held by its descriptor for the
//! life of the process. Every relay reaches a socket in it through that descriptor
//! (`/proc/self/fd/<n>/<socket>`), so a path swapped after startup changes nothing a run can
//! reach, and the published record never names a host path.

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::os::fd::{AsFd as _, AsRawFd as _, BorrowedFd, OwnedFd};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use substrate_wire::EgressApertureFact;

use crate::DriverError;

/// The code a declaration the driver cannot use is refused with.
pub const LOCAL_ENDPOINT_INVALID: &str = "config.local-endpoint-invalid";

/// The mode bits a declared directory may not carry: write for its group or for others, which
/// would let another user place a socket a run then reaches (ADR 0054 decision 2).
const SHARED_WRITE: u32 = 0o022;

/// One operator-declared local endpoint: its aperture name, the port its listener takes inside a
/// run, and the directory of sockets, held by descriptor.
#[derive(Debug, Clone)]
pub struct LocalEndpoint {
    pub name: String,
    pub port: u16,
    directory: Arc<OwnedFd>,
}

impl LocalEndpoint {
    /// Opens `directory` once and holds it for the life of the value (ADR 0054 decision 2).
    ///
    /// # Errors
    ///
    /// `config.local-endpoint-invalid` for a name that is not an aperture name, a port of zero,
    /// a path that is not absolute, an entry that is not a directory or is a symbolic link, and a
    /// directory not owned by this process's effective uid or writable by its group or by others.
    /// The message names the endpoint and the rule, never the path.
    pub fn open(name: &str, port: u16, directory: &Path) -> Result<Self, DriverError> {
        if !substrate_wire::valid_aperture_name(name) {
            return Err(invalid(format!(
                "A local endpoint name must match [a-z][a-z0-9_]{{0,63}}; {name:?} does not."
            )));
        }
        if port == 0 {
            return Err(invalid(format!("Local endpoint {name} declares no port.")));
        }
        if !directory.is_absolute() {
            return Err(invalid(format!(
                "Local endpoint {name} names its directory by a path that is not absolute."
            )));
        }
        let opened = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory)
            .map_err(|error| {
                invalid(format!(
                    "Local endpoint {name}: the directory cannot be opened as a directory that is \
                     not a symbolic link ({error})."
                ))
            })?;
        let metadata = opened.metadata().map_err(|error| {
            invalid(format!(
                "Local endpoint {name}: the directory cannot be read ({error})."
            ))
        })?;
        Self::check(name, metadata.uid(), metadata.mode(), metadata.is_dir())?;
        Ok(Self {
            name: name.to_owned(),
            port,
            directory: Arc::new(OwnedFd::from(opened)),
        })
    }

    /// The ownership rule of ADR 0054 decision 2, on what the opened descriptor reports.
    fn check(name: &str, owner: u32, mode: u32, is_directory: bool) -> Result<(), DriverError> {
        if !is_directory {
            return Err(invalid(format!(
                "Local endpoint {name}: the declared entry is not a directory."
            )));
        }
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        let effective = unsafe { libc::geteuid() };
        if owner != effective {
            return Err(invalid(format!(
                "Local endpoint {name}: the directory is not owned by this daemon's user."
            )));
        }
        if mode & SHARED_WRITE != 0 {
            return Err(invalid(format!(
                "Local endpoint {name}: the directory is writable by its group or by others."
            )));
        }
        Ok(())
    }

    /// The directory, for a relay that reaches a socket in it.
    pub(crate) fn directory(&self) -> BorrowedFd<'_> {
        self.directory.as_fd()
    }

    /// The descriptor number, which every process forked from this one shares.
    pub(crate) fn directory_number(&self) -> i32 {
        self.directory.as_raw_fd()
    }

    /// The capability fact: its name and `local:<port>`, never the directory (ADR 0054
    /// decision 7). A local endpoint declares no byte ceiling.
    pub(crate) fn fact(&self) -> EgressApertureFact {
        EgressApertureFact {
            name: self.name.clone(),
            destination: substrate_wire::local_endpoint_destination(self.port),
            max_bytes: None,
        }
    }
}

fn invalid(message: String) -> DriverError {
    DriverError::refused(LOCAL_ENDPOINT_INVALID, message, "config.local_endpoints")
}

/// Which socket of which local endpoint each live run holds (ADR 0054 decision 6).
///
/// One live run holds a socket name per endpoint. A start whose name another live run holds is
/// refused `exec.local-endpoint-key-in-use` before anything is spawned, and the name is free
/// again once that run's apertures are dropped, which happens before its terminal observation is
/// published.
#[derive(Debug, Default, Clone)]
pub(crate) struct Claims {
    held: Arc<Mutex<BTreeSet<(String, String)>>>,
}

/// One held socket name, released when it is dropped.
#[derive(Debug)]
pub(crate) struct Claim {
    held: Arc<Mutex<BTreeSet<(String, String)>>>,
    entry: (String, String),
}

impl Claims {
    /// Claims `socket` on `endpoint` for one run, or `None` when another live run holds it.
    pub(crate) fn claim(&self, endpoint: &str, socket: &str) -> Option<Claim> {
        let entry = (endpoint.to_owned(), socket.to_owned());
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.insert(entry.clone()).then(|| Claim {
            held: Arc::clone(&self.held),
            entry,
        })
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.entry);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::{Claims, LOCAL_ENDPOINT_INVALID, LocalEndpoint};

    fn refusal(name: &str, port: u16, path: &std::path::Path) -> String {
        let error = LocalEndpoint::open(name, port, path).expect_err("refused");
        assert_eq!(error.code, LOCAL_ENDPOINT_INVALID);
        assert!(
            !error.message.contains(&path.display().to_string()),
            "{}",
            error.message
        );
        error.message
    }

    #[test]
    fn a_private_directory_of_this_user_is_declared_and_its_fact_names_no_path() {
        let directory = tempfile::tempdir().expect("directory");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("mode");
        let endpoint = LocalEndpoint::open("agent", 7000, directory.path()).expect("declared");
        let fact = endpoint.fact();
        assert_eq!(fact.name, "agent");
        assert_eq!(fact.destination, "local:7000");
        assert_eq!(fact.max_bytes, None);
        // The descriptor names the directory that was opened, whatever happens to its path.
        let moved = directory.path().with_extension("moved");
        std::fs::rename(directory.path(), &moved).expect("rename");
        std::fs::write(moved.join("marker"), b"").expect("marker");
        let seen = std::fs::read_dir(format!("/proc/self/fd/{}", endpoint.directory_number()))
            .expect("read through the descriptor")
            .count();
        assert_eq!(seen, 1);
        std::fs::remove_dir_all(&moved).expect("cleanup");
    }

    #[test]
    fn a_directory_writable_by_its_group_or_by_others_is_refused() {
        for mode in [0o770, 0o707, 0o777, 0o720] {
            let directory = tempfile::tempdir().expect("directory");
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(mode))
                .expect("mode");
            let message = refusal("agent", 7000, directory.path());
            assert!(
                message.contains("writable by its group or by others"),
                "{mode:o}"
            );
        }
    }

    #[test]
    fn a_directory_of_another_user_is_refused() {
        assert!(LocalEndpoint::check("agent", u32::MAX - 1, 0o40700, true).is_err());
        // SAFETY: `geteuid` has no preconditions.
        let effective = unsafe { libc::geteuid() };
        assert!(LocalEndpoint::check("agent", effective, 0o40700, true).is_ok());
        assert!(LocalEndpoint::check("agent", effective, 0o40700, false).is_err());
    }

    #[test]
    fn a_link_a_file_a_relative_path_a_bad_name_and_port_zero_are_refused() {
        let directory = tempfile::tempdir().expect("directory");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("mode");
        let target = directory.path().join("target");
        std::fs::create_dir(&target).expect("target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).expect("mode");
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        refusal("agent", 7000, &link);
        let file = directory.path().join("file");
        std::fs::write(&file, b"").expect("file");
        refusal("agent", 7000, &file);
        refusal("agent", 7000, std::path::Path::new("relative"));
        refusal("Agent", 7000, &target);
        refusal("agent", 0, &target);
        assert!(LocalEndpoint::open("agent", 7000, &target).is_ok());
    }

    #[test]
    fn one_live_run_holds_a_socket_name_per_endpoint_until_its_claim_is_dropped() {
        let claims = Claims::default();
        let first = claims.claim("agent", "worker-1").expect("free");
        assert!(claims.claim("agent", "worker-1").is_none());
        let other_endpoint = claims.claim("audit", "worker-1").expect("another endpoint");
        let other_name = claims.claim("agent", "worker-2").expect("another name");
        drop(first);
        assert!(claims.claim("agent", "worker-1").is_some());
        drop((other_endpoint, other_name));
    }
}
