#![allow(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod fs;
mod probe;
mod process;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use substrate_wire::{
    CapabilitySnapshot, ExecOutputQuery, ExecSignalInput, ExecStartInput, FileAbsence,
    FileObservation, FileReadQuery, FileReadResult, OutputSlice, Workspace, WorkspaceAbsence,
    WorkspaceCreateInput,
};
use thiserror::Error;

pub use process::ExecObservation;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub workspace_root: PathBuf,
    pub cgroup_root: Option<PathBuf>,
    pub bubblewrap: PathBuf,
    pub config_generation: u64,
    pub max_file_bytes: u64,
    pub read_limit_bytes: u64,
    pub list_limit_items: u32,
    pub output_limit_bytes: u64,
    pub max_concurrent_execs: usize,
    pub max_tracked_execs: usize,
}

impl HostConfig {
    pub fn minimum(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            cgroup_root: None,
            bubblewrap: PathBuf::from("/usr/bin/bwrap"),
            config_generation: 1,
            max_file_bytes: substrate_wire::MAX_FILE_BYTES,
            read_limit_bytes: substrate_wire::MAX_IO_BYTES,
            list_limit_items: substrate_wire::MAX_LIST_ITEMS,
            output_limit_bytes: substrate_wire::MAX_IO_BYTES,
            max_concurrent_execs: 16,
            max_tracked_execs: 128,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverErrorClass {
    Refused,
    Conflict,
    Unserved,
    Exhausted,
    Failed,
    NotFound,
}

#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct DriverError {
    pub class: DriverErrorClass,
    pub code: &'static str,
    pub message: String,
    pub address: Option<String>,
    pub retriable: bool,
}

impl DriverError {
    pub fn refused(code: &'static str, message: impl Into<String>, address: &str) -> Self {
        Self {
            class: DriverErrorClass::Refused,
            code,
            message: message.into(),
            address: Some(address.to_owned()),
            retriable: false,
        }
    }

    pub fn not_found() -> Self {
        Self {
            class: DriverErrorClass::NotFound,
            code: "resource.not-found",
            message: "Resource was not found.".to_owned(),
            address: Some("resource".to_owned()),
            retriable: false,
        }
    }

    pub fn unserved(code: &'static str, message: impl Into<String>, address: &str) -> Self {
        Self {
            class: DriverErrorClass::Unserved,
            code,
            message: message.into(),
            address: Some(address.to_owned()),
            retriable: false,
        }
    }

    pub fn exhausted(code: &'static str, message: impl Into<String>, address: &str) -> Self {
        Self {
            class: DriverErrorClass::Exhausted,
            code,
            message: message.into(),
            address: Some(address.to_owned()),
            retriable: true,
        }
    }

    pub fn failed(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            class: DriverErrorClass::Failed,
            code,
            message: message.into(),
            address: None,
            retriable: true,
        }
    }
}

#[async_trait]
pub trait Driver: Send + Sync {
    fn machine(&self) -> CapabilitySnapshot;

    async fn create_workspace(
        &self,
        id: &str,
        input: &WorkspaceCreateInput,
    ) -> Result<(String, Workspace), DriverError>;

    async fn observe_workspace(
        &self,
        id: &str,
        root_name: &str,
        previous: &Workspace,
    ) -> Result<Workspace, DriverError>;

    async fn read_workspace_path(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<FileReadResult, DriverError>;

    async fn write_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        content: &[u8],
    ) -> Result<FileObservation, DriverError>;

    async fn delete_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
    ) -> Result<FileAbsence, DriverError>;

    async fn destroy_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<WorkspaceAbsence, DriverError>;

    async fn start_exec(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> Result<ExecObservation, DriverError>;

    async fn observe_exec(&self, id: &str) -> Result<ExecObservation, DriverError>;

    async fn output(&self, id: &str, query: &ExecOutputQuery) -> Result<OutputSlice, DriverError>;

    async fn signal(
        &self,
        id: &str,
        input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError>;
}

pub struct HostDriver {
    config: HostConfig,
    filesystem: fs::GuardedFilesystem,
    capability: CapabilitySnapshot,
    processes: process::ProcessRuntime,
}

impl HostDriver {
    /// Opens the guarded filesystem, probes the host, and reconciles orphaned exec cgroups.
    ///
    /// # Errors
    ///
    /// Returns a typed driver error if the workspace root or reconciliation cannot be secured.
    pub fn open(config: HostConfig) -> Result<Arc<Self>, DriverError> {
        std::fs::create_dir_all(&config.workspace_root).map_err(|error| {
            DriverError::failed("workspace.root-failed", format!("workspace root: {error}"))
        })?;
        let filesystem = fs::GuardedFilesystem::open(
            &config.workspace_root,
            config.max_file_bytes,
            config.read_limit_bytes,
            config.list_limit_items,
        )?;
        let capability = probe::probe(&config, filesystem.openat2_available());
        let processes = process::ProcessRuntime::new(config.clone(), capability.clone())?;
        Ok(Arc::new(Self {
            config,
            filesystem,
            capability,
            processes,
        }))
    }

    /// Resolves a validated internal workspace-root identity.
    ///
    /// # Errors
    ///
    /// Returns a refusal when the identity could escape the configured root.
    pub fn workspace_path(&self, root_name: &str) -> Result<PathBuf, DriverError> {
        if !root_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(DriverError::refused(
                "workspace.path-escape",
                "Workspace root identity is invalid.",
                "workspace",
            ));
        }
        Ok(self.config.workspace_root.join(root_name))
    }

    pub fn root(&self) -> &Path {
        &self.config.workspace_root
    }
}

#[async_trait]
impl Driver for HostDriver {
    fn machine(&self) -> CapabilitySnapshot {
        self.capability.clone()
    }

    async fn create_workspace(
        &self,
        id: &str,
        input: &WorkspaceCreateInput,
    ) -> Result<(String, Workspace), DriverError> {
        if !input.source.is_empty() {
            return Err(DriverError::unserved(
                "workspace.source-unserved",
                "Phase 2 serves only empty workspaces.",
                "workspace.git",
            ));
        }
        self.filesystem.create_workspace(id)?;
        Ok((
            id.to_owned(),
            Workspace {
                id: id.to_owned(),
                kind: substrate_wire::WorkspaceKind::Workspace,
                labels: input.labels.clone(),
                observed_at: Utc::now(),
                state: substrate_wire::WorkspaceState::Ready,
            },
        ))
    }

    async fn observe_workspace(
        &self,
        id: &str,
        root_name: &str,
        previous: &Workspace,
    ) -> Result<Workspace, DriverError> {
        self.filesystem.observe_workspace(root_name)?;
        let mut result = previous.clone();
        id.clone_into(&mut result.id);
        result.state = substrate_wire::WorkspaceState::Ready;
        result.observed_at = Utc::now();
        Ok(result)
    }

    async fn read_workspace_path(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<FileReadResult, DriverError> {
        self.filesystem.read(workspace_id, root_name, path, query)
    }

    async fn write_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        content: &[u8],
    ) -> Result<FileObservation, DriverError> {
        self.filesystem
            .write_atomic(workspace_id, root_name, path, content)
    }

    async fn delete_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
    ) -> Result<FileAbsence, DriverError> {
        self.filesystem.delete_file(workspace_id, root_name, path)
    }

    async fn destroy_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<WorkspaceAbsence, DriverError> {
        self.filesystem.destroy_workspace(root_name)?;
        Ok(WorkspaceAbsence {
            kind: substrate_wire::WorkspaceKind::Workspace,
            id: workspace_id.to_owned(),
            absent: true,
            observed_at: Utc::now(),
        })
    }

    async fn start_exec(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> Result<ExecObservation, DriverError> {
        let workspace = self.workspace_path(workspace_root_name)?;
        self.processes.start(id, &workspace, input).await
    }

    async fn observe_exec(&self, id: &str) -> Result<ExecObservation, DriverError> {
        self.processes.observe(id)
    }

    async fn output(&self, id: &str, query: &ExecOutputQuery) -> Result<OutputSlice, DriverError> {
        self.processes.output(id, query)
    }

    async fn signal(
        &self,
        id: &str,
        input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        self.processes.signal(id, input).await
    }
}
