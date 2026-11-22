//! The `git` module of a build without the `git` feature
//! (`story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes`).
//!
//! Every entry point the driver calls in `git.rs` is here with the same signature. The probe
//! proves no Git mechanism, so no `workspace.git` fact is published, and the driver refuses every
//! Git source `workspace.git-source-unserved` before dispatch ([`SERVED`]). What it reaches after
//! that refusal answers the same refusal. This build writes no Git workspace, no commit-id file
//! and no baseline store, and removes no staging tree beside the workspaces. What an earlier
//! build with `git` wrote in the baseline root still goes as in that build (`git_baselines.rs`),
//! wherever the driver asks for it: with its workspace, and at the start of a driver with a Git
//! source configured when it belongs to no workspace directory that exists.
//! Invariant 3: a missing mechanism is a named refusal, never a degraded Git workspace.

use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use substrate_wire::{
    GitBaselineFileResult, GitChangeSet, GitChangesQuery, GitSource, StorageLimit, StorageUsage,
};
use zeroize::Zeroizing;

use crate::{DriverError, GitSourceBinding};

/// Whether this build serves a Git source at all; `git.rs` holds `true`.
pub(super) const SERVED: bool = false;

/// The refusal of every Git source in this build: the code a host without the source answers,
/// with a message that names the missing feature. The driver answers it before dispatch too.
pub(super) fn source_unserved() -> DriverError {
    DriverError::unserved(
        "workspace.git-source-unserved",
        "This build of the host driver has no `git` feature, so it serves no Git source.",
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

pub(super) fn materialize(
    _target: &Path,
    _locator: &url::Url,
    _binding: &GitSourceBinding,
    _source: &GitSource,
    _authority: &Zeroizing<String>,
    _limit: StorageLimit,
    _interrupt: &Arc<AtomicBool>,
) -> Result<StorageUsage, DriverError> {
    Err(source_unserved())
}

pub(super) fn mechanism_is_provable(_workspace_root: &Path) -> bool {
    false
}

/// The start rule of the baseline root, as `git.rs` applies it. The staging trees of Git
/// creates beside the workspaces are left: this build makes none.
pub(super) fn reconcile(workspace_root: &Path, baseline_root: &Path) -> Result<(), DriverError> {
    crate::git_baselines::reconcile(workspace_root, baseline_root)
}

pub(super) fn write_baseline(
    _baseline_root: &Path,
    _root_name: &str,
    _repository: &Path,
    _commit: &str,
    _limit: StorageLimit,
) -> Result<(), DriverError> {
    Err(source_unserved())
}

/// Removes the commit-id file and the store a build with `git` wrote, as `git.rs` does:
/// forgetting or destroying a workspace removes both whichever build does it, so a later build
/// with `git` never reads a same-named workspace as materialized from Git.
pub(super) fn remove_baseline(baseline_root: &Path, root_name: &str) -> Result<(), DriverError> {
    crate::git_baselines::remove(baseline_root, root_name)
}

pub(super) fn baseline_file(
    _workspace_id: &str,
    _baseline_root: &Path,
    _root_name: &str,
    _path: &str,
    _max_bytes: u64,
) -> Result<GitBaselineFileResult, DriverError> {
    Err(source_unserved())
}

pub(super) fn changes(
    _workspace_id: &str,
    _root: &OwnedFd,
    _workspace: &Path,
    _baseline_root: &Path,
    _root_name: &str,
    _query: GitChangesQuery,
) -> Result<GitChangeSet, DriverError> {
    Err(source_unserved())
}

/// No workspace of this build was materialized from Git, so none holds a baseline.
#[allow(clippy::unnecessary_wraps)] // The signature is `git.rs`'s, which can fail.
pub(crate) fn has_baseline(_baseline_root: &Path, _root_name: &str) -> Result<bool, DriverError> {
    Ok(false)
}

pub(super) fn sync_workspace_root(_root: &Path) -> Result<(), DriverError> {
    Err(source_unserved())
}
