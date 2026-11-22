use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::ops::Bound;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use base64::Engine as _;
use chrono::Utc;
use git2::{DiffOptions, ObjectType, Oid, Patch};
use sha2::{Digest as _, Sha256};
use substrate_guarded::fs as guarded;
use substrate_wire::{
    Base64Content, Base64Encoding, GitBaselineFile, GitBaselineFileResult, GitChange, GitChangeSet,
    GitChangeSide, GitChangeStatus, GitChangesQuery, GitSource, StorageLimit, StorageUsage,
    UnifiedDiff, validate_relative_path,
};
use zeroize::Zeroizing;

use crate::{DriverError, GitSourceBinding};

#[cfg(test)]
mod materialization_tests;
mod network;
mod worktree;

/// Whether this build serves a Git source at all; `git_unserved.rs`, the module of a build
/// without the `git` feature, holds `false`.
pub(super) const SERVED: bool = true;

/// The refusal of a Git source this build does not serve: one the configuration does not bind.
pub(super) fn source_unserved() -> DriverError {
    DriverError::unserved(
        "workspace.git-source-unserved",
        "The requested Git source is not configured.",
        "workspace.git",
    )
}

pub(super) struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub(super) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub(super) fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[allow(clippy::too_many_lines)] // Source validation, fetch, checkout and durable accounting are one ordered operation.
pub(super) fn materialize(
    target: &Path,
    locator: &url::Url,
    binding: &GitSourceBinding,
    source: &GitSource,
    authority: &Zeroizing<String>,
    limit: StorageLimit,
    interrupt: &Arc<AtomicBool>,
) -> Result<StorageUsage, DriverError> {
    let branch_ref = format!("refs/heads/{}", source.reference);
    if !git2::Reference::is_valid_name(&branch_ref)
        || source.commit.len() != 40
        || !source.commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(refused("workspace.git-reference-invalid"));
    }
    if !(1..=50).contains(&source.depth) {
        return Err(refused("workspace.git-depth-invalid"));
    }
    if !binding.admits(locator, locator.as_str()) {
        return Err(refused("workspace.git-locator-refused"));
    }
    if authority.is_empty()
        || authority.len() > 16 * 1024
        || authority.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(refused("workspace.git-authority-invalid"));
    }
    let expected =
        Oid::from_str(&source.commit).map_err(|_| refused("workspace.git-commit-invalid"))?;
    let mut init = git2::RepositoryInitOptions::new();
    init.external_template(false)
        .no_reinit(true)
        .initial_head(&source.reference);
    let repository = git2::Repository::init_opts(target, &init)
        .map_err(|_| failed("workspace.git-init-failed"))?;
    let mut config = repository
        .config()
        .map_err(|_| failed("workspace.git-trust-failed"))?;
    config
        .set_bool("http.followRedirects", false)
        .map_err(|_| failed("workspace.git-trust-failed"))?;
    if let Some(ca_bundle) = &binding.ca_bundle {
        config
            .set_str("http.sslCAInfo", &ca_bundle.to_string_lossy())
            .map_err(|_| failed("workspace.git-trust-failed"))?;
    }
    drop(config);
    repository
        .remote("origin", locator.as_str())
        .map_err(|_| failed("workspace.git-remote-failed"))?;
    let destination = format!("refs/remotes/origin/{}", source.reference);
    let refspec = format!("+{branch_ref}:{destination}");
    let fetch_repository = gix::open::Options::isolated()
        .strict_config(true)
        .config_overrides([
            "protocol.version=2",
            "pack.threads=2",
            "gitoxide.tracePacket=false",
            "user.name=Substrate",
            "user.email=substrate@example.invalid",
        ])
        .open(target)
        .map_err(|_| failed("workspace.git-init-failed"))?
        .to_thread_local();
    let remote = fetch_repository
        .remote_at_without_url_rewrite(locator.as_str())
        .map_err(|_| failed("workspace.git-remote-failed"))?
        .with_refspecs([refspec.as_str()], gix::remote::Direction::Fetch)
        .map_err(|_| refused("workspace.git-reference-invalid"))?
        .with_fetch_tags(gix::remote::fetch::Tags::None);
    let control = network::Control::new(limit.max_bytes, Arc::clone(interrupt));
    control.check()?;
    let transport = network::V2Transport::new(locator, binding, authority, &control)?;
    let discovery_started = Instant::now();
    let fetch = remote
        .to_connection_with_transport(transport)
        .with_credentials(no_credentials)
        .prepare_fetch(
            gix::progress::Discard,
            gix::remote::ref_map::Options::default(),
        )
        .map_err(|_| control.error())?;
    let mut branch_refs = fetch.ref_map().remote_refs.iter().filter_map(|reference| {
        let (name, target, _) = reference.unpack();
        (name == branch_ref.as_bytes()).then_some(target)
    });
    let observed = branch_refs.next().flatten();
    if observed.is_none_or(|oid| oid.as_bytes() != expected.as_bytes())
        || branch_refs.next().is_some()
    {
        return Err(DriverError::refused(
            "workspace.git-commit-moved",
            "The provider branch no longer resolves to the admitted commit.",
            "workspace.git",
        ));
    }
    tracing::debug!(
        stage = "git_discovery",
        elapsed_ms = discovery_started.elapsed().as_millis(),
        received_bytes = control.received_bytes(),
        "Git materialization stage completed"
    );
    let fetch_started = Instant::now();
    let outcome = fetch
        .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(
            u32::from(source.depth)
                .try_into()
                .expect("validated positive depth"),
        ))
        .receive(gix::progress::Discard, interrupt)
        .map_err(|_| control.error())?;
    if outcome.handshake.server_protocol_version != gix::protocol::transport::Protocol::V2 {
        return Err(refused("workspace.git-protocol-refused"));
    }
    control.check()?;
    tracing::debug!(
        stage = "git_fetch",
        elapsed_ms = fetch_started.elapsed().as_millis(),
        received_bytes = control.received_bytes(),
        "Git materialization stage completed"
    );
    let checkout_started = Instant::now();
    finish_materialization(&repository, &destination, expected)?;
    control.check()?;
    tracing::debug!(
        stage = "git_checkout",
        elapsed_ms = checkout_started.elapsed().as_millis(),
        "Git materialization stage completed"
    );
    let sync_started = Instant::now();
    let usage = sync_and_bounded_usage(target, limit, interrupt)?;
    tracing::debug!(
        stage = "git_sync_accounting",
        elapsed_ms = sync_started.elapsed().as_millis(),
        used_bytes = usage.used_bytes,
        used_inodes = usage.used_inodes,
        "Git materialization stage completed"
    );
    Ok(usage)
}

#[allow(clippy::result_large_err, clippy::unnecessary_wraps)] // The callback's result type belongs to the pinned gix API.
fn no_credentials(_: gix::credentials::helper::Action) -> gix::credentials::protocol::Result {
    Ok(None)
}

pub(super) fn mechanism_is_provable(workspace_root: &Path) -> bool {
    let Ok(directory) = tempfile::Builder::new()
        .prefix(".substrate-git-probe-")
        .tempdir_in(workspace_root)
    else {
        return false;
    };
    let mut init = git2::RepositoryInitOptions::new();
    init.external_template(false).no_reinit(true);
    curl::Version::get()
        .protocols()
        .any(|protocol| protocol == "https")
        && git2::Repository::init_opts(directory.path(), &init).is_ok()
        && gix::open::Options::isolated()
            .strict_config(true)
            .open(directory.path())
            .is_ok()
}

pub(super) fn reconcile(workspace_root: &Path, baseline_root: &Path) -> Result<(), DriverError> {
    for entry in
        std::fs::read_dir(workspace_root).map_err(|_| failed("workspace.git-reconcile-failed"))?
    {
        let entry = entry.map_err(|_| failed("workspace.git-reconcile-failed"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(".substrate-git-") && name != ".substrate-git-baselines" {
            let metadata = entry
                .path()
                .symlink_metadata()
                .map_err(|_| failed("workspace.git-reconcile-failed"))?;
            if metadata.is_dir() {
                std::fs::remove_dir_all(entry.path())
                    .map_err(|_| failed("workspace.git-reconcile-failed"))?;
            } else {
                std::fs::remove_file(entry.path())
                    .map_err(|_| failed("workspace.git-reconcile-failed"))?;
            }
        }
    }
    for entry in
        std::fs::read_dir(baseline_root).map_err(|_| failed("workspace.git-reconcile-failed"))?
    {
        let entry = entry.map_err(|_| failed("workspace.git-reconcile-failed"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !workspace_root.join(name).is_dir() {
            let metadata = entry
                .path()
                .symlink_metadata()
                .map_err(|_| failed("workspace.git-reconcile-failed"))?;
            if metadata.is_file() {
                std::fs::remove_file(entry.path())
                    .map_err(|_| failed("workspace.git-reconcile-failed"))?;
            }
        }
    }
    sync_directory(workspace_root, "workspace.git-reconcile-failed")?;
    sync_directory(baseline_root, "workspace.git-reconcile-failed")
}

fn finish_materialization(
    repository: &git2::Repository,
    destination: &str,
    expected: Oid,
) -> Result<(), DriverError> {
    let observed = repository
        .refname_to_id(destination)
        .map_err(|_| failed("workspace.git-reference-missing"))?;
    if observed != expected {
        return Err(DriverError::refused(
            "workspace.git-commit-moved",
            "The provider branch no longer resolves to the admitted commit.",
            "workspace.git",
        ));
    }
    let commit = repository
        .find_commit(expected)
        .map_err(|_| failed("workspace.git-commit-missing"))?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force().disable_filters(true);
    repository
        .checkout_tree(commit.as_object(), Some(&mut checkout))
        .and_then(|()| repository.set_head_detached(expected))
        .map_err(|_| failed("workspace.git-checkout-failed"))?;
    Ok(())
}

pub(super) fn write_baseline(
    baseline_root: &Path,
    root_name: &str,
    commit: &str,
) -> Result<(), DriverError> {
    let commit = Oid::from_str(commit)
        .map_err(|_| failed("workspace.git-baseline-write-failed"))?
        .to_string();
    let mut staged = tempfile::NamedTempFile::new_in(baseline_root)
        .map_err(|_| failed("workspace.git-baseline-write-failed"))?;
    staged
        .as_file()
        .set_permissions(
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
        )
        .and_then(|()| staged.write_all(commit.as_bytes()))
        .and_then(|()| staged.write_all(b"\n"))
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|_| failed("workspace.git-baseline-write-failed"))?;
    staged
        .persist(baseline_root.join(root_name))
        .map_err(|_| failed("workspace.git-baseline-write-failed"))?;
    sync_directory(baseline_root, "workspace.git-baseline-write-failed")
}

pub(super) fn remove_baseline(baseline_root: &Path, root_name: &str) -> Result<(), DriverError> {
    match std::fs::remove_file(baseline_root.join(root_name)) {
        Ok(()) => sync_directory(baseline_root, "workspace.git-baseline-remove-failed"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(failed("workspace.git-baseline-remove-failed")),
    }
}

pub(super) fn baseline_file(
    workspace_id: &str,
    workspace: &Path,
    baseline_root: &Path,
    root_name: &str,
    path: &str,
    max_bytes: u64,
) -> Result<GitBaselineFileResult, DriverError> {
    validate_git_path(path)?;
    let commit = read_baseline(baseline_root, root_name)?;
    let repository =
        git2::Repository::open(workspace).map_err(|_| failed("workspace.git-repository-failed"))?;
    let oid = Oid::from_str(&commit).map_err(|_| failed("workspace.git-baseline-invalid"))?;
    let commit_object = repository
        .find_commit(oid)
        .map_err(|_| failed("workspace.git-baseline-missing"))?;
    let tree = commit_object
        .tree()
        .map_err(|_| failed("workspace.git-baseline-missing"))?;
    let entry = match tree.get_path(Path::new(path)) {
        Ok(entry) => entry,
        Err(error) if error.code() == git2::ErrorCode::NotFound => {
            return Ok(GitBaselineFileResult {
                workspace: workspace_id.to_owned(),
                commit,
                file: None,
                observed_at: Utc::now(),
            });
        }
        Err(_) => return Err(failed("workspace.git-baseline-read-failed")),
    };
    let object = entry
        .to_object(&repository)
        .map_err(|_| failed("workspace.git-baseline-read-failed"))?;
    let Some(blob) = object.as_blob() else {
        return Err(DriverError::refused(
            "workspace.git-baseline-not-file",
            "The Git baseline path is not a regular file.",
            "path",
        ));
    };
    let size = u64::try_from(blob.content().len()).expect("blob length fits u64");
    if size > max_bytes {
        return Err(DriverError::exhausted(
            "workspace.git-baseline-limit",
            "The Git baseline file exceeds the admitted byte limit.",
            "max_bytes",
        ));
    }
    Ok(GitBaselineFileResult {
        workspace: workspace_id.to_owned(),
        commit,
        file: Some(GitBaselineFile {
            path: path.to_owned(),
            size,
            sha256: hex::encode(Sha256::digest(blob.content())),
            content: Base64Content {
                encoding: Base64Encoding::Base64,
                data: base64::engine::general_purpose::STANDARD.encode(blob.content()),
            },
        }),
        observed_at: Utc::now(),
    })
}

/// The changes of a Git workspace against the baseline commit the daemon holds for it.
///
/// Every byte of the answer comes from the baseline's objects, each read by its id, or from a
/// file beneath the workspace root, read with the guarded file system within
/// `query.max_file_bytes`. The workspace's repository is never opened as a repository: its
/// configuration is not read, its object store is opened beneath the pinned root, and its index
/// only says which paths are tracked and which of them it has seen unchanged.
pub(super) fn changes(
    workspace_id: &str,
    root: &OwnedFd,
    workspace: &Path,
    baseline_root: &Path,
    root_name: &str,
    query: GitChangesQuery,
) -> Result<GitChangeSet, DriverError> {
    let commit = read_baseline(baseline_root, root_name)?;
    let oid = Oid::from_str(&commit).map_err(|_| failed("workspace.git-baseline-invalid"))?;
    let workspace = guarded::pin_workspace_directory_beneath(root, workspace)
        .map_err(|_| failed("workspace.git-repository-failed"))?;
    let objects = worktree::open_directory(&workspace, ".git/objects")?
        .ok_or_else(|| failed("workspace.git-repository-failed"))?;
    let store = object_store(&objects).map_err(|_| failed("workspace.git-repository-failed"))?;
    let (baseline, links) = baseline_entries(&store, oid)?;
    let staged = Staged::read(&workspace)?;
    let reader = Reader {
        workspace: &workspace,
        store: &store,
        bound: query.max_file_bytes,
    };
    let mut found = reader.tracked_changes(&baseline, &staged)?;
    // A tracked file or link is compared above. A path recorded as another repository's commit
    // is neither listed nor entered.
    let claimed = |path: &str, is_directory: bool| {
        links.contains(path)
            || staged.links.contains(path)
            || (!is_directory
                && (baseline.contains_key(path)
                    || staged.entries.contains_key(path)
                    || staged.conflicted.contains(path)))
    };
    let tracked_beneath = |directory: &str| {
        let from: (Bound<&str>, Bound<&str>) = (Bound::Included(directory), Bound::Unbounded);
        let first_baseline = baseline.range::<str, _>(from).next();
        let first_staged = staged.entries.range::<str, _>(from).next();
        first_baseline.is_some_and(|(path, _)| path.starts_with(directory))
            || first_staged.is_some_and(|(path, _)| path.starts_with(directory))
    };
    found.extend(
        worktree::unclaimed(&workspace, &claimed, &tracked_beneath)?
            .into_iter()
            .map(|(path, observed)| Change {
                path,
                status: GitChangeStatus::Untracked,
                baseline: None,
                current: Some(observed),
            }),
    );
    found.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    let observed_count = found.len();
    found.truncate(usize::try_from(query.max_files).expect("public list bound fits usize"));
    let (items, returned_bytes) = reader.items(found, query.max_total_bytes)?;
    let truncated = observed_count > items.len() || items.iter().any(|item| item.patch.truncated);
    Ok(GitChangeSet {
        workspace: workspace_id.to_owned(),
        commit,
        items,
        returned_bytes,
        truncated,
        observed_at: Utc::now(),
    })
}

/// A baseline or index entry of a file or a link: its Git mode and its object.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Entry {
    mode: u32,
    id: Oid,
}

impl Entry {
    /// The entry of a blob recorded with the raw Git mode `mode`.
    fn of_blob(mode: u32, id: Oid) -> Self {
        let mode = match mode {
            worktree::LINK => worktree::LINK,
            mode if mode & 0o111 != 0 => worktree::EXECUTABLE,
            _ => worktree::FILE,
        };
        Self { mode, id }
    }
}

/// What the workspace's index records. It classifies paths and vouches for entries it has seen
/// unchanged; it never chooses a directory or the way an entry is read.
#[derive(Default)]
struct Staged {
    entries: BTreeMap<String, StagedEntry>,
    conflicted: BTreeSet<String>,
    /// Paths recorded as another repository's commit: neither compared nor entered.
    links: BTreeSet<String>,
    /// When the index file was last written, in seconds and nanoseconds.
    written: (i64, i64),
}

struct StagedEntry {
    entry: Entry,
    size: u32,
    modified: (i64, i64),
}

impl Staged {
    /// Reads `.git/index` beneath the pinned workspace; a workspace without one tracks nothing.
    fn read(workspace: &OwnedFd) -> Result<Self, DriverError> {
        let mut staged = Self::default();
        let Some(file) = worktree::open(workspace, ".git/index")? else {
            return Ok(staged);
        };
        let metadata = file
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .ok_or_else(|| failed("workspace.git-repository-failed"))?;
        staged.written = (metadata.mtime(), metadata.mtime_nsec());
        let index = git2::Index::open(Path::new(&worktree::descriptor_path(&file)))
            .map_err(|_| failed("workspace.git-repository-failed"))?;
        for entry in index.iter() {
            let path = String::from_utf8(entry.path)
                .map_err(|_| failed("workspace.git-path-unrepresentable"))?;
            if entry.flags & 0x3000 != 0 {
                staged.conflicted.insert(path);
            } else if entry.mode == SUBMODULE {
                staged.links.insert(path);
            } else {
                staged.entries.insert(
                    path,
                    StagedEntry {
                        entry: Entry::of_blob(entry.mode, entry.id),
                        size: entry.file_size,
                        modified: (
                            i64::from(entry.mtime.seconds()),
                            i64::from(entry.mtime.nanoseconds()),
                        ),
                    },
                );
            }
        }
        Ok(staged)
    }
}

/// The Git mode of an entry that records another repository's commit.
const SUBMODULE: u32 = 0o160_000;

/// The workspace's object store as a repository with no worktree, no index and no configuration
/// of the workspace's. Objects are read from it by id, so each is bound to the baseline commit.
fn object_store(objects: &OwnedFd) -> Result<git2::Repository, git2::Error> {
    let store = git2::Odb::new()?;
    store.add_disk_alternate(&worktree::descriptor_path(objects))?;
    git2::Repository::from_odb(store)
}

/// The files and links of the baseline commit by path, and the paths it records as another
/// repository's commit.
fn baseline_entries(
    store: &git2::Repository,
    commit: Oid,
) -> Result<(BTreeMap<String, Entry>, BTreeSet<String>), DriverError> {
    let tree = store
        .find_commit(commit)
        .and_then(|commit| commit.tree())
        .map_err(|_| failed("workspace.git-baseline-missing"))?;
    let mut entries = BTreeMap::new();
    let mut links = BTreeSet::new();
    let mut representable = true;
    let walked = tree.walk(git2::TreeWalkMode::PreOrder, |directory, entry| {
        let Some(name) = entry.name() else {
            representable = false;
            return git2::TreeWalkResult::Abort;
        };
        let path = format!("{directory}{name}");
        let mode = u32::try_from(entry.filemode()).unwrap_or_default();
        match entry.kind() {
            Some(ObjectType::Blob) => {
                entries.insert(path, Entry::of_blob(mode, entry.id()));
            }
            Some(ObjectType::Commit) => {
                links.insert(path);
            }
            _ => {}
        }
        git2::TreeWalkResult::Ok
    });
    if !representable {
        return Err(failed("workspace.git-path-unrepresentable"));
    }
    walked.map_err(|_| failed("workspace.git-baseline-read-failed"))?;
    Ok((entries, links))
}

/// One path that differs from the baseline, before its bytes are read for a patch.
struct Change {
    path: String,
    status: GitChangeStatus,
    baseline: Option<Entry>,
    current: Option<worktree::Observed>,
}

/// One side of a change. `bytes` is `None` for a side larger than the per-file bound, which is
/// not read.
struct Side {
    record: GitChangeSide,
    bytes: Option<Vec<u8>>,
}

/// Compares and reads the two sides of a path: the baseline's objects in `store`, and the files
/// beneath the pinned `workspace`, no file beyond `bound` bytes.
struct Reader<'a> {
    workspace: &'a OwnedFd,
    store: &'a git2::Repository,
    bound: u64,
}

impl Reader<'_> {
    /// The tracked paths that differ from the baseline: every path of the baseline or the
    /// index, compared with what is beneath the workspace root.
    fn tracked_changes(
        &self,
        baseline: &BTreeMap<String, Entry>,
        staged: &Staged,
    ) -> Result<Vec<Change>, DriverError> {
        let paths: BTreeSet<&String> = baseline
            .keys()
            .chain(staged.entries.keys())
            .chain(&staged.conflicted)
            .collect();
        let mut found = Vec::new();
        for path in paths {
            let before = baseline.get(path).copied();
            let current = worktree::observe(self.workspace, path)?;
            let status = if staged.conflicted.contains(path) {
                (before.is_some() || current.is_some()).then_some(GitChangeStatus::Conflicted)
            } else {
                match (before, staged.entries.get(path), current) {
                    (Some(_), None, _) | (Some(_), Some(_), None) => Some(GitChangeStatus::Deleted),
                    (None, _, None) | (None, None, Some(_)) => None,
                    (None, Some(_), Some(_)) => Some(GitChangeStatus::Added),
                    (Some(before), Some(recorded), Some(current)) => {
                        if self.unchanged(path, before, recorded, staged.written, current)? {
                            None
                        } else if (before.mode == worktree::LINK)
                            == (current.mode == worktree::LINK)
                        {
                            Some(GitChangeStatus::Modified)
                        } else {
                            Some(GitChangeStatus::TypeChanged)
                        }
                    }
                }
            };
            if let Some(status) = status {
                found.push(Change {
                    path: path.clone(),
                    status,
                    baseline: before,
                    current: current.filter(|_| status != GitChangeStatus::Deleted),
                });
            }
        }
        Ok(found)
    }

    /// Whether the file or link observed at `path` still holds the baseline's bytes.
    ///
    /// The index vouches for an entry it records with the baseline's object, this size and this
    /// modification time, unless the index was written no later than the entry was modified.
    /// Anything else within the per-file bound is read and compared. A larger file is never
    /// read: it is unchanged only when the index records its time and the baseline object has
    /// its size.
    fn unchanged(
        &self,
        path: &str,
        before: Entry,
        recorded: &StagedEntry,
        written: (i64, i64),
        current: worktree::Observed,
    ) -> Result<bool, DriverError> {
        if before.mode != current.mode {
            return Ok(false);
        }
        let vouched = recorded.entry == before
            && recorded.modified.0 == current.modified.0
            && (recorded.modified.1 == 0 || recorded.modified.1 == current.modified.1);
        if current.size > self.bound {
            return Ok(vouched && self.blob_size(before.id)? == current.size);
        }
        if vouched && u64::from(recorded.size) == current.size && current.modified < written {
            return Ok(true);
        }
        Ok(
            worktree::read(self.workspace, path, current, self.bound)?.is_some_and(|bytes| {
                Oid::hash_object(ObjectType::Blob, &bytes).ok() == Some(before.id)
            }),
        )
    }

    /// The changes as the wire names them, each with Git's patch while the patches rendered so
    /// far fit `max_total_bytes`. A side that was not read, being over the per-file bound, has
    /// no patch. The bytes of one change are held only until the next one.
    fn items(
        &self,
        changes: Vec<Change>,
        max_total_bytes: u64,
    ) -> Result<(Vec<GitChange>, u64), DriverError> {
        let scratch = git2::Odb::new()
            .and_then(git2::Repository::from_odb)
            .map_err(|_| failed("workspace.git-diff-failed"))?;
        let scratch_objects = scratch
            .odb()
            .map_err(|_| failed("workspace.git-diff-failed"))?;
        let memory = scratch_objects
            .add_new_mempack_backend(1)
            .map_err(|_| failed("workspace.git-diff-failed"))?;
        let mut items = Vec::with_capacity(changes.len());
        let mut returned_bytes = 0_u64;
        for change in changes {
            validate_git_path(&change.path)?;
            let baseline = change
                .baseline
                .map(|entry| self.baseline_side(&change.path, entry))
                .transpose()?;
            let current = change
                .current
                .map(|observed| self.current_side(&change.path, observed))
                .transpose()?;
            let mut patch = UnifiedDiff {
                text: String::new(),
                truncated: false,
                binary: true,
            };
            if [&baseline, &current]
                .into_iter()
                .flatten()
                .all(|side| side.bytes.is_some())
            {
                let (patch_bytes, binary) =
                    render(&scratch, &change.path, baseline.as_ref(), current.as_ref())
                        .and_then(|rendered| memory.reset().map(|()| rendered))
                        .map_err(|_| failed("workspace.git-diff-failed"))?;
                patch.binary = binary;
                let remaining = max_total_bytes.saturating_sub(returned_bytes);
                let patch_len = u64::try_from(patch_bytes.len()).expect("patch length fits u64");
                if patch_len <= remaining {
                    patch.text = String::from_utf8(patch_bytes).map_err(|_| {
                        DriverError::failed(
                            "workspace.git-diff-encoding",
                            "Git produced a non-UTF-8 textual patch.",
                        )
                    })?;
                    returned_bytes = returned_bytes.saturating_add(patch_len);
                } else {
                    patch.truncated = true;
                }
            }
            items.push(GitChange {
                status: change.status,
                baseline: baseline.map(|side| side.record),
                current: current.map(|side| side.record),
                patch,
            });
        }
        Ok((items, returned_bytes))
    }

    fn blob_size(&self, id: Oid) -> Result<u64, DriverError> {
        self.store
            .odb()
            .and_then(|objects| objects.read_header(id))
            .map(|(size, _)| u64::try_from(size).expect("object size fits u64"))
            .map_err(|_| failed("workspace.git-baseline-read-failed"))
    }

    fn baseline_side(&self, path: &str, entry: Entry) -> Result<Side, DriverError> {
        let size = self.blob_size(entry.id)?;
        let bytes = if size > self.bound {
            None
        } else {
            let blob = self
                .store
                .find_blob(entry.id)
                .map_err(|_| failed("workspace.git-baseline-read-failed"))?;
            Some(blob.content().to_vec())
        };
        Ok(Side {
            record: GitChangeSide {
                path: path.to_owned(),
                mode: entry.mode,
                size,
                oid: Some(entry.id.to_string()),
            },
            bytes,
        })
    }

    fn current_side(&self, path: &str, observed: worktree::Observed) -> Result<Side, DriverError> {
        let bytes = worktree::read(self.workspace, path, observed, self.bound)?;
        let oid = bytes
            .as_deref()
            .and_then(|bytes| Oid::hash_object(ObjectType::Blob, bytes).ok())
            .map(|id| id.to_string());
        let size = bytes.as_ref().map_or(observed.size, |bytes| {
            u64::try_from(bytes.len()).expect("file length fits u64")
        });
        Ok(Side {
            record: GitChangeSide {
                path: path.to_owned(),
                mode: observed.mode,
                size,
                oid,
            },
            bytes,
        })
    }
}

/// Git's patch text for one path from the bytes of its two sides, and whether Git reads them as
/// binary. `scratch` is a repository in memory that holds these bytes and nothing else.
fn render(
    scratch: &git2::Repository,
    path: &str,
    before: Option<&Side>,
    after: Option<&Side>,
) -> Result<(Vec<u8>, bool), git2::Error> {
    let staged = |side: Option<&Side>| -> Result<git2::Index, git2::Error> {
        let mut index = git2::Index::new()?;
        if let Some(side) = side {
            let bytes = side.bytes.as_deref().unwrap_or_default();
            index.add(&git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: side.record.mode,
                uid: 0,
                gid: 0,
                file_size: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
                id: scratch.blob(bytes)?,
                flags: 0,
                flags_extended: 0,
                path: path.as_bytes().to_vec(),
            })?;
        }
        Ok(index)
    };
    let (before, after) = (staged(before)?, staged(after)?);
    let mut options = DiffOptions::new();
    options.include_typechange(true);
    let diff = scratch.diff_index_to_index(&before, &after, Some(&mut options))?;
    if diff.get_delta(0).is_none() {
        return Ok((Vec::new(), false));
    }
    let Some(mut rendered) = Patch::from_diff(&diff, 0)? else {
        return Ok((Vec::new(), false));
    };
    let text = rendered.to_buf()?.to_vec();
    Ok((text, rendered.delta().flags().is_binary()))
}

fn sync_and_bounded_usage(
    root: &Path,
    limit: StorageLimit,
    interrupt: &AtomicBool,
) -> Result<StorageUsage, DriverError> {
    let mut bytes = 0_u64;
    let mut inodes = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    let mut directories = Vec::new();
    while let Some(directory) = pending.pop() {
        directories.push(directory.clone());
        for entry in
            std::fs::read_dir(&directory).map_err(|_| failed("workspace.git-measure-failed"))?
        {
            if interrupt.load(Ordering::Relaxed) {
                return Err(failed("workspace.git-fetch-cancelled"));
            }
            let entry = entry.map_err(|_| failed("workspace.git-measure-failed"))?;
            let metadata = entry
                .path()
                .symlink_metadata()
                .map_err(|_| failed("workspace.git-measure-failed"))?;
            inodes = inodes.saturating_add(1);
            bytes = bytes.saturating_add(metadata.len());
            if bytes > limit.max_bytes || inodes > limit.max_inodes {
                return Err(DriverError::exhausted(
                    "workspace.git-storage-limit",
                    "The Git workspace exceeds its storage limit.",
                    "storage",
                ));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                std::fs::File::open(entry.path())
                    .and_then(|file| file.sync_all())
                    .map_err(|_| failed("workspace.git-sync-failed"))?;
            }
        }
    }
    for directory in directories.into_iter().rev() {
        if interrupt.load(Ordering::Relaxed) {
            return Err(failed("workspace.git-fetch-cancelled"));
        }
        sync_directory(&directory, "workspace.git-sync-failed")?;
    }
    Ok(StorageUsage {
        limit,
        used_bytes: bytes,
        used_inodes: inodes,
        observed_at: Utc::now(),
    })
}

fn refused(code: &'static str) -> DriverError {
    DriverError::refused(code, "The Git source request is invalid.", "workspace.git")
}

fn failed(code: &'static str) -> DriverError {
    DriverError::failed(code, "Git workspace materialization failed.")
}

/// Whether the workspace at `root_name` was materialized from a Git source: it holds a valid
/// materialization baseline. An empty or adopted workspace holds none.
///
/// # Errors
///
/// A baseline that exists but cannot be read or is malformed.
pub(crate) fn has_baseline(baseline_root: &Path, root_name: &str) -> Result<bool, DriverError> {
    match read_baseline(baseline_root, root_name) {
        Ok(_) => Ok(true),
        Err(error) if error.code == "workspace.git-workspace-required" => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_baseline(baseline_root: &Path, root_name: &str) -> Result<String, DriverError> {
    let value = match std::fs::read_to_string(baseline_root.join(root_name)) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(DriverError::refused(
                "workspace.git-workspace-required",
                "The workspace has no Git materialization baseline.",
                "workspace.git",
            ));
        }
        Err(_) => return Err(failed("workspace.git-baseline-read-failed")),
    };
    let value = value.trim_end_matches('\n');
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(failed("workspace.git-baseline-invalid"));
    }
    Ok(value.to_owned())
}

fn validate_git_path(path: &str) -> Result<(), DriverError> {
    validate_relative_path(path).map_err(|_| {
        DriverError::refused(
            "workspace.path-escape",
            "Workspace path escapes its root.",
            "path",
        )
    })?;
    if path.split('/').any(|component| component == ".git") {
        return Err(DriverError::not_found());
    }
    Ok(())
}

pub(super) fn sync_workspace_root(root: &Path) -> Result<(), DriverError> {
    sync_directory(root, "workspace.git-install-failed")
}

fn sync_directory(path: &Path, code: &'static str) -> Result<(), DriverError> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| failed(code))
}

#[cfg(test)]
mod tests {
    use git2::{Repository, Signature};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use substrate_wire::{GitChangeSet, GitChangeStatus, GitChangesQuery, GitSource, StorageLimit};
    use tempfile::tempdir;
    use zeroize::Zeroizing;

    use super::{
        baseline_file, changes, finish_materialization, materialize, sync_and_bounded_usage,
        write_baseline,
    };
    use crate::GitSourceBinding;

    fn source_repository() -> (tempfile::TempDir, git2::Oid) {
        let directory = tempdir().expect("source directory");
        let repository = Repository::init(directory.path()).expect("source repository");
        std::fs::write(
            directory.path().join("README.md"),
            b"bounded git workspace\n",
        )
        .expect("worktree file");
        let mut index = repository.index().expect("index");
        index
            .add_path(std::path::Path::new("README.md"))
            .expect("stage file");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("tree id");
        let tree = repository.find_tree(tree_id).expect("tree");
        let signature =
            Signature::now("Substrate Test", "substrate@example.invalid").expect("signature");
        let commit = repository
            .commit(
                Some("refs/heads/provider-default"),
                &signature,
                &signature,
                "fixture",
                &tree,
                &[],
            )
            .expect("commit");
        drop(tree);
        drop(index);
        drop(repository);
        (directory, commit)
    }

    #[test]
    fn exact_provider_branch_commit_is_verified_and_checked_out_detached() {
        let (target, commit) = source_repository();
        let observed = Repository::open(target.path()).expect("repository");
        observed
            .reference(
                "refs/remotes/origin/provider-default",
                commit,
                true,
                "fixture remote branch",
            )
            .expect("remote branch");
        finish_materialization(&observed, "refs/remotes/origin/provider-default", commit)
            .expect("exact checkout");

        assert_eq!(observed.head().expect("HEAD").target(), Some(commit));
        assert!(observed.head_detached().expect("detached state"));
        assert_eq!(
            std::fs::read(target.path().join("README.md")).expect("checked-out file"),
            b"bounded git workspace\n"
        );
    }

    #[test]
    fn materialize_errors_do_not_disclose_transient_authority() {
        let target = tempdir().expect("target directory");
        let binding =
            GitSourceBinding::new("connector", "https://127.0.0.1:1/", None).expect("binding");
        let source = GitSource {
            source: "connector".to_owned(),
            locator: "unused".to_owned(),
            reference: "provider-default".to_owned(),
            commit: "0123456789012345678901234567890123456789".to_owned(),
            depth: 1,
        };
        let authority = "authority-must-never-escape";
        let error = materialize(
            target.path(),
            &url::Url::parse("https://127.0.0.1:1/repository.git").expect("URL"),
            &binding,
            &source,
            &Zeroizing::new(authority.to_owned()),
            StorageLimit {
                max_bytes: 1024,
                max_inodes: 100,
            },
            &Arc::new(AtomicBool::new(false)),
        )
        .expect_err("unreachable repository");
        assert!(!error.to_string().contains(authority));
        assert_eq!(error.code, "workspace.git-fetch-failed");
    }

    #[test]
    fn baseline_files_and_changes_are_read_against_host_private_commit_metadata() {
        let (workspace, commit) = source_repository();
        let baselines = tempdir().expect("baseline root");
        write_baseline(baselines.path(), "ws_git", &commit.to_string()).expect("baseline metadata");

        let file = baseline_file(
            "ws_git",
            workspace.path(),
            baselines.path(),
            "ws_git",
            "README.md",
            1024,
        )
        .expect("baseline read")
        .file
        .expect("baseline file");
        assert_eq!(file.path, "README.md");
        assert_eq!(file.size, 22);
        assert_eq!(
            file.content.decode().expect("baseline content"),
            b"bounded git workspace\n"
        );
        assert!(
            baseline_file(
                "ws_git",
                workspace.path(),
                baselines.path(),
                "ws_git",
                "absent.txt",
                1024,
            )
            .expect("absent baseline read")
            .file
            .is_none()
        );

        std::fs::write(workspace.path().join("README.md"), b"changed workspace\n")
            .expect("modify worktree");
        std::fs::write(workspace.path().join("new.txt"), b"new\n").expect("untracked file");
        let observed = changes(
            "ws_git",
            &workspaces_root(workspace.path()),
            workspace.path(),
            baselines.path(),
            "ws_git",
            GitChangesQuery {
                max_files: 10,
                max_file_bytes: 1024,
                max_total_bytes: 4096,
            },
        )
        .expect("change set");
        assert_eq!(observed.commit, commit.to_string());
        assert_eq!(observed.items.len(), 2);
        assert_eq!(observed.items[0].status, GitChangeStatus::Modified);
        assert_eq!(
            observed.items[0]
                .current
                .as_ref()
                .expect("current side")
                .path,
            "README.md"
        );
        assert!(observed.items[0].patch.text.contains("changed workspace"));
        assert_eq!(observed.items[1].status, GitChangeStatus::Untracked);
        assert_eq!(
            observed.items[1]
                .current
                .as_ref()
                .expect("current side")
                .path,
            "new.txt"
        );
        assert!(!observed.truncated);
    }

    const OUTSIDE: &str = "bytes-outside-the-workspace";

    fn baseline_root(commit: git2::Oid) -> tempfile::TempDir {
        let baselines = tempdir().expect("baseline root");
        write_baseline(baselines.path(), "ws_git", &commit.to_string()).expect("baseline metadata");
        baselines
    }

    /// The directory that holds the workspace, as the driver holds its workspaces root.
    fn workspaces_root(workspace: &std::path::Path) -> std::os::fd::OwnedFd {
        std::fs::File::open(workspace.parent().expect("parent directory"))
            .expect("workspaces root")
            .into()
    }

    fn observe(
        workspace: &std::path::Path,
        baselines: &std::path::Path,
        max_file_bytes: u64,
    ) -> Result<GitChangeSet, crate::DriverError> {
        changes(
            "ws_git",
            &workspaces_root(workspace),
            workspace,
            baselines,
            "ws_git",
            GitChangesQuery {
                max_files: 100,
                max_file_bytes,
                max_total_bytes: 1024 * 1024,
            },
        )
    }

    fn configure(metadata: &std::path::Path, section: &str) {
        let mut configuration = std::fs::OpenOptions::new()
            .append(true)
            .open(metadata.join("config"))
            .expect("repository configuration");
        std::io::Write::write_all(&mut configuration, section.as_bytes())
            .expect("configuration entry");
    }

    fn paths(set: &GitChangeSet) -> Vec<&str> {
        set.items
            .iter()
            .filter_map(|item| item.current.as_ref().or(item.baseline.as_ref()))
            .map(|side| side.path.as_str())
            .collect()
    }

    fn holds(set: &GitChangeSet, bytes: &str) -> bool {
        set.items.iter().any(|item| {
            item.patch.text.contains(bytes)
                || [&item.baseline, &item.current]
                    .into_iter()
                    .flatten()
                    .any(|side| side.path.contains(bytes))
        })
    }

    /// The reader reads the admitted workspace whatever worktree its configuration names.
    #[test]
    fn changes_read_the_admitted_workspace_whatever_worktree_its_configuration_names() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(commit);
        let elsewhere = tempdir().expect("directory outside the workspace");
        std::fs::write(
            elsewhere.path().join(format!("{OUTSIDE}.txt")),
            format!("{OUTSIDE}\n"),
        )
        .expect("file outside the workspace");
        std::fs::write(workspace.path().join("own.txt"), b"own\n").expect("workspace file");
        configure(
            &workspace.path().join(".git"),
            &format!("[core]\n\tworktree = {}\n", elsewhere.path().display()),
        );

        let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        assert!(!holds(&observed, OUTSIDE), "{observed:#?}");
        assert_eq!(paths(&observed), ["own.txt"], "{observed:#?}");
    }

    /// The reader reads a link in the workspace as a link, whatever its configuration says.
    #[test]
    fn changes_read_a_workspace_link_as_a_link_whatever_its_configuration_says() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(commit);
        let elsewhere = tempdir().expect("directory outside the workspace");
        let target = elsewhere.path().join("target.txt");
        std::fs::write(&target, format!("{OUTSIDE}\n")).expect("file outside the workspace");
        std::os::unix::fs::symlink(&target, workspace.path().join("link")).expect("link");
        configure(
            &workspace.path().join(".git"),
            "[core]\n\tsymlinks = false\n",
        );

        let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        assert!(!holds(&observed, OUTSIDE), "{observed:#?}");
        assert_eq!(paths(&observed), ["link"], "{observed:#?}");
        assert_eq!(
            observed.items[0].current.as_ref().expect("link").mode,
            0o120_000
        );
    }

    /// The per-file byte bound holds for every file the reader opens.
    #[test]
    fn the_per_file_byte_bound_holds_for_every_file_the_reader_opens() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(commit);
        let elsewhere = tempdir().expect("directory outside the workspace");
        let target = elsewhere.path().join("target.txt");
        std::fs::write(&target, "line outside\n".repeat(4096)).expect("file over the bound");
        std::os::unix::fs::symlink(&target, workspace.path().join("link")).expect("link");
        std::fs::write(workspace.path().join("wide.txt"), "wide line\n".repeat(512))
            .expect("workspace file over the bound");
        configure(
            &workspace.path().join(".git"),
            "[core]\n\tsymlinks = false\n",
        );

        let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        assert_eq!(paths(&observed), ["link", "wide.txt"], "{observed:#?}");
        for item in &observed.items {
            assert!(
                item.patch.text.len() <= 2048,
                "{} carries {} bytes",
                paths(&observed).join(" "),
                item.patch.text.len()
            );
        }
    }

    /// The reader takes repository metadata only from beneath the workspace root.
    #[test]
    fn changes_take_repository_metadata_only_from_beneath_the_workspace_root() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(commit);
        let elsewhere = tempdir().expect("directory outside the workspace");
        let metadata = elsewhere.path().join("metadata");
        std::fs::rename(workspace.path().join(".git"), &metadata).expect("metadata moved out");
        std::fs::write(
            workspace.path().join(".git"),
            format!("gitdir: {}\n", metadata.display()),
        )
        .expect("metadata named by a file");
        std::fs::write(
            elsewhere.path().join(format!("{OUTSIDE}.txt")),
            format!("{OUTSIDE}\n"),
        )
        .expect("file outside the workspace");
        configure(
            &metadata,
            &format!("[core]\n\tworktree = {}\n", elsewhere.path().display()),
        );

        let error = observe(workspace.path(), baselines.path(), 1024)
            .expect_err("metadata outside the workspace root is not read");
        assert_eq!(error.code, "workspace.git-repository-failed");
    }

    /// Ignore rules come only from regular files beneath the workspace root.
    #[test]
    fn changes_take_ignore_rules_only_from_regular_files_beneath_the_workspace_root() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(commit);
        let elsewhere = tempdir().expect("directory outside the workspace");
        let rules = elsewhere.path().join("rules");
        std::fs::write(&rules, b"own.txt\n").expect("rules outside the workspace");
        std::os::unix::fs::symlink(&rules, workspace.path().join(".gitignore")).expect("link");
        std::fs::write(workspace.path().join("own.txt"), b"own\n").expect("workspace file");

        let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        assert_eq!(paths(&observed), [".gitignore", "own.txt"], "{observed:#?}");
    }

    /// Ignore rules come from the workspace's files whatever its configuration names.
    #[test]
    fn changes_take_ignore_rules_from_the_workspace_whatever_its_configuration_names() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(commit);
        let elsewhere = tempdir().expect("directory outside the workspace");
        let rules = elsewhere.path().join("rules");
        std::fs::write(&rules, b"own.txt\n").expect("rules outside the workspace");
        std::fs::write(workspace.path().join("own.txt"), b"own\n").expect("workspace file");
        std::fs::write(workspace.path().join("build.log"), b"log\n").expect("workspace file");
        std::fs::write(workspace.path().join(".gitignore"), b"*.log\n").expect("workspace rules");
        configure(
            &workspace.path().join(".git"),
            &format!("[core]\n\texcludesFile = {}\n", rules.display()),
        );

        let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        assert_eq!(paths(&observed), [".gitignore", "own.txt"], "{observed:#?}");
    }

    /// The baseline is read from the object store beneath the root, packed as well as loose.
    #[test]
    fn changes_read_a_packed_baseline_from_the_store_beneath_the_workspace_root() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(commit);
        {
            let repository = Repository::open(workspace.path()).expect("repository");
            let mut builder = repository.packbuilder().expect("pack builder");
            builder.insert_commit(commit).expect("baseline objects");
            let mut pack = git2::Buf::new();
            builder.write_buf(&mut pack).expect("pack");
            let objects = repository.odb().expect("object store");
            let mut writer = objects.packwriter().expect("pack writer");
            std::io::Write::write_all(&mut writer, &pack).expect("pack bytes");
            writer.commit().expect("pack index");
        }
        for entry in std::fs::read_dir(workspace.path().join(".git/objects")).expect("objects") {
            let entry = entry.expect("entry");
            if entry.file_name().len() == 2 {
                std::fs::remove_dir_all(entry.path()).expect("loose objects removed");
            }
        }
        std::fs::write(workspace.path().join("README.md"), b"changed workspace\n")
            .expect("modify worktree");

        let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        assert_eq!(paths(&observed), ["README.md"], "{observed:#?}");
        assert!(
            observed.items[0]
                .patch
                .text
                .contains("-bounded git workspace\n+changed workspace\n"),
            "{observed:#?}"
        );
    }

    /// Commits `paths` of the worktree on top of `parent`.
    fn commit_paths(workspace: &std::path::Path, parent: git2::Oid, paths: &[&str]) -> git2::Oid {
        let repository = Repository::open(workspace).expect("repository");
        let mut index = repository.index().expect("index");
        for path in paths {
            index
                .add_path(std::path::Path::new(path))
                .expect("stage path");
        }
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("tree id");
        let tree = repository.find_tree(tree_id).expect("tree");
        let signature =
            Signature::now("Substrate Test", "substrate@example.invalid").expect("signature");
        let parent = repository.find_commit(parent).expect("parent commit");
        repository
            .commit(
                Some("refs/heads/provider-default"),
                &signature,
                &signature,
                "fixture",
                &tree,
                &[&parent],
            )
            .expect("commit")
    }

    /// A workspace whose baseline holds files, an executable, a link and ignore rules.
    fn tracked_workspace() -> (tempfile::TempDir, tempfile::TempDir) {
        use std::os::unix::fs::PermissionsExt as _;
        let (workspace, first) = source_repository();
        let root = workspace.path();
        std::fs::create_dir_all(root.join("src")).expect("directory");
        for (path, content) in [
            ("src/gone.txt", "gone\n"),
            ("src/unstaged.txt", "unstaged\n"),
            ("tool.sh", "#!/bin/sh\n"),
            ("was-file.txt", "file\n"),
            ("wide.txt", "narrow\n"),
            (".gitignore", "ignored/\n*.log\n!kept.log\n"),
        ] {
            std::fs::write(root.join(path), content).expect("baseline file");
        }
        std::fs::set_permissions(root.join("tool.sh"), std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        std::os::unix::fs::symlink("README.md", root.join("was-link")).expect("link");
        let commit = commit_paths(
            root,
            first,
            &[
                "src/gone.txt",
                "src/unstaged.txt",
                "tool.sh",
                "was-file.txt",
                "was-link",
                "wide.txt",
                ".gitignore",
            ],
        );
        let baselines = baseline_root(commit);
        (workspace, baselines)
    }

    /// Changes every kind of entry of [`tracked_workspace`] and adds untracked ones.
    fn change_every_kind_of_entry(root: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt as _;
        for directory in ["nested/deep", "ignored", "vendored/.git"] {
            std::fs::create_dir_all(root.join(directory)).expect("directory");
        }
        std::fs::remove_file(root.join("src/gone.txt")).expect("removed file");
        std::fs::remove_file(root.join("was-file.txt")).expect("removed file");
        std::fs::remove_file(root.join("was-link")).expect("removed link");
        std::os::unix::fs::symlink("README.md", root.join("was-file.txt")).expect("link");
        std::os::unix::fs::symlink("README.md", root.join("ulink")).expect("link");
        for (path, content) in [
            ("README.md", "changed workspace\n".to_owned()),
            ("staged.txt", "staged\n".to_owned()),
            ("new.txt", "new\n".to_owned()),
            ("nested/deep/new.txt", "deep\n".to_owned()),
            ("ignored/x.txt", "x\n".to_owned()),
            ("vendored/file.txt", "another repository\n".to_owned()),
            ("debug.log", "log\n".to_owned()),
            ("kept.log", "kept log\n".to_owned()),
            ("was-link", "now a file\n".to_owned()),
            ("wide.txt", "wide line\n".repeat(205)),
            ("large.txt", "large line\n".repeat(190)),
            ("blob.bin", "\0\u{1}\u{2}\0".to_owned()),
            ("empty.txt", String::new()),
        ] {
            std::fs::write(root.join(path), content).expect("workspace file");
        }
        std::fs::set_permissions(root.join("tool.sh"), std::fs::Permissions::from_mode(0o644))
            .expect("not executable");
        let repository = Repository::open(root).expect("repository");
        let mut index = repository.index().expect("index");
        index
            .add_path(std::path::Path::new("staged.txt"))
            .expect("staged file");
        index
            .remove_path(std::path::Path::new("src/unstaged.txt"))
            .expect("unstaged file");
        index.write().expect("write index");
    }

    fn item<'a>(set: &'a GitChangeSet, path: &str) -> &'a substrate_wire::GitChange {
        set.items
            .iter()
            .find(|item| {
                [&item.baseline, &item.current]
                    .into_iter()
                    .flatten()
                    .any(|side| side.path == path)
            })
            .expect("change of the path")
    }

    /// Every entry beneath the root is classified against the baseline and the index, ignore
    /// rules apply, and a file over the per-file bound is reported without its bytes.
    #[test]
    fn changes_classify_every_entry_beneath_the_root_against_the_baseline_and_the_index() {
        use GitChangeStatus::{Added, Deleted, Modified, TypeChanged, Untracked};
        let (workspace, baselines) = tracked_workspace();
        change_every_kind_of_entry(workspace.path());

        let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        let statuses: Vec<_> = paths(&observed)
            .into_iter()
            .zip(observed.items.iter().map(|item| item.status))
            .collect();
        assert_eq!(
            statuses,
            [
                ("README.md", Modified),
                ("blob.bin", Untracked),
                ("empty.txt", Untracked),
                ("kept.log", Untracked),
                ("large.txt", Untracked),
                ("nested/deep/new.txt", Untracked),
                ("new.txt", Untracked),
                ("src/gone.txt", Deleted),
                ("src/unstaged.txt", Deleted),
                ("staged.txt", Added),
                ("tool.sh", Modified),
                ("ulink", Untracked),
                ("was-file.txt", TypeChanged),
                ("was-link", TypeChanged),
                ("wide.txt", Modified),
            ],
            "{observed:#?}"
        );
        assert!(!observed.truncated);
        assert!(
            item(&observed, "README.md")
                .patch
                .text
                .contains("-bounded git workspace\n+changed workspace\n")
        );
        assert!(
            item(&observed, "src/gone.txt")
                .patch
                .text
                .contains("deleted file mode 100644")
        );
        assert!(item(&observed, "src/unstaged.txt").current.is_none());
        assert!(
            item(&observed, "staged.txt")
                .patch
                .text
                .contains("new file mode 100644")
        );
        assert_eq!(
            item(&observed, "tool.sh").patch.text,
            "diff --git a/tool.sh b/tool.sh\nold mode 100755\nnew mode 100644\n"
        );
        let link = item(&observed, "ulink");
        assert_eq!(link.current.as_ref().expect("link").mode, 0o120_000);
        assert!(link.patch.text.contains("+README.md\n"), "{link:#?}");
        assert!(item(&observed, "blob.bin").patch.binary);
        for unread in ["large.txt", "wide.txt"] {
            let change = item(&observed, unread);
            assert!(
                change.patch.binary && change.patch.text.is_empty(),
                "{change:#?}"
            );
            assert_eq!(change.current.as_ref().expect("current side").oid, None);
        }
    }

    #[test]
    fn usage_measurement_refuses_byte_and_inode_overflow() {
        let directory = tempdir().expect("workspace");
        std::fs::write(directory.path().join("one"), b"1234").expect("first file");
        std::fs::write(directory.path().join("two"), b"5678").expect("second file");

        let byte_error = sync_and_bounded_usage(
            directory.path(),
            StorageLimit {
                max_bytes: 3,
                max_inodes: 10,
            },
            &AtomicBool::new(false),
        )
        .expect_err("byte ceiling");
        assert_eq!(byte_error.code, "workspace.git-storage-limit");

        let inode_error = sync_and_bounded_usage(
            directory.path(),
            StorageLimit {
                max_bytes: 1024,
                max_inodes: 1,
            },
            &AtomicBool::new(false),
        )
        .expect_err("inode ceiling");
        assert_eq!(inode_error.code, "workspace.git-storage-limit");
    }

    #[test]
    fn synchronization_accounts_nested_git_metadata_and_never_follows_symlinks() {
        let directory = tempdir().expect("workspace");
        let outside = tempdir().expect("outside directory");
        let large = std::fs::File::create(outside.path().join("large")).expect("outside file");
        large.set_len(10 * 1024 * 1024).expect("outside allocation");
        std::fs::create_dir_all(directory.path().join(".git/objects")).expect("nested Git objects");
        std::fs::write(directory.path().join(".git/objects/pack"), b"git").expect("Git bytes");
        std::fs::write(directory.path().join("README.md"), b"file").expect("workspace bytes");
        std::os::unix::fs::symlink(outside.path(), directory.path().join("external"))
            .expect("symlink");
        let paths = [
            ".git",
            ".git/objects",
            ".git/objects/pack",
            "README.md",
            "external",
        ];
        let expected: u64 = paths
            .iter()
            .map(|path| {
                directory
                    .path()
                    .join(path)
                    .symlink_metadata()
                    .expect("entry metadata")
                    .len()
            })
            .sum();
        let usage = sync_and_bounded_usage(
            directory.path(),
            StorageLimit {
                max_bytes: 1024 * 1024,
                max_inodes: 5,
            },
            &AtomicBool::new(false),
        )
        .expect("bounded synchronized tree");
        assert_eq!(usage.used_bytes, expected);
        assert_eq!(usage.used_inodes, 5);
    }

    #[test]
    fn dropping_the_async_cancellation_guard_prevents_later_sync_install_stages() {
        let guard = super::Cancellation::new();
        let interrupt = guard.flag();
        drop(guard);
        let directory = tempdir().expect("workspace");
        let error = sync_and_bounded_usage(
            directory.path(),
            StorageLimit {
                max_bytes: 1024,
                max_inodes: 1,
            },
            &interrupt,
        )
        .expect_err("cancelled synchronization");
        assert_eq!(error.code, "workspace.git-fetch-cancelled");
    }

    #[test]
    fn startup_reconciliation_removes_only_git_staging_and_orphan_baselines() {
        let workspace_root = tempdir().expect("workspace root");
        let baseline_root = workspace_root.path().join(".substrate-git-baselines");
        std::fs::create_dir(&baseline_root).expect("baseline root");
        std::fs::create_dir(workspace_root.path().join(".substrate-git-orphan"))
            .expect("staging directory");
        std::fs::create_dir(workspace_root.path().join("ws_live")).expect("live workspace");
        std::fs::write(baseline_root.join("ws_live"), b"live").expect("live baseline");
        std::fs::write(baseline_root.join("ws_gone"), b"gone").expect("orphan baseline");
        std::fs::create_dir(workspace_root.path().join("unrelated")).expect("unrelated directory");

        super::reconcile(workspace_root.path(), &baseline_root).expect("reconcile");
        assert!(!workspace_root.path().join(".substrate-git-orphan").exists());
        assert!(!baseline_root.join("ws_gone").exists());
        assert!(baseline_root.join("ws_live").exists());
        assert!(workspace_root.path().join("unrelated").exists());
    }
}
