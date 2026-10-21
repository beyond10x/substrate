//! The host driver on an operating system other than Linux: it compiles, and it refuses to open.
//!
//! The Linux driver's isolation floor is kernel interfaces no other system offers in the same
//! form, and invariant 3 forbids serving a weaker floor under the same name. So [`HostDriver::open`]
//! returns [`PLATFORM_UNSERVED`](crate::PLATFORM_UNSERVED) before it touches the host, and no value
//! of this type can exist: every method below is statically unreachable.

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use substrate_wire::{
    CapabilitySnapshot, ExecOutputQuery, ExecSignalInput, ExecStartInput, FileAbsence,
    FileObservation, FileReadQuery, FileReadResult, LeaseObservation, OutputSlice, PtyWindow,
    Workspace, WorkspaceCreateInput,
};

use crate::{
    DispatchOutcome, Driver, DriverError, ExecObservation, HostConfig, PLATFORM_UNSERVED,
    PipeFrame, WorkspaceDestroyProgress,
};

/// Uninhabited off Linux: [`HostDriver::open`] is the only constructor, and it always refuses.
pub struct HostDriver {
    never: Infallible,
}

#[allow(clippy::unused_async)] // The signatures mirror the Linux driver, whose methods await.
impl HostDriver {
    /// Refuses by name before touching the host.
    ///
    /// # Errors
    ///
    /// Always `host.platform-unserved`.
    pub fn open(_config: HostConfig) -> Result<Arc<Self>, DriverError> {
        Err(DriverError::unserved(
            PLATFORM_UNSERVED,
            format!(
                "The substrate host driver needs Linux; this host runs {}.",
                std::env::consts::OS
            ),
            "host.platform",
        ))
    }

    /// # Errors
    ///
    /// Unreachable: no driver exists off Linux.
    pub fn workspace_path(&self, _root_name: &str) -> Result<PathBuf, DriverError> {
        match self.never {}
    }

    pub fn root(&self) -> &Path {
        match self.never {}
    }

    pub async fn start_pipe_session(
        &self,
        _id: &str,
        _workspace_root_name: &str,
        _input: &substrate_wire::PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        match self.never {}
    }

    /// # Errors
    ///
    /// Unreachable: no driver exists off Linux.
    pub async fn write_pipe_session(&self, _id: &str, _bytes: &[u8]) -> Result<(), DriverError> {
        match self.never {}
    }

    /// # Errors
    ///
    /// Unreachable: no driver exists off Linux.
    pub async fn read_pipe_session(
        &self,
        _id: &str,
        _timeout: std::time::Duration,
    ) -> Result<Option<PipeFrame>, DriverError> {
        match self.never {}
    }

    /// # Errors
    ///
    /// Unreachable: no driver exists off Linux.
    pub async fn close_pipe_session_input(&self, _id: &str) -> Result<(), DriverError> {
        match self.never {}
    }

    /// # Errors
    ///
    /// Unreachable: no driver exists off Linux.
    pub fn resize_pty_session(&self, _id: &str, _window: PtyWindow) -> Result<(), DriverError> {
        match self.never {}
    }
}

#[async_trait]
impl Driver for HostDriver {
    fn machine(&self) -> CapabilitySnapshot {
        match self.never {}
    }

    async fn shutdown(&self) -> Result<(), DriverError> {
        match self.never {}
    }

    fn workspace_root_identity(&self, _id: &str) -> Result<String, DriverError> {
        match self.never {}
    }

    async fn create_workspace(
        &self,
        _id: &str,
        _root_name: &str,
        _input: &WorkspaceCreateInput,
    ) -> DispatchOutcome<Workspace> {
        match self.never {}
    }

    async fn observe_workspace(
        &self,
        _id: &str,
        _root_name: &str,
        _previous: &Workspace,
    ) -> Result<Workspace, DriverError> {
        match self.never {}
    }

    async fn read_workspace_path(
        &self,
        _workspace_id: &str,
        _root_name: &str,
        _path: &str,
        _query: &FileReadQuery,
    ) -> Result<FileReadResult, DriverError> {
        match self.never {}
    }

    async fn write_workspace_file(
        &self,
        _workspace_id: &str,
        _root_name: &str,
        _path: &str,
        _content: &[u8],
    ) -> Result<FileObservation, DriverError> {
        match self.never {}
    }

    async fn delete_workspace_file(
        &self,
        _workspace_id: &str,
        _root_name: &str,
        _path: &str,
    ) -> Result<FileAbsence, DriverError> {
        match self.never {}
    }

    async fn destroy_workspace(
        &self,
        _workspace_id: &str,
        _root_name: &str,
    ) -> Result<WorkspaceDestroyProgress, DriverError> {
        match self.never {}
    }

    async fn start_exec(
        &self,
        _id: &str,
        _workspace_root_name: &str,
        _input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        match self.never {}
    }

    async fn observe_exec(&self, _id: &str) -> Result<ExecObservation, DriverError> {
        match self.never {}
    }

    async fn output(
        &self,
        _id: &str,
        _query: &ExecOutputQuery,
    ) -> Result<OutputSlice, DriverError> {
        match self.never {}
    }

    async fn signal(
        &self,
        _id: &str,
        _input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        match self.never {}
    }

    fn completed_execs(&self) -> Vec<ExecObservation> {
        match self.never {}
    }

    fn set_exec_lease(&self, _id: &str, _lease: Option<LeaseObservation>) {
        match self.never {}
    }

    fn acknowledge_exec(&self, _persisted: &ExecObservation) {
        match self.never {}
    }

    fn discard_superseded_exec(&self, _id: &str) {
        match self.never {}
    }
}

#[cfg(test)]
mod tests {
    use crate::{DriverErrorClass, HostConfig, HostDriver, PLATFORM_UNSERVED};

    #[test]
    fn open_refuses_by_name_and_writes_nothing() {
        let parent = tempfile::tempdir().expect("temporary directory");
        let root = parent.path().join("workspaces");
        let Err(error) = HostDriver::open(HostConfig::minimum(&root)) else {
            panic!("the host driver opened off Linux");
        };
        assert_eq!(error.code, PLATFORM_UNSERVED);
        assert_eq!(error.class, DriverErrorClass::Unserved);
        assert_eq!(error.address.as_deref(), Some("host.platform"));
        assert!(!root.exists(), "a refused open created the workspace root");
    }
}
