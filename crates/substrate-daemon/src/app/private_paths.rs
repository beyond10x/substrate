//! The host paths this daemon keeps private, which the source of a read-only root (ADR 0010) or
//! a read-only file (ADR 0033) must not meet (their amendments of 2026-11-22).
//!
//! The set is a module of its own so that its one field is out of reach of the composition root:
//! `serve` can obtain a set only from [`PrivateHostPaths::of`], and an [`App`](super::App) cannot
//! be built without one.

use std::path::{Component, Path, PathBuf};

use anyhow::Context as _;

use crate::runtime::{DaemonConfig, DriverSelection, resolved_prefix, state_directory};

/// How many links of an entry that does not resolve are followed by hand at one admission.
const MAX_LINK_HOPS: usize = 8;

/// The read set: the host paths this daemon keeps private.
///
/// - the state directory, the parent of the durable state database: the ledger and what is keyed
///   beside it (the aperture and host-process state roots and the instance lock);
/// - the workspace root: every subject's workspaces and the directories the host driver keeps
///   under it;
/// - the production TLS private key, the static-bearer file, every secret-slot source, the
///   namespace driver's kubeconfig, the API socket (a read-only bind does not stop a connect)
///   and the delegated cgroup root.
///
/// Narrower than what a writable root is kept clear of (`protected_host_paths`): the daemon
/// executable, the bubblewrap binary, the TLS certificate chain and the CA bundles are protected
/// against replacement, not against reading, and a toolchain root such as `/usr` may hold them.
///
/// There is no empty set to take by default. A daemon's set is [`Self::of`] its configuration,
/// and the constructor the composition root uses takes it as an argument.
#[derive(Debug, Clone)]
pub struct PrivateHostPaths {
    /// Each entry as the configuration writes it, made absolute.
    entries: Vec<PathBuf>,
}

impl PrivateHostPaths {
    /// The read set of `config`. A path the configuration leaves empty names nothing: the
    /// workspace root of a daemon that opens the Kubernetes driver.
    ///
    /// # Errors
    ///
    /// Returns an error when the state path has no parent directory, or a relative entry cannot
    /// be made absolute.
    pub fn of(config: &DaemonConfig) -> anyhow::Result<Self> {
        let mut written = vec![
            state_directory(&config.state)?.to_path_buf(),
            config.workspaces.clone(),
            config.socket.clone(),
        ];
        written.extend(config.tls.iter().map(|tls| tls.private_key.clone()));
        written.extend(config.tcp.iter().map(|tcp| tcp.bearer_file.clone()));
        written.extend(config.secret_slots.iter().map(|slot| slot.path.clone()));
        if let DriverSelection::Kubernetes { config: kubeconfig } = &config.driver {
            written.push(kubeconfig.clone());
        }
        written.extend(config.cgroup_root.clone());
        let entries = written
            .iter()
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| {
                std::path::absolute(path)
                    .with_context(|| format!("make {} absolute", path.display()))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self { entries })
    }

    /// The set of an application built without a daemon configuration: no path. Only the harness
    /// constructors of [`App`](super::App) call it (`App::new`, `App::with_authority`), which is
    /// how the integration tests of this crate have always built an application; it is not
    /// visible outside `crate::app`, so the composition root cannot.
    pub(super) fn of_no_daemon_configuration() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The set as one start compares it: every entry as written, and at every place it names at
    /// this moment.
    ///
    /// Resolved at each admission, never once at startup: an operator may replace a secret-slot
    /// source without a restart (ADR 0012). This reads the filesystem, so the caller runs it off
    /// the async worker, once per start. An entry that cannot be resolved at this moment is
    /// compared as written only; the admission does not fail for it.
    pub(crate) fn resolve(&self) -> ResolvedPrivatePaths {
        let mut places = Vec::new();
        for written in &self.entries {
            places.push(lexical(written));
            places.extend(named_places(written));
        }
        ResolvedPrivatePaths { places }
    }

    /// The set with every entry as written and nothing read from the filesystem: what a start
    /// that names no read-only source is given, and never compares.
    pub(crate) fn as_written(&self) -> ResolvedPrivatePaths {
        ResolvedPrivatePaths {
            places: self
                .entries
                .iter()
                .map(|written| lexical(written.as_path()))
                .collect(),
        }
    }
}

/// [`PrivateHostPaths`] at one admission: the places a read-only source is compared with.
///
/// Compared by path component, never by string prefix. The caller's path is compared as text:
/// the wire admits only an absolute canonical host path, and the driver pins it without
/// following a symlink in any component, so a path that binds is its own place.
#[derive(Debug)]
pub(crate) struct ResolvedPrivatePaths {
    places: Vec<PathBuf>,
}

impl ResolvedPrivatePaths {
    /// Whether the host path of any declared read-only root equals, lies under or contains an
    /// entry of the set.
    pub(crate) fn meets_a_root<'a>(&self, host_paths: impl IntoIterator<Item = &'a str>) -> bool {
        host_paths.into_iter().map(Path::new).any(|source| {
            self.places
                .iter()
                .any(|private| source.starts_with(private) || private.starts_with(source))
        })
    }

    /// Whether the host path of any declared read-only file equals or lies under an entry of the
    /// set. A regular file contains nothing, so that direction is not asked.
    pub(crate) fn meets_a_file<'a>(&self, host_paths: impl IntoIterator<Item = &'a str>) -> bool {
        host_paths.into_iter().map(Path::new).any(|source| {
            self.places
                .iter()
                .any(|private| source.starts_with(private))
        })
    }
}

/// `path` without its `.` and `..` components, by text alone: the written form of an entry. A
/// directory the configured path only passes through with `..` is then not taken for one that
/// holds the entry. Where a component before a `..` is a link, the text is not the place; the
/// resolved forms of [`named_places`] are what protects it.
fn lexical(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    normal
}

/// Every place `written` names at this moment.
///
/// One place when the whole path resolves. Otherwise its existing prefix resolved with the rest
/// appended ([`resolved_prefix`]), and, when what stops the resolution is a link whose target
/// does not exist yet, also the place that link names, followed by hand for at most
/// [`MAX_LINK_HOPS`] links: a source that is a link to a file not written yet keeps the place
/// it will be written to private. Nothing when the path cannot be resolved at all.
fn named_places(written: &Path) -> Vec<PathBuf> {
    let mut places = Vec::new();
    let mut next = written.to_path_buf();
    for _ in 0..=MAX_LINK_HOPS {
        if let Ok(real) = std::fs::canonicalize(&next) {
            places.push(real);
            break;
        }
        let Ok(place) = resolved_prefix(&next) else {
            break;
        };
        let beyond = beyond_a_dangling_link(&place);
        places.push(place);
        let Some(beyond) = beyond else {
            break;
        };
        next = beyond;
    }
    places
}

/// Where `place` leads when its first component that is a link is replaced by that link's
/// target text; `None` when no component that exists is a link.
fn beyond_a_dangling_link(place: &Path) -> Option<PathBuf> {
    let prefixes: Vec<&Path> = place.ancestors().collect();
    // From the root down: the first prefix that is a link is the one that stopped the resolution.
    for prefix in prefixes.into_iter().rev() {
        let metadata = std::fs::symlink_metadata(prefix).ok()?;
        if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(prefix).ok()?;
            let rest = place.strip_prefix(prefix).ok()?;
            let mut beyond = prefix.parent()?.join(target);
            if !rest.as_os_str().is_empty() {
                beyond.push(rest);
            }
            return Some(beyond);
        }
    }
    None
}
