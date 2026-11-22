//! Where the driver keeps the baseline of each Git workspace, and how those entries go.
//!
//! Design 21, § *The baseline store*. In the directory that holds all workspaces,
//! `.substrate-git-baselines/<root name>` is a workspace's commit-id file and
//! `.substrate-git-baselines/.objects/<root name>/` its store: the objects of that commit. A
//! root name has no `.`, so no workspace's commit-id file is named `.objects` or like a staging
//! entry.
//!
//! This module is built with and without the `git` feature. A build without it writes neither
//! entry, and removes both wherever a build with it does. When that is, the driver decides: with
//! no Git source configured it neither reads nor removes anything here at start or at destroy
//! (design 23).

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use crate::DriverError;

/// The directory of the stores, beside the commit-id files.
const STORES: &str = ".objects";

/// The byte limit of a Git create that requests no storage limit, and the bound of the copy a
/// store is built from for a workspace under no project quota.
pub(crate) const DEFAULT_BYTE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

/// A Git workspace as an observation names it.
#[cfg_attr(not(feature = "git"), allow(dead_code))] // Read only by the build that serves Git.
pub(crate) struct GitWorkspace<'a> {
    pub(crate) id: &'a str,
    /// The directory that holds all workspaces, as the driver pinned it.
    pub(crate) root: &'a OwnedFd,
    /// The workspace's own directory in it.
    pub(crate) path: &'a Path,
    pub(crate) baseline_root: &'a Path,
    pub(crate) root_name: &'a str,
    /// The byte bound of the copy a store is built from. It is asked only when a store is
    /// built, so an observation of a workspace that has its store asks nothing.
    pub(crate) copy_bound: &'a dyn Fn() -> u64,
}

/// One build of a store runs at a time in a daemon: whoever holds this builds.
static BUILD_TURN: LazyLock<Arc<tokio::sync::Mutex<()>>> = LazyLock::new(Arc::default);

/// The turn to build a store. It ends when this is dropped.
pub(crate) struct BuildTurn(#[allow(dead_code)] tokio::sync::OwnedMutexGuard<()>);

impl BuildTurn {
    /// Waits for the turn. An observation waits here and nowhere else, so while it waits it
    /// holds none of the places the driver keeps for file operations.
    pub(crate) async fn taken() -> Self {
        Self(Arc::clone(&BUILD_TURN).lock_owned().await)
    }

    /// Waits for the turn on a thread that runs no task of the driver.
    #[cfg(all(test, feature = "git"))]
    pub(crate) fn taken_blocking() -> Self {
        Self(Arc::clone(&BUILD_TURN).blocking_lock_owned())
    }
}

/// What a Git observation gave.
#[cfg_attr(not(feature = "git"), allow(dead_code))] // A build that serves no Git builds no store.
pub(crate) enum Observed<T> {
    Answer(T),
    /// The workspace's store has to be built, and the observation held no turn to build it:
    /// it made nothing. It is run again with the turn.
    NeedsBuild,
}

/// The two entries the driver keeps for one root name.
pub(crate) struct Entries {
    pub(crate) commit_id: PathBuf,
    pub(crate) store: PathBuf,
}

/// The entries of `root_name` beneath `baseline_root`.
///
/// # Errors
///
/// A name outside the root-name set, before either path is formed: no caller writes or removes
/// an entry of such a name.
pub(crate) fn entries(baseline_root: &Path, root_name: &str) -> Result<Entries, DriverError> {
    if !substrate_wire::is_root_name(root_name) {
        return Err(DriverError::refused(
            "workspace.path-escape",
            "Workspace root identity is invalid.",
            "workspace",
        ));
    }
    Ok(Entries {
        commit_id: baseline_root.join(root_name),
        store: stores(baseline_root).join(root_name),
    })
}

/// The directory of the stores beneath `baseline_root`.
pub(crate) fn stores(baseline_root: &Path) -> PathBuf {
    baseline_root.join(STORES)
}

/// Whether `root_name` has a commit-id file or a store.
///
/// # Errors
///
/// A name outside the root-name set, and an entry that cannot be observed.
pub(crate) fn present(baseline_root: &Path, root_name: &str) -> Result<bool, DriverError> {
    let entries = entries(baseline_root, root_name)?;
    for entry in [&entries.commit_id, &entries.store] {
        match entry.symlink_metadata() {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(failed("workspace.git-baseline-write-failed")),
        }
    }
    Ok(false)
}

/// Removes the commit-id file and the store of `root_name`. A workspace that has neither leaves
/// nothing to remove.
///
/// # Errors
///
/// A name outside the root-name set, before anything is removed, and an entry that could not
/// be removed.
pub(crate) fn remove(baseline_root: &Path, root_name: &str) -> Result<(), DriverError> {
    const CODE: &str = "workspace.git-baseline-remove-failed";
    let entries = entries(baseline_root, root_name)?;
    match std::fs::remove_file(&entries.commit_id) {
        Ok(()) => sync_directory(baseline_root, CODE)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(failed(CODE)),
    }
    let stores = stores(baseline_root);
    if is_directory(&stores) && remove_entry(&entries.store, CODE)? {
        sync_directory(&stores, CODE)?;
    }
    Ok(())
}

/// The start rule: removes every entry of `baseline_root` that is neither the commit-id file
/// nor the store of a workspace directory that exists in `workspace_root`. A staging directory
/// an interrupted write left is such an entry.
///
/// # Errors
///
/// An entry that could not be listed or removed.
pub(crate) fn reconcile(workspace_root: &Path, baseline_root: &Path) -> Result<(), DriverError> {
    const CODE: &str = "workspace.git-reconcile-failed";
    let stores = stores(baseline_root);
    let live = |name: &std::ffi::OsStr| {
        name.to_str().is_some_and(|name| {
            substrate_wire::is_root_name(name) && workspace_root.join(name).is_dir()
        })
    };
    for name in names(baseline_root, CODE)? {
        if name == STORES && is_directory(&stores) {
            for store in names(&stores, CODE)? {
                if !live(&store) {
                    remove_entry(&stores.join(&store), CODE)?;
                }
            }
            sync_directory(&stores, CODE)?;
        } else if !live(&name) {
            remove_entry(&baseline_root.join(&name), CODE)?;
        }
    }
    sync_directory(baseline_root, CODE)
}

/// Whether `path` is a directory itself, not a link to one.
pub(crate) fn is_directory(path: &Path) -> bool {
    path.symlink_metadata()
        .is_ok_and(|metadata| metadata.is_dir())
}

/// The names in `directory`.
fn names(directory: &Path, code: &'static str) -> Result<Vec<std::ffi::OsString>, DriverError> {
    std::fs::read_dir(directory)
        .and_then(|listing| {
            listing
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect()
        })
        .map_err(|_| failed(code))
}

/// Removes the entry at `path`, a directory with everything in it, without following a link.
/// Answers whether there was one.
pub(crate) fn remove_entry(path: &Path, code: &'static str) -> Result<bool, DriverError> {
    let removed = match path.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) => Err(error),
    };
    match removed {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(failed(code)),
    }
}

pub(crate) fn sync_directory(path: &Path, code: &'static str) -> Result<(), DriverError> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| failed(code))
}

fn failed(code: &'static str) -> DriverError {
    DriverError::failed(code, "Git workspace materialization failed.")
}
