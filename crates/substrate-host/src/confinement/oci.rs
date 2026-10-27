//! The `oci` adapter: Docker or podman, an image by digest (ADR 0109 decisions 3 and 5). A stub in
//! this tree: `open` answers `confinement.adapter-unserved` until `story:oci-adapter` replaces this
//! file.

use std::path::PathBuf;
use std::sync::Arc;

use super::{AdapterName, ConfinementAdapter, ConfinementError};

/// What opening `oci` takes. The adapter story adds what it needs here.
#[derive(Debug, Clone)]
pub struct OciRequest {
    /// The container runtime program, `docker` or `podman`.
    pub runtime: PathBuf,
}

pub(crate) fn open(_request: OciRequest) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    super::unserved(
        AdapterName::Oci,
        "the oci adapter is not served by this build",
    )
}
