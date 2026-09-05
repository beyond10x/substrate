use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use base64::Engine as _;
use chrono::Utc;
use git2::{Delta, DiffFile, DiffOptions, Oid, Patch};
use sha2::{Digest as _, Sha256};
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

pub(super) fn changes(
    workspace_id: &str,
    workspace: &Path,
    baseline_root: &Path,
    root_name: &str,
    query: GitChangesQuery,
) -> Result<GitChangeSet, DriverError> {
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
    let mut options = DiffOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .include_typechange(true)
        .ignore_submodules(true)
        .max_size(i64::try_from(query.max_file_bytes).expect("public I/O bound fits i64"));
    let diff = repository
        .diff_tree_to_workdir_with_index(Some(&tree), Some(&mut options))
        .map_err(|_| failed("workspace.git-diff-failed"))?;
    let observed_count = diff.deltas().count();
    let item_limit = usize::try_from(query.max_files).expect("public list bound fits usize");
    let mut items = Vec::with_capacity(observed_count.min(item_limit));
    let mut returned_bytes = 0_u64;
    for (index, delta) in diff.deltas().take(item_limit).enumerate() {
        let status = change_status(delta.status())?;
        let baseline_file = delta.old_file();
        let current_file = delta.new_file();
        let baseline = change_side(&baseline_file)?;
        let current = change_side(&current_file)?;
        let binary = delta.old_file().is_binary() || delta.new_file().is_binary();
        let mut patch = UnifiedDiff {
            text: String::new(),
            truncated: false,
            binary,
        };
        if !binary {
            let patch_bytes = match Patch::from_diff(&diff, index)
                .map_err(|_| failed("workspace.git-diff-failed"))?
            {
                Some(mut value) => value
                    .to_buf()
                    .map_err(|_| failed("workspace.git-diff-failed"))?
                    .to_vec(),
                None => Vec::new(),
            };
            let remaining = query.max_total_bytes.saturating_sub(returned_bytes);
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
            status,
            baseline,
            current,
            patch,
        });
    }
    items.sort_by(|left, right| {
        change_sort_path(left)
            .as_bytes()
            .cmp(change_sort_path(right).as_bytes())
    });
    let truncated = observed_count > item_limit || items.iter().any(|item| item.patch.truncated);
    Ok(GitChangeSet {
        workspace: workspace_id.to_owned(),
        commit,
        items,
        returned_bytes,
        truncated,
        observed_at: Utc::now(),
    })
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

fn change_status(status: Delta) -> Result<GitChangeStatus, DriverError> {
    match status {
        Delta::Added => Ok(GitChangeStatus::Added),
        Delta::Deleted => Ok(GitChangeStatus::Deleted),
        Delta::Modified => Ok(GitChangeStatus::Modified),
        Delta::Renamed => Ok(GitChangeStatus::Renamed),
        Delta::Copied => Ok(GitChangeStatus::Copied),
        Delta::Untracked => Ok(GitChangeStatus::Untracked),
        Delta::Typechange => Ok(GitChangeStatus::TypeChanged),
        Delta::Conflicted => Ok(GitChangeStatus::Conflicted),
        Delta::Unmodified | Delta::Ignored | Delta::Unreadable => {
            Err(failed("workspace.git-diff-state-invalid"))
        }
    }
}

fn change_side(file: &DiffFile<'_>) -> Result<Option<GitChangeSide>, DriverError> {
    if !file.exists() {
        return Ok(None);
    }
    let path = file
        .path()
        .and_then(Path::to_str)
        .ok_or_else(|| failed("workspace.git-path-unrepresentable"))?;
    validate_git_path(path)?;
    let mode = match file.mode() {
        git2::FileMode::Unreadable => 0,
        git2::FileMode::Tree => 0o040_000,
        git2::FileMode::Blob | git2::FileMode::BlobGroupWritable => 0o100_644,
        git2::FileMode::BlobExecutable => 0o100_755,
        git2::FileMode::Link => 0o120_000,
        git2::FileMode::Commit => 0o160_000,
    };
    Ok(Some(GitChangeSide {
        path: path.to_owned(),
        mode,
        size: file.size(),
        oid: file.is_valid_id().then(|| file.id().to_string()),
    }))
}

fn change_sort_path(change: &GitChange) -> &str {
    change
        .current
        .as_ref()
        .or(change.baseline.as_ref())
        .map(|side| side.path.as_str())
        .expect("a diff delta has at least one side")
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
    use substrate_wire::{GitChangeStatus, GitChangesQuery, GitSource, StorageLimit};
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
