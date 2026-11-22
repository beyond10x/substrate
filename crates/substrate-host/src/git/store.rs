//! The baseline store of a Git workspace (design 21, § *The baseline store*).
//!
//! The driver keeps, for each Git workspace, the objects of its baseline commit in a directory
//! of its own: the commit, every tree it reaches and every file and link object those trees
//! name, as one pack and its index. Both observations read the baseline from that store.
//!
//! The Git library is only ever pointed at a directory the driver wrote: a store, the staging
//! tree of a create, or the copy a store is built from. It is never pointed at a workspace.

use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

use git2::{ObjectType, Oid};
use nix::errno::Errno;
use nix::fcntl::RenameFlags;
use substrate_guarded::fs as guarded;
use substrate_wire::StorageLimit;

use super::{failed, object_store, read_baseline, storage_limit, worktree};
use crate::DriverError;
use crate::git_baselines::{self as baselines, BuildTurn, GitWorkspace};

const WRITE_FAILED: &str = "workspace.git-baseline-write-failed";
const READ_FAILED: &str = "workspace.git-baseline-read-failed";

/// How deep the directories of a baseline may nest (design 21, § *The baseline store*): the
/// path of a directory of the baseline has at most this many components.
pub(super) const MAX_BASELINE_DEPTH: usize = 1024;

/// The bytes a name of the baseline may have: no checkout can make a longer one.
pub(super) const MAX_NAME_BYTES: usize = 255;

/// The bytes a path of the baseline may have: no checkout can make a longer one.
pub(super) const MAX_PATH_BYTES: u64 = 4096;

/// The bytes the paths of a baseline may take together, for each path the inode limit admits.
pub(super) const PATH_BYTES_PER_PATH: u64 = 256;

/// The bytes the header of a tree may state, for each path the inode limit admits: the most
/// one entry with a name of [`MAX_NAME_BYTES`] takes, which is a mode of six digits, a space,
/// the name, a zero byte and an id of twenty bytes.
pub(super) const TREE_BYTES_PER_PATH: u64 = 283;

/// Why a store was not written.
pub(super) enum Unwritten {
    /// An object of the baseline is absent from the source, or is not what its id names.
    Object,
    /// The store, or an object of the baseline, is over the byte bound; or the baseline names
    /// more paths than the inode limit, or its paths together or one of its trees take more
    /// bytes than that limit admits.
    Limit,
    /// The baseline names a path no checkout can make: its directories nest deeper than
    /// [`MAX_BASELINE_DEPTH`], a name is longer than [`MAX_NAME_BYTES`] or a path longer than
    /// [`MAX_PATH_BYTES`].
    Path,
    /// The root name already has a store.
    Exists,
    /// The store could not be written, synced or put into place.
    Write,
}

/// Which place a finished store is put into.
#[derive(Clone, Copy)]
pub(super) enum Place {
    /// The place of a workspace that is being created: a store that stands there is another
    /// workspace's and is not replaced.
    Free,
    /// The place of a workspace whose store is built: a store that stands there does not hold
    /// the workspace's commit, and the finished store replaces it.
    Taken,
}

/// What stands in the place of a workspace's store.
enum Standing {
    /// A store that holds the workspace's commit, opened for reading.
    Held(git2::Repository),
    /// Nothing.
    Absent,
    /// A store that does not hold the workspace's commit.
    Lacking,
}

/// The store of the workspace, opened for reading, when it holds the workspace's baseline
/// commit `commit`. `None` for a workspace that has no store, and for one whose store does not
/// hold `commit`: its store has to be built ([`built`]). Nothing is made here.
///
/// # Errors
///
/// `workspace.git-baseline-read-failed` for a store that cannot be opened: it is not absent,
/// and nothing of it is removed.
pub(super) fn held(
    workspace: &GitWorkspace<'_>,
    commit: Oid,
) -> Result<Option<git2::Repository>, DriverError> {
    let entries = baselines::entries(workspace.baseline_root, workspace.root_name)?;
    match open(&entries.store, commit)? {
        Standing::Held(store) => Ok(Some(store)),
        Standing::Absent | Standing::Lacking => Ok(None),
    }
}

/// Gives the workspace its store, which holds its baseline commit `commit`.
///
/// A workspace that predates the store has none, and gets it here, from a copy of the
/// workspace's object store of at most `max_entries` entries. A store that does not hold
/// `commit` is replaced the same way. Either is a build, one build runs at a time in a daemon,
/// and whoever builds holds the turn.
///
/// # Errors
///
/// `workspace.git-baseline-read-failed` for a store that cannot be opened. Otherwise the
/// refusal that ended the build, which leaves what stood in the store's place before, and no
/// copy.
pub(super) fn built(
    workspace: &GitWorkspace<'_>,
    commit: Oid,
    max_entries: usize,
    _turn: &BuildTurn,
) -> Result<(), DriverError> {
    let entries = baselines::entries(workspace.baseline_root, workspace.root_name)?;
    // Another observation may have built the store while this one waited for its turn.
    if let Standing::Held(_) = open(&entries.store, commit)? {
        return Ok(());
    }
    build(workspace, &entries.store, commit, max_entries)?;
    match open(&entries.store, commit)? {
        Standing::Held(_) => Ok(()),
        Standing::Absent | Standing::Lacking => Err(failed("workspace.git-baseline-missing")),
    }
}

/// What stands at `store`. Only a store that is not there, and one that does not hold `commit`,
/// are answered as such; a store that cannot be opened or read is an error.
fn open(store: &Path, commit: Oid) -> Result<Standing, DriverError> {
    match store.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Standing::Absent);
        }
        Ok(_) | Err(_) => return Err(failed(READ_FAILED)),
    }
    readable(store)?;
    let repository = store
        .to_str()
        .and_then(|store| object_store(store).ok())
        .ok_or_else(|| failed(READ_FAILED))?;
    match repository.find_commit(commit).map(|_| ()) {
        Ok(()) => Ok(Standing::Held(repository)),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(Standing::Lacking),
        Err(_) => Err(failed(READ_FAILED)),
    }
}

/// Opens the store's directory, its pack directory and every file of its pack, as a reader
/// has to. The Git library reads a directory or a file it cannot open as one that holds
/// nothing, which would make such a store look like one that lacks its commit.
fn readable(store: &Path) -> Result<(), DriverError> {
    let opened = || -> std::io::Result<()> {
        std::fs::read_dir(store)?;
        let pack = match std::fs::read_dir(store.join("pack")) {
            Ok(pack) => pack,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in pack {
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(entry?.path())?;
        }
        Ok(())
    };
    opened().map_err(|_| failed(READ_FAILED))
}

/// Builds the store of a workspace, in the three steps of the design: copy the pack files and
/// the loose objects of the workspace's object store beneath its pinned directory, index the
/// copied packs and write the store from the copy, and remove the copy whatever the outcome.
/// What stands in the store's place is replaced only by the finished store.
fn build(
    workspace: &GitWorkspace<'_>,
    store: &Path,
    commit: Oid,
    max_entries: usize,
) -> Result<(), DriverError> {
    let pinned = guarded::pin_workspace_directory_beneath(workspace.root, workspace.path)
        .map_err(|_| failed("workspace.git-repository-failed"))?;
    let objects = worktree::open_directory(&pinned, ".git/objects")?
        .ok_or_else(|| failed("workspace.git-repository-failed"))?;
    let limit = (workspace.build_limit)();
    let copy = tempfile::Builder::new()
        .prefix(".copy-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(workspace.baseline_root)
        .map_err(|_| failed(WRITE_FAILED))?;
    // The object store the Git library is pointed at, and beside it the packs as they were
    // copied, which it is not pointed at.
    let (source, packs) = (copy.path().join("objects"), copy.path().join("packs"));
    let indexed = source.join("pack");
    let built = [&source, &packs, &indexed]
        .into_iter()
        .try_for_each(|directory| std::fs::DirBuilder::new().mode(0o700).create(directory))
        .map_err(|_| failed(WRITE_FAILED))
        .and_then(|()| {
            worktree::copy_store(&objects, &source, &packs, max_entries, limit.max_bytes)
        })
        .and_then(|()| index_packs(&packs, &indexed))
        .and_then(|()| {
            let written = write(
                workspace.baseline_root,
                store,
                &source,
                commit,
                limit,
                Place::Taken,
            );
            written.map_err(|unwritten| match unwritten {
                Unwritten::Object => failed("workspace.git-baseline-missing"),
                Unwritten::Limit => storage_limit(),
                Unwritten::Path => failed("workspace.git-path-unrepresentable"),
                Unwritten::Exists | Unwritten::Write => failed(WRITE_FAILED),
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

/// Indexes each pack in `packs`, the packs as they were copied, into the directory `target`
/// of the copy. The driver reads the pack and hands its bytes to the Git library's indexer,
/// which writes the pack and an index of its own. The indexer names every object by the hash
/// of what it read or resolved, and ends in an error for a pack that is not whole or holds an
/// object it cannot resolve. No index a workspace wrote is read.
///
/// # Errors
///
/// `workspace.git-baseline-missing` for a pack whose objects do not all resolve.
fn index_packs(packs: &Path, target: &Path) -> Result<(), DriverError> {
    let unwritten = || failed(WRITE_FAILED);
    let unresolved = || failed("workspace.git-baseline-missing");
    let mut buffer = vec![0; 64 * 1024];
    for entry in std::fs::read_dir(packs).map_err(|_| unwritten())? {
        let path = entry.map_err(|_| unwritten())?.path();
        let mut pack = std::fs::File::open(&path).map_err(|_| unwritten())?;
        // A pack of a workspace need not hold the history of its commits, so the indexer does
        // not look for the objects its commits and trees refer to.
        let mut indexer =
            git2::Indexer::new(None, target, 0o600, false).map_err(|_| unwritten())?;
        loop {
            let read = match pack.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(unwritten()),
            };
            indexer
                .write_all(&buffer[..read])
                .map_err(|_| unresolved())?;
        }
        indexer.commit().map_err(|_| unresolved())?;
        std::fs::remove_file(&path).map_err(|_| unwritten())?;
    }
    Ok(())
}

/// Writes the store `store` of `commit` from the object store at `source`, a directory the
/// driver owns, and puts it into `place`: one pack and its index, made in a staging directory
/// inside `baseline_root`, synced, and renamed. The baseline is checked first
/// ([`check_baseline`]) against `limit`. Every object is read by its id and checked against
/// it, and none whose header states more than `limit.max_bytes` is read.
///
/// # Errors
///
/// Why nothing was written. What was in place of `store` before is there unchanged then.
pub(super) fn write(
    baseline_root: &Path,
    store: &Path,
    source: &Path,
    commit: Oid,
    limit: StorageLimit,
    place: Place,
) -> Result<(), Unwritten> {
    let repository =
        object_store(source.to_str().ok_or(Unwritten::Write)?).map_err(|_| Unwritten::Object)?;
    check_baseline(&repository, commit, limit)?;
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
    write_pack(&repository, commit, &pack, limit.max_bytes)?;
    seal(staging.path(), &pack, limit.max_bytes)?;
    install(baseline_root, staging, store, place)
}

/// A directory of the baseline the check is in.
struct Directory {
    id: Oid,
    /// The bytes of the name the directory it lies in holds it by; none for the root.
    name: u64,
    /// Its directories the check has not entered yet, each with the bytes of its name: one
    /// for each name it holds a directory by.
    pending: Vec<(Oid, u64)>,
    /// What is known of it so far: its own entries, and what the directories the check has
    /// left hold.
    shape: Shape,
}

/// What a tree of the baseline holds, wherever the baseline names it. Paths are relative to
/// the tree: what they come to at a place follows from the path of that place.
#[derive(Clone, Copy)]
struct Shape {
    /// How many directories nest beneath it.
    beneath: usize,
    /// How many paths it names: each of its entries, and each path of each directory it holds,
    /// once for each name it holds that directory by.
    paths: u64,
    /// The bytes of those paths together.
    path_bytes: u64,
    /// The bytes of the longest of them.
    longest: u64,
}

impl Shape {
    /// Adds a directory this one holds by a name of `name` bytes: each of its paths is here
    /// that name, a separator and the path.
    fn hold(&mut self, name: u64, held: Shape) {
        self.beneath = self.beneath.max(held.beneath + 1);
        self.paths = self.paths.saturating_add(held.paths);
        self.path_bytes = self
            .path_bytes
            .saturating_add(held.path_bytes)
            .saturating_add(held.paths.saturating_mul(name + 1));
        if held.paths > 0 {
            self.longest = self.longest.max(name + 1 + held.longest);
        }
    }

    /// Whether the paths of the tree are within the bounds of `limit`: none longer than
    /// [`MAX_PATH_BYTES`], no more than the inode limit, and together no more than
    /// [`PATH_BYTES_PER_PATH`] for each path that limit admits.
    fn within(&self, limit: StorageLimit) -> Result<(), Unwritten> {
        if self.longest > MAX_PATH_BYTES {
            return Err(Unwritten::Path);
        }
        if self.paths > limit.max_inodes
            || self.path_bytes > PATH_BYTES_PER_PATH.saturating_mul(limit.max_inodes)
        {
            return Err(Unwritten::Limit);
        }
        Ok(())
    }
}

/// Reads the header of the object `id` and nothing more of it: the object is of `kind`, and
/// states no more than `limit.max_bytes`; a tree also no more than [`TREE_BYTES_PER_PATH`] for
/// each path the inode limit admits.
fn stated_within(
    objects: &git2::Odb<'_>,
    id: Oid,
    kind: ObjectType,
    limit: StorageLimit,
) -> Result<(), Unwritten> {
    let (size, found) = objects.read_header(id).map_err(|_| Unwritten::Object)?;
    if found != kind {
        return Err(Unwritten::Object);
    }
    let bound = if kind == ObjectType::Tree {
        limit
            .max_bytes
            .min(TREE_BYTES_PER_PATH.saturating_mul(limit.max_inodes))
    } else {
        limit.max_bytes
    };
    if u64::try_from(size).unwrap_or(u64::MAX) > bound {
        return Err(Unwritten::Limit);
    }
    Ok(())
}

/// What a check that passed read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Checked {
    /// The trees it read: each tree of the baseline once.
    pub(super) trees: usize,
    /// The paths the baseline names.
    pub(super) paths: u64,
    /// The bytes of those paths together.
    pub(super) path_bytes: u64,
    /// The bytes of the longest of them.
    pub(super) longest: u64,
}

/// Checks the baseline of `commit` before anything walks it, and before any of its objects is
/// read whole (design 21, § *The baseline store*).
///
/// - **Object sizes.** The header of every object is read before the object is: the commit,
///   every tree it reaches and every file and link object those trees name. None states more
///   than `limit.max_bytes`, and no tree more than [`TREE_BYTES_PER_PATH`] for each path the
///   inode limit admits. A tree entry that records another repository's commit names no
///   object of the baseline.
/// - **Nesting.** No directory lies deeper than [`MAX_BASELINE_DEPTH`].
/// - **Names and path lengths.** No name is longer than [`MAX_NAME_BYTES`] and no path longer
///   than [`MAX_PATH_BYTES`], and the paths together take at most [`PATH_BYTES_PER_PATH`] for
///   each path the inode limit admits.
/// - **Paths.** The baseline names at most `limit.max_inodes` paths. Every entry of every tree
///   counts once for each place the baseline names it, a directory that holds nothing
///   included.
///
/// The check reads each tree once, however often the baseline names it: what it found in a
/// tree it has left it keeps by the tree's id, relative to the tree. It keeps the directories
/// it is in as a list, never longer than the nesting bound. So its work follows the objects of
/// the baseline, not the paths those objects name.
pub(super) fn check_baseline(
    repository: &git2::Repository,
    commit: Oid,
    limit: StorageLimit,
) -> Result<Checked, Unwritten> {
    let objects = repository.odb().map_err(|_| Unwritten::Object)?;
    let within = |id: Oid, kind: ObjectType| stated_within(&objects, id, kind, limit);
    within(commit, ObjectType::Commit)?;
    let root = repository
        .find_commit(commit)
        .map_err(|_| Unwritten::Object)?
        .tree_id();
    let mut files = HashSet::new();
    let mut trees = 0_usize;
    let mut enter = |id: Oid, name: u64| {
        within(id, ObjectType::Tree)?;
        let tree = repository.find_tree(id).map_err(|_| Unwritten::Object)?;
        trees += 1;
        let mut pending = Vec::new();
        let mut shape = Shape {
            beneath: 0,
            paths: u64::try_from(tree.len()).unwrap_or(u64::MAX),
            path_bytes: 0,
            longest: 0,
        };
        for entry in &tree {
            let bytes = entry.name_bytes().len();
            if bytes > MAX_NAME_BYTES {
                return Err(Unwritten::Path);
            }
            let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
            shape.path_bytes = shape.path_bytes.saturating_add(bytes);
            shape.longest = shape.longest.max(bytes);
            match entry.kind() {
                Some(ObjectType::Tree) => pending.push((entry.id(), bytes)),
                Some(ObjectType::Blob) if files.insert(entry.id()) => {
                    within(entry.id(), ObjectType::Blob)?;
                }
                _ => {}
            }
        }
        Ok(Directory {
            id,
            name,
            pending,
            shape,
        })
    };
    // Every tree the check has left, with what it holds.
    let mut left: HashMap<Oid, Shape> = HashMap::new();
    let mut inside = vec![enter(root, 0)?];
    loop {
        // The path of a directory of the innermost one has this many components.
        let depth = inside.len();
        let Some(innermost) = inside.last_mut() else {
            break;
        };
        let Some((next, name)) = innermost.pending.pop() else {
            let (shape, name) = (innermost.shape, innermost.name);
            // A path that is too long relative to its tree is too long wherever the tree is.
            shape.within(limit)?;
            left.insert(innermost.id, shape);
            inside.pop();
            if let Some(parent) = inside.last_mut() {
                parent.shape.hold(name, shape);
            }
            continue;
        };
        if let Some(&shape) = left.get(&next) {
            if depth + shape.beneath > MAX_BASELINE_DEPTH {
                return Err(Unwritten::Path);
            }
            innermost.shape.hold(name, shape);
        } else {
            if depth > MAX_BASELINE_DEPTH {
                return Err(Unwritten::Path);
            }
            let entered = enter(next, name)?;
            inside.push(entered);
        }
    }
    // The root's tree was left last.
    let shape = left.get(&root).ok_or(Unwritten::Object)?;
    Ok(Checked {
        trees,
        paths: shape.paths,
        path_bytes: shape.path_bytes,
        longest: shape.longest,
    })
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

/// Renames the staged store into `place` and syncs the directory of the stores, which is made
/// here when there is none. A link or a file in place of that directory is not followed. A
/// store that replaces another changes places with it in one step, and the replaced one is
/// removed. When the sync fails, the staged store is not kept: what stood in its place before
/// stands there again.
fn install(
    baseline_root: &Path,
    staging: tempfile::TempDir,
    store: &Path,
    place: Place,
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
    let rename = |flags: RenameFlags| {
        nix::fcntl::renameat2(
            nix::fcntl::AT_FDCWD,
            staging.path(),
            nix::fcntl::AT_FDCWD,
            store,
            flags,
        )
    };
    let replaced = match (rename(RenameFlags::RENAME_NOREPLACE), place) {
        (Ok(()), _) => false,
        (Err(Errno::EEXIST | Errno::ENOTEMPTY), Place::Taken) => {
            rename(RenameFlags::RENAME_EXCHANGE).map_err(|_| Unwritten::Write)?;
            true
        }
        (Err(Errno::EEXIST | Errno::ENOTEMPTY), Place::Free) => return Err(Unwritten::Exists),
        (Err(_), _) => return Err(Unwritten::Write),
    };
    if baselines::sync_directory(&stores, WRITE_FAILED).is_err() {
        // The stores change places again; what is then under the staging name goes with it.
        if !(replaced && rename(RenameFlags::RENAME_EXCHANGE).is_ok()) {
            let _ = baselines::remove_entry(store, WRITE_FAILED);
        }
        return Err(Unwritten::Write);
    }
    // Under the staging name is the replaced store, or nothing.
    baselines::remove_entry(&staging.keep(), WRITE_FAILED).map_err(|_| Unwritten::Write)?;
    Ok(())
}
