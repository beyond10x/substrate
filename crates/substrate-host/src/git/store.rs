//! The baseline store of a Git workspace (design 21, § *The baseline store*).
//!
//! The driver keeps, for each Git workspace, the objects of its baseline commit in a directory
//! of its own: the commit, every tree it reaches and every file and link object those trees
//! name, as one pack and its index. Both observations read the baseline from that store.
//!
//! The Git library is only ever pointed at a directory the driver wrote: a store, the staging
//! tree of a create, or the copy a store is built from. It is never pointed at a workspace.

use std::collections::HashSet;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::Path;

use git2::{ObjectType, Oid};
use parking_lot::Mutex;
use substrate_guarded::fs as guarded;

use super::{failed, object_store, read_baseline, storage_limit, worktree};
use crate::DriverError;
use crate::git_baselines::{self as baselines, GitWorkspace};

const WRITE_FAILED: &str = "workspace.git-baseline-write-failed";

/// One build runs at a time in a daemon: an observation that needs one holds this for it.
pub(super) static BUILD_TURN: Mutex<()> = Mutex::new(());

/// Why a store was not written.
pub(super) enum Unwritten {
    /// An object of the baseline is absent from the source, or is not what its id names.
    Object,
    /// The store, or an object of the baseline, is over the byte bound.
    Limit,
    /// The root name already has a store.
    Exists,
    /// The store could not be written, synced or put into place.
    Write,
}

/// The store of the workspace, which holds its baseline commit `commit`, opened for reading.
///
/// A workspace that predates the store has none, and gets it here: one build at a time, from a
/// copy of the workspace's object store of at most `max_entries` entries. A store that does not
/// hold `commit` is replaced the same way.
///
/// # Errors
///
/// The refusal that ended the build; no store and no copy is left then.
pub(super) fn held(
    workspace: &GitWorkspace<'_>,
    commit: Oid,
    max_entries: usize,
) -> Result<git2::Repository, DriverError> {
    let entries = baselines::entries(workspace.baseline_root, workspace.root_name)?;
    if let Some(store) = open(&entries.store, commit) {
        return Ok(store);
    }
    let _turn = BUILD_TURN.lock();
    // Another observation may have built it while this one waited for its turn.
    if let Some(store) = open(&entries.store, commit) {
        return Ok(store);
    }
    build(workspace, &entries.store, commit, max_entries)?;
    open(&entries.store, commit).ok_or_else(|| failed("workspace.git-baseline-missing"))
}

/// The store at `store` when it is a directory that holds `commit`.
fn open(store: &Path, commit: Oid) -> Option<git2::Repository> {
    if !baselines::is_directory(store) {
        return None;
    }
    let repository = object_store(store.to_str()?).ok()?;
    repository.find_commit(commit).ok()?;
    Some(repository)
}

/// Builds the store of a workspace that has none, in the three steps of the design: copy the
/// workspace's object store beneath its pinned directory, write the store from the copy, and
/// remove the copy whatever the outcome.
fn build(
    workspace: &GitWorkspace<'_>,
    store: &Path,
    commit: Oid,
    max_entries: usize,
) -> Result<(), DriverError> {
    // What stands in the store's place does not hold the commit: it is treated as absent.
    baselines::remove_entry(store, WRITE_FAILED)?;
    let pinned = guarded::pin_workspace_directory_beneath(workspace.root, workspace.path)
        .map_err(|_| failed("workspace.git-repository-failed"))?;
    let objects = worktree::open_directory(&pinned, ".git/objects")?
        .ok_or_else(|| failed("workspace.git-repository-failed"))?;
    let bound = (workspace.copy_bound)();
    let copy = tempfile::Builder::new()
        .prefix(".copy-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(workspace.baseline_root)
        .map_err(|_| failed(WRITE_FAILED))?;
    let built = worktree::copy_store(&objects, copy.path(), max_entries, bound).and_then(|()| {
        write(workspace.baseline_root, store, copy.path(), commit, bound).map_err(|unwritten| {
            match unwritten {
                Unwritten::Object => failed("workspace.git-baseline-missing"),
                Unwritten::Limit => storage_limit(),
                Unwritten::Exists | Unwritten::Write => failed(WRITE_FAILED),
            }
        })
    });
    let removed = copy.close();
    built?;
    removed.map_err(|_| failed(WRITE_FAILED))?;
    // The store stays only while the commit-id file names the commit it was built for.
    match read_baseline(workspace.baseline_root, workspace.root_name) {
        Ok(named) if named == commit.to_string() => Ok(()),
        named => {
            let _ = baselines::remove_entry(store, WRITE_FAILED);
            named.and(Err(failed("workspace.git-baseline-missing")))
        }
    }
}

/// Writes the store `store` of `commit` from the object store at `source`, a directory the
/// driver owns, and puts it into place: one pack and its index, made in a staging directory
/// inside `baseline_root`, synced, and renamed. Every object is read by its id and checked
/// against it, and none whose header states more than `max_bytes` is read. A store already at
/// `store` is not replaced.
///
/// # Errors
///
/// Why nothing was written. What was in place of `store` before is there unchanged then.
pub(super) fn write(
    baseline_root: &Path,
    store: &Path,
    source: &Path,
    commit: Oid,
    max_bytes: u64,
) -> Result<(), Unwritten> {
    let repository =
        object_store(source.to_str().ok_or(Unwritten::Write)?).map_err(|_| Unwritten::Object)?;
    check_sizes(&repository, commit, max_bytes)?;
    let staging = tempfile::Builder::new()
        .prefix(".store-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(baseline_root)
        .map_err(|_| Unwritten::Write)?;
    let pack = staging.path().join("pack");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&pack)
        .map_err(|_| Unwritten::Write)?;
    write_pack(&repository, commit, &pack, max_bytes)?;
    seal(staging.path(), &pack, max_bytes)?;
    install(baseline_root, staging, store)
}

/// Checks the header of every object of the baseline before the object is read: the commit,
/// every tree it reaches and every file and link object those trees name. A tree entry that
/// records another repository's commit names no object of the baseline.
fn check_sizes(
    repository: &git2::Repository,
    commit: Oid,
    max_bytes: u64,
) -> Result<(), Unwritten> {
    let objects = repository.odb().map_err(|_| Unwritten::Object)?;
    let within = |id: Oid, kind: ObjectType| {
        let (size, found) = objects.read_header(id).map_err(|_| Unwritten::Object)?;
        if found != kind {
            return Err(Unwritten::Object);
        }
        if u64::try_from(size).unwrap_or(u64::MAX) > max_bytes {
            return Err(Unwritten::Limit);
        }
        Ok(())
    };
    within(commit, ObjectType::Commit)?;
    let root = repository
        .find_commit(commit)
        .map_err(|_| Unwritten::Object)?
        .tree_id();
    let mut seen = HashSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        within(id, ObjectType::Tree)?;
        let tree = repository.find_tree(id).map_err(|_| Unwritten::Object)?;
        for entry in &tree {
            match entry.kind() {
                Some(ObjectType::Tree) => pending.push(entry.id()),
                Some(ObjectType::Blob) if seen.insert(entry.id()) => {
                    within(entry.id(), ObjectType::Blob)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Writes the pack of `commit` and its index into the directory `pack`. The pack builder reads
/// each object by its id and checks it against that id; a pack over `max_bytes` is not finished.
fn write_pack(
    repository: &git2::Repository,
    commit: Oid,
    pack: &Path,
    max_bytes: u64,
) -> Result<(), Unwritten> {
    let mut builder = repository.packbuilder().map_err(|_| Unwritten::Write)?;
    builder
        .insert_commit(commit)
        .map_err(|_| Unwritten::Object)?;
    // The pack holds no history, so it is not checked for the objects its commit refers to.
    let mut indexer = git2::Indexer::new(None, pack, 0o400, false).map_err(|_| Unwritten::Write)?;
    let mut written = 0_u64;
    let mut stopped = None;
    let packed = builder.foreach(|bytes| {
        written = written.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if written > max_bytes {
            stopped = Some(Unwritten::Limit);
        } else if indexer.write_all(bytes).is_err() {
            stopped = Some(Unwritten::Write);
        }
        stopped.is_none()
    });
    if let Some(stopped) = stopped {
        return Err(stopped);
    }
    packed.map_err(|_| Unwritten::Object)?;
    indexer.commit().map_err(|_| Unwritten::Write)?;
    Ok(())
}

/// Makes the files of the staged store read-only for the daemon's user, its directories that
/// user's alone, and syncs them all. A store of more than `max_bytes` is refused.
fn seal(staging: &Path, pack: &Path, max_bytes: u64) -> Result<(), Unwritten> {
    let sealed = || -> std::io::Result<u64> {
        let mut bytes = 0_u64;
        for entry in std::fs::read_dir(pack)? {
            let file = std::fs::File::open(entry?.path())?;
            bytes = bytes.saturating_add(file.metadata()?.len());
            file.set_permissions(std::fs::Permissions::from_mode(0o400))?;
            file.sync_all()?;
        }
        for directory in [pack, staging] {
            let directory = std::fs::File::open(directory)?;
            directory.set_permissions(std::fs::Permissions::from_mode(0o700))?;
            directory.sync_all()?;
        }
        Ok(bytes)
    };
    match sealed() {
        Ok(bytes) if bytes > max_bytes => Err(Unwritten::Limit),
        Ok(_) => Ok(()),
        Err(_) => Err(Unwritten::Write),
    }
}

/// Renames the staged store into place and syncs the directory of the stores, which is made
/// here when there is none. A link or a file in place of that directory is not followed.
fn install(
    baseline_root: &Path,
    staging: tempfile::TempDir,
    store: &Path,
) -> Result<(), Unwritten> {
    let stores = baselines::stores(baseline_root);
    match std::fs::DirBuilder::new().mode(0o700).create(&stores) {
        Ok(()) => {
            baselines::sync_directory(baseline_root, WRITE_FAILED).map_err(|_| Unwritten::Write)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Unwritten::Write),
    }
    if !baselines::is_directory(&stores) {
        return Err(Unwritten::Write);
    }
    match nix::fcntl::renameat2(
        nix::fcntl::AT_FDCWD,
        staging.path(),
        nix::fcntl::AT_FDCWD,
        store,
        nix::fcntl::RenameFlags::RENAME_NOREPLACE,
    ) {
        Ok(()) => {}
        Err(nix::errno::Errno::EEXIST | nix::errno::Errno::ENOTEMPTY) => {
            return Err(Unwritten::Exists);
        }
        Err(_) => return Err(Unwritten::Write),
    }
    // The staging name is gone with the rename: its handle has nothing left to remove.
    let _ = staging.keep();
    baselines::sync_directory(&stores, WRITE_FAILED).map_err(|_| Unwritten::Write)
}
