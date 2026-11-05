//! The `git` module of a build without the `git` feature
//! (`story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes`).
//!
//! Every entry point the driver calls in `git.rs` is here with the same signature. The probe
//! proves no Git mechanism, so no `workspace.git` fact is published, and the driver refuses every
//! Git source `workspace.git-source-unserved` before dispatch ([`SERVED`]). What it reaches after
//! that refusal answers the same refusal; what tidies Git state does nothing, because this
//! build writes no Git workspace and no materialization baseline, except that a baseline an
//! earlier build with `git` wrote is still removed with its workspace. Invariant 3: a missing
//! mechanism is a named refusal, never a degraded Git workspace.

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

#[allow(clippy::unnecessary_wraps)] // The signature is `git.rs`'s, which can fail.
pub(super) fn reconcile(_workspace_root: &Path, _baseline_root: &Path) -> Result<(), DriverError> {
    Ok(())
}

pub(super) fn write_baseline(
    _baseline_root: &Path,
    _root_name: &str,
    _commit: &str,
) -> Result<(), DriverError> {
    Err(source_unserved())
}

/// Removes a baseline a build with `git` wrote, as `git.rs` does: forgetting or destroying a
/// workspace removes its baseline whichever build does it, so a later build with `git` never reads
/// a same-named workspace as materialized from Git.
pub(super) fn remove_baseline(baseline_root: &Path, root_name: &str) -> Result<(), DriverError> {
    match std::fs::remove_file(baseline_root.join(root_name)) {
        Ok(()) => std::fs::File::open(baseline_root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| removal_failed()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(removal_failed()),
    }
}

fn removal_failed() -> DriverError {
    DriverError::failed(
        "workspace.git-baseline-remove-failed",
        "Git workspace materialization failed.",
    )
}

pub(super) fn baseline_file(
    _workspace_id: &str,
    _workspace: &Path,
    _baseline_root: &Path,
    _root_name: &str,
    _path: &str,
    _max_bytes: u64,
) -> Result<GitBaselineFileResult, DriverError> {
    Err(source_unserved())
}

pub(super) fn changes(
    _workspace_id: &str,
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
