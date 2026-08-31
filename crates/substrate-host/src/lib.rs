#![allow(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod egress;
mod fs;
mod probe;
mod process;
mod seccomp;
mod secrets;

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use sha2::{Digest as _, Sha256};
use substrate_wire::{
    CapabilitySnapshot, DigestedFileSlice, ExecOutputQuery, ExecSignalInput, ExecStartInput,
    FileAbsence, FileEditInput, FileMutationResult, FileObservation, FilePatchInput, FileReadQuery,
    FileReadResult, FileReplaceInput, LeaseObservation, OutputSlice, Workspace, WorkspaceAbsence,
    WorkspaceCreateInput, WorkspaceTree, WorkspaceTreeQuery,
};
use thiserror::Error;
use tokio::sync::Semaphore;

pub use egress::EgressAperture;
pub use process::{ExecObservation, PipeFrame, PipeStream};

/// One operator-declared secret slot: a name, and the bounded owner-private file behind it
/// (ADR 0012).
///
/// Daemon configuration and never request data. The path never leaves this struct — not into a
/// capability fact, not into an event, not into an error — so rotating the file behind a declared
/// name changes nothing any client can observe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretSlot {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub workspace_root: PathBuf,
    pub capsule_root: PathBuf,
    pub cgroup_root: Option<PathBuf>,
    pub bubblewrap: PathBuf,
    pub config_generation: u64,
    pub max_file_bytes: u64,
    pub read_limit_bytes: u64,
    pub list_limit_items: u32,
    pub output_limit_bytes: u64,
    pub max_concurrent_execs: usize,
    pub max_tracked_execs: usize,
    pub event_retention: u64,
    pub max_current_workspaces: u64,
    pub max_current_execs: u64,
    pub snapshot_provenance_events: u64,
    /// Every slot this operator declared. Empty means the capability is absent, not that slots
    /// degrade to something weaker.
    pub secret_slots: Vec<SecretSlot>,
    /// Every egress aperture this operator declared, each already resolved to a pinned address
    /// (ADR 0013). Empty means the capability is absent and the sandbox keeps `--unshare-net`.
    pub egress_apertures: Vec<EgressAperture>,
    /// The certificate bundle a run with an aperture gets a private read-only copy of.
    ///
    /// `None` means no trust anchor: TLS still crosses the forwarder byte for byte, and a child
    /// that verifies certificates will refuse the connection rather than trust something it cannot
    /// check. Absent and unverifiable, never present and unverified (invariant 3).
    pub ca_bundle: Option<PathBuf>,
}

impl HostConfig {
    pub fn minimum(workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        let generation_digest: [u8; 32] =
            Sha256::digest(workspace_root.as_os_str().as_encoded_bytes()).into();
        let config_generation = u64::from_be_bytes([
            generation_digest[0],
            generation_digest[1],
            generation_digest[2],
            generation_digest[3],
            generation_digest[4],
            generation_digest[5],
            generation_digest[6],
            generation_digest[7],
        ])
        .max(1);
        Self {
            capsule_root: workspace_root.join(".substrate-capsules"),
            workspace_root,
            cgroup_root: None,
            bubblewrap: PathBuf::from("/usr/bin/bwrap"),
            config_generation,
            max_file_bytes: substrate_wire::MAX_FILE_BYTES,
            read_limit_bytes: substrate_wire::MAX_IO_BYTES,
            list_limit_items: substrate_wire::MAX_LIST_ITEMS,
            output_limit_bytes: substrate_wire::MAX_IO_BYTES,
            max_concurrent_execs: 16,
            max_tracked_execs: 128,
            event_retention: 10_000,
            max_current_workspaces: substrate_wire::MAX_CURRENT_WORKSPACES,
            max_current_execs: substrate_wire::MAX_CURRENT_EXECS,
            snapshot_provenance_events: substrate_wire::MAX_SNAPSHOT_PROVENANCE_EVENTS,
            secret_slots: Vec::new(),
            egress_apertures: Vec::new(),
            ca_bundle: None,
        }
    }

    fn aperture_root(&self) -> PathBuf {
        self.workspace_root.join(".substrate-apertures")
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

pub enum DispatchOutcome<T> {
    Observed(T),
    NotDispatched(DriverError),
    ContainedAbsent(DriverError),
    OutcomeUnknown(DriverError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceDestroyProgress {
    /// One bounded cleanup claim completed without yet proving the workspace root absent.
    Pending { removed_items: u64 },
    /// The guarded root descriptor proved that the workspace root is absent.
    Absent(WorkspaceAbsence),
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

    /// Declares the deterministic physical root identity before any workspace mutation.
    ///
    /// # Errors
    ///
    /// Returns a typed refusal when the logical identifier cannot be represented safely.
    fn workspace_root_identity(&self, id: &str) -> Result<String, DriverError>;

    async fn create_workspace(
        &self,
        id: &str,
        root_name: &str,
        input: &WorkspaceCreateInput,
    ) -> DispatchOutcome<Workspace>;

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

    async fn read_workspace_file_v2(
        &self,
        _workspace_id: &str,
        _root_name: &str,
        _path: &str,
        _query: &FileReadQuery,
    ) -> Result<DigestedFileSlice, DriverError> {
        Err(DriverError::unserved(
            "workspace.file-v2-unserved",
            "The active driver does not serve digest-bound file reads.",
            "workspace.file.read-v2",
        ))
    }

    async fn list_workspace_tree_v2(
        &self,
        _workspace_id: &str,
        _root_name: &str,
        _query: &WorkspaceTreeQuery,
    ) -> Result<WorkspaceTree, DriverError> {
        Err(DriverError::unserved(
            "workspace.tree-v2-unserved",
            "The active driver does not serve bounded recursive workspace trees.",
            "workspace.tree.list-v2",
        ))
    }

    async fn replace_workspace_file_v2(
        &self,
        _workspace_id: &str,
        _root_name: &str,
        _path: &str,
        _input: &FileReplaceInput,
    ) -> Result<FileMutationResult, DriverError> {
        Err(DriverError::unserved(
            "workspace.file-v2-unserved",
            "The active driver does not serve compare-and-set replacement.",
            "workspace.file.replace-v2",
        ))
    }

    async fn edit_workspace_file_v2(
        &self,
        _workspace_id: &str,
        _root_name: &str,
        _path: &str,
        _input: &FileEditInput,
    ) -> Result<FileMutationResult, DriverError> {
        Err(DriverError::unserved(
            "workspace.file-v2-unserved",
            "The active driver does not serve structured text edits.",
            "workspace.file.edit-v2",
        ))
    }

    async fn patch_workspace_file_v2(
        &self,
        _workspace_id: &str,
        _root_name: &str,
        _path: &str,
        _input: &FilePatchInput,
    ) -> Result<FileMutationResult, DriverError> {
        Err(DriverError::unserved(
            "workspace.file-v2-unserved",
            "The active driver does not serve line patches.",
            "workspace.file.patch-v2",
        ))
    }

    async fn destroy_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<WorkspaceDestroyProgress, DriverError>;

    async fn start_exec(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation>;

    /// Starts one bounded raw-pipe process. Drivers which do not implement the direct byte plane
    /// refuse explicitly; callers must never fall back to a direct host child.
    async fn start_pipe_session(
        &self,
        _id: &str,
        _workspace_root_name: &str,
        _input: &substrate_wire::PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        DispatchOutcome::NotDispatched(DriverError::unserved(
            "session.unserved",
            "The selected driver does not serve raw-pipe sessions.",
            "session",
        ))
    }

    /// Writes one bounded frame to an admitted raw-pipe process.
    async fn write_pipe_session(&self, _id: &str, _bytes: &[u8]) -> Result<(), DriverError> {
        Err(DriverError::unserved(
            "session.unserved",
            "The selected driver does not serve raw-pipe sessions.",
            "session",
        ))
    }

    /// Reads one attributed raw-pipe output frame before the supplied deadline.
    async fn read_pipe_session(
        &self,
        _id: &str,
        _timeout: std::time::Duration,
    ) -> Result<Option<PipeFrame>, DriverError> {
        Err(DriverError::unserved(
            "session.unserved",
            "The selected driver does not serve raw-pipe sessions.",
            "session",
        ))
    }

    /// Half-closes the admitted raw-pipe process input.
    async fn close_pipe_session_input(&self, _id: &str) -> Result<(), DriverError> {
        Err(DriverError::unserved(
            "session.unserved",
            "The selected driver does not serve raw-pipe sessions.",
            "session",
        ))
    }

    async fn observe_exec(&self, id: &str) -> Result<ExecObservation, DriverError>;

    async fn output(&self, id: &str, query: &ExecOutputQuery) -> Result<OutputSlice, DriverError>;

    async fn signal(
        &self,
        id: &str,
        input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError>;

    fn completed_execs(&self) -> Vec<ExecObservation>;

    fn set_exec_lease(&self, id: &str, lease: Option<LeaseObservation>);

    fn acknowledge_exec(&self, persisted: &ExecObservation);

    fn discard_superseded_exec(&self, id: &str);
}

pub struct HostDriver {
    config: HostConfig,
    filesystem: Arc<fs::GuardedFilesystem>,
    capability: CapabilitySnapshot,
    processes: process::ProcessRuntime,
    blocking_slots: Arc<Semaphore>,
    workspace_destroy_namespace: (libc::dev_t, libc::ino_t),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WorkspaceDestroyKey {
    device: libc::dev_t,
    inode: libc::ino_t,
    name: String,
}

struct WorkspaceDestroyOwnership {
    key: WorkspaceDestroyKey,
}

impl WorkspaceDestroyOwnership {
    fn acquire(
        namespace: (libc::dev_t, libc::ino_t),
        root_name: &str,
    ) -> Result<Self, DriverError> {
        let key = WorkspaceDestroyKey {
            device: namespace.0,
            inode: namespace.1,
            name: root_name.to_owned(),
        };
        {
            let mut owned = workspace_destroy_roots().lock();
            if !owned.insert(key.clone()) {
                return Err(DriverError {
                    class: DriverErrorClass::Conflict,
                    code: "workspace.destroy-busy",
                    message: "Workspace cleanup already has a process-local owner.".to_owned(),
                    address: Some("workspace".to_owned()),
                    retriable: true,
                });
            }
        }
        Ok(Self { key })
    }
}

impl Drop for WorkspaceDestroyOwnership {
    fn drop(&mut self) {
        workspace_destroy_roots().lock().remove(&self.key);
    }
}

fn workspace_destroy_roots() -> &'static Mutex<HashSet<WorkspaceDestroyKey>> {
    static ROOTS: OnceLock<Mutex<HashSet<WorkspaceDestroyKey>>> = OnceLock::new();
    ROOTS.get_or_init(|| Mutex::new(HashSet::new()))
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
        std::fs::create_dir_all(&config.capsule_root).map_err(|error| {
            DriverError::failed("capsule.root-failed", format!("capsule root: {error}"))
        })?;
        std::fs::set_permissions(&config.capsule_root, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| {
                DriverError::failed("capsule.root-failed", format!("capsule root mode: {error}"))
            })?;
        let aperture_root = config.aperture_root();
        std::fs::create_dir_all(&aperture_root).map_err(|error| {
            DriverError::failed("aperture.root-failed", format!("aperture root: {error}"))
        })?;
        std::fs::set_permissions(&aperture_root, std::fs::Permissions::from_mode(0o700)).map_err(
            |error| {
                DriverError::failed(
                    "aperture.root-failed",
                    format!("aperture root mode: {error}"),
                )
            },
        )?;
        let filesystem = Arc::new(fs::GuardedFilesystem::open(
            &config.workspace_root,
            config.max_file_bytes,
            config.read_limit_bytes,
            config.list_limit_items,
        )?);
        let capability = probe::probe(&config, filesystem.openat2_available());
        let processes = process::ProcessRuntime::new(config.clone(), capability.clone())?;
        let workspace_destroy_namespace = filesystem.root_identity()?;
        Ok(Arc::new(Self {
            config,
            filesystem,
            capability,
            processes,
            blocking_slots: Arc::new(Semaphore::new(16)),
            workspace_destroy_namespace,
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

    /// Starts one development raw-pipe session inside the same fail-closed confinement used by
    /// exec. This host-level API precedes the durable public daemon session resource.
    pub async fn start_pipe_session(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &substrate_wire::PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        let workspace = match self.workspace_path(workspace_root_name) {
            Ok(value) => value,
            Err(error) => return DispatchOutcome::NotDispatched(error),
        };
        self.processes.start_pipe(id, &workspace, input).await
    }

    /// Writes one already-bounded stdin frame.
    ///
    /// # Errors
    ///
    /// Refuses missing/non-pipe/closed sessions and frame or total-input overflow; reports write
    /// failures without pretending the bytes were delivered.
    pub async fn write_pipe_session(&self, id: &str, bytes: &[u8]) -> Result<(), DriverError> {
        self.processes.write_pipe(id, bytes).await
    }

    /// Reads one attributed stdout/stderr frame before the supplied deadline.
    ///
    /// # Errors
    ///
    /// Refuses missing/non-pipe sessions and zero deadlines, and reports deadline expiry.
    pub async fn read_pipe_session(
        &self,
        id: &str,
        timeout: std::time::Duration,
    ) -> Result<Option<PipeFrame>, DriverError> {
        self.processes.read_pipe(id, timeout).await
    }

    /// Half-closes stdin without ending output observation.
    ///
    /// # Errors
    ///
    /// Refuses missing resources and execs which were not started as pipe sessions.
    pub async fn close_pipe_session_input(&self, id: &str) -> Result<(), DriverError> {
        self.processes.close_pipe_input(id).await
    }

    async fn filesystem_io<T, F>(&self, operation: F) -> Result<T, DriverError>
    where
        T: Send + 'static,
        F: FnOnce(&fs::GuardedFilesystem) -> Result<T, DriverError> + Send + 'static,
    {
        let permit = Arc::clone(&self.blocking_slots)
            .acquire_owned()
            .await
            .map_err(|_| DriverError::failed("host.blocking-closed", "Blocking I/O is closed."))?;
        let filesystem = Arc::clone(&self.filesystem);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation(&filesystem)
        })
        .await
        .map_err(|error| {
            DriverError::failed(
                "host.blocking-failed",
                format!("Blocking I/O task failed: {error}"),
            )
        })?
    }
}

#[async_trait]
impl Driver for HostDriver {
    fn machine(&self) -> CapabilitySnapshot {
        self.capability.clone()
    }

    fn workspace_root_identity(&self, id: &str) -> Result<String, DriverError> {
        self.workspace_path(id)?;
        Ok(id.to_owned())
    }

    async fn create_workspace(
        &self,
        id: &str,
        root_name: &str,
        input: &WorkspaceCreateInput,
    ) -> DispatchOutcome<Workspace> {
        if !input.source.is_empty() {
            return DispatchOutcome::NotDispatched(DriverError::unserved(
                "workspace.source-unserved",
                "Phase 2 serves only empty workspaces.",
                "workspace.git",
            ));
        }
        let id = id.to_owned();
        let root_name = root_name.to_owned();
        let recovery_root_name = root_name.clone();
        let labels = input.labels.clone();
        let result = self
            .filesystem_io(move |filesystem| {
                filesystem.create_workspace(&root_name)?;
                Ok(Workspace {
                    id,
                    kind: substrate_wire::WorkspaceKind::Workspace,
                    labels,
                    observed_at: Utc::now(),
                    state: substrate_wire::WorkspaceState::Ready,
                    lease: None,
                })
            })
            .await;
        match result {
            Ok(value) => DispatchOutcome::Observed(value),
            Err(error) => {
                let absence = self
                    .filesystem_io(move |filesystem| {
                        filesystem.observe_workspace(&recovery_root_name)
                    })
                    .await;
                if absence.is_err_and(|observed| observed.class == DriverErrorClass::NotFound) {
                    DispatchOutcome::ContainedAbsent(error)
                } else {
                    DispatchOutcome::OutcomeUnknown(error)
                }
            }
        }
    }

    async fn observe_workspace(
        &self,
        id: &str,
        root_name: &str,
        previous: &Workspace,
    ) -> Result<Workspace, DriverError> {
        let id = id.to_owned();
        let root_name = root_name.to_owned();
        let mut result = previous.clone();
        self.filesystem_io(move |filesystem| {
            filesystem.observe_workspace(&root_name)?;
            id.clone_into(&mut result.id);
            result.state = substrate_wire::WorkspaceState::Ready;
            result.observed_at = Utc::now();
            Ok(result)
        })
        .await
    }

    async fn read_workspace_path(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<FileReadResult, DriverError> {
        let workspace_id = workspace_id.to_owned();
        let root_name = root_name.to_owned();
        let path = path.to_owned();
        let query = query.clone();
        self.filesystem_io(move |filesystem| {
            filesystem.read(&workspace_id, &root_name, &path, &query)
        })
        .await
    }

    async fn write_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        content: &[u8],
    ) -> Result<FileObservation, DriverError> {
        let workspace_id = workspace_id.to_owned();
        let root_name = root_name.to_owned();
        let path = path.to_owned();
        let content = content.to_vec();
        self.filesystem_io(move |filesystem| {
            filesystem.write_atomic(&workspace_id, &root_name, &path, &content)
        })
        .await
    }

    async fn delete_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
    ) -> Result<FileAbsence, DriverError> {
        let workspace_id = workspace_id.to_owned();
        let root_name = root_name.to_owned();
        let path = path.to_owned();
        self.filesystem_io(move |filesystem| {
            filesystem.delete_file(&workspace_id, &root_name, &path)
        })
        .await
    }

    async fn read_workspace_file_v2(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<DigestedFileSlice, DriverError> {
        let workspace_id = workspace_id.to_owned();
        let root_name = root_name.to_owned();
        let path = path.to_owned();
        let query = query.clone();
        self.filesystem_io(move |filesystem| {
            filesystem.read_digested(&workspace_id, &root_name, &path, &query)
        })
        .await
    }

    async fn list_workspace_tree_v2(
        &self,
        workspace_id: &str,
        root_name: &str,
        query: &WorkspaceTreeQuery,
    ) -> Result<WorkspaceTree, DriverError> {
        let workspace_id = workspace_id.to_owned();
        let root_name = root_name.to_owned();
        let query = query.clone();
        self.filesystem_io(move |filesystem| {
            filesystem.list_tree(&workspace_id, &root_name, &query)
        })
        .await
    }

    async fn replace_workspace_file_v2(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        input: &FileReplaceInput,
    ) -> Result<FileMutationResult, DriverError> {
        let workspace_id = workspace_id.to_owned();
        let root_name = root_name.to_owned();
        let path = path.to_owned();
        let input = input.clone();
        self.filesystem_io(move |filesystem| {
            filesystem.replace_cas(&workspace_id, &root_name, &path, &input)
        })
        .await
    }

    async fn edit_workspace_file_v2(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        input: &FileEditInput,
    ) -> Result<FileMutationResult, DriverError> {
        let workspace_id = workspace_id.to_owned();
        let root_name = root_name.to_owned();
        let path = path.to_owned();
        let input = input.clone();
        self.filesystem_io(move |filesystem| {
            filesystem.edit_cas(&workspace_id, &root_name, &path, &input)
        })
        .await
    }

    async fn patch_workspace_file_v2(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        input: &FilePatchInput,
    ) -> Result<FileMutationResult, DriverError> {
        let workspace_id = workspace_id.to_owned();
        let root_name = root_name.to_owned();
        let path = path.to_owned();
        let input = input.clone();
        self.filesystem_io(move |filesystem| {
            filesystem.patch_cas(&workspace_id, &root_name, &path, &input)
        })
        .await
    }

    async fn destroy_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<WorkspaceDestroyProgress, DriverError> {
        let workspace_id = workspace_id.to_owned();
        let root_name = root_name.to_owned();
        let workspace_destroy_namespace = self.workspace_destroy_namespace;
        self.filesystem_io(move |filesystem| {
            let _ownership =
                WorkspaceDestroyOwnership::acquire(workspace_destroy_namespace, &root_name)?;
            match filesystem.destroy_workspace_batch(&root_name)? {
                fs::WorkspaceDestroyBatch::Pending { removed_items } => {
                    Ok(WorkspaceDestroyProgress::Pending { removed_items })
                }
                fs::WorkspaceDestroyBatch::Absent => {
                    Ok(WorkspaceDestroyProgress::Absent(WorkspaceAbsence {
                        kind: substrate_wire::WorkspaceKind::Workspace,
                        id: workspace_id,
                        absent: true,
                        observed_at: Utc::now(),
                    }))
                }
            }
        })
        .await
    }

    async fn start_exec(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        let workspace = match self.workspace_path(workspace_root_name) {
            Ok(value) => value,
            Err(error) => return DispatchOutcome::NotDispatched(error),
        };
        self.processes.start(id, &workspace, input).await
    }

    async fn start_pipe_session(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &substrate_wire::PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        HostDriver::start_pipe_session(self, id, workspace_root_name, input).await
    }

    async fn write_pipe_session(&self, id: &str, bytes: &[u8]) -> Result<(), DriverError> {
        HostDriver::write_pipe_session(self, id, bytes).await
    }

    async fn read_pipe_session(
        &self,
        id: &str,
        timeout: std::time::Duration,
    ) -> Result<Option<PipeFrame>, DriverError> {
        HostDriver::read_pipe_session(self, id, timeout).await
    }

    async fn close_pipe_session_input(&self, id: &str) -> Result<(), DriverError> {
        HostDriver::close_pipe_session_input(self, id).await
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

    fn completed_execs(&self) -> Vec<ExecObservation> {
        self.processes.completed()
    }

    fn set_exec_lease(&self, id: &str, lease: Option<LeaseObservation>) {
        self.processes.set_lease(id, lease);
    }

    fn acknowledge_exec(&self, persisted: &ExecObservation) {
        self.processes.acknowledge(persisted);
    }

    fn discard_superseded_exec(&self, id: &str) {
        self.processes.discard_terminal(id);
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{
        Driver as _, HostConfig, HostDriver, WorkspaceDestroyOwnership, WorkspaceDestroyProgress,
        fs,
    };

    #[tokio::test]
    async fn destroy_driver_call_is_one_batch_then_eventually_absent() {
        let directory = tempdir().expect("tempdir");
        let driver = HostDriver::open(HostConfig::minimum(directory.path())).expect("host driver");
        if !driver.filesystem.openat2_available() {
            return;
        }
        driver
            .filesystem
            .create_workspace("ws_batched")
            .expect("workspace");
        let workspace = directory.path().join("ws_batched");
        for index in 0..=fs::DESTROY_BATCH_ITEMS {
            std::fs::write(workspace.join(format!("item-{index:05}")), b"").expect("fixture item");
        }

        let first = driver
            .destroy_workspace("ws_batched", "ws_batched")
            .await
            .expect("first cleanup batch");
        let WorkspaceDestroyProgress::Pending { removed_items } = first else {
            panic!("fixture must require more than one cleanup claim");
        };
        assert!(removed_items > 0);
        assert!(removed_items <= u64::try_from(fs::DESTROY_BATCH_ITEMS).expect("batch fits u64"));
        assert!(workspace.is_dir(), "pending cleanup retains its root");

        let mut absent = false;
        for _ in 0..4 {
            match driver
                .destroy_workspace("ws_batched", "ws_batched")
                .await
                .expect("next cleanup batch")
            {
                WorkspaceDestroyProgress::Pending { removed_items } => {
                    assert!(removed_items <= u64::try_from(fs::DESTROY_BATCH_ITEMS).unwrap());
                    assert!(workspace.is_dir(), "pending cleanup retains its root");
                }
                WorkspaceDestroyProgress::Absent(_) => {
                    absent = true;
                    break;
                }
            }
        }
        assert!(absent, "bounded retries must eventually prove absence");
        assert!(!workspace.exists());
    }

    #[tokio::test]
    async fn dropped_blocking_join_keeps_one_process_local_destroy_owner() {
        let directory = tempdir().expect("tempdir");
        let driver = HostDriver::open(HostConfig::minimum(directory.path())).expect("host driver");
        if !driver.filesystem.openat2_available() {
            return;
        }
        driver
            .filesystem
            .create_workspace("ws_busy")
            .expect("workspace");

        let namespace = driver.workspace_destroy_namespace;
        let (ready_send, ready_receive) = tokio::sync::oneshot::channel();
        let (done_send, done_receive) = tokio::sync::oneshot::channel();
        let (release_send, release_receive) = std::sync::mpsc::channel();
        let owner = tokio::task::spawn_blocking(move || {
            let ownership =
                WorkspaceDestroyOwnership::acquire(namespace, "ws_busy").expect("owner");
            ready_send.send(()).expect("announce ownership");
            release_receive.recv().expect("release ownership");
            drop(ownership);
            done_send.send(()).expect("announce release");
        });
        ready_receive.await.expect("ownership ready");
        drop(owner);

        let error = driver
            .destroy_workspace("ws_busy", "ws_busy")
            .await
            .expect_err("concurrent cleanup must be busy");
        assert_eq!(error.code, "workspace.destroy-busy");
        assert!(error.retriable);

        release_send.send(()).expect("release owner");
        done_receive.await.expect("ownership task completed");
        let result = driver
            .destroy_workspace("ws_busy", "ws_busy")
            .await
            .expect("retry cleanup");
        assert!(matches!(result, WorkspaceDestroyProgress::Absent(_)));
    }
}
