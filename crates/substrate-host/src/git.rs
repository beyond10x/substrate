use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::ops::Bound;
use std::os::fd::OwnedFd;
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

use crate::git_baselines::{self as baselines, GitWorkspace};
use crate::{DriverError, GitSourceBinding};

#[cfg(test)]
mod materialization_tests;
mod network;
mod store;
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
    sync_directory(workspace_root, "workspace.git-reconcile-failed")?;
    baselines::reconcile(workspace_root, baseline_root)
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

/// Writes the baseline of a Git workspace that is being created: first its store, the objects
/// of `commit` read from the staging materialization at `repository`, then its commit-id file
/// (design 21, § *The baseline store*).
///
/// # Errors
///
/// `workspace.path-escape` for a root name outside the root-name set, and
/// `workspace.git-target-exists` for one that already has a commit-id file or a store, each
/// before either is written. `workspace.git-storage-limit` for a store over `max_bytes`, and
/// `workspace.git-baseline-write-failed` for every other failure. A failure leaves neither
/// entry of this create.
pub(super) fn write_baseline(
    baseline_root: &Path,
    root_name: &str,
    repository: &Path,
    commit: &str,
    max_bytes: u64,
) -> Result<(), DriverError> {
    const CODE: &str = "workspace.git-baseline-write-failed";
    let entries = baselines::entries(baseline_root, root_name)?;
    let commit = Oid::from_str(commit).map_err(|_| failed(CODE))?;
    if baselines::present(baseline_root, root_name)? {
        return Err(target_exists());
    }
    store::write(
        baseline_root,
        &entries.store,
        &repository.join(".git/objects"),
        commit,
        max_bytes,
    )
    .map_err(|unwritten| match unwritten {
        store::Unwritten::Limit => storage_limit(),
        store::Unwritten::Exists => target_exists(),
        store::Unwritten::Object | store::Unwritten::Write => failed(CODE),
    })?;
    write_commit_id(baseline_root, &entries.commit_id, commit).inspect_err(|_| {
        // The store this create wrote goes with its failure.
        let _ = std::fs::remove_dir_all(&entries.store);
    })
}

/// Writes the commit-id file `commit_id`, read-only for the daemon's user, without replacing
/// one that exists.
fn write_commit_id(baseline_root: &Path, commit_id: &Path, commit: Oid) -> Result<(), DriverError> {
    const CODE: &str = "workspace.git-baseline-write-failed";
    let mut staged = tempfile::NamedTempFile::new_in(baseline_root).map_err(|_| failed(CODE))?;
    staged
        .write_all(format!("{commit}\n").as_bytes())
        .and_then(|()| {
            staged.as_file().set_permissions(
                <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o400),
            )
        })
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|_| failed(CODE))?;
    staged.persist_noclobber(commit_id).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            target_exists()
        } else {
            failed(CODE)
        }
    })?;
    sync_directory(baseline_root, CODE)
}

/// Removes the commit-id file and the store of `root_name`.
pub(super) fn remove_baseline(baseline_root: &Path, root_name: &str) -> Result<(), DriverError> {
    baselines::remove(baseline_root, root_name)
}

/// One file of the baseline commit, read from the store the driver holds. Once the workspace
/// has its store, nothing in the workspace is opened.
pub(super) fn baseline_file(
    workspace: &GitWorkspace<'_>,
    path: &str,
    max_bytes: u64,
) -> Result<GitBaselineFileResult, DriverError> {
    let workspace_id = workspace.id;
    validate_git_path(path)?;
    let commit = read_baseline(workspace.baseline_root, workspace.root_name)?;
    let oid = Oid::from_str(&commit).map_err(|_| failed("workspace.git-baseline-invalid"))?;
    let repository = store::held(workspace, oid, worktree::MAX_STORE_ENTRIES)?;
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

/// What one observation may hash: the tracked files it compares with the baseline, and the
/// baseline objects it reads for their sizes.
const COMPARISON_BUDGET: u64 = 1024 * 1024 * 1024;

/// The changes of a Git workspace against the baseline commit the daemon holds for it.
///
/// Every byte and every size of the answer comes from the baseline's objects, each read by its
/// id and checked against it, or from a file beneath the workspace root, read with the guarded
/// file system. A tracked path is unchanged only when the hash of its content equals the
/// baseline's object id. The workspace's repository is never opened as a repository: its
/// configuration is not read, its index only labels a path, and its object store is not
/// opened. The baseline's objects come from the store the driver holds; the one exception is
/// the build of that store for a workspace that predates it ([`store::held`]).
pub(super) fn changes(
    workspace: &GitWorkspace<'_>,
    query: GitChangesQuery,
) -> Result<GitChangeSet, DriverError> {
    changes_within(workspace, query, COMPARISON_BUDGET)
}

/// [`changes`] with `budget` bytes to hash. Paths are compared and listed in byte order. A
/// tracked file larger than what is left of the budget is reported as modified without being
/// read; a baseline object larger than what is left ends the answer. Either marks the answer
/// truncated.
fn changes_within(
    held: &GitWorkspace<'_>,
    query: GitChangesQuery,
    budget: u64,
) -> Result<GitChangeSet, DriverError> {
    let workspace_id = held.id;
    let commit = read_baseline(held.baseline_root, held.root_name)?;
    let oid = Oid::from_str(&commit).map_err(|_| failed("workspace.git-baseline-invalid"))?;
    let store = store::held(held, oid, worktree::MAX_STORE_ENTRIES)?;
    let workspace = guarded::pin_workspace_directory_beneath(held.root, held.path)
        .map_err(|_| failed("workspace.git-repository-failed"))?;
    let (baseline, links) = baseline_entries(&store, oid)?;
    let labels = Labels::read(&workspace, &links)?;
    // A file or link of the baseline or the index is compared below. A path the baseline
    // records as another repository's commit is neither listed nor entered.
    let claimed = |path: &str, is_directory: bool| {
        links.contains(path)
            || (!is_directory
                && (baseline.contains_key(path)
                    || labels.staged.contains(path)
                    || labels.conflicted.contains(path)))
    };
    let tracked_beneath = |directory: &str| {
        let from: (Bound<&str>, Bound<&str>) = (Bound::Included(directory), Bound::Unbounded);
        let first_baseline = baseline.range::<str, _>(from).next();
        let first_staged = labels.staged.range::<str, _>(from).next();
        first_baseline.is_some_and(|(path, _)| path.starts_with(directory))
            || first_staged.is_some_and(|path| path.starts_with(directory))
    };
    let untracked = worktree::unclaimed(&workspace, &claimed, &tracked_beneath)?;
    // Every path to look at, in byte order; an untracked one carries what the walk observed.
    let mut candidates: Vec<(&str, Option<worktree::Observed>)> = baseline
        .keys()
        .chain(&labels.staged)
        .chain(&labels.conflicted)
        .map(|path| (path.as_str(), None))
        .chain(
            untracked
                .found
                .iter()
                .map(|(path, observed)| (path.as_str(), Some(*observed))),
        )
        .collect();
    candidates.sort_by_key(|candidate| candidate.0);
    candidates.dedup_by_key(|candidate| candidate.0);
    let scratch = git2::Odb::new()
        .and_then(git2::Repository::from_odb)
        .map_err(|_| failed("workspace.git-diff-failed"))?;
    let scratch_objects = scratch
        .odb()
        .map_err(|_| failed("workspace.git-diff-failed"))?;
    let memory = scratch_objects
        .add_new_mempack_backend(1)
        .map_err(|_| failed("workspace.git-diff-failed"))?;
    let mut reader = Reader {
        workspace: &workspace,
        store: &store,
        scratch: &scratch,
        memory: &memory,
        bound: query.max_file_bytes,
        budget,
        uncompared: false,
        patch_bytes_left: query.max_total_bytes,
    };
    let item_limit = usize::try_from(query.max_files).expect("public list bound fits usize");
    let (items, cut) = reader.answer(candidates, item_limit, &baseline, &labels)?;
    Ok(GitChangeSet {
        workspace: workspace_id.to_owned(),
        commit,
        returned_bytes: query.max_total_bytes - reader.patch_bytes_left,
        truncated: cut
            || reader.uncompared
            || untracked.truncated
            || items.iter().any(|item| item.patch.truncated),
        items,
        observed_at: Utc::now(),
    })
}

/// A file or link of the baseline: its Git mode and its object.
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

/// How the workspace's index labels paths. It never says whether a path changed or is listed,
/// which directory is read or how an entry is read.
#[derive(Default)]
struct Labels {
    /// Paths the index records: absent from the baseline, such a path is added, not untracked.
    staged: BTreeSet<String>,
    /// Paths the index records in conflict stages.
    conflicted: BTreeSet<String>,
}

impl Labels {
    /// Reads `.git/index` beneath the pinned workspace; a workspace without one labels nothing.
    /// `links` are the paths the baseline records as another repository's commit: an index
    /// entry of that kind labels nothing there, and labels like any other entry elsewhere.
    fn read(workspace: &OwnedFd, links: &BTreeSet<String>) -> Result<Self, DriverError> {
        let mut labels = Self::default();
        let Some(file) = worktree::open(workspace, ".git/index")? else {
            return Ok(labels);
        };
        if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
            return Err(failed("workspace.git-repository-failed"));
        }
        let index = git2::Index::open(Path::new(&worktree::descriptor_path(&file)))
            .map_err(|_| failed("workspace.git-repository-failed"))?;
        for entry in index.iter() {
            let path = String::from_utf8(entry.path)
                .map_err(|_| failed("workspace.git-path-unrepresentable"))?;
            if entry.flags & 0x3000 != 0 {
                labels.conflicted.insert(path);
            } else if entry.mode != SUBMODULE || !links.contains(&path) {
                labels.staged.insert(path);
            }
        }
        Ok(labels)
    }
}

/// The Git mode of an entry that records another repository's commit.
const SUBMODULE: u32 = 0o160_000;

/// The object store at `objects`, a directory the driver owns, as a repository with no worktree,
/// no index and no configuration of a workspace's. Objects are read from it by id, so each is
/// bound to the baseline commit.
fn object_store(objects: &str) -> Result<git2::Repository, git2::Error> {
    let store = git2::Odb::new()?;
    store.add_disk_alternate(objects)?;
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

/// What comparing one path gave, before its bytes are read for a patch. A `status` of `None`
/// is a path that holds the baseline's bytes.
struct Change {
    status: Option<GitChangeStatus>,
    baseline: Option<Entry>,
    current: Option<worktree::Observed>,
    /// The object id of the current content, when comparing the path hashed it.
    hashed: Option<Oid>,
    /// The current content is larger than what was left to hash: it was not read and is not
    /// read for a patch.
    uncompared: bool,
}

/// One side of a change. `bytes` is `None` for a side larger than the per-file bound, whose
/// text is withheld.
struct Side {
    record: GitChangeSide,
    bytes: Option<Vec<u8>>,
}

/// Compares and reads the two sides of a path: the baseline's objects in `store`, and the files
/// beneath the pinned `workspace`. `scratch` is a repository in memory, backed by `memory`,
/// that holds the bytes of one change while its patch is rendered. `bound` is the per-file
/// bound on text, `budget` what is left to hash in this observation, `uncompared` whether a
/// path was too large for it, and `patch_bytes_left` what is left of the bound on all patches
/// together.
struct Reader<'a> {
    workspace: &'a OwnedFd,
    store: &'a git2::Repository,
    scratch: &'a git2::Repository,
    memory: &'a git2::Mempack<'a>,
    bound: u64,
    budget: u64,
    uncompared: bool,
    patch_bytes_left: u64,
}

impl Reader<'_> {
    /// The items of the answer for `candidates`, which are in byte order and of which an
    /// untracked one carries what the walk observed, and whether the answer ends before the
    /// last of them: at a change past `item_limit`, or at one whose baseline object the budget
    /// no longer covers.
    fn answer(
        &mut self,
        candidates: Vec<(&str, Option<worktree::Observed>)>,
        item_limit: usize,
        baseline: &BTreeMap<String, Entry>,
        labels: &Labels,
    ) -> Result<(Vec<GitChange>, bool), DriverError> {
        let mut items = Vec::new();
        for (path, observed) in candidates {
            let change = if let Some(observed) = observed {
                Change {
                    status: Some(GitChangeStatus::Untracked),
                    baseline: None,
                    current: Some(observed),
                    hashed: None,
                    uncompared: false,
                }
            } else {
                self.compare(path, baseline.get(path).copied(), labels)?
            };
            if change.status.is_none() {
                continue;
            }
            let item = if items.len() < item_limit {
                self.item(path, &change)?
            } else {
                None
            };
            let Some(item) = item else {
                return Ok((items, true));
            };
            items.push(item);
        }
        Ok((items, false))
    }

    /// Compares the path of the baseline or the index with what is beneath the workspace root.
    ///
    /// A file or link of the baseline is unchanged only when the hash of its content is the
    /// baseline's object id and its mode agrees. One larger than what is left of the budget is
    /// not read: it is reported as modified and the reader records that it was not compared.
    /// The index decides a label only: added for a path the baseline does not hold, conflicted
    /// for a path it records in conflict stages.
    fn compare(
        &mut self,
        path: &str,
        before: Option<Entry>,
        labels: &Labels,
    ) -> Result<Change, DriverError> {
        let current = worktree::observe(self.workspace, path)?;
        let mut hashed = None;
        let mut uncompared = false;
        let status = if labels.conflicted.contains(path) {
            (before.is_some() || current.is_some()).then_some(GitChangeStatus::Conflicted)
        } else {
            match (before, current) {
                (None, None) => None,
                (Some(_), None) => Some(GitChangeStatus::Deleted),
                (None, Some(_)) => Some(GitChangeStatus::Added),
                (Some(before), Some(current)) if before.mode != current.mode => {
                    let links = (
                        before.mode == worktree::LINK,
                        current.mode == worktree::LINK,
                    );
                    Some(if links.0 == links.1 {
                        GitChangeStatus::Modified
                    } else {
                        GitChangeStatus::TypeChanged
                    })
                }
                (Some(before), Some(current)) => {
                    match worktree::blob_id(self.workspace, path, current, &mut self.budget)? {
                        worktree::Hashed::OverBudget => {
                            uncompared = true;
                            self.uncompared = true;
                            Some(GitChangeStatus::Modified)
                        }
                        worktree::Hashed::Id(id) if id == before.id => None,
                        worktree::Hashed::Id(id) => {
                            hashed = Some(id);
                            Some(GitChangeStatus::Modified)
                        }
                        worktree::Hashed::Unstable => Some(GitChangeStatus::Modified),
                    }
                }
            }
        };
        Ok(Change {
            status,
            baseline: before,
            current,
            hashed,
            uncompared,
        })
    }

    /// The change as the wire names it, with Git's patch while the patches rendered so far fit
    /// the bound on all of them. A side over the per-file bound, and a current side that was
    /// not compared, has no text. `None` is a change whose baseline object the budget no longer
    /// covers.
    fn item(&mut self, path: &str, change: &Change) -> Result<Option<GitChange>, DriverError> {
        let Some(status) = change.status else {
            return Ok(None);
        };
        validate_git_path(path)?;
        let baseline = if let Some(entry) = change.baseline {
            let Some(side) = self.baseline_side(path, entry)? else {
                return Ok(None);
            };
            Some(side)
        } else {
            None
        };
        let current = change
            .current
            .map(|observed| self.current_side(path, observed, change))
            .transpose()?;
        let mut diff = UnifiedDiff {
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
                render(self.scratch, path, baseline.as_ref(), current.as_ref())
                    .and_then(|rendered| self.memory.reset().map(|()| rendered))
                    .map_err(|_| failed("workspace.git-diff-failed"))?;
            diff.binary = binary;
            let patch_len = u64::try_from(patch_bytes.len()).expect("patch length fits u64");
            if let Some(left) = self.patch_bytes_left.checked_sub(patch_len) {
                diff.text = String::from_utf8(patch_bytes).map_err(|_| {
                    DriverError::failed(
                        "workspace.git-diff-encoding",
                        "Git produced a non-UTF-8 textual patch.",
                    )
                })?;
                self.patch_bytes_left = left;
            } else {
                diff.truncated = true;
            }
        }
        Ok(Some(GitChange {
            status,
            baseline: baseline.map(|side| side.record),
            current: current.map(|side| side.record),
            patch: diff,
        }))
    }

    /// The baseline side of a path, or `None` when the budget no longer covers its object.
    ///
    /// The size reported is the length of the object the Git library read whole and checked
    /// against its id. The size the object's header states is only taken from the budget
    /// before that read.
    fn baseline_side(&mut self, path: &str, entry: Entry) -> Result<Option<Side>, DriverError> {
        let stated = self
            .store
            .odb()
            .and_then(|objects| objects.read_header(entry.id))
            .map(|(size, _)| u64::try_from(size).unwrap_or(u64::MAX))
            .map_err(|_| failed("workspace.git-baseline-read-failed"))?;
        let Some(left) = self.budget.checked_sub(stated) else {
            return Ok(None);
        };
        self.budget = left;
        let blob = self
            .store
            .find_blob(entry.id)
            .map_err(|_| failed("workspace.git-baseline-read-failed"))?;
        let size = u64::try_from(blob.content().len()).expect("object size fits u64");
        Ok(Some(Side {
            record: GitChangeSide {
                path: path.to_owned(),
                mode: entry.mode,
                size,
                oid: Some(entry.id.to_string()),
            },
            bytes: (size <= self.bound).then(|| blob.content().to_vec()),
        }))
    }

    /// The current side of a path. Its bytes are read when they are within the per-file bound
    /// and the comparison did not find them too large to hash.
    fn current_side(
        &self,
        path: &str,
        observed: worktree::Observed,
        change: &Change,
    ) -> Result<Side, DriverError> {
        let bytes = if change.uncompared {
            None
        } else {
            worktree::read(self.workspace, path, observed, self.bound)?
        };
        let oid = bytes
            .as_deref()
            .and_then(|bytes| Oid::hash_object(ObjectType::Blob, bytes).ok())
            .or(change.hashed)
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
                return Err(storage_limit());
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

fn target_exists() -> DriverError {
    DriverError::refused(
        "workspace.git-target-exists",
        "The workspace target already exists.",
        "workspace.git",
    )
}

fn storage_limit() -> DriverError {
    DriverError::exhausted(
        "workspace.git-storage-limit",
        "The Git workspace exceeds its storage limit.",
        "storage",
    )
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
        baseline_file, changes, changes_within, finish_materialization, materialize,
        sync_and_bounded_usage, write_baseline,
    };
    use crate::GitSourceBinding;
    use crate::git_baselines::{DEFAULT_BYTE_LIMIT, GitWorkspace};

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
        let baselines = baseline_root(workspace.path(), commit);
        let read = |path: &str| {
            target(
                workspace.path(),
                baselines.path(),
                DEFAULT_BYTE_LIMIT,
                |held| baseline_file(held, path, 1024),
            )
        };

        let file = read("README.md")
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
            read("absent.txt")
                .expect("absent baseline read")
                .file
                .is_none()
        );

        std::fs::write(workspace.path().join("README.md"), b"changed workspace\n")
            .expect("modify worktree");
        std::fs::write(workspace.path().join("new.txt"), b"new\n").expect("untracked file");
        let observed = target(
            workspace.path(),
            baselines.path(),
            DEFAULT_BYTE_LIMIT,
            |held| {
                changes(
                    held,
                    GitChangesQuery {
                        max_files: 10,
                        max_file_bytes: 1024,
                        max_total_bytes: 4096,
                    },
                )
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

    /// The baseline root of a workspace created from the repository at `workspace` at `commit`:
    /// its store and its commit-id file, under the root name `ws_git`.
    fn baseline_root(workspace: &std::path::Path, commit: git2::Oid) -> tempfile::TempDir {
        let baselines = tempdir().expect("baseline root");
        write_baseline(
            baselines.path(),
            "ws_git",
            workspace,
            &commit.to_string(),
            u64::MAX,
        )
        .expect("baseline");
        baselines
    }

    /// The store of `ws_git` beneath `baselines`.
    fn store_of(baselines: &std::path::Path) -> std::path::PathBuf {
        baselines.join(".objects/ws_git")
    }

    /// The ids of every object the store at `store` holds.
    pub(super) fn objects_of(store: &std::path::Path) -> std::collections::BTreeSet<git2::Oid> {
        assert!(store.is_dir(), "the workspace has no store");
        let objects = git2::Odb::new().expect("object database");
        objects
            .add_disk_alternate(store.to_str().expect("store path"))
            .expect("store");
        let mut ids = std::collections::BTreeSet::new();
        objects
            .foreach(|id| {
                ids.insert(*id);
                true
            })
            .expect("objects of the store");
        ids
    }

    /// Leaves `ws_git` as a workspace materialized before the store existed: its commit-id
    /// file, and no store.
    fn remove_store(baselines: &std::path::Path) {
        match std::fs::remove_dir_all(store_of(baselines)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                panic!("the store is removed: {error}")
            }
            _ => {}
        }
    }

    /// The directory that holds the workspace, as the driver holds its workspaces root.
    fn workspaces_root(workspace: &std::path::Path) -> std::os::fd::OwnedFd {
        std::fs::File::open(workspace.parent().expect("parent directory"))
            .expect("workspaces root")
            .into()
    }

    /// What `read` answers for the workspace as an observation names it, with `copy_bound`
    /// bytes for the copy a store is built from.
    fn target<T>(
        workspace: &std::path::Path,
        baselines: &std::path::Path,
        copy_bound: u64,
        read: impl FnOnce(&GitWorkspace<'_>) -> T,
    ) -> T {
        read(&GitWorkspace {
            id: "ws_git",
            root: &workspaces_root(workspace),
            path: workspace,
            baseline_root: baselines,
            root_name: "ws_git",
            copy_bound: &|| copy_bound,
        })
    }

    fn observe(
        workspace: &std::path::Path,
        baselines: &std::path::Path,
        max_file_bytes: u64,
    ) -> Result<GitChangeSet, crate::DriverError> {
        target(workspace, baselines, DEFAULT_BYTE_LIMIT, |held| {
            changes(
                held,
                GitChangesQuery {
                    max_files: 100,
                    max_file_bytes,
                    max_total_bytes: 1024 * 1024,
                },
            )
        })
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
        let baselines = baseline_root(workspace.path(), commit);
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
        let baselines = baseline_root(workspace.path(), commit);
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

    /// The per-file byte bound holds for the text of every worktree file and link in a patch.
    #[test]
    fn the_per_file_byte_bound_holds_for_the_text_of_every_worktree_file_and_link() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(workspace.path(), commit);
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
        let baselines = baseline_root(workspace.path(), commit);
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

        // The baseline comes from the driver's store and the worktree from beneath the root.
        let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        assert!(!holds(&observed, OUTSIDE), "{observed:#?}");
        assert!(paths(&observed).is_empty(), "{observed:#?}");

        // No store is built from metadata outside the workspace root.
        remove_store(baselines.path());
        let error = observe(workspace.path(), baselines.path(), 1024)
            .expect_err("metadata outside the workspace root is not read");
        assert_eq!(error.code, "workspace.git-repository-failed");
        assert!(!store_of(baselines.path()).exists());
    }

    /// Ignore rules come only from regular files beneath the workspace root.
    #[test]
    fn changes_take_ignore_rules_only_from_regular_files_beneath_the_workspace_root() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(workspace.path(), commit);
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
        let baselines = baseline_root(workspace.path(), commit);
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

    /// The store of a workspace that predates it is built from the object store beneath the
    /// root, packed as well as loose.
    #[test]
    fn changes_read_a_packed_baseline_of_a_workspace_that_predates_the_store() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(workspace.path(), commit);
        remove_store(baselines.path());
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
        assert!(store_of(baselines.path()).join("pack").is_dir());
    }

    /// The loose object file of the baseline's `README.md` in the workspace's object store.
    fn baseline_object(workspace: &std::path::Path, commit: git2::Oid) -> std::path::PathBuf {
        let repository = Repository::open(workspace).expect("repository");
        let id = repository
            .find_commit(commit)
            .expect("baseline commit")
            .tree()
            .expect("baseline tree")
            .get_path(std::path::Path::new("README.md"))
            .expect("baseline entry")
            .id()
            .to_string();
        workspace.join(".git/objects").join(&id[..2]).join(&id[2..])
    }

    /// A size the reader reports for a baseline entry is the size of the object the daemon-held
    /// commit names, whatever the workspace's object store holds in that object's place.
    #[test]
    fn changes_report_a_baseline_size_only_from_the_baseline_the_daemon_holds() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(workspace.path(), commit);
        let object = baseline_object(workspace.path(), commit);
        let elsewhere = tempdir().expect("directory outside the workspace");
        let outside = elsewhere.path().join("outside");
        std::fs::write(&outside, [0xb7, 0x55, 0x00, 0x00]).expect("file outside the workspace");
        std::fs::remove_file(&object).expect("object removed");
        std::os::unix::fs::symlink(&outside, &object).expect("link");
        std::fs::remove_file(workspace.path().join("README.md")).expect("tracked file removed");

        // The answer carries the size of the object in the driver's store: 22 bytes.
        let observed = observe(workspace.path(), baselines.path(), 16).expect("change set");
        let sizes: Vec<u64> = observed
            .items
            .iter()
            .filter_map(|item| item.baseline.as_ref())
            .map(|side| side.size)
            .collect();
        assert_eq!(sizes, [22], "{observed:#?}");

        // No store is built from an object store that holds a link.
        remove_store(baselines.path());
        let error = observe(workspace.path(), baselines.path(), 16)
            .expect_err("a link in the object store is not followed");
        assert_eq!(error.code, "workspace.git-repository-failed");
        assert!(!store_of(baselines.path()).exists());
    }

    /// The index labels a path and removes none from the answer: a new file beneath the root
    /// is listed whatever the index records at its path or at a directory above it.
    #[test]
    fn a_new_file_is_listed_whatever_the_index_records_at_its_path_or_above_it() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(workspace.path(), commit);
        std::fs::create_dir(workspace.path().join("new")).expect("directory");
        std::fs::write(workspace.path().join("new/file.txt"), b"new\n").expect("new file");
        std::fs::write(workspace.path().join("other.txt"), b"other\n").expect("new file");
        {
            let repository = Repository::open(workspace.path()).expect("repository");
            let mut index = repository.index().expect("index");
            for path in ["new", "other.txt"] {
                index
                    .add(&git2::IndexEntry {
                        ctime: git2::IndexTime::new(0, 0),
                        mtime: git2::IndexTime::new(0, 0),
                        dev: 0,
                        ino: 0,
                        mode: 0o160_000,
                        uid: 0,
                        gid: 0,
                        file_size: 0,
                        id: commit,
                        flags: 0,
                        flags_extended: 0,
                        path: path.as_bytes().to_vec(),
                    })
                    .expect("index entry");
            }
            index.write().expect("write index");
        }

        let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        assert_eq!(
            paths(&observed),
            ["new/file.txt", "other.txt"],
            "{observed:#?}"
        );
    }

    /// The baseline's objects are read only from the driver's store: an object store the
    /// workspace names by a file changes no answer, and no store is built from one.
    #[test]
    fn changes_take_baseline_objects_only_from_the_store_the_driver_holds() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(workspace.path(), commit);
        std::fs::write(workspace.path().join("README.md"), b"changed workspace\n")
            .expect("modify worktree");
        let untouched = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        let elsewhere = tempdir().expect("directory outside the workspace");
        let store = workspace.path().join(".git/objects");
        let outside = elsewhere.path().join("objects");
        std::fs::rename(&store, &outside).expect("object store moved out");
        std::fs::create_dir_all(store.join("info")).expect("empty object store");
        std::fs::write(
            store.join("info/alternates"),
            format!("{}\n", outside.display()),
        )
        .expect("object store named by a file");

        let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
        assert_eq!(observed.items, untouched.items);

        remove_store(baselines.path());
        let error = observe(workspace.path(), baselines.path(), 1024)
            .expect_err("an object store outside the workspace root is not read");
        assert_eq!(error.code, "workspace.git-repository-failed");
        assert!(!store_of(baselines.path()).exists());
    }

    /// The reader opens no entry of the workspace in a way that waits: the observation answers
    /// from the driver's store when the workspace's object store holds an entry that is not a
    /// regular file.
    #[test]
    fn changes_answer_when_the_object_store_holds_an_entry_that_is_not_a_regular_file() {
        use std::os::unix::fs::OpenOptionsExt as _;
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(workspace.path(), commit);
        let object = baseline_object(workspace.path(), commit);
        std::fs::remove_file(&object).expect("object removed");
        nix::unistd::mkfifo(&object, nix::sys::stat::Mode::from_bits_truncate(0o600))
            .expect("entry that is not a regular file");
        std::fs::remove_file(workspace.path().join("README.md")).expect("tracked file removed");

        let (answer, answered) = std::sync::mpsc::channel();
        let (root, baseline) = (workspace.path().to_owned(), baselines.path().to_owned());
        std::thread::spawn(move || {
            let sizes = observe(&root, &baseline, 1024).map(|set| {
                set.items
                    .iter()
                    .filter_map(|item| item.baseline.as_ref())
                    .map(|side| side.size)
                    .collect::<Vec<u64>>()
            });
            let _ = answer.send(sizes.map_err(|error| error.code));
        });
        let outcome = answered.recv_timeout(std::time::Duration::from_secs(2));
        // Lets a reader that waits return, so the test process keeps no thread.
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&object);
        assert_eq!(
            outcome.expect("the observation waited on an entry that is not a regular file"),
            Ok(vec![22])
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
        let baselines = baseline_root(workspace.path(), commit);
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
        // A tracked file over the bound was hashed to compare it; an untracked one was not.
        for (unread, hashed) in [("large.txt", false), ("wide.txt", true)] {
            let change = item(&observed, unread);
            assert!(
                change.patch.binary && change.patch.text.is_empty(),
                "{change:#?}"
            );
            let current = change.current.as_ref().expect("current side");
            assert_eq!(current.oid.is_some(), hashed, "{change:#?}");
        }
    }

    /// A path of the baseline whose bytes are the baseline's is not reported, whatever the
    /// index records for it.
    #[test]
    fn a_tracked_file_that_holds_the_baseline_bytes_is_not_reported_whatever_the_index_records() {
        let (workspace, first) = source_repository();
        let root = workspace.path();
        std::fs::write(root.join("kept.txt"), b"kept\n").expect("baseline file");
        let commit = commit_paths(root, first, &["kept.txt"]);
        let baselines = baseline_root(workspace.path(), commit);
        std::fs::write(root.join("README.md"), b"staged\n").expect("other bytes");
        {
            let repository = Repository::open(root).expect("repository");
            let mut index = repository.index().expect("index");
            index
                .add_path(std::path::Path::new("README.md"))
                .expect("staged change");
            index
                .remove_path(std::path::Path::new("kept.txt"))
                .expect("entry removed");
            index.write().expect("write index");
        }
        std::fs::write(root.join("README.md"), b"bounded git workspace\n").expect("baseline bytes");

        let observed = observe(root, baselines.path(), 1024).expect("change set");
        assert!(paths(&observed).is_empty(), "{observed:#?}");
    }

    /// A tracked file over the per-file bound is compared by its content; its text is withheld
    /// and its baseline side reports the size of the object that was read.
    #[test]
    fn a_tracked_file_over_the_per_file_bound_is_compared_by_content_and_its_text_withheld() {
        let (workspace, first) = source_repository();
        let root = workspace.path();
        let wide = "wide line\n".repeat(512);
        std::fs::write(root.join("big.txt"), &wide).expect("baseline file");
        let commit = commit_paths(root, first, &["big.txt"]);
        let baselines = baseline_root(workspace.path(), commit);
        std::fs::write(root.join("big.txt"), wide.replacen('w', "W", 1)).expect("one other byte");

        let observed = observe(root, baselines.path(), 1024).expect("change set");
        assert_eq!(paths(&observed), ["big.txt"], "{observed:#?}");
        let change = &observed.items[0];
        assert_eq!(change.status, GitChangeStatus::Modified);
        assert_eq!(change.baseline.as_ref().expect("baseline side").size, 5120);
        assert!(change.current.as_ref().expect("current side").oid.is_some());
        assert!(
            change.patch.binary && change.patch.text.is_empty(),
            "{change:#?}"
        );
    }

    /// When the bytes an observation may hash are spent, every path from the first one not
    /// compared on is left out and the answer says so.
    #[test]
    fn a_spent_hashing_budget_leaves_out_what_was_not_compared_and_marks_the_answer() {
        let (workspace, first) = source_repository();
        let root = workspace.path();
        let tracked = ["a.txt", "m.txt", "z.txt"];
        for path in tracked {
            std::fs::write(root.join(path), "line\n".repeat(20)).expect("baseline file");
        }
        let commit = commit_paths(root, first, &tracked);
        let baselines = baseline_root(workspace.path(), commit);
        for path in tracked {
            std::fs::write(root.join(path), "LINE\n".repeat(20)).expect("changed file");
        }
        for path in ["b-new.txt", "y-new.txt"] {
            std::fs::write(root.join(path), b"new\n").expect("untracked file");
        }
        let within = |budget: u64| {
            target(root, baselines.path(), DEFAULT_BYTE_LIMIT, |held| {
                changes_within(
                    held,
                    GitChangesQuery {
                        max_files: 100,
                        max_file_bytes: 1024,
                        max_total_bytes: 1024 * 1024,
                    },
                    budget,
                )
            })
            .expect("change set")
        };

        let whole = within(u64::MAX);
        assert_eq!(
            paths(&whole),
            ["a.txt", "b-new.txt", "m.txt", "y-new.txt", "z.txt"],
            "{whole:#?}"
        );
        assert!(!whole.truncated);
        // The budget covers `README.md`, `a.txt` and its baseline object, and not `m.txt`.
        let cut = within(22 + 100 + 100 + 50);
        assert_eq!(paths(&cut), ["a.txt", "b-new.txt"], "{cut:#?}");
        assert!(cut.truncated);
    }

    /// Directories nested deeper than the walk goes are not entered: the observation answers
    /// and marks the answer.
    #[test]
    fn directories_nested_deeper_than_the_walk_goes_mark_the_answer_truncated() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(workspace.path(), commit);
        let root = workspace.path();
        let nest = root.join(["d"; 101].join("/"));
        std::fs::create_dir_all(&nest).expect("nested directories");
        std::fs::write(nest.join("deep.txt"), b"deep\n").expect("file beyond the walk");
        std::fs::write(root.join("own.txt"), b"own\n").expect("workspace file");

        let observed = observe(root, baselines.path(), 1024).expect("change set");
        assert_eq!(paths(&observed), ["own.txt"], "{observed:#?}");
        assert!(observed.truncated);
    }

    /// An ignore file over the bound applies the rules within the bound and none beyond it.
    #[test]
    fn an_ignore_file_over_the_bound_applies_only_the_rules_within_the_bound() {
        let (workspace, commit) = source_repository();
        let baselines = baseline_root(workspace.path(), commit);
        let root = workspace.path();
        let rules = format!("first.tmp\n{}last.tmp\n", "# filler line\n".repeat(80_000));
        std::fs::write(root.join(".gitignore"), rules).expect("ignore file over the bound");
        for path in ["first.tmp", "last.tmp"] {
            std::fs::write(root.join(path), b"x\n").expect("workspace file");
        }

        let observed = observe(root, baselines.path(), 1024).expect("change set");
        assert_eq!(
            paths(&observed),
            [".gitignore", "last.tmp"],
            "{observed:#?}"
        );
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

    /// The start rule removes every entry of the baseline root that is neither the commit-id
    /// file nor the store of a workspace directory that exists, a staging directory among
    /// them, and nothing of a workspace that exists.
    #[test]
    fn startup_reconciliation_removes_orphan_stores_and_staging_and_keeps_a_live_workspace() {
        let names = |directory: &std::path::Path| {
            let mut names: Vec<String> = std::fs::read_dir(directory)
                .expect("directory")
                .map(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .into_string()
                        .expect("name")
                })
                .collect();
            names.sort();
            names
        };
        let workspace_root = tempdir().expect("workspace root");
        let elsewhere = tempdir().expect("directory outside the workspaces");
        std::fs::write(elsewhere.path().join("kept"), b"kept").expect("file outside");
        let baseline_root = workspace_root.path().join(".substrate-git-baselines");
        let stores = baseline_root.join(".objects");
        for name in ["ws_live", "ws_gone", ".left"] {
            let pack = stores.join(name).join("pack");
            std::fs::create_dir_all(&pack).expect("store");
            std::fs::write(pack.join("pack-0.pack"), name).expect("pack");
        }
        for name in ["ws_live", "ws_gone", ".tmpleft"] {
            std::fs::write(baseline_root.join(name), name).expect("commit-id file");
        }
        for staging in [".store-left/pack", ".copy-left/ab"] {
            std::fs::create_dir_all(baseline_root.join(staging)).expect("staging directory");
            std::fs::write(baseline_root.join(staging).join("file"), b"left").expect("file");
        }
        std::os::unix::fs::symlink(elsewhere.path(), stores.join("ws_link")).expect("link");
        std::os::unix::fs::symlink(elsewhere.path(), baseline_root.join("ws_alias")).expect("link");
        std::fs::create_dir(workspace_root.path().join("ws_live")).expect("live workspace");

        super::reconcile(workspace_root.path(), &baseline_root).expect("reconcile");
        assert_eq!(names(&baseline_root), [".objects", "ws_live"]);
        assert_eq!(names(&stores), ["ws_live"]);
        assert_eq!(
            std::fs::read(baseline_root.join("ws_live")).expect("commit-id file"),
            b"ws_live"
        );
        assert_eq!(
            std::fs::read(stores.join("ws_live/pack/pack-0.pack")).expect("pack"),
            b"ws_live"
        );
        assert!(workspace_root.path().join("ws_live").is_dir());
        assert_eq!(names(elsewhere.path()), ["kept"]);
    }

    mod adversary_cases;
    mod store_cases;
    pub(super) mod watch;
}
