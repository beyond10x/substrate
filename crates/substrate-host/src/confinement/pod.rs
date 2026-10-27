//! The `pod` adapter: one Kubernetes Pod per run (ADR 0109 decision 3). A stub in this tree:
//! `open` answers `confinement.adapter-unserved` until `story:pod-adapter` replaces this file.

use std::path::PathBuf;
use std::sync::Arc;

use super::{AdapterName, ConfinementAdapter, ConfinementError};

/// What opening `pod` takes. The adapter story adds what it needs here.
#[derive(Debug, Clone)]
pub struct PodRequest {
    /// The cluster configuration the operator provides; absent means no cluster.
    pub cluster: Option<PathBuf>,
}

pub(crate) fn open(_request: PodRequest) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    super::unserved(
        AdapterName::Pod,
        "the pod adapter is not served by this build",
    )
}
