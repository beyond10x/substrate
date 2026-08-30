#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
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
pub const MAX_EXECUTION_CAPSULE_FILES: u32 = 32;
pub const MAX_EXECUTION_CAPSULE_FILE_BYTES: u64 = 262_144;
pub const MAX_EXECUTION_CAPSULE_BYTES: u64 = 524_288;
/// How many host directories one start may declare read-only (ADR 0010).
///
/// Small on purpose. A closure that needs many roots is a closure that should be assembled rather
/// than enumerated, and a long list is a request nobody reviewed.
pub const MAX_READ_ONLY_ROOTS: u32 = 4;
/// How many secret slots one start may name (ADR 0012).
///
/// A slot is a credential, and a process that needs many credentials is a process that has been
/// handed somebody else's authority. Small enough that the `close_range` gap list stays bounded.
pub const MAX_SECRET_SLOTS: u32 = 8;
/// Lowest descriptor a secret slot may be delivered at: above stdio, never on it.
pub const MIN_SECRET_SLOT_FD: u32 = 3;
/// Highest descriptor a secret slot may be delivered at
/// (`docs/design/11-sealed-secret-slots.md` § 10 decision 3).
pub const MAX_SECRET_SLOT_FD: u32 = 63;
/// Largest declared slot file the driver will seal into a `memfd`.
pub const MAX_SECRET_SLOT_BYTES: u64 = 65_536;
/// The one shaped-environment name that carries the slot mapping — names and descriptors only.
///
/// A caller cannot collide with it: every caller-set name containing `secret` is already refused
/// (`crates/substrate-daemon/src/app/operations.rs`, `crates/substrate-host/src/process.rs`).
pub const SECRET_SLOTS_ENV: &str = "SUBSTRATE_SECRET_SLOTS";
/// How many egress apertures one deployment may declare (ADR 0013).
///
/// An aperture is outbound authority. A deployment that needs many of them is a deployment whose
/// reach nobody reviewed, and design 10 § 9 decision 5 gives one run exactly one of them anyway.
pub const MAX_EGRESS_APERTURES: u32 = 4;
/// The address the per-run forwarder listens on inside the run's own network namespace.
///
/// Loopback in a namespace whose only interface is loopback: nothing outside the sandbox can reach
/// it, and the sandbox can reach nothing else (`docs/design/10a-egress-mechanism-spike.md` § 3.1).
pub const APERTURE_LOOPBACK_ADDRESS: &str = "127.0.0.1";
/// Where a generated per-run host mapping is bound, so a declared name resolves to the forwarder.
///
/// The sandbox has no `/etc` (`docs/design/10-destination-bound-egress.md` § 2). This is the whole
/// resolver a run gets: exactly the declared name, exactly loopback, and no `resolv.conf`.
pub const APERTURE_HOSTS_PATH: &str = "/etc/hosts";
/// Where a generated per-run certificate authority bundle is bound.
///
/// TLS is byte-transparent through the forwarder but unverifiable without a trust anchor, which is
/// the open question `docs/design/10a-egress-mechanism-spike.md` § 6 row 4 left. The bytes are
/// snapshotted per run rather than bound from a live host path, so a rotation mid-run cannot change
/// what a running child trusts — the same shape design 10 § 9 decision 2 gives `/etc/hosts`.
pub const APERTURE_CA_BUNDLE_PATH: &str = "/etc/ssl/certs/ca-certificates.crt";
pub const EXECUTION_CAPSULE_MOUNT: &str = "/runtime";
/// Hash-domain separator for the capsule manifest. A wire-visible protocol byte string:
/// `contracts/substrate-wire/0.4.0/hashing.json` const-pins the same value, and
/// `capsule_manifest_hash_domain_matches_the_contract` binds the two.
pub const EXECUTION_CAPSULE_HASH_DOMAIN: &str = "b10x.execution-capsule.v1";
pub const OPERATION_LEDGER_SUBJECT_MAX_ROWS: u64 = 100_000;
pub const OPERATION_LEDGER_SUBJECT_MAX_BYTES: u64 = 512 * 1024 * 1024;
pub const OPERATION_LEDGER_GLOBAL_MAX_ROWS: u64 = 1_000_000;
pub const OPERATION_LEDGER_GLOBAL_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MIN_LEASE_TTL_MS: u64 = 1_000;
pub const MAX_LEASE_TTL_MS: u64 = 86_400_000;
pub const LEASE_CLOCK_TOLERANCE_MS: u64 = 30_000;
/// The widest terminal a pty session may declare or resize to, in cells (design 13).
///
/// The kernel field is an `unsigned short`, so 65535 is deliverable; a 65535x65535 window is not a
/// display but an amplification knob, because programs allocate per-cell buffers when the size
/// changes and that allocation is spent from the run's own memory bound. 1000 is above any real
/// terminal — a 4K display at a small font is roughly 400 columns by 100 rows.
pub const MAX_PTY_WINDOW_COLUMNS: u16 = 1_000;
/// The tallest terminal a pty session may declare or resize to, in cells (design 13).
pub const MAX_PTY_WINDOW_ROWS: u16 = 1_000;

/// A `pty` start named a window when the mode forbids one, or omitted the one the mode requires.
pub const SESSION_WINDOW_INVALID: &str = "session.window-invalid";
/// This deployment never proved it can give a confined process a controlling terminal.
pub const SESSION_PTY_UNSERVED: &str = "session.pty-unserved";
/// The host's pty count is full. Retriable: it is a resource other tenants fill and free.
pub const SESSION_PTY_EXHAUSTED: &str = "session.pty-exhausted";
/// A pty was allocated and the driver could not make it usable.
pub const SESSION_PTY_FAILED: &str = "session.pty-failed";
/// A resize named a window outside 1..=1000 cells on an axis.
pub const SESSION_RESIZE_INVALID: &str = "session.resize-invalid";
/// The exec this operation names is not a pty session at all.
pub const SESSION_NOT_PTY: &str = "session.not-pty";
/// The exec is a pty session whose child has already finished, so nothing can observe a change.
pub const SESSION_PTY_ENDED: &str = "session.pty-ended";
/// The kernel refused the resize.
pub const SESSION_RESIZE_FAILED: &str = "session.resize-failed";
/// A pty has no half-close; a client ends input with the terminal's own end-of-file character.
pub const SESSION_INPUT_CLOSE_UNSERVED: &str = "session.input-close-unserved";
/// The declared output bound ended the session (ADR 0014's refusal field, design 13).
pub const SESSION_OUTPUT_LIMIT: &str = "session.output-limit";

/// Every refusal code a `pty` session can raise, in one place, sorted.
///
/// This exists so the *class* is checkable rather than a list somebody remembers to extend. Each
/// emission site in `substrate-host` and `substrate-daemon` binds its constant from here instead of
/// writing a literal, so a code cannot exist without being on this list; and
/// `xtask`'s bundle checker requires the released bundle to name every entry, so a code cannot
/// exist without being readable by a client of the contract. A code nobody can look up is a code
/// nobody can handle.
pub const SESSION_PTY_REFUSAL_CODES: [&str; 10] = [
    SESSION_INPUT_CLOSE_UNSERVED,
    SESSION_NOT_PTY,
    SESSION_OUTPUT_LIMIT,
    SESSION_PTY_ENDED,
    SESSION_PTY_EXHAUSTED,
    SESSION_PTY_FAILED,
    SESSION_PTY_UNSERVED,
    SESSION_RESIZE_FAILED,
    SESSION_RESIZE_INVALID,
    SESSION_WINDOW_INVALID,
];

/// The one audience a delegated-context document may name (ADR 0011).
///
/// A wire-visible identifier carrying a former brand name, frozen with the rest of them
/// (`AGENTS.md` § *Safety envelope*). Adopted from identity's published vocabulary
/// (`identity/README.md:144`), not minted here.
pub const DELEGATED_CONTEXT_AUDIENCE: &str = "urn:b10x:substrate";
/// The JOSE `typ` a delegated-context document declares (ADR 0011). Carries no brand token.
pub const DELEGATED_CONTEXT_TYPE: &str = "substrate-delegated-context+jwt";
/// The byte bound a delegated-context document is parsed within, before any decode.
pub const MAX_DELEGATED_CONTEXT_BYTES: usize = 4_096;
/// The longest total lifetime (`exp - iat`) a delegated-context document may declare.
pub const MAX_DELEGATED_CONTEXT_LIFETIME_SECONDS: i64 = 300;
/// The clock skew allowed on either side of the `nbf`/`exp` window.
pub const DELEGATED_CONTEXT_CLOCK_SKEW_SECONDS: i64 = 30;

pub type Labels = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutation<T> {
    pub op: String,
    pub input: T,
    /// The optional signed delegated-context document (ADR 0011).
    ///
    /// A sibling of `op` and `input`, never a member of `input`, so it stays outside the canonical
    /// request hash (design 09 § 4): replaying the same `op` with a *fresh* context is the same
    /// operation and returns the original outcome. Absent, the serialized bytes are exactly what a
    /// `0.6.0` client sent, which is what keeps every frozen bundle's vectors true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_context: Option<String>,
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

/// Expected state of a path before a guarded mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedFileState {
    /// The destination must not exist.
    Absent,
    /// The destination must be a regular file with this complete-content digest.
    Sha256 { sha256: String },
}

impl ExpectedFileState {
    /// Validates the closed lowercase SHA-256 representation.
    ///
    /// # Errors
    ///
    /// Returns [`WireValidationError::InvalidDigest`] for a malformed digest.
    pub fn validate(&self) -> Result<(), WireValidationError> {
        match self {
            Self::Absent => Ok(()),
            Self::Sha256 { sha256 } if valid_sha256(sha256) => Ok(()),
            Self::Sha256 { .. } => Err(WireValidationError::InvalidDigest),
        }
    }
}

/// Complete-file compare-and-set replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileReplaceInput {
    pub content: Base64Content,
    pub expected: ExpectedFileState,
    #[serde(default)]
    pub create_parents: bool,
}

/// Matching policy for one textual replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextMatchPolicy {
    Exact,
    LineWhitespaceNormalized,
}

/// One compare-and-set textual replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEditInput {
    pub expected_sha256: String,
    pub old_text: String,
    pub new_text: String,
    pub match_policy: TextMatchPolicy,
}

/// One line-based patch edit, addressed against the original file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LinePatchEdit {
    InsertBefore {
        line: u32,
        text: String,
    },
    InsertAfter {
        line: u32,
        text: String,
    },
    ReplaceRange {
        start_line: u32,
        end_line: u32,
        text: String,
    },
    DeleteRange {
        start_line: u32,
        end_line: u32,
    },
}

/// Compare-and-set line patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePatchInput {
    pub expected_sha256: String,
    pub edits: Vec<LinePatchEdit>,
}

/// Directory creation request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryCreateInput {
    #[serde(default)]
    pub parents: bool,
}

/// Atomic path move request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathMoveInput {
    pub destination: String,
    #[serde(default)]
    pub create_parents: bool,
}

/// Bounded unified diff returned by textual mutations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnifiedDiff {
    pub text: String,
    pub truncated: bool,
    pub binary: bool,
}

/// Result of one guarded file mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileMutationResult {
    pub kind: FileKind,
    pub workspace: String,
    pub path: String,
    pub size: u64,
    pub before_sha256: Option<String>,
    pub after_sha256: String,
    pub atomic_replacement: bool,
    pub diff: UnifiedDiff,
    pub observed_at: DateTime<Utc>,
}

/// V2 file read slice with a digest of the complete file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigestedFileSlice {
    pub kind: FileKind,
    pub workspace: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub offset: u64,
    pub returned_bytes: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub content: Base64Content,
    pub observed_at: DateTime<Utc>,
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

/// Bounded recursive workspace-tree query. Hidden path components are omitted unless requested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTreeQuery {
    #[serde(default = "default_tree_limit")]
    pub limit_items: u32,
    #[serde(default)]
    pub include_hidden: bool,
}

const fn default_tree_limit() -> u32 {
    MAX_LIST_ITEMS
}

impl WorkspaceTreeQuery {
    /// Validates the closed recursive-list bound.
    ///
    /// # Errors
    ///
    /// Returns [`WireValidationError::InvalidQueryShape`] when the requested limit is zero or
    /// exceeds the protocol maximum.
    pub fn validate(&self) -> Result<(), WireValidationError> {
        if self.limit_items == 0 || self.limit_items > MAX_LIST_ITEMS {
            return Err(WireValidationError::InvalidQueryShape);
        }
        Ok(())
    }
}

/// One root-relative entry from a descriptor-confined recursive walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTreeEntry {
    pub path: String,
    pub kind: DirectoryEntryKind,
    pub size: Option<u64>,
}

/// Deterministically path-sorted bounded recursive tree observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTree {
    pub workspace: String,
    pub items: Vec<WorkspaceTreeEntry>,
    pub truncated: bool,
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
    /// The operator-declared aperture this start selects, by name (ADR 0013).
    ///
    /// A name and never a destination, at any depth, in any field: configuration owns reach and a
    /// request only selects among what configuration already permitted. Absent is the default and
    /// the floor — `--unshare-net` and no interface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aperture: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule: Option<AppliedExecutionCapsule>,
    /// Every declared host root that was mounted (ADR 0010).
    ///
    /// Reported rather than inferred. Unlike a capsule there is no manifest and no digest here —
    /// hashing a package registry on every exec is not a thing anybody would run twice — so what
    /// substrate can guarantee is narrower: that this directory was mounted read-only at this
    /// point. **What cannot be verified must at least be visible**, and this is where it is
    /// visible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_only_roots: Vec<ReadOnlyRoot>,
    /// Every secret slot that was placed, by name and descriptor (ADR 0012).
    ///
    /// An auditor sees that this run held `vendor_api_key` and where it arrived, and never what was
    /// in it. Applied, not requested: this is the record of what the driver actually placed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_slots: Vec<SecretSlotRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppliedFilesystem {
    #[serde(rename = "workspace-rw-system-ro")]
    WorkspaceReadWriteSystemReadOnly,
    #[serde(rename = "workspace-rw-capsule-ro-system-ro")]
    WorkspaceReadWriteCapsuleReadOnlySystemReadOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedExecutionCapsule {
    pub manifest_sha256: String,
    pub entrypoint: String,
    pub mount: String,
    pub file_count: u32,
    pub total_bytes: u64,
}

/// What the network actually was, reported and never inferred (ADR 0013).
///
/// `"none"` is the floor and stays a bare string, so every 0.4.0 and 0.5.0 reader keeps parsing
/// every run that used no aperture. An applied aperture is an object, because "which aperture, to
/// what address, by what mechanism, and how many bytes" are four separate observations and a
/// reader that only has the name cannot audit any of the other three.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "AppliedNetworkRepr", into = "AppliedNetworkRepr")]
pub enum AppliedNetwork {
    None,
    Aperture(AppliedAperture),
}

/// The serialized shape of [`AppliedNetwork`]: a bare `"none"`, or the applied-aperture object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum AppliedNetworkRepr {
    None(AppliedNoNetwork),
    Aperture(AppliedAperture),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum AppliedNoNetwork {
    #[serde(rename = "none")]
    None,
}

impl From<AppliedNetworkRepr> for AppliedNetwork {
    fn from(value: AppliedNetworkRepr) -> Self {
        match value {
            AppliedNetworkRepr::None(AppliedNoNetwork::None) => Self::None,
            AppliedNetworkRepr::Aperture(aperture) => Self::Aperture(aperture),
        }
    }
}

impl From<AppliedNetwork> for AppliedNetworkRepr {
    fn from(value: AppliedNetwork) -> Self {
        match value {
            AppliedNetwork::None => Self::None(AppliedNoNetwork::None),
            AppliedNetwork::Aperture(aperture) => Self::Aperture(aperture),
        }
    }
}

/// The aperture a run actually got: name, pinned destination, mechanism and bytes (ADR 0013).
///
/// `destination` is the address the forwarder dialled, not the configured host string. Both exist
/// because they can differ and `docs/design/04-security-and-isolation.md` matches on the resolved
/// one; a reader auditing where a credential was spent needs the resolved one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedAperture {
    pub mode: ApertureMode,
    /// The operator-declared name the request selected.
    pub name: String,
    /// The pinned `address:port` the forwarder dialled, resolved once at declaration.
    pub destination: String,
    pub mechanism: ApertureMechanism,
    /// What crossed, counted where the bytes are: in the forwarder and nowhere else.
    pub bytes: ApertureBytes,
    /// The declared ceiling this run ran under, over both directions summed (ADR 0014).
    ///
    /// Absent is the shape every `0.7.0` reader already parses: an aperture declared without the
    /// term reports exactly what it reported before the term existed. Present, it is what `bytes`
    /// was measured against, so a reader auditing a stopped run does not have to go and find the
    /// deployment's argv to know why it stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApertureMode {
    #[serde(rename = "aperture")]
    Aperture,
}

/// How the aperture was installed. One value today, and named rather than assumed: a client that
/// reads this is reading an observation, not selecting an implementation (invariant 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApertureMechanism {
    /// A per-run forwarder listening inside the run's own network namespace
    /// (`docs/design/10-destination-bound-egress.md` § 4 option (c)).
    #[serde(rename = "loopback-forwarder")]
    LoopbackForwarder,
}

/// Byte accounting for one applied aperture, from the only place that sees the bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApertureBytes {
    /// Bytes the confined process sent towards the pinned destination.
    pub to_destination: u64,
    /// Bytes the pinned destination sent back.
    pub from_destination: u64,
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
    /// Cumulative CPU-time budget across the complete execution cgroup, not a rate or share.
    pub cpu_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionCapsuleFileRole {
    Runtime,
    Configuration,
    Hook,
    ProtocolSidecar,
}

impl ExecutionCapsuleFileRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Configuration => "configuration",
            Self::Hook => "hook",
            Self::ProtocolSidecar => "protocol-sidecar",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCapsuleFile {
    pub path: String,
    pub role: ExecutionCapsuleFileRole,
    pub executable: bool,
    pub sha256: String,
    pub content: Base64Content,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCapsuleInput {
    pub manifest_sha256: String,
    pub entrypoint: String,
    pub files: Vec<ExecutionCapsuleFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionCapsuleValidation {
    pub file_count: u32,
    pub total_bytes: u64,
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
    /// Host directories admitted read-only inside this process (ADR 0010).
    ///
    /// Empty on every existing consumer, and empty is what keeps the isolation verified from
    /// inside a sandbox exactly as it was: nothing outside `/usr`, `/bin`, `/lib`, `/lib64` and the
    /// workspace is reachable. A root is how a caller brings a closure **in** — a toolchain, a
    /// package registry — for a process that still has no network to fetch one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_only_roots: Vec<ReadOnlyRoot>,
    /// Operator-declared credentials this start wants, and the descriptors they must arrive at
    /// (ADR 0012).
    ///
    /// A name and a number each; never a value, a path or a length. Absent on every existing
    /// consumer, and absent is what keeps the ledger hash of an unchanged start unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_slots: Vec<SecretSlotRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule: Option<ExecutionCapsuleInput>,
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
    /// The named bound that ended this run, when one did (ADR 0014).
    ///
    /// `state` says a run was stopped; before this field nothing said by what. A timeout, a CPU
    /// budget and a client cancel are all `cancelled` and stay that way — naming them here is a
    /// later change with its own vectors. The declared aperture byte ceiling is this field's only
    /// user, and a run that hit no bound carries nothing, so every observation a `0.7.0` reader
    /// already parses is byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<ExecRefusal>,
}

/// The class, code and message of the bound a run was stopped by (ADR 0014).
///
/// The same three fields an [`ErrorDetail`] carries, and deliberately not an `ErrorDetail`: this is
/// an observation of a run that happened, not the failure of a request. There is no `address` —
/// design 10 § 5 row 5 gives the byte ceiling none, because nothing in the request is at fault —
/// and no `retriable`, because whether to run it again is not substrate's claim to make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecRefusal {
    pub class: ErrorClass,
    pub code: String,
    pub message: String,
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

impl Event {
    /// Validate the correlated event fields before an owner persists or publishes the value.
    ///
    /// The wire schema is a closed union. Keeping this check beside the wire type prevents stores
    /// from independently combining a valid transition with the wrong resource or cause branch.
    ///
    /// # Errors
    ///
    /// Returns [`WireValidationError::InvalidEventShape`] when the transition, resource kind,
    /// cause, or observation do not form one of the protocol's closed event variants.
    pub fn validate_closed_shape(&self) -> Result<(), WireValidationError> {
        let operation_cause = matches!(self.cause, EventCause::Operation { .. });
        let control_cause = matches!(self.cause, EventCause::Control { .. });
        let valid = match self.transition.as_str() {
            "operation.accepted" | "operation.refused" | "operation.failed"
            | "operation.unknown" | "operation.terminal" => {
                self.resource_kind == "operation"
                    && operation_cause
                    && serde_json::from_value::<OperationRecord>(self.observation.clone()).is_ok()
            }
            "workspace.created"
            | "workspace.lease-renewed"
            | "workspace.lease-expiring"
            | "workspace.lease-expired"
            | "workspace.file-written"
            | "workspace.file-deleted"
            | "workspace.destroyed"
            | "workspace.cleanup-failed" => self.resource_kind == "workspace" && operation_cause,
            "exec.accepted"
            | "exec.running"
            | "exec.observed"
            | "exec.exited"
            | "exec.cancelled"
            | "exec.unknown"
            | "exec.lease-renewed"
            | "exec.lease-expiring"
            | "exec.lease-expired"
            | "exec.retired"
            | "exec.cleanup-failed" => self.resource_kind == "exec" && operation_cause,
            "snapshot.created" | "snapshot.refused" => {
                self.resource_kind == "snapshot" && control_cause
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(WireValidationError::InvalidEventShape)
        }
    }
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

/// Development raw-pipe start shape. The daemon route is implemented but not released; this closed
/// shape does not change immutable 0.1.0 or 0.2.0 bundle bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipeSessionStartInput {
    pub exec: ExecStartInput,
    pub input_limit_bytes: u64,
    pub frame_limit_bytes: u64,
    pub queued_frames: u32,
    /// The channel this session carries. Omitted means [`SessionMode::Pipes`], and that default is
    /// what enforces design 05 section 2 mechanically: no existing client can be handed a terminal.
    #[serde(default)]
    pub mode: SessionMode,
    /// The initial terminal window, required for `pty` and refused for `pipes` (design 13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<PtyWindow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionKind {
    #[serde(rename = "session")]
    Session,
}

/// The channel a session carries, never the kind of resource it is (design 13).
///
/// `SessionKind` is the *resource* axis and holds one variant; growing it would make a terminal a
/// different kind of resource from a pipe session, which is the split ADR 0008 refused. This is the
/// channel axis, already carried as `mode` on the durable resource, and it is what grows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    /// Three descriptors, separately attributed, and a machine protocol may run on them (ADR 0007).
    #[default]
    Pipes,
    /// One terminal: merged output, a line discipline, a window and a hangup (design 13).
    Pty,
}

/// A terminal window in cells. Pixel dimensions are not on the wire and are set to zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PtyWindow {
    pub columns: u16,
    pub rows: u16,
}

impl PtyWindow {
    /// Whether both axes are within 1..=1000 cells.
    ///
    /// Zero is outside the bounds rather than a request for a default: a zero dimension is how a
    /// terminal says *I do not know*, which is not what a client that sent a window meant.
    #[must_use]
    pub const fn within_bounds(&self) -> bool {
        self.columns >= 1
            && self.columns <= MAX_PTY_WINDOW_COLUMNS
            && self.rows >= 1
            && self.rows <= MAX_PTY_WINDOW_ROWS
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Accepted,
    Ready,
    Attached,
    Exited,
    Cancelled,
    Expired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionAttachmentState {
    Pending,
    Available,
    Attached,
    Consumed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipeSessionLimits {
    pub input_bytes: u64,
    pub frame_bytes: u64,
    pub queued_frames: u32,
}

/// Durable raw-pipe session resource. Its lease is the projection of the single underlying exec
/// lease; it is evidence, not a second cleanup authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipeSession {
    pub id: String,
    pub kind: SessionKind,
    pub mode: SessionMode,
    pub exec: String,
    pub workspace: String,
    pub state: SessionState,
    pub attachment: SessionAttachmentState,
    pub observed_at: DateTime<Utc>,
    pub capability_snapshot: String,
    pub limits: PipeSessionLimits,
    pub exit: Option<ExecExit>,
    pub lease: LeaseObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionAbsence {
    pub kind: SessionKind,
    pub id: String,
    pub absent: bool,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipeSessionCapabilities {
    pub contract: String,
    pub transport: String,
    pub capability_snapshot: String,
    pub lease_required: bool,
    pub single_attachment: bool,
    pub network: AppliedNetwork,
    pub max_input_bytes: u64,
    pub max_frame_bytes: u64,
    pub max_queued_frames: u32,
    /// The modes this daemon serves, derived from the capability facts and never a second source
    /// of truth. `pty` appears only when `sessions.pty` was probe-verified (design 13).
    pub modes: Vec<SessionMode>,
    pub max_window_columns: u16,
    pub max_window_rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PipeClientFrame {
    Stdin {
        sequence: u64,
        content: Base64Content,
    },
    CloseInput {
        sequence: u64,
    },
    Signal {
        sequence: u64,
        signal: Signal,
        grace_ms: u64,
    },
    /// A new terminal window for a `pty` session (design 13). There is no `close-input` companion:
    /// a pty has no half-close, so a client ends input by sending the terminal's own EOF character
    /// as ordinary input bytes.
    Resize {
        sequence: u64,
        window: PtyWindow,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PipeServerFrame {
    Output {
        sequence: u64,
        stream: OutputStream,
        content: Base64Content,
    },
    Truncated {
        sequence: u64,
        stream: OutputStream,
    },
    Exit {
        sequence: u64,
        state: ExecState,
        exit: Option<ExecExit>,
    },
    ProtocolError {
        sequence: u64,
        code: String,
        message: String,
    },
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
        rename = "exec.inline-capsule",
        skip_serializing_if = "Option::is_none"
    )]
    pub exec_inline_capsule: Option<ExecutionCapsuleFacts>,
    /// The apertures this deployment declared, by name and pinned destination (ADR 0013).
    ///
    /// Answers "what could this daemon ever reach" — deployment vocabulary, not secret material
    /// (`docs/design/04-security-and-isolation.md` § 6). Published **only** after the mechanism was
    /// exercised in a throwaway sandbox, never after reading configuration: configured intent is
    /// not a fact. Absent leaves every aperture request `unserved`, because a run that cannot get
    /// the aperture it asked for must not get a quieter one instead (invariant 3).
    #[serde(
        rename = "exec.egress-apertures",
        skip_serializing_if = "Option::is_none"
    )]
    pub exec_egress_apertures: Option<Vec<EgressApertureFact>>,
    /// The sorted names of the slots this driver can deliver sealed (ADR 0012).
    ///
    /// Names only — never a path, a length or a digest of a value — so adding or removing a slot
    /// moves the snapshot digest while **rotating a value moves nothing observable**. Published
    /// only from a probe that proved sealing and descriptor pass-through; absent otherwise, because
    /// a missing guarantee is a named refusal and never a weaker delivery (invariant 3).
    #[serde(rename = "secrets.slots", skip_serializing_if = "Option::is_none")]
    pub secrets_slots: Option<Vec<String>>,
    /// Whether this driver proved it can give a confined process a controlling terminal.
    ///
    /// Published **only** after a startup probe allocated a pty pair, made it controlling inside a
    /// throwaway sandbox and round-tripped a window size through the child — never from a constant
    /// and never from configuration (invariant 4, design 13). Absent leaves every `mode: "pty"`
    /// request `unserved` by name; it never falls back to pipes (invariant 3).
    #[serde(rename = "sessions.pty", skip_serializing_if = "Option::is_none")]
    pub sessions_pty: Option<bool>,
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
            exec_inline_capsule: None,
            exec_egress_apertures: None,
            secrets_slots: None,
            sessions_pty: None,
            snapshot_provenance_events: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCapsuleFacts {
    pub mount: String,
    pub max_files: u32,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
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
    /// The declared grant this operation ran under, verbatim from a verified delegated context.
    ///
    /// Absent when no context was presented, which is every operation a `0.6.0` client sends: the
    /// member is skipped rather than serialized as `null`, so an unattributed row is byte-for-byte
    /// the row `0.6.0` produced (ADR 0011; invariant 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_ref: Option<String>,
    /// The initiating platform principal, verbatim from a verified delegated context.
    ///
    /// Deliberately **not** `principal`, which keeps its `pid:` meaning: collapsing the local
    /// process and the platform principal into one field is the confusion design 06 § 2 forbids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_principal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WireValidationError {
    #[error("request query does not match its route-specific shape")]
    InvalidQueryShape,
    #[error("digest is not lowercase SHA-256")]
    InvalidDigest,
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
    #[error("execution capsule is outside the closed bounds")]
    InvalidCapsuleBounds,
    #[error("execution capsule files are not in canonical path order")]
    InvalidCapsuleOrder,
    #[error("execution capsule digest is not canonical SHA-256")]
    InvalidCapsuleDigest,
    #[error("execution capsule entrypoint is not an executable file")]
    InvalidCapsuleEntrypoint,
    #[error("execution capsule file content does not match its digest")]
    CapsuleContentMismatch,
    #[error("event fields do not select one closed event schema branch")]
    InvalidEventShape,
    #[error("declared read-only roots are outside the closed bounds")]
    InvalidReadOnlyRootBounds,
    #[error("a declared read-only root path is not absolute and canonical")]
    InvalidReadOnlyRootPath,
    #[error("a declared read-only root mount collides with one substrate owns")]
    ReservedReadOnlyRootMount,
    #[error("two declared read-only roots name the same mount point")]
    DuplicateReadOnlyRootMount,
    #[error("two declared read-only root mount trees overlap")]
    OverlappingReadOnlyRootMount,
    #[error("named secret slots are outside the closed bounds")]
    InvalidSecretSlotBounds,
    #[error("a named secret slot is not a legal slot name")]
    InvalidSecretSlotName,
    #[error("a secret slot descriptor is outside the closed range")]
    InvalidSecretSlotDescriptor,
    #[error("two named secret slots ask for the same descriptor")]
    DuplicateSecretSlotDescriptor,
    #[error("one secret slot is named twice in the same start")]
    DuplicateSecretSlotName,
    #[error("the selected egress aperture is not a legal aperture name")]
    InvalidApertureName,
    #[error("an egress aperture name carries a destination where a name belongs")]
    ApertureDestinationInRequest,
    #[error("an egress aperture selection carries a declared byte ceiling")]
    ApertureCeilingInRequest,
    #[error("a network mode and an egress aperture selection disagree")]
    ApertureModeMismatch,
    #[error("execution capsule manifest does not match its digest")]
    CapsuleManifestMismatch,
    #[error("a session window is absent, unwanted, or outside the closed cell bounds")]
    InvalidSessionWindow,
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

/// Computes the canonical identity of an execution-capsule manifest.
///
/// The hash covers a domain tag, entrypoint, and each already path-sorted file's path, role,
/// executable bit, and lowercase content digest. Every field is prefixed by a four-byte big-endian
/// length. File bytes are covered through their independently verified digest.
///
/// # Errors
///
/// Returns a typed validation error for invalid paths, order, bounds, digests, or entrypoint.
pub fn canonical_execution_capsule_hash(
    entrypoint: &str,
    files: &[ExecutionCapsuleFile],
) -> Result<String, WireValidationError> {
    validate_relative_path(entrypoint)?;
    let max_files = usize::try_from(MAX_EXECUTION_CAPSULE_FILES)
        .map_err(|_| WireValidationError::InvalidCapsuleBounds)?;
    if files.is_empty() || files.len() > max_files {
        return Err(WireValidationError::InvalidCapsuleBounds);
    }
    let mut previous: Option<&str> = None;
    let mut found_entrypoint = false;
    let mut framed = Vec::with_capacity(files.len().saturating_mul(160));
    append_framed(&mut framed, EXECUTION_CAPSULE_HASH_DOMAIN.as_bytes())?;
    append_framed(&mut framed, entrypoint.as_bytes())?;
    for file in files {
        validate_relative_path(&file.path)?;
        if previous.is_some_and(|value| value >= file.path.as_str()) {
            return Err(WireValidationError::InvalidCapsuleOrder);
        }
        if !is_canonical_sha256(&file.sha256) {
            return Err(WireValidationError::InvalidCapsuleDigest);
        }
        found_entrypoint |= file.path == entrypoint && file.executable;
        append_framed(&mut framed, file.path.as_bytes())?;
        append_framed(&mut framed, file.role.as_str().as_bytes())?;
        append_framed(&mut framed, if file.executable { b"1" } else { b"0" })?;
        append_framed(&mut framed, file.sha256.as_bytes())?;
        previous = Some(&file.path);
    }
    if !found_entrypoint {
        return Err(WireValidationError::InvalidCapsuleEntrypoint);
    }
    Ok(hex::encode(Sha256::digest(framed)))
}

/// The mount points substrate owns and a caller may not take (ADR 0010).
///
/// A root landing on one of these would either shadow the read-only base system the process needs
/// or the workspace it is supposed to write to. Refused rather than re-pointed: a caller who asked
/// for `/usr` meant something, and quietly moving it elsewhere would run a different request.
pub const RESERVED_MOUNTS: [&str; 9] = [
    "/",
    "/usr",
    "/bin",
    "/lib",
    "/lib64",
    "/proc",
    "/dev",
    "/tmp",
    "/workspace",
];

/// A host directory admitted read-only inside a confined process (ADR 0010).
///
/// Both halves are the caller's: which directory, and where it appears. Substrate knows nothing
/// about what is in it — a toolchain, a package registry, a data set — and deliberately does not,
/// because the moment it did it would carry one client's vendor semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOnlyRoot {
    /// The host directory, absolute and canonical.
    pub host_path: String,
    /// Where it appears inside, absolute and canonical.
    pub mount: String,
}

/// Checks a start's declared roots against every rule ADR 0010 names.
///
/// # Errors
///
/// Returns the rule that was broken. **Nothing is adjusted**: a root that cannot be mounted as
/// asked refuses the dispatch, because a request silently re-pointed is a different request and the
/// caller would have no way to tell.
pub fn validate_read_only_roots(roots: &[ReadOnlyRoot]) -> Result<(), WireValidationError> {
    if roots.len() > MAX_READ_ONLY_ROOTS as usize {
        return Err(WireValidationError::InvalidReadOnlyRootBounds);
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for root in roots {
        if !is_absolute_canonical(&root.host_path) || !is_absolute_canonical(&root.mount) {
            return Err(WireValidationError::InvalidReadOnlyRootPath);
        }
        if RESERVED_MOUNTS.iter().any(|owned| {
            root.mount == *owned || (*owned != "/" && is_path_beneath(&root.mount, owned))
        }) || root.mount == EXECUTION_CAPSULE_MOUNT
            || is_path_beneath(&root.mount, EXECUTION_CAPSULE_MOUNT)
        {
            return Err(WireValidationError::ReservedReadOnlyRootMount);
        }
        if !seen.insert(root.mount.as_str()) {
            return Err(WireValidationError::DuplicateReadOnlyRootMount);
        }
        if seen.iter().any(|other| {
            *other != root.mount
                && (is_path_beneath(other, &root.mount) || is_path_beneath(&root.mount, other))
        }) {
            return Err(WireValidationError::OverlappingReadOnlyRootMount);
        }
    }
    Ok(())
}

fn is_path_beneath(path: &str, ancestor: &str) -> bool {
    path.strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

/// One operator-declared secret slot, named and placed by a start (ADR 0012).
///
/// A name and a number, and deliberately nothing else. There is no value here, no path and no
/// length, because the ledger frames the whole request (`canonical_request_hash_v2`): the only way
/// the material stays out of the hash is for it never to be in the request. Rotating what is behind
/// `slot` therefore changes no byte a client can read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretSlotRequest {
    /// The operator-declared slot name.
    pub slot: String,
    /// The descriptor the sealed `memfd` must arrive at inside the child.
    pub fd: u32,
}

/// A slot name: `[a-z][a-z0-9_]{0,63}`.
///
/// Non-secret by construction — it is the `memfd` name the child reads back through
/// `/proc/self/fd`, and it is what an error is allowed to say
/// (`docs/design/04-security-and-isolation.md:79`).
pub fn valid_secret_slot_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_lowercase()
            } else {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
            }
        })
}

/// Checks a start's named slots against every rule ADR 0012 names.
///
/// Whether the *name* is declared by this operator is the driver's question — this is the shape.
///
/// # Errors
///
/// Returns the rule that was broken. Nothing is adjusted: a slot that cannot be placed exactly
/// where it was asked for refuses the dispatch.
pub fn validate_secret_slots(slots: &[SecretSlotRequest]) -> Result<(), WireValidationError> {
    if slots.len() > MAX_SECRET_SLOTS as usize {
        return Err(WireValidationError::InvalidSecretSlotBounds);
    }
    let mut names: BTreeSet<&str> = BTreeSet::new();
    let mut descriptors: BTreeSet<u32> = BTreeSet::new();
    for slot in slots {
        if !valid_secret_slot_name(&slot.slot) {
            return Err(WireValidationError::InvalidSecretSlotName);
        }
        if !(MIN_SECRET_SLOT_FD..=MAX_SECRET_SLOT_FD).contains(&slot.fd) {
            return Err(WireValidationError::InvalidSecretSlotDescriptor);
        }
        if !names.insert(slot.slot.as_str()) {
            return Err(WireValidationError::DuplicateSecretSlotName);
        }
        if !descriptors.insert(slot.fd) {
            return Err(WireValidationError::DuplicateSecretSlotDescriptor);
        }
    }
    Ok(())
}

/// The `SUBSTRATE_SECRET_SLOTS` value for a start: `name=fd`, comma-separated, sorted by name.
///
/// `None` when no slot is named, so the variable is absent rather than empty — an empty mapping and
/// no mapping are different claims and a child should not have to tell them apart.
pub fn secret_slot_environment(slots: &[SecretSlotRequest]) -> Option<String> {
    if slots.is_empty() {
        return None;
    }
    let sorted: BTreeMap<&str, u32> = slots
        .iter()
        .map(|slot| (slot.slot.as_str(), slot.fd))
        .collect();
    Some(
        sorted
            .into_iter()
            .map(|(name, fd)| format!("{name}={fd}"))
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// One declared egress aperture as a capability fact: a name and the address it is pinned to.
///
/// Deployment vocabulary, published so `/v1/machine` can answer "what could this daemon ever
/// reach". `destination` is the resolved `address:port`, not the configured host string, because
/// the resolved one is what the kernel would actually be asked for
/// (`docs/design/04-security-and-isolation.md` § 4).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressApertureFact {
    pub name: String,
    pub destination: String,
    /// The declared byte ceiling, if this aperture carries one (ADR 0014).
    ///
    /// Published so `/v1/machine` answers "how much could this daemon ever pass" rather than only
    /// "where could it reach". Absent means unbounded, which is what every aperture was before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

/// An aperture name: `[a-z][a-z0-9_]{0,63}`, the same shape a secret slot name has.
///
/// Deliberately excludes `.` and `:`, so a destination can never be spelled as a name by accident
/// — `api.example.com:443` fails this and is then refused *by name* rather than reported as an
/// aperture nobody declared (design 10 § 5, row 3).
pub fn valid_aperture_name(name: &str) -> bool {
    valid_secret_slot_name(name)
}

/// Reads as a destination rather than a name: anything carrying a host separator or a port.
///
/// The successor input schema has no destination field at all, so a conforming client's raw
/// destination is `schema-invalid` first. This exists for the one case the schema cannot see: a
/// *name* that parses as `host:port`, which is a rejected escalation and not a configuration typo.
/// Reads as a declared byte ceiling: any term spelling `max=`, at any position, in any case.
///
/// A ceiling is deployment vocabulary and `ConfinementRequest` is `deny_unknown_fields`, so a
/// conforming client that adds a ceiling *field* is `schema-invalid` before this runs. This exists
/// for the one shape the schema cannot see — a ceiling smuggled into the name — and it is checked
/// before [`reads_as_destination`], which would otherwise answer "destination" for the `/` in
/// `model/max=1MiB` and send an operator looking for the wrong escalation (ADR 0014).
///
/// **The comparison is on bytes, and it has to be.** `value` is whatever a client put in the
/// request; this and [`reads_as_destination`] run *before* [`valid_aperture_name`], because a name
/// that reads as an escalation must be refused as that escalation rather than as a name typo, and
/// that ordering is the whole point of both codes. So neither may assume the name is ASCII: a
/// `&str[..4]` here panics whenever byte index 4 lands inside a multi-byte character, turning a
/// `422` into a dropped connection — invariant 3 inverted. `get(..4)` on the bytes is total for
/// every input, including one shorter than the prefix.
fn reads_as_ceiling(value: &str) -> bool {
    value.split('/').any(|term| {
        term.len() > 4
            && term
                .as_bytes()
                .get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"max="))
    })
}

fn reads_as_destination(value: &str) -> bool {
    value.contains(':')
        || value.contains('/')
        || value.contains('.')
        || value.parse::<std::net::IpAddr>().is_ok()
}

/// Checks the aperture half of one sandbox request against every rule ADR 0013 names.
///
/// Shape only. Whether *this* deployment declared the name is the driver's question, and whether
/// the mechanism verified is the capability's; both are answered after this and neither here.
///
/// # Errors
///
/// Returns the rule that was broken. Nothing is adjusted: a start that cannot have the aperture it
/// named gets a refusal, never a quieter network.
pub fn validate_aperture_request(
    network: NetworkMode,
    aperture: Option<&str>,
) -> Result<(), WireValidationError> {
    match (network, aperture) {
        (NetworkMode::None, None) => Ok(()),
        (NetworkMode::None, Some(_)) | (NetworkMode::Aperture, None) => {
            Err(WireValidationError::ApertureModeMismatch)
        }
        (NetworkMode::Aperture, Some(name)) => {
            if reads_as_ceiling(name) {
                return Err(WireValidationError::ApertureCeilingInRequest);
            }
            if reads_as_destination(name) {
                return Err(WireValidationError::ApertureDestinationInRequest);
            }
            if !valid_aperture_name(name) {
                return Err(WireValidationError::InvalidApertureName);
            }
            Ok(())
        }
    }
}

/// Validates the initial window a session start declares against the mode it declared.
///
/// A `pty` start states its window and a `pipes` start states none: substrate has nothing to
/// observe about a client's terminal, the client does, and inventing 80x24 would be manufacturing a
/// fact (design 13). The same bounds decide a `resize` frame, so a window admitted at start and a
/// window admitted mid-session are the same rule read from one place.
///
/// # Errors
///
/// Returns [`WireValidationError::InvalidSessionWindow`] when a `pty` start carries no window, a
/// `pipes` start carries one, or either axis is outside 1..=1000 cells.
pub fn validate_session_window(
    mode: SessionMode,
    window: Option<&PtyWindow>,
) -> Result<(), WireValidationError> {
    match (mode, window) {
        (SessionMode::Pipes, None) => Ok(()),
        (SessionMode::Pipes, Some(_)) | (SessionMode::Pty, None) => {
            Err(WireValidationError::InvalidSessionWindow)
        }
        (SessionMode::Pty, Some(window)) => {
            if window.within_bounds() {
                Ok(())
            } else {
                Err(WireValidationError::InvalidSessionWindow)
            }
        }
    }
}

/// An absolute path with no `.`, no `..`, no empty component and no trailing slash.
///
/// Canonical in the textual sense only. Whether the host path exists and is a directory is the
/// driver's question, because only it can answer without racing.
fn is_absolute_canonical(path: &str) -> bool {
    if !path.starts_with('/') || path.len() > 1 && path.ends_with('/') || path.contains("//") {
        return false;
    }
    let components: Vec<&str> = path
        .split('/')
        .skip(1)
        .filter(|part| !part.is_empty())
        .collect();
    !components.is_empty()
        && components.len() <= MAX_PATH_DEPTH
        && components.iter().all(|part| *part != "." && *part != "..")
}

/// Validates all capsule bytes and returns the exact admitted size observation.
///
/// # Errors
///
/// Returns a typed validation error when any manifest or byte invariant is false.
pub fn validate_execution_capsule(
    capsule: &ExecutionCapsuleInput,
) -> Result<ExecutionCapsuleValidation, WireValidationError> {
    if !is_canonical_sha256(&capsule.manifest_sha256) {
        return Err(WireValidationError::InvalidCapsuleDigest);
    }
    let computed = canonical_execution_capsule_hash(&capsule.entrypoint, &capsule.files)?;
    if computed != capsule.manifest_sha256 {
        return Err(WireValidationError::CapsuleManifestMismatch);
    }
    let mut total_bytes = 0_u64;
    for file in &capsule.files {
        let bytes = file.content.decode()?;
        let size =
            u64::try_from(bytes.len()).map_err(|_| WireValidationError::InvalidCapsuleBounds)?;
        if size > MAX_EXECUTION_CAPSULE_FILE_BYTES {
            return Err(WireValidationError::InvalidCapsuleBounds);
        }
        total_bytes = total_bytes
            .checked_add(size)
            .filter(|value| *value <= MAX_EXECUTION_CAPSULE_BYTES)
            .ok_or(WireValidationError::InvalidCapsuleBounds)?;
        if hex::encode(Sha256::digest(&bytes)) != file.sha256 {
            return Err(WireValidationError::CapsuleContentMismatch);
        }
    }
    Ok(ExecutionCapsuleValidation {
        file_count: u32::try_from(capsule.files.len())
            .map_err(|_| WireValidationError::InvalidCapsuleBounds)?,
        total_bytes,
    })
}

fn is_canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
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
        Base64Content, Base64Encoding, ExecutionCapsuleFile, ExecutionCapsuleFileRole,
        ExecutionCapsuleInput, OutputStream, PipeClientFrame, PipeServerFrame,
        canonical_execution_capsule_hash, canonical_json, canonical_query, canonical_request_hash,
        canonical_request_hash_v2, validate_execution_capsule, validate_relative_path,
    };
    use super::{
        CapabilityFacts, PipeSessionStartInput, PtyWindow, SessionMode, validate_session_window,
    };
    use super::{
        EXECUTION_CAPSULE_HASH_DOMAIN, EXECUTION_CAPSULE_MOUNT, MAX_PTY_WINDOW_COLUMNS,
        MAX_PTY_WINDOW_ROWS, MAX_READ_ONLY_ROOTS, ReadOnlyRoot, WireValidationError,
        validate_read_only_roots,
    };
    use serde::Deserialize as _;
    use serde_json::Value;
    use sha2::{Digest as _, Sha256};

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
    fn execution_capsule_hash_binds_order_metadata_and_bytes() {
        let bytes = b"#!/bin/sh\nprintf capsule";
        let mut files = vec![ExecutionCapsuleFile {
            path: "bin/harness".to_owned(),
            role: ExecutionCapsuleFileRole::Runtime,
            executable: true,
            sha256: hex::encode(Sha256::digest(bytes)),
            content: Base64Content {
                encoding: Base64Encoding::Base64,
                data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes),
            },
        }];
        let digest = canonical_execution_capsule_hash("bin/harness", &files).expect("hashes");
        let capsule = ExecutionCapsuleInput {
            manifest_sha256: digest.clone(),
            entrypoint: "bin/harness".to_owned(),
            files: files.clone(),
        };
        let observed = validate_execution_capsule(&capsule).expect("capsule validates");
        assert_eq!(observed.file_count, 1);
        assert_eq!(observed.total_bytes, u64::try_from(bytes.len()).unwrap());

        files[0].content.data = "dGFtcGVyZWQ=".to_owned();
        let tampered = ExecutionCapsuleInput {
            manifest_sha256: digest,
            entrypoint: "bin/harness".to_owned(),
            files,
        };
        assert!(validate_execution_capsule(&tampered).is_err());
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

    #[test]
    fn raw_pipe_frames_are_closed_ordered_and_stream_attributed() {
        let output: PipeServerFrame = serde_json::from_value(serde_json::json!({
            "kind": "output",
            "sequence": 1,
            "stream": "stdout",
            "content": {"encoding": "base64", "data": "aGVsbG8="}
        }))
        .expect("selected frame decodes");
        assert!(matches!(
            output,
            PipeServerFrame::Output {
                sequence: 1,
                stream: OutputStream::Stdout,
                ..
            }
        ));
        assert!(
            serde_json::from_value::<PipeClientFrame>(serde_json::json!({
                "kind": "stdin",
                "sequence": 1,
                "content": {"encoding": "base64", "data": "eA=="},
                "future": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<PipeClientFrame>(serde_json::json!({
                "kind": "resize",
                "sequence": 1
            }))
            .is_err()
        );
    }

    /// Design 13: `SessionMode` grows, `SessionKind` does not, and an omitted `mode` can only ever
    /// mean `pipes` — the mechanical half of design 05 § 2's "a PTY is never substituted for
    /// pipes". A `0.4.0` client that never heard of a terminal cannot be handed one.
    #[test]
    fn an_omitted_session_mode_is_pipes_and_a_pty_start_carries_its_own_window() {
        let start: PipeSessionStartInput = serde_json::from_value(serde_json::json!({
            "exec": exec_start_value(),
            "input_limit_bytes": 65_536,
            "frame_limit_bytes": 4_096,
            "queued_frames": 4
        }))
        .expect("a start without a mode decodes");
        assert_eq!(start.mode, SessionMode::Pipes);
        assert_eq!(start.window, None);

        let pty: PipeSessionStartInput = serde_json::from_value(serde_json::json!({
            "exec": exec_start_value(),
            "input_limit_bytes": 65_536,
            "frame_limit_bytes": 4_096,
            "queued_frames": 4,
            "mode": "pty",
            "window": {"columns": 132, "rows": 40}
        }))
        .expect("a pty start decodes");
        assert_eq!(pty.mode, SessionMode::Pty);
        assert_eq!(
            pty.window,
            Some(PtyWindow {
                columns: 132,
                rows: 40
            })
        );
        assert_eq!(
            serde_json::to_value(SessionMode::Pty).expect("mode serialises"),
            serde_json::json!("pty")
        );
    }

    /// Design 13: the client half of the pty vocabulary is `input`, `resize` and `signal`. The
    /// resize frame carries a window and nothing else, and a resize without one stays outside the
    /// closed vocabulary.
    #[test]
    fn the_resize_frame_carries_a_window_and_joins_the_closed_client_vocabulary() {
        let frame: PipeClientFrame = serde_json::from_value(serde_json::json!({
            "kind": "resize",
            "sequence": 3,
            "window": {"columns": 100, "rows": 30}
        }))
        .expect("a resize frame decodes");
        assert!(matches!(
            frame,
            PipeClientFrame::Resize {
                sequence: 3,
                window: PtyWindow {
                    columns: 100,
                    rows: 30
                }
            }
        ));
        assert!(
            serde_json::from_value::<PipeClientFrame>(serde_json::json!({
                "kind": "resize",
                "sequence": 3,
                "window": {"columns": 100, "rows": 30, "pixels": 4}
            }))
            .is_err(),
            "pixel dimensions are not on the wire"
        );
    }

    /// Design 13: 1–1000 cells on each axis, and **zero is refused rather than mapped to 80×24** —
    /// a zero dimension is how a terminal says *I do not know*, which is not what a client that
    /// sent a resize meant. A `pty` start without a window is refused for the same reason.
    #[test]
    fn a_window_is_one_to_one_thousand_cells_and_is_never_defaulted() {
        assert_eq!(MAX_PTY_WINDOW_COLUMNS, 1_000);
        assert_eq!(MAX_PTY_WINDOW_ROWS, 1_000);
        for (columns, rows) in [(1, 1), (80, 24), (1_000, 1_000)] {
            assert!(
                PtyWindow { columns, rows }.within_bounds(),
                "{columns}x{rows}"
            );
        }
        for (columns, rows) in [(0, 24), (80, 0), (0, 0), (1_001, 24), (80, 1_001)] {
            assert!(
                !PtyWindow { columns, rows }.within_bounds(),
                "{columns}x{rows}"
            );
        }
        let window = PtyWindow {
            columns: 80,
            rows: 24,
        };
        assert_eq!(
            validate_session_window(SessionMode::Pty, None),
            Err(WireValidationError::InvalidSessionWindow),
            "a pty start with no window is refused, never defaulted to 80x24"
        );
        assert_eq!(
            validate_session_window(SessionMode::Pipes, Some(&window)),
            Err(WireValidationError::InvalidSessionWindow),
            "a pipes start carrying a window is refused"
        );
        assert_eq!(
            validate_session_window(
                SessionMode::Pty,
                Some(&PtyWindow {
                    columns: 0,
                    rows: 24
                })
            ),
            Err(WireValidationError::InvalidSessionWindow)
        );
        assert_eq!(
            validate_session_window(SessionMode::Pty, Some(&window)),
            Ok(())
        );
        assert_eq!(validate_session_window(SessionMode::Pipes, None), Ok(()));
    }

    /// Invariant 4 and design 13: `sessions.pty` is a driver fact, absent by default, and carries
    /// its own wire name. Absent means every terminal request is refused by name; it never means
    /// "probably yes".
    #[test]
    fn the_sessions_pty_fact_is_absent_by_default_and_carries_its_wire_name() {
        let facts = CapabilityFacts::default();
        assert_eq!(facts.sessions_pty, None);
        let rendered = serde_json::to_value(&facts).expect("facts serialise");
        assert!(
            rendered.get("sessions.pty").is_none(),
            "an absent fact is absent from the document, not false"
        );
        let published = CapabilityFacts {
            sessions_pty: Some(true),
            ..CapabilityFacts::default()
        };
        assert_eq!(
            serde_json::to_value(&published).expect("facts serialise")["sessions.pty"],
            serde_json::json!(true)
        );
    }

    fn exec_start_value() -> Value {
        serde_json::json!({
            "workspace": "ws_test",
            "argv": ["/bin/sh"],
            "env": {"allow": [], "set": {}},
            "sandbox": {
                "capability_snapshot": format!("sha256:{}", "7".repeat(64)),
                "network": "none",
                "profile": "workspace",
                "require": true
            },
            "limits": {
                "timeout_ms": 5_000,
                "output_bytes": 65_536,
                "processes": 16,
                "memory_bytes": 67_108_864,
                "cpu_millis": 1_000
            },
            "wait": false,
            "read_only_roots": [],
            "secret_slots": [],
            "lease_ttl_ms": 60_000
        })
    }

    /// The capsule hash domain is a wire-visible protocol byte string that another party
    /// reproduces. Nothing else in the suite reads it: every capsule test computes the digest
    /// and feeds it straight back, so the whole workspace passes with an arbitrary domain.
    /// This binds the implementation to the contract that publishes it.
    #[test]
    fn capsule_manifest_hash_domain_matches_the_contract() {
        let hashing: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../contracts/substrate-wire/0.4.0/hashing.json"
        ))
        .expect("hashing authority JSON");
        assert_eq!(
            hashing["capsule_manifest"]["domain"]
                .as_str()
                .expect("declared capsule manifest hash domain"),
            EXECUTION_CAPSULE_HASH_DOMAIN
        );
    }

    #[test]
    fn successor_bundle_manifest_has_reviewed_digest() {
        let bytes = include_bytes!("../../../contracts/substrate-wire/0.4.0/bundle.json");
        assert_eq!(
            hex::encode(Sha256::digest(bytes)),
            "002337bd011a0b68f8680cc157ee4d0424d49392c36a0f85e5fa0449ea4ea0da"
        );
    }

    fn root(host: &str, mount: &str) -> ReadOnlyRoot {
        ReadOnlyRoot {
            host_path: host.to_owned(),
            mount: mount.to_owned(),
        }
    }

    #[test]
    fn a_declared_root_is_admitted_when_both_paths_are_absolute_and_canonical() {
        assert!(
            validate_read_only_roots(&[
                root("/home/someone/.cargo", "/toolchain/cargo"),
                root("/home/someone/.rustup", "/toolchain/rustup"),
            ])
            .is_ok()
        );
        assert!(
            validate_read_only_roots(&[]).is_ok(),
            "and none is the default"
        );
    }

    #[test]
    fn a_root_that_could_escape_or_be_re_pointed_is_refused_rather_than_normalised() {
        // Nothing is adjusted. A request silently re-pointed is a different request, and the
        // caller would have no way to tell which one ran.
        for (host, mount) in [
            ("relative/path", "/toolchain"),
            ("/home/../etc", "/toolchain"),
            ("/home/someone/.cargo", "/toolchain/.."),
            ("/home/someone/.cargo", "toolchain"),
            ("/home/someone/.cargo", "/toolchain/"),
            ("/home//someone", "/toolchain"),
            ("/home/someone/.cargo", "/"),
        ] {
            assert_eq!(
                validate_read_only_roots(&[root(host, mount)]).expect_err("refused"),
                WireValidationError::InvalidReadOnlyRootPath,
                "{host} -> {mount}"
            );
        }
    }

    #[test]
    fn a_root_may_not_take_a_mount_substrate_owns() {
        // Landing on one of these would shadow either the read-only base system the process needs
        // or the workspace it is supposed to write to.
        for mount in [
            "/usr",
            "/bin",
            "/lib",
            "/lib64",
            "/proc",
            "/dev",
            "/tmp",
            "/workspace",
        ] {
            assert_eq!(
                validate_read_only_roots(&[root("/home/someone/.cargo", mount)])
                    .expect_err("refused"),
                WireValidationError::ReservedReadOnlyRootMount,
                "{mount}"
            );
        }
        assert_eq!(
            validate_read_only_roots(&[root("/home/someone/.cargo", EXECUTION_CAPSULE_MOUNT)])
                .expect_err("refused"),
            WireValidationError::ReservedReadOnlyRootMount,
            "including the capsule's, which ADR 0009 owns"
        );
        for mount in ["/runtime/bin", "/usr/local", "/workspace/cache"] {
            assert_eq!(
                validate_read_only_roots(&[root("/home/someone/.cargo", mount)])
                    .expect_err("refused"),
                WireValidationError::ReservedReadOnlyRootMount,
                "{mount}"
            );
        }
        assert!(
            validate_read_only_roots(&[root("/home/someone/.cargo", "/runtime2")]).is_ok(),
            "component comparison must not reject a similarly prefixed name"
        );
    }

    #[test]
    fn two_roots_cannot_name_one_mount_point() {
        // Whichever won, the other would be a directory the caller believed was mounted.
        assert_eq!(
            validate_read_only_roots(&[
                root("/home/someone/.cargo", "/toolchain"),
                root("/home/someone/.rustup", "/toolchain"),
            ])
            .expect_err("refused"),
            WireValidationError::DuplicateReadOnlyRootMount
        );
    }

    #[test]
    fn two_roots_cannot_overlap_mount_trees() {
        assert_eq!(
            validate_read_only_roots(&[
                root("/home/someone/.cargo", "/toolchain"),
                root("/home/someone/.rustup", "/toolchain/rustup"),
            ])
            .expect_err("refused"),
            WireValidationError::OverlappingReadOnlyRootMount
        );
    }

    #[test]
    fn the_bound_is_the_one_the_capability_snapshot_publishes() {
        let too_many: Vec<ReadOnlyRoot> = (0..=MAX_READ_ONLY_ROOTS)
            .map(|index| root("/home/someone/.cargo", &format!("/toolchain/{index}")))
            .collect();
        assert_eq!(
            validate_read_only_roots(&too_many).expect_err("refused"),
            WireValidationError::InvalidReadOnlyRootBounds
        );
    }
}

#[cfg(test)]
mod secret_slot_tests {
    use super::{
        MAX_SECRET_SLOTS, SECRET_SLOTS_ENV, SecretSlotRequest, WireValidationError,
        secret_slot_environment, valid_secret_slot_name, validate_secret_slots,
    };

    fn slot(name: &str, fd: u32) -> SecretSlotRequest {
        SecretSlotRequest {
            slot: name.to_owned(),
            fd,
        }
    }

    #[test]
    fn a_slot_name_is_lowercase_bounded_and_never_empty() {
        for admitted in ["a", "vendor_api_key", "registry_token", "k9"] {
            assert!(valid_secret_slot_name(admitted), "{admitted}");
        }
        for refused in ["", "Vendor", "9lives", "vendor-api-key", "vendor.key", " x"] {
            assert!(!valid_secret_slot_name(refused), "{refused:?}");
        }
        assert!(valid_secret_slot_name(&"a".repeat(64)));
        assert!(!valid_secret_slot_name(&"a".repeat(65)));
    }

    #[test]
    fn descriptors_are_bounded_above_stdio_and_below_the_staging_floor() {
        assert!(validate_secret_slots(&[slot("vendor_api_key", 3)]).is_ok());
        assert!(validate_secret_slots(&[slot("vendor_api_key", 63)]).is_ok());
        assert!(
            validate_secret_slots(&[]).is_ok(),
            "and none is the default"
        );
        for illegal in [0, 1, 2, 64, 4096] {
            assert_eq!(
                validate_secret_slots(&[slot("vendor_api_key", illegal)]).expect_err("refused"),
                WireValidationError::InvalidSecretSlotDescriptor,
                "fd {illegal}"
            );
        }
    }

    #[test]
    fn one_descriptor_and_one_name_are_each_claimed_once() {
        // Whichever won, the other would be a credential the caller believed had arrived.
        assert_eq!(
            validate_secret_slots(&[slot("vendor_api_key", 7), slot("registry_token", 7)])
                .expect_err("refused"),
            WireValidationError::DuplicateSecretSlotDescriptor
        );
        assert_eq!(
            validate_secret_slots(&[slot("vendor_api_key", 7), slot("vendor_api_key", 9)])
                .expect_err("refused"),
            WireValidationError::DuplicateSecretSlotName
        );
    }

    #[test]
    fn the_bound_is_the_one_the_design_publishes() {
        let too_many: Vec<SecretSlotRequest> = (0..=MAX_SECRET_SLOTS)
            .map(|index| slot(&format!("slot_{index}"), 3 + index))
            .collect();
        assert_eq!(
            validate_secret_slots(&too_many).expect_err("refused"),
            WireValidationError::InvalidSecretSlotBounds
        );
    }

    #[test]
    fn the_mapping_is_sorted_by_name_and_absent_when_nothing_is_named() {
        assert_eq!(secret_slot_environment(&[]), None);
        assert_eq!(
            secret_slot_environment(&[slot("vendor_api_key", 7), slot("registry_token", 9)])
                .as_deref(),
            Some("registry_token=9,vendor_api_key=7"),
        );
        assert_eq!(SECRET_SLOTS_ENV, "SUBSTRATE_SECRET_SLOTS");
    }

    #[test]
    fn a_slot_carries_a_name_and_a_number_and_nothing_else() {
        let rendered = serde_json::to_string(&slot("vendor_api_key", 7)).expect("serialize");
        assert_eq!(rendered, r#"{"slot":"vendor_api_key","fd":7}"#);
        // `deny_unknown_fields`: a value, a path or a length cannot be smuggled in beside them.
        assert!(
            serde_json::from_str::<SecretSlotRequest>(
                r#"{"slot":"vendor_api_key","fd":7,"value":"x"}"#
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod egress_aperture_tests {
    use super::{
        ApertureBytes, ApertureMechanism, ApertureMode, AppliedAperture, AppliedNetwork,
        NetworkMode, WireValidationError, valid_aperture_name, validate_aperture_request,
    };

    /// `"none"` stays a bare string, so every reader of an earlier bundle keeps parsing every run
    /// that used no aperture; the aperture branch is an object because it carries four claims.
    #[test]
    fn applied_network_is_a_string_or_an_object_and_round_trips() {
        let none = serde_json::to_value(AppliedNetwork::None).unwrap();
        assert_eq!(none, serde_json::json!("none"));
        assert_eq!(
            serde_json::from_value::<AppliedNetwork>(none).unwrap(),
            AppliedNetwork::None
        );
        let applied = AppliedNetwork::Aperture(AppliedAperture {
            mode: ApertureMode::Aperture,
            name: "model".to_owned(),
            destination: "203.0.113.7:443".to_owned(),
            mechanism: ApertureMechanism::LoopbackForwarder,
            bytes: ApertureBytes {
                to_destination: 12,
                from_destination: 34,
            },
            max_bytes: None,
        });
        let rendered = serde_json::to_value(applied.clone()).unwrap();
        assert_eq!(rendered["mode"], "aperture");
        assert_eq!(rendered["destination"], "203.0.113.7:443");
        assert_eq!(rendered["mechanism"], "loopback-forwarder");
        assert_eq!(rendered["bytes"]["from_destination"], 34);
        assert_eq!(
            serde_json::from_value::<AppliedNetwork>(rendered).unwrap(),
            applied
        );
    }

    /// A name is a name. Anything that reads as a destination is a rejected escalation, told apart
    /// from a configuration typo by name so an operator is not sent looking for the wrong bug.
    #[test]
    fn a_destination_where_a_name_belongs_is_refused_as_such() {
        for escalation in [
            "api.example.com:443",
            "10.0.0.1",
            "10.0.0.1:443",
            "example.com",
            "http://example.com/",
        ] {
            assert_eq!(
                validate_aperture_request(NetworkMode::Aperture, Some(escalation))
                    .expect_err("refused"),
                WireValidationError::ApertureDestinationInRequest,
                "{escalation}"
            );
        }
        assert_eq!(
            validate_aperture_request(NetworkMode::Aperture, Some("Model")).expect_err("refused"),
            WireValidationError::InvalidApertureName
        );
    }

    /// The floor is the default, and the two halves of a selection may not disagree.
    #[test]
    fn the_mode_and_the_name_are_one_statement() {
        assert!(validate_aperture_request(NetworkMode::None, None).is_ok());
        assert!(validate_aperture_request(NetworkMode::Aperture, Some("model")).is_ok());
        assert_eq!(
            validate_aperture_request(NetworkMode::None, Some("model")).expect_err("refused"),
            WireValidationError::ApertureModeMismatch
        );
        assert_eq!(
            validate_aperture_request(NetworkMode::Aperture, None).expect_err("refused"),
            WireValidationError::ApertureModeMismatch
        );
    }

    /// A start that names no aperture serializes no aperture field, so its ledger hash is the hash
    /// it had before this bundle existed.
    #[test]
    fn egress_defaults_to_none() {
        let request = super::ConfinementRequest {
            capability_snapshot: format!("sha256:{}", "0".repeat(64)),
            network: NetworkMode::None,
            aperture: None,
            profile: super::SandboxProfile::Workspace,
            required: true,
        };
        let rendered = serde_json::to_value(&request).unwrap();
        assert!(
            rendered.get("aperture").is_none(),
            "an absent aperture appeared on the wire: {rendered}"
        );
        assert_eq!(rendered["network"], "none");
        assert!(valid_aperture_name("model"));
        assert!(!valid_aperture_name("api.example.com"));
    }
    /// A ceiling is deployment vocabulary. A request that carries one is refused *as a ceiling*,
    /// not as a destination and not as a name typo, so a rejected escalation reads as one
    /// (ADR 0014).
    #[test]
    fn a_ceiling_where_a_name_belongs_is_refused_as_such() {
        for escalation in ["model/max=1MiB", "model/max=1048576", "model/MAX=64MiB"] {
            assert_eq!(
                validate_aperture_request(NetworkMode::Aperture, Some(escalation))
                    .expect_err("refused"),
                WireValidationError::ApertureCeilingInRequest,
                "{escalation}"
            );
        }
        // A destination is still a destination: the ceiling refusal never swallows ADR 0013's.
        assert_eq!(
            validate_aperture_request(NetworkMode::Aperture, Some("api.example.com:443"))
                .expect_err("refused"),
            WireValidationError::ApertureDestinationInRequest
        );
    }

    /// The run states the ceiling it ran under beside the bytes that crossed, and a run declared
    /// without one serializes exactly the bytes a `0.7.0` reader already parses.
    #[test]
    fn the_applied_aperture_states_the_ceiling_it_ran_under() {
        let mut applied = AppliedAperture {
            mode: ApertureMode::Aperture,
            name: "model".to_owned(),
            destination: "203.0.113.7:443".to_owned(),
            mechanism: ApertureMechanism::LoopbackForwarder,
            bytes: ApertureBytes {
                to_destination: 12,
                from_destination: 34,
            },
            max_bytes: None,
        };
        let rendered = serde_json::to_value(&applied).unwrap();
        assert!(
            rendered.get("max_bytes").is_none(),
            "an aperture declared without a ceiling reported one: {rendered}"
        );
        applied.max_bytes = Some(1_048_576);
        let rendered = serde_json::to_value(&applied).unwrap();
        assert_eq!(rendered["max_bytes"], 1_048_576);
        assert_eq!(
            serde_json::from_value::<AppliedAperture>(rendered).unwrap(),
            applied
        );
    }

    /// `/v1/machine` answers how much this daemon could ever pass, and answers nothing when no
    /// ceiling was declared.
    #[test]
    fn the_capability_fact_publishes_the_declared_ceiling() {
        let mut fact = super::EgressApertureFact {
            name: "model".to_owned(),
            destination: "203.0.113.7:443".to_owned(),
            max_bytes: None,
        };
        assert!(
            serde_json::to_value(&fact)
                .unwrap()
                .get("max_bytes")
                .is_none(),
            "an undeclared ceiling was published"
        );
        fact.max_bytes = Some(67_108_864);
        assert_eq!(
            serde_json::to_value(&fact).unwrap()["max_bytes"],
            67_108_864
        );
    }

    /// At `0.7.0` a mid-run bound had nowhere to live: a ceiling and a client cancel were both a
    /// bare `cancelled`. The observation now carries the class, code and message beside that state
    /// — and carries nothing at all for the runs that end the way they always did (ADR 0014).
    #[test]
    fn an_exec_names_the_bound_that_ended_it() {
        let mut exec = super::Exec {
            id: "exec_01JPCEIL".to_owned(),
            kind: super::ExecKind::Exec,
            workspace: "ws_01JPCEIL".to_owned(),
            state: super::ExecState::Cancelled,
            observed_at: chrono::Utc::now(),
            requested: super::ConfinementRequest {
                capability_snapshot: format!("sha256:{}", "0".repeat(64)),
                network: NetworkMode::Aperture,
                aperture: Some("model".to_owned()),
                profile: super::SandboxProfile::Workspace,
                required: true,
            },
            applied: None,
            exit: None,
            lease: None,
            refusal: None,
        };
        let rendered = serde_json::to_value(&exec).unwrap();
        assert!(
            rendered.get("refusal").is_none(),
            "a run that hit no bound named one: {rendered}"
        );
        exec.refusal = Some(super::ExecRefusal {
            class: super::ErrorClass::Exhausted,
            code: "exec.aperture-byte-limit".to_owned(),
            message: "The declared egress aperture byte ceiling was reached.".to_owned(),
        });
        let rendered = serde_json::to_value(&exec).unwrap();
        assert_eq!(rendered["state"], "cancelled");
        assert_eq!(rendered["refusal"]["class"], "exhausted");
        assert_eq!(rendered["refusal"]["code"], "exec.aperture-byte-limit");
        assert!(
            rendered["refusal"]["message"].is_string(),
            "the refusal carries no message: {rendered}"
        );
        assert_eq!(
            serde_json::from_value::<super::Exec>(rendered).unwrap(),
            exec
        );
    }
}
