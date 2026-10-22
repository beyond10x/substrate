//! Writable roots (ADR 0038), bound by descriptor: operator-declared host roots (kind 1) and Git
//! checkout workspaces (kind 2).
//!
//! The operator declares a directory by name ([`DeclaredWritableRoot`]); a start names it and a
//! mount. At each start the driver opens the declared path once, `O_PATH | O_DIRECTORY |
//! O_NOFOLLOW | O_CLOEXEC` through `openat2` with `RESOLVE_NO_SYMLINKS`, checks on that descriptor
//! that it is a directory owned by this process's effective uid, and bubblewrap mounts that
//! descriptor writable (`--bind-fd`). A rename or symlink swap after admission therefore cannot
//! redirect the mount, as for read-only roots (design 26 § 3). A root that is gone, a symlink, not
//! a directory or not owned is refused by its name; nothing is widened or silently dropped
//! (invariant 3).
//!
//! A Git root names a workspace. The daemon resolves it in the caller's scope to a root name
//! ([`GitRootBinding`]); the driver pins that directory beneath the workspace-root descriptor
//! exactly as it pins the start's own workspace, `.git` and all, and binds it the same way. Only a
//! workspace with a Git materialization baseline is a Git root: an empty or adopted one is refused
//! by name.

use std::os::fd::{AsRawFd as _, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::Command;

use substrate_wire::{AppliedWritableRoot, WritableRoot};

use crate::{DeclaredWritableRoot, DriverError, GitRootBinding};

const ADDRESS: &str = "writable_roots";

/// A declared writable root that is not present on this host.
pub const WRITABLE_ROOT_ABSENT: &str = substrate_wire::EXEC_WRITABLE_ROOT_ABSENT;
/// A declared writable root that is not a directory, or is reached through a symlink.
pub const WRITABLE_ROOT_NOT_A_DIRECTORY: &str = substrate_wire::EXEC_WRITABLE_ROOT_NOT_A_DIRECTORY;
/// A declared writable root that this process's effective uid does not own.
pub const WRITABLE_ROOT_NOT_OWNED: &str = substrate_wire::EXEC_WRITABLE_ROOT_NOT_OWNED;

/// A root, opened and checked, pinned for the bind.
#[derive(Debug)]
pub(crate) struct PinnedWritableRoot {
    descriptor: OwnedFd,
    applied: AppliedWritableRoot,
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
        self.applied.clone()
    }
}

/// Where a start's writable Git roots are resolved: the workspace-root descriptor its own
/// workspace is pinned beneath, that root's path for the applied record, the Git baseline
/// directory, the start's own root name, and the bindings the daemon admitted.
pub(crate) struct GitRoots<'a> {
    pub(crate) root: &'a OwnedFd,
    pub(crate) root_path: &'a Path,
    pub(crate) baseline_root: &'a Path,
    pub(crate) own_root_name: Option<&'a str>,
    pub(crate) bindings: &'a [GitRootBinding],
}

/// Opens, checks and pins every writable root a start names, in request order. The lexical rules
/// and the facts are the caller's, already applied; a name this host did not declare, or a Git
/// root without an admitted binding, is still refused here by name rather than skipped.
pub(crate) fn pin(
    declared: &[DeclaredWritableRoot],
    requested: &[WritableRoot],
    git: Option<&GitRoots<'_>>,
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
                    applied: AppliedWritableRoot::Host {
                        name: root.host.clone(),
                        host_path,
                        mount: root.mount.clone(),
                    },
                    mount: root.mount.clone(),
                })
            }
            WritableRoot::Git(root) => pin_git(git, &root.workspace, &root.mount),
        })
        .collect()
}

/// Pins the workspace a Git root names, beneath the same workspace-root descriptor the start's own
/// workspace is pinned beneath (`openat2`, beneath, no symlinks), and checks on the host that it was
/// materialized from Git. A root without a binding from the daemon is unserved, never skipped.
fn pin_git(
    git: Option<&GitRoots<'_>>,
    workspace: &str,
    mount: &str,
) -> Result<PinnedWritableRoot, DriverError> {
    let Some(git) = git else {
        return Err(crate::git_root_unserved(workspace));
    };
    let Some(binding) = git
        .bindings
        .iter()
        .find(|binding| binding.workspace == workspace)
    else {
        return Err(crate::git_root_unserved(workspace));
    };
    let root_name = binding.root_name.as_str();
    if root_name.is_empty()
        || !root_name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte == b'.'
        })
        || root_name.starts_with('.')
    {
        return Err(git_absent(workspace));
    }
    if git.own_root_name == Some(root_name) {
        return Err(crate::writable_root_own_workspace(workspace));
    }
    let host_path = git.root_path.join(root_name);
    let opened =
        crate::fs::pin_workspace_directory_beneath(git.root, &host_path).map_err(|error| {
            match error.code {
                "resource.not-found" => git_absent(workspace),
                "workspace.path-escape" | "workspace.not-directory" => {
                    git_not_a_directory(workspace)
                }
                _ => DriverError {
                    address: Some(ADDRESS.to_owned()),
                    ..error
                },
            }
        })?;
    if !crate::git::has_baseline(git.baseline_root, root_name)? {
        return Err(crate::writable_root_not_git(workspace));
    }
    let descriptor = crate::process::above_secret_slots(&opened).ok_or_else(|| {
        DriverError::failed(
            "exec.spawn-failed",
            "A writable Git root could not be pinned above the secret-slot range.",
        )
    })?;
    Ok(PinnedWritableRoot {
        descriptor,
        applied: AppliedWritableRoot::Git {
            workspace: workspace.to_owned(),
            host_path: host_path
                .to_str()
                .ok_or_else(|| git_absent(workspace))?
                .to_owned(),
            mount: mount.to_owned(),
        },
        mount: mount.to_owned(),
    })
}

fn git_absent(workspace: &str) -> DriverError {
    DriverError::refused(
        WRITABLE_ROOT_ABSENT,
        format!("Writable Git root {workspace} is not present on this host."),
        ADDRESS,
    )
}

fn git_not_a_directory(workspace: &str) -> DriverError {
    DriverError::refused(
        WRITABLE_ROOT_NOT_A_DIRECTORY,
        format!(
            "Writable Git root {workspace} is not a directory, or is reached through a symlink."
        ),
        ADDRESS,
    )
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
        None,
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
    use std::os::fd::OwnedFd;
    use std::path::{Path, PathBuf};

    use super::{
        DeclaredWritableRoot, GitRoots, WRITABLE_ROOT_ABSENT, WRITABLE_ROOT_NOT_A_DIRECTORY,
        WRITABLE_ROOT_NOT_OWNED, pin,
    };
    use crate::GitRootBinding;
    use substrate_wire::{AppliedWritableRoot, WritableGitRoot, WritableHostRoot, WritableRoot};

    fn host(name: &str, mount: &str) -> WritableRoot {
        WritableRoot::Host(WritableHostRoot {
            host: name.to_owned(),
            mount: mount.to_owned(),
        })
    }

    fn git(workspace: &str, mount: &str) -> WritableRoot {
        WritableRoot::Git(WritableGitRoot {
            workspace: workspace.to_owned(),
            mount: mount.to_owned(),
        })
    }

    fn declared(name: &str, path: &std::path::Path) -> DeclaredWritableRoot {
        DeclaredWritableRoot {
            name: name.to_owned(),
            path: path.to_owned(),
        }
    }

    fn binding(workspace: &str, root_name: &str) -> GitRootBinding {
        GitRootBinding {
            workspace: workspace.to_owned(),
            root_name: root_name.to_owned(),
        }
    }

    /// A workspace root holding `ws_own` (the start's), `ws_checkout` and `ws_adopted_name` with
    /// Git baselines, and `ws_empty` without one: what an empty workspace create leaves.
    struct Workspaces {
        _directory: tempfile::TempDir,
        root_path: PathBuf,
        baselines: PathBuf,
        root: OwnedFd,
    }

    impl Workspaces {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root_path = std::fs::canonicalize(directory.path()).unwrap();
            let baselines = root_path.join(".substrate-git-baselines");
            std::fs::create_dir(&baselines).unwrap();
            for name in ["ws_own", "ws_checkout", "ws_empty"] {
                std::fs::create_dir_all(root_path.join(name).join(".git")).unwrap();
            }
            for name in ["ws_own", "ws_checkout"] {
                std::fs::write(
                    baselines.join(name),
                    "0123456789abcdef0123456789abcdef01234567",
                )
                .unwrap();
            }
            let root = OwnedFd::from(std::fs::File::open(&root_path).unwrap());
            Self {
                _directory: directory,
                root_path,
                baselines,
                root,
            }
        }

        fn roots<'a>(&'a self, bindings: &'a [GitRootBinding]) -> GitRoots<'a> {
            GitRoots {
                root: &self.root,
                root_path: &self.root_path,
                baseline_root: &self.baselines,
                own_root_name: Some("ws_own"),
                bindings,
            }
        }

        fn path(&self, name: &str) -> &Path {
            // Leaked for the test's lifetime: a borrowed path keeps the assertions short.
            Box::leak(self.root_path.join(name).into_boxed_path())
        }
    }

    #[test]
    fn a_declared_directory_is_pinned_and_bound_writable_by_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let pinned = pin(
            &[declared("cache", &root)],
            &[host("cache", "/srv/cache")],
            None,
        )
        .unwrap();
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
            let error = pin(
                &[declared("cache", &path)],
                &[host("cache", "/srv/cache")],
                None,
            )
            .expect_err(&path.display().to_string());
            assert_eq!(error.code, code, "{}", path.display());
            assert!(error.message.contains("cache"), "{}", error.message);
            assert_eq!(error.address.as_deref(), Some("writable_roots"));
        }
    }

    #[test]
    fn an_undeclared_name_and_a_git_root_are_unserved_never_skipped() {
        let error = pin(&[], &[host("cache", "/srv/cache")], None).unwrap_err();
        assert_eq!(
            error.code,
            substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED
        );
        assert!(error.message.contains("cache"));
        let error = pin(&[], &[git("ws_checkout", "/srv/checkout")], None).unwrap_err();
        assert_eq!(error.code, substrate_wire::EXEC_WRITABLE_ROOTS_GIT_UNSERVED);
        assert!(error.message.contains("ws_checkout"));
    }

    #[test]
    fn a_git_root_the_daemon_did_not_bind_is_unserved_never_skipped() {
        let workspaces = Workspaces::new();
        let error = pin(
            &[],
            &[git("ws_checkout", "/srv/checkout")],
            Some(&workspaces.roots(&[binding("ws_other", "ws_other")])),
        )
        .unwrap_err();
        assert_eq!(error.code, substrate_wire::EXEC_WRITABLE_ROOTS_GIT_UNSERVED);
        assert!(error.message.contains("ws_checkout"), "{}", error.message);
    }

    #[test]
    fn a_git_workspace_is_pinned_beneath_the_root_and_bound_writable_with_its_git_directory() {
        let workspaces = Workspaces::new();
        let bindings = [binding("ws_checkout", "ws_checkout")];
        let pinned = pin(
            &[],
            &[git("ws_checkout", "/srv/checkout")],
            Some(&workspaces.roots(&bindings)),
        )
        .unwrap();
        assert_eq!(pinned.len(), 1);
        let [flag, fd, mount] = pinned[0].bind_args();
        assert_eq!(flag, "--bind-fd");
        assert!(fd.parse::<u32>().unwrap() > substrate_wire::MAX_SECRET_SLOT_FD);
        assert_eq!(mount, "/srv/checkout");
        let host_path = workspaces.path("ws_checkout");
        assert_eq!(
            pinned[0].applied(),
            AppliedWritableRoot::Git {
                workspace: "ws_checkout".to_owned(),
                host_path: host_path.to_str().unwrap().to_owned(),
                mount: "/srv/checkout".to_owned(),
            }
        );
        // The descriptor is the workspace directory itself, `.git` beneath it.
        let held = std::fs::read_link(format!("/proc/self/fd/{fd}")).unwrap();
        assert_eq!(held, host_path);
        assert!(held.join(".git").is_dir());
    }

    #[test]
    fn a_git_root_without_a_git_baseline_or_reached_through_a_link_is_refused_by_name() {
        let workspaces = Workspaces::new();
        std::os::unix::fs::symlink(workspaces.path("ws_checkout"), workspaces.path("ws_linked"))
            .unwrap();
        std::fs::write(
            workspaces.baselines.join("ws_linked"),
            "0123456789abcdef0123456789abcdef01234567",
        )
        .unwrap();
        for (workspace, root_name, code) in [
            ("ws_empty", "ws_empty", crate::WRITABLE_ROOT_NOT_GIT),
            ("ws_adopted", "operator_dir", crate::WRITABLE_ROOT_NOT_GIT),
            ("ws_own_alias", "ws_own", crate::WRITABLE_ROOT_OWN_WORKSPACE),
        ] {
            if root_name == "operator_dir" {
                std::fs::create_dir_all(workspaces.path(root_name)).unwrap();
            }
            let bindings = [binding(workspace, root_name)];
            let error = pin(
                &[],
                &[git(workspace, "/srv/checkout")],
                Some(&workspaces.roots(&bindings)),
            )
            .expect_err(workspace);
            assert_eq!(error.code, code, "{workspace}: {}", error.message);
            assert!(error.message.contains(workspace), "{}", error.message);
            assert_eq!(error.address.as_deref(), Some("writable_roots"));
        }
        // A symlinked workspace entry is never followed: refused before any bind, even with a
        // baseline beside it.
        let bindings = [binding("ws_linked", "ws_linked")];
        let error = pin(
            &[],
            &[git("ws_linked", "/srv/checkout")],
            Some(&workspaces.roots(&bindings)),
        )
        .expect_err("a symlinked workspace entry");
        assert_eq!(error.address.as_deref(), Some("writable_roots"));
        assert_ne!(error.code, crate::WRITABLE_ROOT_NOT_GIT);
    }

    #[test]
    fn the_request_only_git_root_checks_name_the_workspace_in_order() {
        let input = |roots: Vec<WritableRoot>| {
            let mut input: substrate_wire::ExecStartInput = serde_json::from_value(
                serde_json::json!({
                    "workspace": "ws_own",
                    "argv": ["/usr/bin/true"],
                    "env": { "allow": [], "set": {} },
                    "sandbox": {
                        "capability_snapshot": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                        "network": "none",
                        "profile": "workspace",
                        "require": true
                    },
                    "limits": {
                        "timeout_ms": 1000,
                        "output_bytes": 1024,
                        "processes": 1,
                        "memory_bytes": 67_108_864,
                        "cpu_millis": 100
                    },
                    "wait": true
                }),
            )
            .unwrap();
            input.writable_roots = roots;
            input
        };
        assert!(crate::check_writable_git_roots(&input(vec![git("ws_a", "/a")])).is_ok());
        let error =
            crate::check_writable_git_roots(&input(vec![git("ws_a", "/a"), git("ws_own", "/b")]))
                .unwrap_err();
        assert_eq!(error.code, crate::WRITABLE_ROOT_OWN_WORKSPACE);
        assert!(error.message.contains("ws_own"));
        let error = crate::check_writable_git_roots(&input(vec![
            git("ws_a", "/a"),
            host("cache", "/c"),
            git("ws_a", "/b"),
        ]))
        .unwrap_err();
        assert_eq!(error.code, crate::WRITABLE_ROOT_WORKSPACE_DUPLICATE);
        assert!(error.message.contains("ws_a"));
        assert_eq!(error.address.as_deref(), Some("writable_roots"));
    }
}
