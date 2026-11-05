//! The `pod` adapter in a build without the `kubernetes` feature
//! (`story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes`).
//!
//! The build has no cluster client, so the adapter never opens: every open is refused
//! `confinement.adapter-unserved` naming `pod` and the feature, whatever the request names. The
//! client's `Api` is a type with no values, so no [`PodAdapter`] exists and what only a run reaches
//! needs no answer. Nothing here is public, so the module's public items are the same in every
//! build.

use std::sync::Arc;

use substrate_wire::Signal;

use super::{
    AdapterName, AdmittedProfile, Api, Command, ConfinementError, PodAdapter, PodRequest, PodRun,
    PodTiming, RunId,
};

/// What `pod.rs` names of `b10x-substrate-kubernetes-client`.
pub(super) mod kubernetes {
    /// The client: no value of it exists in this build.
    pub(crate) enum Api {}

    /// The client's `RUN_CONTAINER`, the one container of a run's Pod.
    pub(crate) const RUN_CONTAINER: &str = "run";
}

/// Why `pod` does not open in this build.
const UNSERVED: &str = "this build of b10x-substrate-host has no `kubernetes` feature, so it has \
                        no cluster client";

pub(super) fn open_cluster(
    _request: PodRequest,
    _timing: PodTiming,
) -> Result<Arc<PodAdapter>, ConfinementError> {
    Err(ConfinementError::AdapterUnserved {
        adapter: AdapterName::Pod,
        detail: UNSERVED.to_owned(),
    })
}

pub(super) async fn read_log(api: &Api, _run: &PodRun) -> Result<(Vec<u8>, bool), String> {
    match api.client {}
}

pub(super) async fn request_stop(
    api: &Api,
    _run: &PodRun,
    _signal: Signal,
    _grace_seconds: u64,
) -> Result<(), String> {
    match api.client {}
}

impl PodAdapter {
    #[allow(clippy::unused_async)] // The signature is `pod.rs`'s, which `start` awaits.
    pub(super) async fn start_run(
        &self,
        _admitted: &AdmittedProfile,
        _command: &Command,
    ) -> Result<RunId, ConfinementError> {
        match self.api.client {}
    }
}
