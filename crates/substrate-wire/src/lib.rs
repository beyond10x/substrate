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
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub id: String,
    pub kind: WorkspaceKind,
    pub labels: Labels,
    pub observed_at: DateTime<Utc>,
    pub state: WorkspaceState,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CapabilityFacts {
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
    #[serde(rename = "exec.signals", skip_serializing_if = "Option::is_none")]
    pub exec_signals: Option<Vec<Signal>>,
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
            return Err(WireValidationError::InvalidPath);
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
    use super::{canonical_json, canonical_request_hash, validate_relative_path};
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

    #[test]
    fn exact_bundle_hash_fixtures_match() {
        let fixture =
            include_str!("../../../contracts/substrate-wire/0.1.0/fixtures/canonical-hash.json");
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
    fn relative_path_is_closed_and_depth_bounded() {
        assert!(validate_relative_path("src/main.rs").is_ok());
        for invalid in ["", "/etc/passwd", "../secret", "a//b", "a/./b", "a\\b"] {
            assert!(validate_relative_path(invalid).is_err(), "{invalid}");
        }
        assert!(validate_relative_path(&["a"; 65].join("/")).is_err());
    }
}
