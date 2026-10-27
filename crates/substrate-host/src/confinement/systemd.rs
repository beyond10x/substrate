//! The `systemd` adapter: a delegated user scope as the bounds provider around `local` or `linux`
//! (ADR 0109 decision 3). A stub in this tree: `open` answers `confinement.adapter-unserved` until
//! `story:systemd-scope-bounds-provider` replaces this file. It composes through
//! [`LocalRequest`] and [`LinuxRequest`], whose `cgroup_root` the scope becomes.

use std::sync::Arc;

use super::linux::LinuxRequest;
use super::local::LocalRequest;
use super::{AdapterName, ConfinementAdapter, ConfinementError, InnerAdapter};

/// The adapter `systemd` wraps, as it would be opened on its own.
#[derive(Debug, Clone)]
pub enum InnerRequest {
    Local(LocalRequest),
    Linux(Box<LinuxRequest>),
}

impl InnerRequest {
    pub const fn inner(&self) -> InnerAdapter {
        match self {
            Self::Local(_) => InnerAdapter::Local,
            Self::Linux(_) => InnerAdapter::Linux,
        }
    }
}

/// What opening `systemd` takes. The adapter story adds what it needs here.
#[derive(Debug, Clone)]
pub struct SystemdRequest {
    pub inner: InnerRequest,
}

pub(crate) fn open(
    _request: SystemdRequest,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    super::unserved(
        AdapterName::Systemd,
        "the systemd adapter is not served by this build",
    )
}
