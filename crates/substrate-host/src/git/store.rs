//! The baseline store of a Git workspace (design 21, § *The baseline store*).
//!
//! The driver keeps, for each Git workspace, the objects of its baseline commit in a directory
//! of its own: the commit, every tree it reaches and every file and link object those trees
//! name, as one pack and its index. Both observations read the baseline from that store.
//!
//! The Git library is only ever pointed at a directory the driver wrote: a store, or the
//! staging tree of a create. It is never pointed at a workspace.

use std::collections::HashMap;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

use git2::{ObjectType, Oid};
use nix::errno::Errno;
use nix::fcntl::RenameFlags;
use substrate_wire::StorageLimit;

use super::{failed, object_store};
use crate::DriverError;
use crate::git_baselines as baselines;

pub(super) mod chains;

use chains::Chains;

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

/// The bytes a tree may state for each of its entries, and its header for each path the inode
/// limit admits: the most one entry with a name of [`MAX_NAME_BYTES`] takes, which is a mode
/// of six digits, a space, the name, a zero byte and an id of twenty bytes.
pub(super) const TREE_BYTES_PER_PATH: u64 = 283;

/// The entries the objects of a baseline may be stored behind together, for each path the
/// inode limit admits: each commit, tree and file object the check reads the header of counts
/// once, with the entries its pack stores it behind ([`chains`]). A pack of the depth Git
/// writes by default, 50, is within it with one object for every path the limit admits.
pub(super) const CHAIN_STEPS_PER_PATH: u64 = 64;

/// Why a baseline was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unadmitted {
    /// The commit is absent from the object store, is no commit, or is not what its id names.
    Commit,
    /// A tree or a file object of the baseline is absent or is not what the baseline names
    /// it as; or [`Chains::read`] cannot read a pack of the object store or its index.
    Object,
    /// An object of the baseline or its files together are over the byte bound; or the
    /// baseline names more paths than the inode limit, or its paths together or one of its
    /// trees take more bytes than that limit or the tree's own entries admit; or a pack
    /// stores an object behind more than [`chains::MAX_CHAIN`] entries, or the objects of the
    /// baseline together behind more than [`CHAIN_STEPS_PER_PATH`] for each path the inode
    /// limit admits.
    Limit,
    /// The baseline names a path no checkout can make as one entry: its directories nest
    /// deeper than [`MAX_BASELINE_DEPTH`], a name is longer than [`MAX_NAME_BYTES`] or holds a
    /// `/`, or a path is longer than [`MAX_PATH_BYTES`].
    Path,
}

/// Why a store was not written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unwritten {
    /// An object of the baseline is absent from the source, or is not what its id names.
    Object,
    /// The baseline is over a bound of [`Unadmitted::Limit`], or the store is over the byte
    /// bound.
    Limit,
    /// The baseline names a path of [`Unadmitted::Path`].
    Path,
    /// The root name already has a store.
    Exists,
    /// The store could not be written, synced or put into place.
    Write,
}

impl From<Unadmitted> for Unwritten {
    fn from(unadmitted: Unadmitted) -> Self {
        match unadmitted {
            Unadmitted::Commit | Unadmitted::Object => Self::Object,
            Unadmitted::Limit => Self::Limit,
            Unadmitted::Path => Self::Path,
        }
    }
}

/// The store of the workspace `root_name`, opened for reading: it holds the workspace's
/// baseline commit `commit`. Nothing is made, removed or replaced here, and nothing of a
/// workspace is opened.
///
/// # Errors
///
/// `workspace.git-baseline-missing` for a workspace that has no store, for a store that does
/// not hold `commit`, and for one whose pack or index the Git library cannot read as one.
/// `workspace.git-baseline-read-failed` for a store that cannot be opened for any other
/// reason.
pub(super) fn held(
    baseline_root: &Path,
    root_name: &str,
    commit: Oid,
) -> Result<git2::Repository, DriverError> {
    let missing = || failed("workspace.git-baseline-missing");
    let store = baselines::entries(baseline_root, root_name)?.store;
    match store.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(missing()),
        Ok(_) | Err(_) => return Err(failed(READ_FAILED)),
    }
    readable(&store)?;
    let repository = store
        .to_str()
        .and_then(|store| object_store(store).ok())
        .ok_or_else(|| failed(READ_FAILED))?;
    match repository.find_commit(commit).map(|_| ()) {
        Ok(()) => Ok(repository),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Err(missing()),
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

/// Writes the store `store` of `commit` from the object store at `source`, a directory the
/// driver owns: one pack and its index, made in a staging directory inside `baseline_root`,
/// synced, and renamed into place. The baseline is checked first ([`check_baseline_keeping`])
/// against `limit`, so the pack builder reads the headers of the same objects, within the same
/// bounds. Every object is read by its id and checked against it, and none whose header
/// states more than `limit.max_bytes` is read.
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
) -> Result<(), Unwritten> {
    let repository =
        object_store(source.to_str().ok_or(Unwritten::Write)?).map_err(|_| Unwritten::Object)?;
    check_in(&repository, source, commit, limit).0?;
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
    install(baseline_root, staging, store)
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
    /// The bytes its files and links state together: each object once for each name the tree
    /// or a directory beneath it holds it by.
    bytes: u64,
}

impl Shape {
    /// Adds a directory this one holds by a name of `name` bytes: each of its paths is here
    /// that name, a separator and the path.
    fn hold(&mut self, name: u64, held: Shape) {
        self.beneath = self.beneath.max(held.beneath + 1);
        self.bytes = self.bytes.saturating_add(held.bytes);
        self.paths = self.paths.saturating_add(held.paths);
        self.path_bytes = self
            .path_bytes
            .saturating_add(held.path_bytes)
            .saturating_add(held.paths.saturating_mul(name + 1));
        if held.paths > 0 {
            self.longest = self.longest.max(name + 1 + held.longest);
        }
    }

    /// Whether the tree is within the bounds of `limit`: no path longer than
    /// [`MAX_PATH_BYTES`], no more paths than the inode limit, the paths together no more than
    /// [`PATH_BYTES_PER_PATH`] for each path that limit admits, and the files together no more
    /// than the byte limit.
    fn within(&self, limit: StorageLimit) -> Result<(), Unadmitted> {
        if self.longest > MAX_PATH_BYTES {
            return Err(Unadmitted::Path);
        }
        if self.paths > limit.max_inodes
            || self.path_bytes > PATH_BYTES_PER_PATH.saturating_mul(limit.max_inodes)
            || self.bytes > limit.max_bytes
        {
            return Err(Unadmitted::Limit);
        }
        Ok(())
    }
}

/// What a check that passed read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Checked {
    /// The entries its objects are stored behind together: what the Git library walks to read
    /// the header of each of them once.
    pub(super) chain_steps: u64,
    /// The trees it read: each tree of the baseline once.
    pub(super) trees: usize,
    /// The paths the baseline names.
    pub(super) paths: u64,
    /// The bytes of those paths together.
    pub(super) path_bytes: u64,
    /// The bytes of the longest of them.
    pub(super) longest: u64,
    /// The bytes the files and links of the baseline state together, each once for each place
    /// the baseline names it.
    pub(super) bytes: u64,
}

/// What a check had read and kept when it ended, whether it passed or refused.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Kept {
    /// The trees it read.
    pub(super) trees: usize,
    /// The entries those trees hold together, each tree once.
    pub(super) entries: u64,
    /// The directories it kept the id of and had not entered.
    pub(super) waiting: usize,
}

/// The check of one baseline, while it runs.
struct Check<'a> {
    repository: &'a git2::Repository,
    objects: git2::Odb<'a>,
    /// How the packs of the object store store their objects behind each other.
    chains: &'a Chains,
    limit: StorageLimit,
    /// The entries the objects the check has read the header of are stored behind together.
    chain_steps: u64,
    /// The bytes every file and link object the check has read the header of states.
    files: HashMap<Oid, u64>,
    /// Every tree the check has left, with what it holds.
    left: HashMap<Oid, Shape>,
    /// The directories the check is in, the root first.
    inside: Vec<Directory>,
    trees: usize,
    entries: u64,
}

impl Check<'_> {
    /// Reads the header of the object `id` and nothing more of it, and answers the bytes it
    /// states: the object is of `kind`, and states no more than `limit.max_bytes`; a tree also
    /// no more than [`TREE_BYTES_PER_PATH`] for each path the inode limit admits.
    ///
    /// The entries the object is stored behind are counted first, and the check ends when
    /// the objects it has read the header of are stored behind more than
    /// [`CHAIN_STEPS_PER_PATH`] for each path the inode limit admits: before the Git library
    /// walks this object's chain. The check reads the header of each object once, so what the
    /// Git library walks for all of them is within that bound.
    fn stated(&mut self, id: Oid, kind: ObjectType) -> Result<u64, Unadmitted> {
        self.chain_steps = self.chain_steps.saturating_add(self.chains.behind(id));
        if self.chain_steps > CHAIN_STEPS_PER_PATH.saturating_mul(self.limit.max_inodes) {
            return Err(Unadmitted::Limit);
        }
        let (size, found) = self
            .objects
            .read_header(id)
            .map_err(|_| Unadmitted::Object)?;
        if found != kind {
            return Err(Unadmitted::Object);
        }
        let size = u64::try_from(size).unwrap_or(u64::MAX);
        let bound = if kind == ObjectType::Tree {
            self.limit
                .max_bytes
                .min(TREE_BYTES_PER_PATH.saturating_mul(self.limit.max_inodes))
        } else {
            self.limit.max_bytes
        };
        if size > bound {
            return Err(Unadmitted::Limit);
        }
        Ok(size)
    }

    /// Reads the tree `id`, which a directory holds by a name of `name` bytes, and answers it
    /// as a directory the check is in, with its own entries counted and its directories yet
    /// to enter.
    ///
    /// The tree's header is read first ([`Check::stated`]). Once the tree is read, its entries
    /// are added to those of the trees read before, and the check ends when they are more than
    /// the inode limit: before anything of this tree is kept and before another tree is read.
    /// So the ids the check keeps of directories it has not entered are never more than the
    /// inode limit.
    fn enter(&mut self, id: Oid, name: u64) -> Result<Directory, Unadmitted> {
        let stated = self.stated(id, ObjectType::Tree)?;
        let tree = self
            .repository
            .find_tree(id)
            .map_err(|_| Unadmitted::Object)?;
        let entries = u64::try_from(tree.len()).unwrap_or(u64::MAX);
        self.trees += 1;
        self.entries = self.entries.saturating_add(entries);
        if self.entries > self.limit.max_inodes {
            return Err(Unadmitted::Limit);
        }
        let mut pending = Vec::new();
        let mut shape = Shape {
            beneath: 0,
            paths: entries,
            path_bytes: 0,
            longest: 0,
            bytes: 0,
        };
        for entry in &tree {
            let name = entry.name_bytes();
            if name.len() > MAX_NAME_BYTES || name.contains(&b'/') {
                return Err(Unadmitted::Path);
            }
            let bytes = u64::try_from(name.len()).unwrap_or(u64::MAX);
            shape.path_bytes = shape.path_bytes.saturating_add(bytes);
            shape.longest = shape.longest.max(bytes);
            match entry.kind() {
                Some(ObjectType::Tree) => pending.push((entry.id(), bytes)),
                Some(ObjectType::Blob) => {
                    let file = if let Some(known) = self.files.get(&entry.id()) {
                        *known
                    } else {
                        let stated = self.stated(entry.id(), ObjectType::Blob)?;
                        self.files.insert(entry.id(), stated);
                        stated
                    };
                    shape.bytes = shape.bytes.saturating_add(file);
                }
                _ => {}
            }
        }
        // No name is longer than the bound here, so an entry takes at most this many bytes.
        if stated > TREE_BYTES_PER_PATH.saturating_mul(entries) {
            return Err(Unadmitted::Limit);
        }
        Ok(Directory {
            id,
            name,
            pending,
            shape,
        })
    }

    /// Enters every tree the tree `root` reaches, each once, and answers what `root` holds.
    fn walk(&mut self, root: Oid) -> Result<Shape, Unadmitted> {
        let entered = self.enter(root, 0)?;
        self.inside.push(entered);
        loop {
            // The path of a directory of the innermost one has this many components.
            let depth = self.inside.len();
            let Some(innermost) = self.inside.last_mut() else {
                break;
            };
            let Some((next, name)) = innermost.pending.pop() else {
                let (id, shape, name) = (innermost.id, innermost.shape, innermost.name);
                // What is over a bound relative to its tree is over it wherever the tree is.
                shape.within(self.limit)?;
                self.left.insert(id, shape);
                self.inside.pop();
                if let Some(parent) = self.inside.last_mut() {
                    parent.shape.hold(name, shape);
                }
                continue;
            };
            if let Some(&shape) = self.left.get(&next) {
                if depth + shape.beneath > MAX_BASELINE_DEPTH {
                    return Err(Unadmitted::Path);
                }
                innermost.shape.hold(name, shape);
            } else {
                if depth > MAX_BASELINE_DEPTH {
                    return Err(Unadmitted::Path);
                }
                let entered = self.enter(next, name)?;
                self.inside.push(entered);
            }
        }
        // The root's tree was left last.
        self.left.get(&root).copied().ok_or(Unadmitted::Object)
    }
}

/// Checks the baseline of `commit` in the repository `repository`, a staging tree the driver
/// wrote, before anything walks it and before any of its objects is read whole (design 21,
/// § *The baseline store*). Beside its answer it gives what the check had read and kept when
/// it ended.
///
/// - **Stored chains.** Before the Git library reads any header, every pack of the object
///   store is read for how many entries each of its entries is stored behind
///   ([`Chains::read`]). None is stored behind more than [`chains::MAX_CHAIN`]. The commit,
///   the trees and the file and link objects the check reads the header of are stored behind
///   at most [`CHAIN_STEPS_PER_PATH`] entries together for each path the inode limit admits:
///   the check ends before it reads the header that would take it over.
/// - **Object sizes.** The header of every object is read before the object is: the commit,
///   every tree it reaches and every file and link object those trees name. None states more
///   than `limit.max_bytes`, and no tree more than [`TREE_BYTES_PER_PATH`] for each path the
///   inode limit admits. A tree that is read states no more than [`TREE_BYTES_PER_PATH`] for
///   each entry it holds. A tree entry that records another repository's commit names no
///   object of the baseline.
/// - **Nesting.** No directory lies deeper than [`MAX_BASELINE_DEPTH`].
/// - **Names and path lengths.** No name is longer than [`MAX_NAME_BYTES`], none holds a `/`,
///   and no path is longer than [`MAX_PATH_BYTES`]; the paths together take at most
///   [`PATH_BYTES_PER_PATH`] for each path the inode limit admits.
/// - **Paths.** The baseline names at most `limit.max_inodes` paths. Every entry of every tree
///   counts once for each place the baseline names it, a directory that holds nothing
///   included. The trees the check has read never hold more entries together, each tree
///   once, than the inode limit: it ends before it reads another.
/// - **Bytes.** The files and links of the baseline state at most `limit.max_bytes` together,
///   each once for each place the baseline names it.
///
/// The check reads each tree once, however often the baseline names it: what it found in a
/// tree it has left it keeps by the tree's id, relative to the tree. It reads the header of
/// each file and link object once, and keeps the bytes it states by the object's id. It keeps
/// the directories it is in as a list, never longer than the nesting bound, and of the
/// directories it has yet to enter their ids, never more than the inode limit. So what it
/// keeps and its reads of object headers follow the objects of the baseline up to that limit
/// and the entries of the fetched pack, not the paths those objects name. Reading a tree
/// whole applies each entry the pack stores it behind: at most [`chains::MAX_CHAIN`], and in
/// a pack the fetch of a create indexed each of at most `limit.max_bytes`.
pub(super) fn check_baseline_keeping(
    repository: &git2::Repository,
    commit: Oid,
    limit: StorageLimit,
) -> (Result<Checked, Unadmitted>, Kept) {
    check_in(
        repository,
        &repository.path().join("objects"),
        commit,
        limit,
    )
}

/// The answer of [`check_baseline_keeping`] alone.
#[cfg(test)]
pub(super) fn check_baseline(
    repository: &git2::Repository,
    commit: Oid,
    limit: StorageLimit,
) -> Result<Checked, Unadmitted> {
    check_baseline_keeping(repository, commit, limit).0
}

/// [`check_baseline_keeping`] for a repository that reads its objects from the directory
/// `objects`.
fn check_in(
    repository: &git2::Repository,
    objects: &Path,
    commit: Oid,
    limit: StorageLimit,
) -> (Result<Checked, Unadmitted>, Kept) {
    let chains = match Chains::read(objects) {
        Ok(chains) => chains,
        Err(unadmitted) => return (Err(unadmitted), Kept::default()),
    };
    let Ok(objects) = repository.odb() else {
        return (Err(Unadmitted::Object), Kept::default());
    };
    let mut check = Check {
        repository,
        objects,
        chains: &chains,
        limit,
        chain_steps: 0,
        files: HashMap::new(),
        left: HashMap::new(),
        inside: Vec::new(),
        trees: 0,
        entries: 0,
    };
    let checked = check
        .stated(commit, ObjectType::Commit)
        .map_err(|unadmitted| match unadmitted {
            Unadmitted::Object => Unadmitted::Commit,
            other => other,
        })
        .and_then(|_| {
            repository
                .find_commit(commit)
                .map_err(|_| Unadmitted::Commit)
        })
        .and_then(|commit| check.walk(commit.tree_id()))
        .map(|shape| Checked {
            chain_steps: check.chain_steps,
            trees: check.trees,
            paths: shape.paths,
            path_bytes: shape.path_bytes,
            longest: shape.longest,
            bytes: shape.bytes,
        });
    let kept = Kept {
        trees: check.trees,
        entries: check.entries,
        waiting: check
            .inside
            .iter()
            .map(|directory| directory.pending.len())
            .sum(),
    };
    (checked, kept)
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

/// Renames the staged store to `store` and syncs the directory of the stores, which is made
/// here when there is none. A link or a file in place of that directory is not followed, and
/// a store that stands at `store` is not replaced. When the sync fails, the store is not kept.
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
        RenameFlags::RENAME_NOREPLACE,
    ) {
        Ok(()) => {}
        Err(Errno::EEXIST | Errno::ENOTEMPTY) => return Err(Unwritten::Exists),
        Err(_) => return Err(Unwritten::Write),
    }
    // The staged directory is the store now: nothing is left under the staging name.
    drop(staging.keep());
    if baselines::sync_directory(&stores, WRITE_FAILED).is_err() {
        let _ = baselines::remove_entry(store, WRITE_FAILED);
        return Err(Unwritten::Write);
    }
    Ok(())
}
