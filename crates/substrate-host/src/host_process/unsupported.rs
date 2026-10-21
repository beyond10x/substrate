//! The host-process supervisor on an operating system other than Linux: it compiles, and it
//! refuses to open.
//!
//! The supervisor holds every process in a delegated cgroup v2 subtree and pins it with a pidfd;
//! neither exists elsewhere. [`HostProcessSupervisor::open`] therefore returns
//! [`HOST_UNSERVED`] before it touches the host, and no value of this type can exist.

use std::convert::Infallible;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::{
    HOST_UNSERVED, HostProcessEnd, HostProcessObservation, HostProcessSpec, HostProcessStarted,
    LogStreamSelection, LogTail, StopCause,
};
use crate::DriverError;

/// Uninhabited off Linux: [`HostProcessSupervisor::open`] is the only constructor, and it always
/// refuses.
#[derive(Debug)]
pub struct HostProcessSupervisor {
    never: Infallible,
}

#[allow(clippy::unused_async)] // The signatures mirror the Linux supervisor, whose methods await.
impl HostProcessSupervisor {
    /// Refuses by name before touching the host.
    ///
    /// # Errors
    ///
    /// Always `process.host-unserved`.
    pub fn open(_cgroup_root: Option<&Path>, _state_dir: &Path) -> Result<Arc<Self>, DriverError> {
        Err(DriverError::unserved(
            HOST_UNSERVED,
            format!(
                "Host processes need a Linux cgroup v2 root; this host runs {}.",
                std::env::consts::OS
            ),
            "process.host",
        ))
    }

    /// # Errors
    ///
    /// Unreachable: no supervisor exists off Linux.
    pub fn spawn(
        self: &Arc<Self>,
        _id: &str,
        _spec: &HostProcessSpec,
    ) -> Result<HostProcessStarted, DriverError> {
        match self.never {}
    }

    /// # Errors
    ///
    /// Unreachable: no supervisor exists off Linux.
    pub async fn stop(
        &self,
        _id: &str,
        _cause: StopCause,
        _grace: Duration,
    ) -> Result<HostProcessEnd, DriverError> {
        match self.never {}
    }

    /// # Errors
    ///
    /// Unreachable: no supervisor exists off Linux.
    pub async fn wait(&self, _id: &str) -> Result<HostProcessEnd, DriverError> {
        match self.never {}
    }

    /// # Errors
    ///
    /// Unreachable: no supervisor exists off Linux.
    pub fn ended(&self, _id: &str) -> Result<Option<HostProcessEnd>, DriverError> {
        match self.never {}
    }

    /// # Errors
    ///
    /// Unreachable: no supervisor exists off Linux.
    pub fn observe(&self, _id: &str) -> Result<HostProcessObservation, DriverError> {
        match self.never {}
    }

    /// # Errors
    ///
    /// Unreachable: no supervisor exists off Linux.
    pub fn log_tail(
        &self,
        _id: &str,
        _selection: LogStreamSelection,
        _max_bytes: u64,
    ) -> Result<LogTail, DriverError> {
        match self.never {}
    }

    /// # Errors
    ///
    /// Unreachable: no supervisor exists off Linux.
    pub fn forget(&self, _id: &str) -> Result<(), DriverError> {
        match self.never {}
    }
}

#[cfg(test)]
mod tests {
    use super::{HOST_UNSERVED, HostProcessSupervisor};
    use crate::DriverErrorClass;

    #[test]
    fn open_refuses_by_name_and_writes_nothing() {
        let parent = tempfile::tempdir().expect("temporary directory");
        let state = parent.path().join("state");
        let Err(error) = HostProcessSupervisor::open(Some(parent.path()), &state) else {
            panic!("the host-process supervisor opened off Linux");
        };
        assert_eq!(error.code, HOST_UNSERVED);
        assert_eq!(error.class, DriverErrorClass::Unserved);
        assert!(
            !state.exists(),
            "a refused open created the state directory"
        );
    }
}
