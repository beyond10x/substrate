//! Operator-declared writable host roots (ADR 0038 kind 1), bound by descriptor.
//!
//! The operator declares a directory by name ([`DeclaredWritableRoot`]); a start names it and a
//! mount. At each start the driver opens the declared path once, `O_PATH | O_DIRECTORY |
//! O_NOFOLLOW | O_CLOEXEC` through `openat2` with `RESOLVE_NO_SYMLINKS`, checks on that descriptor
//! that it is a directory owned by this process's effective uid, and bubblewrap mounts that
//! descriptor writable (`--bind-fd`). A rename or symlink swap after admission therefore cannot
//! redirect the mount, as for read-only roots (design 26 § 3). A root that is gone, a symlink, not
//! a directory or not owned is refused by its name; nothing is widened or silently dropped
//! (invariant 3).

use std::os::fd::{AsRawFd as _, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::Command;

use substrate_wire::{AppliedWritableRoot, WritableRoot};

use crate::{DeclaredWritableRoot, DriverError};

const ADDRESS: &str = "writable_roots";

/// A declared writable root that is not present on this host.
pub const WRITABLE_ROOT_ABSENT: &str = "exec.writable-root-absent";
/// A declared writable root that is not a directory, or is reached through a symlink.
pub const WRITABLE_ROOT_NOT_A_DIRECTORY: &str = "exec.writable-root-not-a-directory";
/// A declared writable root that this process's effective uid does not own.
pub const WRITABLE_ROOT_NOT_OWNED: &str = "exec.writable-root-not-owned";

/// A declared root, opened and checked, pinned for the bind.
#[derive(Debug)]
pub(crate) struct PinnedWritableRoot {
    descriptor: OwnedFd,
    name: String,
    host_path: String,
    mount: String,
}

impl PinnedWritableRoot {
    pub(crate) fn raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }

    /// The pinned directory itself, which the mount binds.
    pub(crate) fn descriptor(&self) -> &OwnedFd {
        &self.descriptor
    }

    /// Where the pinned directory is mounted in the sandbox.
    pub(crate) fn mount(&self) -> &str {
        &self.mount
    }

    /// The bubblewrap bind: the pinned directory, writable, never the declared path, which
    /// bubblewrap would resolve again when it mounts.
    pub(crate) fn bind_args(&self) -> [String; 3] {
        [
            "--bind-fd".to_owned(),
            self.descriptor.as_raw_fd().to_string(),
            self.mount.clone(),
        ]
    }

    /// What was bound, for the applied record (ADR 0038 decision 6).
    pub(crate) fn applied(&self) -> AppliedWritableRoot {
        AppliedWritableRoot::Host {
            name: self.name.clone(),
            host_path: self.host_path.clone(),
            mount: self.mount.clone(),
        }
    }
}

/// Opens, checks and pins every writable root a start names, in request order. The lexical rules
/// and the facts are the caller's, already applied; a name this host did not declare, or a Git
/// root, is still refused here by name rather than skipped.
pub(crate) fn pin(
    declared: &[DeclaredWritableRoot],
    requested: &[WritableRoot],
) -> Result<Vec<PinnedWritableRoot>, DriverError> {
    requested
        .iter()
        .map(|root| match root {
            WritableRoot::Host(root) => {
                let Some(declaration) = declared.iter().find(|item| item.name == root.host) else {
                    return Err(unserved_host(&root.host));
                };
                let host_path = declaration
                    .path
                    .to_str()
                    .ok_or_else(|| absent(&root.host))?
                    .to_owned();
                Ok(PinnedWritableRoot {
                    descriptor: pin_one(&root.host, &declaration.path)?,
                    name: root.host.clone(),
                    host_path,
                    mount: root.mount.clone(),
                })
            }
            WritableRoot::Git(root) => Err(DriverError::unserved(
                substrate_wire::EXEC_WRITABLE_ROOTS_GIT_UNSERVED,
                format!(
                    "Writable Git root {} is not served by this host.",
                    root.workspace
                ),
                ADDRESS,
            )),
        })
        .collect()
}

fn pin_one(name: &str, path: &Path) -> Result<OwnedFd, DriverError> {
    let declared = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| absent(name))?;
    let opened = crate::read_only_roots::open(
        &declared,
        libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    )
    .map_err(|error| classify(name, &declared, &error))?;
    // SAFETY: an all-zero `stat` is a valid out-parameter for `fstat`.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: a live descriptor and a writable `stat`.
    if unsafe { libc::fstat(opened.as_raw_fd(), &raw mut status) } != 0 {
        return Err(absent(name));
    }
    if status.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(not_a_directory(name));
    }
    // SAFETY: `geteuid` cannot fail and touches no memory.
    if status.st_uid != unsafe { libc::geteuid() } {
        return Err(DriverError::refused(
            WRITABLE_ROOT_NOT_OWNED,
            format!("Writable host root {name} is not owned by this host's user."),
            ADDRESS,
        ));
    }
    // Above every descriptor a secret slot may be placed at, so placing one can never replace a
    // pinned root.
    crate::process::above_secret_slots(&opened).ok_or_else(|| {
        DriverError::failed(
            "exec.spawn-failed",
            "A writable host root could not be pinned above the secret-slot range.",
        )
    })
}

fn classify(name: &str, path: &std::ffi::CString, error: &std::io::Error) -> DriverError {
    match error.raw_os_error() {
        // A symlink at a component above the last, refused by the kernel during resolution.
        Some(libc::ELOOP) => not_a_directory(name),
        // The last component is a file or a symlink (`O_NOFOLLOW` opens it as itself), or a
        // component above it is not a directory: asked again without `O_DIRECTORY`, only the first
        // opens or meets a symlink.
        Some(libc::ENOTDIR) => match crate::read_only_roots::open(
            path,
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        ) {
            Ok(_) => not_a_directory(name),
            Err(again) if again.raw_os_error() == Some(libc::ELOOP) => not_a_directory(name),
            Err(_) => absent(name),
        },
        Some(libc::ENOSYS) => DriverError::unserved(
            "workspace.openat2-unavailable",
            "The kernel cannot resolve a writable host root without following symlinks.",
            "workspace.openat2-beneath",
        ),
        _ => absent(name),
    }
}

fn unserved_host(name: &str) -> DriverError {
    DriverError::unserved(
        substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED,
        format!("Writable host root {name} is not served by this host."),
        ADDRESS,
    )
}

fn absent(name: &str) -> DriverError {
    DriverError::refused(
        WRITABLE_ROOT_ABSENT,
        format!("Writable host root {name} is not present on this host."),
        ADDRESS,
    )
}

fn not_a_directory(name: &str) -> DriverError {
    DriverError::refused(
        WRITABLE_ROOT_NOT_A_DIRECTORY,
        format!("Writable host root {name} is not a directory, or is reached through a symlink."),
        ADDRESS,
    )
}

/// The sandbox path the probe binds its parent at, read-only, and its root beneath, writable.
const PROBE_PARENT: &str = "/writable-root-probe";

/// Proves the mechanism in a throwaway sandbox before `exec.writable-roots.host` is published
/// (ADR 0038 decision 5): a probe root, pinned by [`pin`]'s own open and owner check, bound with
/// `--bind-fd` beneath its parent bound read-only by descriptor. The child writes the root and is
/// refused a write to the parent and to a sibling, holds no descriptor that resolves to either, and
/// on the host the write is in the root and nothing else changed.
pub(crate) fn mechanism_is_provable(bubblewrap: &Path) -> bool {
    let Ok(directory) = tempfile::tempdir() else {
        return false;
    };
    let Ok(host) = std::fs::canonicalize(directory.path()) else {
        return false;
    };
    let parent = host.join("parent");
    let root = parent.join("root");
    let sibling = parent.join("sibling");
    if std::fs::create_dir_all(&root).is_err() || std::fs::create_dir_all(&sibling).is_err() {
        return false;
    }
    let Ok(parent_fd) = pin_one("probe_parent", &parent) else {
        return false;
    };
    let Ok(pinned) = pin(
        &[DeclaredWritableRoot {
            name: "probe".to_owned(),
            path: root.clone(),
        }],
        &[WritableRoot::Host(substrate_wire::WritableHostRoot {
            host: "probe".to_owned(),
            mount: format!("{PROBE_PARENT}/root"),
        })],
    ) else {
        return false;
    };
    let descriptors: Vec<RawFd> = std::iter::once(parent_fd.as_raw_fd())
        .chain(pinned.iter().map(PinnedWritableRoot::raw_fd))
        .collect();
    let mut command = Command::new(bubblewrap);
    command
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
            "--ro-bind-fd",
        ])
        .arg(parent_fd.as_raw_fd().to_string())
        .arg(PROBE_PARENT)
        .args(pinned.iter().flat_map(PinnedWritableRoot::bind_args))
        .args(["--", "/bin/sh", "-c"])
        .arg(format!(
            "echo yes >{PROBE_PARENT}/root/probe \
             && ! (echo no >{PROBE_PARENT}/escape) 2>/dev/null \
             && ! (echo no >{PROBE_PARENT}/sibling/escape) 2>/dev/null \
             && {}",
            crate::process::no_descriptor_resolves_to(
                &[PROBE_PARENT, &format!("{PROBE_PARENT}/root")],
                "/descriptor-probe"
            )
        ));
    // SAFETY: `pre_exec` runs after the fork and calls only async-signal-safe `fcntl`; the vector is
    // built before the fork and only read after it.
    unsafe {
        command.pre_exec(move || crate::process::inherit(&descriptors));
    }
    let status = command.status();
    drop(pinned);
    drop(parent_fd);
    let entries = |path: &Path| {
        std::fs::read_dir(path).map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect::<std::collections::BTreeSet<_>>()
        })
    };
    status.is_ok_and(|status| status.success())
        && std::fs::read_to_string(root.join("probe")).is_ok_and(|text| text == "yes\n")
        && entries(&parent)
            .is_ok_and(|names| names == ["root", "sibling"].into_iter().map(Into::into).collect())
        && entries(&sibling).is_ok_and(|names| names.is_empty())
        && entries(&root).is_ok_and(|names| names.len() == 1)
}

#[cfg(test)]
mod tests {
    use super::{
        DeclaredWritableRoot, WRITABLE_ROOT_ABSENT, WRITABLE_ROOT_NOT_A_DIRECTORY,
        WRITABLE_ROOT_NOT_OWNED, pin,
    };
    use substrate_wire::{AppliedWritableRoot, WritableGitRoot, WritableHostRoot, WritableRoot};

    fn host(name: &str, mount: &str) -> WritableRoot {
        WritableRoot::Host(WritableHostRoot {
            host: name.to_owned(),
            mount: mount.to_owned(),
        })
    }

    fn declared(name: &str, path: &std::path::Path) -> DeclaredWritableRoot {
        DeclaredWritableRoot {
            name: name.to_owned(),
            path: path.to_owned(),
        }
    }

    #[test]
    fn a_declared_directory_is_pinned_and_bound_writable_by_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let pinned = pin(&[declared("cache", &root)], &[host("cache", "/srv/cache")]).unwrap();
        assert_eq!(pinned.len(), 1);
        let [flag, fd, mount] = pinned[0].bind_args();
        assert_eq!(flag, "--bind-fd");
        assert!(fd.parse::<u32>().unwrap() > substrate_wire::MAX_SECRET_SLOT_FD);
        assert_eq!(mount, "/srv/cache");
        assert_eq!(
            pinned[0].applied(),
            AppliedWritableRoot::Host {
                name: "cache".to_owned(),
                host_path: root.to_str().unwrap().to_owned(),
                mount: "/srv/cache".to_owned(),
            }
        );
    }

    #[test]
    fn a_changed_root_is_refused_by_name_with_its_code() {
        let directory = tempfile::tempdir().unwrap();
        let host_dir = std::fs::canonicalize(directory.path()).unwrap();
        let real = host_dir.join("real");
        std::fs::create_dir_all(real.join("inner")).unwrap();
        std::fs::write(host_dir.join("file"), "x").unwrap();
        std::os::unix::fs::symlink(&real, host_dir.join("linked")).unwrap();
        let mut cases = vec![
            (host_dir.join("absent"), WRITABLE_ROOT_ABSENT),
            (host_dir.join("file"), WRITABLE_ROOT_NOT_A_DIRECTORY),
            (host_dir.join("linked"), WRITABLE_ROOT_NOT_A_DIRECTORY),
            (host_dir.join("linked/inner"), WRITABLE_ROOT_NOT_A_DIRECTORY),
            (host_dir.join("file/beneath"), WRITABLE_ROOT_ABSENT),
        ];
        // SAFETY: `geteuid` cannot fail.
        if unsafe { libc::geteuid() } != 0 {
            cases.push(("/usr".into(), WRITABLE_ROOT_NOT_OWNED));
        }
        for (path, code) in cases {
            let error = pin(&[declared("cache", &path)], &[host("cache", "/srv/cache")])
                .expect_err(&path.display().to_string());
            assert_eq!(error.code, code, "{}", path.display());
            assert!(error.message.contains("cache"), "{}", error.message);
            assert_eq!(error.address.as_deref(), Some("writable_roots"));
        }
    }

    #[test]
    fn an_undeclared_name_and_a_git_root_are_unserved_never_skipped() {
        let error = pin(&[], &[host("cache", "/srv/cache")]).unwrap_err();
        assert_eq!(
            error.code,
            substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED
        );
        assert!(error.message.contains("cache"));
        let error = pin(
            &[],
            &[WritableRoot::Git(WritableGitRoot {
                workspace: "ws_checkout".to_owned(),
                mount: "/srv/checkout".to_owned(),
            })],
        )
        .unwrap_err();
        assert_eq!(error.code, substrate_wire::EXEC_WRITABLE_ROOTS_GIT_UNSERVED);
        assert!(error.message.contains("ws_checkout"));
    }
}
