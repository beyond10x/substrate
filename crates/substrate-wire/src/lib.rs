#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

pub const API_VERSION: &str = "v1";
pub const MAX_FILE_BYTES: u64 = 1_048_576;
pub const MAX_IO_BYTES: u64 = 1_048_576;
pub const MAX_LIST_ITEMS: u32 = 1_000;
pub const MAX_PATH_DEPTH: usize = 64;
pub const MAX_EVENT_PAGE_ITEMS: u32 = 1_000;
pub const MAX_SNAPSHOT_PAGE_ITEMS: u32 = 1_000;
pub const MAX_CURRENT_WORKSPACES: u64 = 1_024;
pub const MAX_CURRENT_EXECS: u64 = 2_048;
pub const MAX_SNAPSHOT_PROVENANCE_EVENTS: u64 = 1_024;
pub const OPERATION_LEDGER_SUBJECT_MAX_ROWS: u64 = 100_000;
pub const OPERATION_LEDGER_SUBJECT_MAX_BYTES: u64 = 512 * 1024 * 1024;
pub const OPERATION_LEDGER_GLOBAL_MAX_ROWS: u64 = 1_000_000;
pub const OPERATION_LEDGER_GLOBAL_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MIN_LEASE_TTL_MS: u64 = 1_000;
pub const MAX_LEASE_TTL_MS: u64 = 86_400_000;
pub const LEASE_CLOCK_TOLERANCE_MS: u64 = 30_000;

pub type Labels = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutation<T> {
    pub op: String,
    pub input: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Success<T> {
    pub api_version: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    pub result: T,
}

impl<T> Success<T> {
    pub fn observed(request_id: impl Into<String>, result: T) -> Self {
        Self {
            api_version: API_VERSION.to_owned(),
            request_id: request_id.into(),
            operation: None,
            result,
        }
    }

    pub fn mutation(
        request_id: impl Into<String>,
        operation: impl Into<String>,
        result: T,
    ) -> Self {
        Self {
            api_version: API_VERSION.to_owned(),
            request_id: request_id.into(),
            operation: Some(operation.into()),
            result,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorClass {
    Refused,
    Conflict,
    Unserved,
    Exhausted,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorDetail {
    pub class: ErrorClass,
    pub code: String,
    pub message: String,
    pub retriable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub api_version: String,
    pub request_id: String,
    pub error: ErrorDetail,
}

impl Failure {
    pub fn new(request_id: impl Into<String>, error: ErrorDetail) -> Self {
        Self {
            api_version: API_VERSION.to_owned(),
            request_id: request_id.into(),
            error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreateInput {
    pub source: WorkspaceSource,
    pub labels: Labels,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceSource {
    Empty(EmptySource),
    Git(GitSourceEnvelope),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmptySource {
    #[serde(rename = "empty")]
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitSourceEnvelope {
    pub git: GitSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitSource {
    pub source: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub depth: u16,
}

impl WorkspaceSource {
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty(EmptySource::Empty))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceState {
    Ready,
    Destroying,
    Destroyed,
    Expired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeaseState {
    Active,
    Expiring,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseObservation {
    pub ttl_ms: u64,
    pub renew_by: DateTime<Utc>,
    pub state: LeaseState,
    pub clock_tolerance_ms: u64,
    pub authorizing_operation: String,
    pub actor: String,
    pub principal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub id: String,
    pub kind: WorkspaceKind,
    pub labels: Labels,
    pub observed_at: DateTime<Utc>,
    pub state: WorkspaceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseObservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceKind {
    #[serde(rename = "workspace")]
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileMode {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileReadQuery {
    pub mode: FileMode,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub limit_bytes: Option<u64>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit_items: Option<u32>,
}

impl FileReadQuery {
    /// Validates that only the fields selected by `mode` are present.
    ///
    /// # Errors
    ///
    /// Returns [`WireValidationError::InvalidQueryShape`] for a mixed or incomplete shape.
    pub fn validate_shape(&self) -> Result<(), WireValidationError> {
        match self.mode {
            FileMode::File
                if self.offset.is_some()
                    && self.limit_bytes.is_some()
                    && self.cursor.is_none()
                    && self.limit_items.is_none() =>
            {
                Ok(())
            }
            FileMode::Directory
                if self.offset.is_none()
                    && self.limit_bytes.is_none()
                    && self.limit_items.is_some() =>
            {
                Ok(())
            }
            _ => Err(WireValidationError::InvalidQueryShape),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Base64Content {
    pub encoding: Base64Encoding,
    pub data: String,
}

impl Base64Content {
    /// Decodes the closed base64 content envelope.
    ///
    /// # Errors
    ///
    /// Returns [`WireValidationError::InvalidBase64`] when `data` is not standard base64.
    pub fn decode(&self) -> Result<Vec<u8>, WireValidationError> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.data)
            .map_err(|_| WireValidationError::InvalidBase64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Base64Encoding {
    #[serde(rename = "base64")]
    Base64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileWriteInput {
    pub content: Base64Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSlice {
    pub kind: FileKind,
    pub workspace: String,
    pub path: String,
    pub offset: u64,
    pub returned_bytes: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub content: Base64Content,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryPage {
    pub kind: DirectoryKind,
    pub workspace: String,
    pub path: String,
    pub items: Vec<DirectoryEntry>,
    pub next_cursor: Option<String>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileReadResult {
    File(FileSlice),
    Directory(DirectoryPage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind {
    #[serde(rename = "file")]
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectoryKind {
    #[serde(rename = "directory")]
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DirectoryEntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryEntry {
    pub name: String,
    pub kind: DirectoryEntryKind,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileObservation {
    pub kind: FileKind,
    pub workspace: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub atomic_replacement: bool,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileAbsence {
    pub kind: FileKind,
    pub workspace: String,
    pub path: String,
    pub absent: bool,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAbsence {
    pub kind: WorkspaceKind,
    pub id: String,
    pub absent: bool,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecAbsence {
    pub kind: ExecKind,
    pub id: String,
    pub absent: bool,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    None,
    Aperture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxProfile {
    #[serde(rename = "workspace")]
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfinementRequest {
    pub capability_snapshot: String,
    pub network: NetworkMode,
    pub profile: SandboxProfile,
    #[serde(rename = "require")]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedConfinement {
    pub capability_snapshot: String,
    pub cgroup: String,
    pub filesystem: AppliedFilesystem,
    pub network: AppliedNetwork,
    pub profile: SandboxProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppliedFilesystem {
    #[serde(rename = "workspace-rw-system-ro")]
    WorkspaceReadWriteSystemReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppliedNetwork {
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecEnvironment {
    pub allow: Vec<BaselineEnvironment>,
    pub set: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BaselineEnvironment {
    #[serde(rename = "LANG")]
    Lang,
    #[serde(rename = "LC_ALL")]
    LcAll,
    #[serde(rename = "PATH")]
    Path,
    #[serde(rename = "TERM")]
    Term,
    #[serde(rename = "TZ")]
    Tz,
}

impl BaselineEnvironment {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lang => "LANG",
            Self::LcAll => "LC_ALL",
            Self::Path => "PATH",
            Self::Term => "TERM",
            Self::Tz => "TZ",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecLimits {
    pub timeout_ms: u64,
    pub output_bytes: u64,
    pub processes: u32,
    pub memory_bytes: u64,
    pub cpu_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecStartInput {
    pub workspace: String,
    pub argv: Vec<String>,
    pub env: ExecEnvironment,
    pub sandbox: ConfinementRequest,
    pub limits: ExecLimits,
    pub wait: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Signal {
    Int,
    Term,
    Kill,
}

impl Signal {
    pub const fn number(self) -> i32 {
        match self {
            Self::Int => 2,
            Self::Term => 15,
            Self::Kill => 9,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecSignalInput {
    pub signal: Signal,
    pub grace_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecState {
    Accepted,
    Running,
    Exited,
    Cancelled,
    Expired,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecExit {
    pub code: Option<u8>,
    pub signal: Option<Signal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exec {
    pub id: String,
    pub kind: ExecKind,
    pub workspace: String,
    pub state: ExecState,
    pub observed_at: DateTime<Utc>,
    pub requested: ConfinementRequest,
    pub applied: Option<AppliedConfinement>,
    pub exit: Option<ExecExit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRenewInput {
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventQuery {
    #[serde(default)]
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// Internal commit-effect carrier. Stream and snapshot envelopes bind events to this scope;
    /// the field is deliberately absent from the serialized event value.
    #[serde(skip)]
    pub source_scope: String,
    pub generation: u64,
    pub seq: u64,
    pub resource: String,
    pub resource_kind: String,
    pub transition: String,
    pub observed_at: DateTime<Utc>,
    pub actor: String,
    pub principal: Option<String>,
    pub cause: EventCause,
    pub observation: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum EventCause {
    Operation { operation: String },
    Control { control: EventControl },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventControl {
    #[serde(rename = "reconciliation.snapshot.create")]
    ReconciliationSnapshotCreate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventPage {
    pub source_scope: String,
    pub generation: u64,
    pub items: Vec<Event>,
    pub next_cursor: String,
    pub through_seq: u64,
    pub first_retained_seq: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotMetadata {
    pub id: String,
    pub source_scope: String,
    pub generation: u64,
    pub through_seq: u64,
    pub resume_cursor: String,
    pub item_count: u64,
    pub partitions: SnapshotPartitions,
    pub history: SnapshotHistory,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotPartitions {
    pub workspaces: u64,
    pub execs: u64,
    pub provenance_events: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotHistory {
    pub first_seq: Option<u64>,
    pub through_seq: u64,
    pub item_count: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotItem {
    pub ordinal: u64,
    pub kind: SnapshotItemKind,
    pub id: String,
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotItemKind {
    Workspace,
    Exec,
    ProvenanceEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotPage {
    pub snapshot: String,
    pub generation: u64,
    pub through_seq: u64,
    pub items: Vec<SnapshotItem>,
    pub next_cursor: Option<String>,
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecKind {
    #[serde(rename = "exec")]
    Exec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecOutputQuery {
    pub stream: OutputStream,
    pub offset: u64,
    pub limit_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSlice {
    pub exec: String,
    pub stream: OutputStream,
    pub offset: u64,
    pub returned_bytes: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub truncated: bool,
    pub content: Base64Content,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySnapshot {
    pub snapshot: String,
    pub driver: HostDriverKind,
    pub driver_version: String,
    pub config_generation: u64,
    pub probed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<DateTime<Utc>>,
    pub facts: CapabilityFacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostDriverKind {
    #[serde(rename = "host")]
    Host,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityFacts {
    #[serde(rename = "events.pull", skip_serializing_if = "Option::is_none")]
    pub events_pull: Option<bool>,
    #[serde(rename = "events.stream", skip_serializing_if = "Option::is_none")]
    pub events_stream: Option<bool>,
    #[serde(
        rename = "events.retention-events",
        skip_serializing_if = "Option::is_none"
    )]
    pub events_retention_events: Option<u64>,
    #[serde(rename = "operation.ledger-subject-max-rows")]
    pub operation_ledger_subject_max_rows: u64,
    #[serde(rename = "operation.ledger-subject-max-bytes")]
    pub operation_ledger_subject_max_bytes: u64,
    #[serde(rename = "operation.ledger-global-max-rows")]
    pub operation_ledger_global_max_rows: u64,
    #[serde(rename = "operation.ledger-global-max-bytes")]
    pub operation_ledger_global_max_bytes: u64,
    #[serde(rename = "leases.explicit", skip_serializing_if = "Option::is_none")]
    pub leases_explicit: Option<bool>,
    #[serde(
        rename = "leases.clock-tolerance-ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub leases_clock_tolerance_ms: Option<u64>,
    #[serde(
        rename = "workspace.guarded-io",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_guarded_io: Option<bool>,
    #[serde(
        rename = "workspace.openat2-beneath",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_openat2_beneath: Option<bool>,
    #[serde(
        rename = "workspace.atomic-replace",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_atomic_replace: Option<bool>,
    #[serde(
        rename = "workspace.max-current",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_max_current: Option<u64>,
    #[serde(
        rename = "workspace.max-file-bytes",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_max_file_bytes: Option<u64>,
    #[serde(
        rename = "workspace.read-limit-bytes",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_read_limit_bytes: Option<u64>,
    #[serde(
        rename = "workspace.list-limit-items",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_list_limit_items: Option<u32>,
    #[serde(rename = "exec.argv-only", skip_serializing_if = "Option::is_none")]
    pub exec_argv_only: Option<bool>,
    #[serde(rename = "exec.namespaces", skip_serializing_if = "Option::is_none")]
    pub exec_namespaces: Option<NamespaceFacts>,
    #[serde(rename = "exec.no-egress", skip_serializing_if = "Option::is_none")]
    pub exec_no_egress: Option<bool>,
    #[serde(rename = "exec.cgroup-limits", skip_serializing_if = "Option::is_none")]
    pub exec_cgroup_limits: Option<CgroupLimitFacts>,
    #[serde(rename = "exec.cgroup-kill", skip_serializing_if = "Option::is_none")]
    pub exec_cgroup_kill: Option<bool>,
    #[serde(
        rename = "exec.output-limit-bytes",
        skip_serializing_if = "Option::is_none"
    )]
    pub exec_output_limit_bytes: Option<u64>,
    #[serde(rename = "exec.max-current", skip_serializing_if = "Option::is_none")]
    pub exec_max_current: Option<u64>,
    #[serde(rename = "exec.signals", skip_serializing_if = "Option::is_none")]
    pub exec_signals: Option<Vec<Signal>>,
    #[serde(
        rename = "snapshot.provenance-events",
        skip_serializing_if = "Option::is_none"
    )]
    pub snapshot_provenance_events: Option<u64>,
}

impl Default for CapabilityFacts {
    fn default() -> Self {
        Self {
            events_pull: None,
            events_stream: None,
            events_retention_events: None,
            operation_ledger_subject_max_rows: OPERATION_LEDGER_SUBJECT_MAX_ROWS,
            operation_ledger_subject_max_bytes: OPERATION_LEDGER_SUBJECT_MAX_BYTES,
            operation_ledger_global_max_rows: OPERATION_LEDGER_GLOBAL_MAX_ROWS,
            operation_ledger_global_max_bytes: OPERATION_LEDGER_GLOBAL_MAX_BYTES,
            leases_explicit: None,
            leases_clock_tolerance_ms: None,
            workspace_guarded_io: None,
            workspace_openat2_beneath: None,
            workspace_atomic_replace: None,
            workspace_max_current: None,
            workspace_max_file_bytes: None,
            workspace_read_limit_bytes: None,
            workspace_list_limit_items: None,
            exec_argv_only: None,
            exec_namespaces: None,
            exec_no_egress: None,
            exec_cgroup_limits: None,
            exec_cgroup_kill: None,
            exec_output_limit_bytes: None,
            exec_max_current: None,
            exec_signals: None,
            snapshot_provenance_events: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // This mirrors the closed contract fact object exactly.
#[serde(deny_unknown_fields)]
pub struct NamespaceFacts {
    pub user: bool,
    pub mount: bool,
    pub pid: bool,
    pub ipc: bool,
    pub uts: bool,
    pub network: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CgroupLimitFacts {
    pub processes: bool,
    pub memory: bool,
    pub cpu: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationState {
    Refused,
    Accepted,
    Unknown,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum OperationOutcome {
    Success { result: Value },
    Error { error: ErrorDetail },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationRecord {
    pub operation: String,
    pub operation_kind: String,
    pub request_hash: String,
    pub state: OperationState,
    pub accepted_at: Option<DateTime<Utc>>,
    pub terminal_at: Option<DateTime<Utc>>,
    pub capability_snapshot: Option<String>,
    pub actor: String,
    pub principal: Option<String>,
    pub resource: Option<String>,
    pub outcome: Option<OperationOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WireValidationError {
    #[error("request query does not match its route-specific shape")]
    InvalidQueryShape,
    #[error("content is not canonical base64")]
    InvalidBase64,
    #[error("path is not a safe relative workspace path")]
    InvalidPath,
    #[error("path exceeds the maximum component depth")]
    InvalidPathDepth,
    #[error("operation id is invalid")]
    InvalidOperationId,
    #[error("request contains a floating-point number")]
    FloatingPoint,
    #[error("request contains an unsupported JSON number")]
    UnsupportedNumber,
}

/// Validates a caller-minted operation identifier.
///
/// # Errors
///
/// Returns [`WireValidationError::InvalidOperationId`] when the identifier is out of bounds.
pub fn validate_operation_id(value: &str) -> Result<(), WireValidationError> {
    if (16..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(WireValidationError::InvalidOperationId)
    }
}

/// Validates a relative path before it reaches a host driver.
///
/// # Errors
///
/// Returns [`WireValidationError::InvalidPath`] for absolute, ambiguous, or over-deep paths.
pub fn validate_relative_path(value: &str) -> Result<(), WireValidationError> {
    if value.is_empty() || value.starts_with('/') || value.contains('\0') || value.contains('\\') {
        return Err(WireValidationError::InvalidPath);
    }
    let mut depth = 0;
    for component in value.split('/') {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(WireValidationError::InvalidPath);
        }
        depth += 1;
        if depth > MAX_PATH_DEPTH {
            return Err(WireValidationError::InvalidPathDepth);
        }
    }
    Ok(())
}

/// Computes the exact phase-2 request digest over the canonical length-delimited tuple.
///
/// # Errors
///
/// Returns a [`WireValidationError`] when the input cannot be represented by the pinned format.
pub fn canonical_request_hash(
    method: &str,
    normalized_address: &str,
    input: &Value,
) -> Result<String, WireValidationError> {
    let canonical = canonical_json(input)?;
    let fields = [
        b"1".as_slice(),
        method.as_bytes(),
        normalized_address.as_bytes(),
        canonical.as_bytes(),
    ];
    let mut framed = Vec::new();
    for field in fields {
        let length =
            u32::try_from(field.len()).map_err(|_| WireValidationError::UnsupportedNumber)?;
        framed.extend_from_slice(&length.to_be_bytes());
        framed.extend_from_slice(field);
    }
    Ok(hex::encode(Sha256::digest(&framed)))
}

/// Computes the phase-3 request digest from the caller's raw JSON input value and query.
///
/// Valid form queries are percent-decoded strictly and represented as a sorted multiset of
/// key/value pairs. Sorting makes pair order irrelevant while preserving duplicates. A malformed
/// percent escape or non-UTF-8 decoded component is represented in a separate `raw` domain using
/// the exact query bytes, so even a bindable query-shape refusal has a deterministic identity.
///
/// # Errors
///
/// Returns a [`WireValidationError`] when a field cannot be represented by the pinned framing.
pub fn canonical_request_hash_v2(
    method: &str,
    normalized_address: &str,
    raw_input: &Value,
    raw_query: Option<&str>,
) -> Result<String, WireValidationError> {
    let canonical_input = canonical_json(raw_input).unwrap_or_else(|_| {
        format!(
            "rejected-number-json:{}",
            deterministic_structural_json(raw_input)
        )
    });
    let canonical_query = canonical_query(raw_query.unwrap_or(""))?;
    let fields = [
        b"2".as_slice(),
        method.as_bytes(),
        normalized_address.as_bytes(),
        canonical_input.as_bytes(),
        canonical_query.as_bytes(),
    ];
    let mut framed = Vec::new();
    for field in fields {
        append_framed(&mut framed, field)?;
    }
    Ok(hex::encode(Sha256::digest(&framed)))
}

fn deterministic_structural_json(value: &Value) -> String {
    fn render(value: &Value, output: &mut String) {
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Value::Number(number) => output.push_str(&number.to_string()),
            Value::String(value) => {
                output.push_str(&serde_json::to_string(value).expect("string serialization"));
            }
            Value::Array(items) => {
                output.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    render(item, output);
                }
                output.push(']');
            }
            Value::Object(entries) => {
                let mut entries = entries.iter().collect::<Vec<_>>();
                entries
                    .sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
                output.push('{');
                for (index, (key, item)) in entries.into_iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    output.push_str(&serde_json::to_string(key).expect("key serialization"));
                    output.push(':');
                    render(item, output);
                }
                output.push('}');
            }
        }
    }
    let mut output = String::new();
    render(value, &mut output);
    output
}

/// Returns the exact phase-3 canonical representation of a raw form query.
///
/// # Errors
///
/// Returns [`WireValidationError::UnsupportedNumber`] if the bounded framing cannot represent a
/// component count or component length.
pub fn canonical_query(raw_query: &str) -> Result<String, WireValidationError> {
    let mut pairs = Vec::new();
    let mut valid = true;
    if !raw_query.is_empty() {
        for pair in raw_query.as_bytes().split(|byte| *byte == b'&') {
            let separator = pair.iter().position(|byte| *byte == b'=');
            let (key, value) = separator.map_or((pair, b"".as_slice()), |index| {
                (&pair[..index], &pair[index + 1..])
            });
            let Some(key) = decode_query_component(key) else {
                valid = false;
                break;
            };
            let Some(value) = decode_query_component(value) else {
                valid = false;
                break;
            };
            pairs.push((key, value));
        }
    }
    if !valid {
        return Ok(format!(
            "malformed-raw\0{}",
            hex::encode(raw_query.as_bytes())
        ));
    }
    pairs.sort_by(|left, right| {
        left.0
            .as_bytes()
            .cmp(right.0.as_bytes())
            .then_with(|| left.1.as_bytes().cmp(right.1.as_bytes()))
    });
    let pairs = pairs
        .into_iter()
        .map(|(key, value)| Value::Array(vec![Value::String(key), Value::String(value)]))
        .collect();
    Ok(format!("pairs\0{}", canonical_json(&Value::Array(pairs))?))
}

fn append_framed(output: &mut Vec<u8>, field: &[u8]) -> Result<(), WireValidationError> {
    let length = u32::try_from(field.len()).map_err(|_| WireValidationError::UnsupportedNumber)?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(field);
    Ok(())
}

fn decode_query_component(encoded: &[u8]) -> Option<String> {
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        match encoded[index] {
            b'+' => decoded.push(b' '),
            b'%' => {
                let high = *encoded.get(index + 1)?;
                let low = *encoded.get(index + 2)?;
                decoded.push(hex_nibble(high)? << 4 | hex_nibble(low)?);
                index += 2;
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8(decoded).ok()
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// Serializes JSON using the integer-only subset of RFC 8785 used by the contract bundle.
///
/// # Errors
///
/// Returns a [`WireValidationError`] for floating-point or unsupported JSON numbers.
pub fn canonical_json(value: &Value) -> Result<String, WireValidationError> {
    fn render(value: &Value, output: &mut String) -> Result<(), WireValidationError> {
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(true) => output.push_str("true"),
            Value::Bool(false) => output.push_str("false"),
            Value::Number(number) => {
                if number.is_f64() {
                    return Err(WireValidationError::FloatingPoint);
                }
                write!(output, "{number}").map_err(|_| WireValidationError::UnsupportedNumber)?;
            }
            Value::String(text) => output.push_str(
                &serde_json::to_string(text).map_err(|_| WireValidationError::UnsupportedNumber)?,
            ),
            Value::Array(items) => {
                output.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    render(item, output)?;
                }
                output.push(']');
            }
            Value::Object(entries) => {
                let mut entries = entries.iter().collect::<Vec<_>>();
                entries
                    .sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
                output.push('{');
                for (index, (key, item)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    output.push_str(
                        &serde_json::to_string(key)
                            .map_err(|_| WireValidationError::UnsupportedNumber)?,
                    );
                    output.push(':');
                    render(item, output)?;
                }
                output.push('}');
            }
        }
        Ok(())
    }

    let mut output = String::new();
    render(value, &mut output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_json, canonical_query, canonical_request_hash, canonical_request_hash_v2,
        validate_relative_path,
    };
    use serde::Deserialize as _;
    use serde_json::Value;

    #[derive(serde::Deserialize)]
    struct HashFixtures {
        cases: Vec<HashCase>,
    }

    #[derive(serde::Deserialize)]
    struct HashCase {
        input: Value,
        jcs_input_hex: String,
        method: String,
        normalized_address: String,
        sha256: String,
    }

    #[derive(serde::Deserialize)]
    struct HashFixturesV2 {
        cases: Vec<HashCaseV2>,
    }

    #[derive(serde::Deserialize)]
    struct HashCaseV2 {
        canonical_input_hex: String,
        canonical_query_hex: String,
        input_mode: String,
        method: String,
        normalized_address: String,
        raw_input: Value,
        raw_query: EncodedBytes,
        sha256: String,
        tuple_hex: String,
    }

    #[derive(serde::Deserialize)]
    struct EncodedBytes {
        data: String,
        encoding: String,
    }

    fn assert_hash_fixtures(fixture: &str) {
        let fixtures = HashFixtures::deserialize(&mut serde_json::Deserializer::from_str(fixture))
            .expect("fixture parses");
        for case in fixtures.cases {
            assert_eq!(
                hex::encode(canonical_json(&case.input).expect("input canonicalizes")),
                case.jcs_input_hex
            );
            assert_eq!(
                canonical_request_hash(&case.method, &case.normalized_address, &case.input)
                    .expect("request hashes"),
                case.sha256
            );
        }
    }

    #[test]
    fn exact_phase_2_bundle_hash_fixtures_match() {
        assert_hash_fixtures(include_str!(
            "../../../contracts/substrate-wire/0.1.0/fixtures/canonical-hash.json"
        ));
    }

    #[test]
    fn exact_phase_3_bundle_hash_fixtures_match() {
        let fixture =
            include_str!("../../../contracts/substrate-wire/0.2.0/fixtures/canonical-hash.json");
        let fixtures =
            HashFixturesV2::deserialize(&mut serde_json::Deserializer::from_str(fixture))
                .expect("fixture parses");
        for case in fixtures.cases {
            assert_eq!(case.raw_query.encoding, "hex");
            let query_bytes = hex::decode(&case.raw_query.data).expect("query hex decodes");
            let raw_query = std::str::from_utf8(&query_bytes).expect("HTTP query is UTF-8");
            let canonical_input = match case.input_mode.as_str() {
                "rfc8785-jcs" => canonical_json(&case.raw_input).expect("input canonicalizes"),
                "rejected-number-json" => format!(
                    "rejected-number-json:{}",
                    super::deterministic_structural_json(&case.raw_input)
                ),
                mode => panic!("unknown fixture input mode {mode}"),
            };
            let query = canonical_query(raw_query).expect("query canonicalizes");
            assert_eq!(
                hex::encode(canonical_input.as_bytes()),
                case.canonical_input_hex
            );
            assert_eq!(hex::encode(query.as_bytes()), case.canonical_query_hex);

            let fields = [
                b"2".as_slice(),
                case.method.as_bytes(),
                case.normalized_address.as_bytes(),
                canonical_input.as_bytes(),
                query.as_bytes(),
            ];
            let mut tuple = Vec::new();
            for field in fields {
                tuple.extend_from_slice(
                    &u32::try_from(field.len())
                        .expect("fixture field length fits u32")
                        .to_be_bytes(),
                );
                tuple.extend_from_slice(field);
            }
            assert_eq!(hex::encode(&tuple), case.tuple_hex);
            assert_eq!(
                canonical_request_hash_v2(
                    &case.method,
                    &case.normalized_address,
                    &case.raw_input,
                    Some(raw_query),
                )
                .expect("request hashes"),
                case.sha256
            );
        }
    }

    #[test]
    fn phase_3_query_binding_is_order_independent_but_duplicate_sensitive() {
        let input = serde_json::json!({"path": "a"});
        let left = canonical_request_hash_v2("POST", "/v1/x", &input, Some("b=2&a=1&a=1"))
            .expect("hashes");
        let reordered = canonical_request_hash_v2("POST", "/v1/x", &input, Some("a=1&b=2&a=1"))
            .expect("hashes");
        let one_duplicate =
            canonical_request_hash_v2("POST", "/v1/x", &input, Some("a=1&b=2")).expect("hashes");
        assert_eq!(left, reordered);
        assert_ne!(left, one_duplicate);
        assert_eq!(
            canonical_query("space=a+b&escaped=a%20b").expect("canonical query"),
            canonical_query("escaped=a+b&space=a%20b").expect("canonical query")
        );
    }

    #[test]
    fn malformed_query_binding_preserves_exact_raw_bytes_in_a_separate_domain() {
        assert_eq!(
            canonical_query("a=%zz").expect("raw query domain"),
            "malformed-raw\0".to_owned() + "613d257a7a"
        );
        assert_ne!(
            canonical_query("a=%zz").expect("raw query domain"),
            canonical_query("a=%ZZ").expect("raw query domain")
        );
        assert!(
            canonical_query("a=%FF")
                .expect("raw query domain")
                .starts_with("malformed-raw\0")
        );
    }

    #[test]
    fn rejected_number_input_is_still_deterministically_bound() {
        let left = serde_json::json!({"limit": 1.5, "nested": {"a": 1}});
        let reordered: Value =
            serde_json::from_str(r#"{"nested":{"a":1},"limit":1.5}"#).expect("raw input");
        assert_eq!(
            canonical_request_hash_v2("POST", "/v1/x", &left, None).expect("fallback hash"),
            canonical_request_hash_v2("POST", "/v1/x", &reordered, None).expect("fallback hash")
        );
        assert_ne!(
            canonical_request_hash_v2("POST", "/v1/x", &left, None).expect("fallback hash"),
            canonical_request_hash_v2(
                "POST",
                "/v1/x",
                &serde_json::json!({"limit": 1.6, "nested": {"a": 1}}),
                None,
            )
            .expect("fallback hash")
        );
    }

    #[test]
    fn relative_path_is_closed_and_depth_bounded() {
        assert!(validate_relative_path("src/main.rs").is_ok());
        for invalid in ["", "/etc/passwd", "../secret", "a//b", "a/./b", "a\\b"] {
            assert!(validate_relative_path(invalid).is_err(), "{invalid}");
        }
        assert!(validate_relative_path(&["a"; 65].join("/")).is_err());
    }
}
