#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc)] // The crate is an internal persistence boundary.

use parking_lot::{Mutex, RwLock};
use rusqlite::Connection;
use substrate_wire::{
    OPERATION_LEDGER_GLOBAL_MAX_BYTES, OPERATION_LEDGER_GLOBAL_MAX_ROWS,
    OPERATION_LEDGER_SUBJECT_MAX_BYTES, OPERATION_LEDGER_SUBJECT_MAX_ROWS,
};
use thiserror::Error;

mod events;
mod execs;
mod leases;
mod operations;
mod recovery;
mod schema;
mod sessions;
mod snapshots;
mod workspaces;

#[cfg(test)]
mod tests;

pub use events::{CommitEffect, CommitEffectSink, EventCursorError};
pub use execs::{ExecRetireReservation, ExecWrite, StoredExec};
pub use leases::{ExpiredLease, LeaseClock, LeaseResource, NewLease};
pub use operations::{
    NewOperation, OperationCapacity, Reservation, ResourceCapacity, StoredAnswer,
};
pub use recovery::{RecoveryExec, RecoveryWorkspace};
pub use sessions::{
    NewSessionAuthority, SessionAttachmentClaim, SessionAuthorityLookup, SessionAuthorityMint,
    SessionRetireReservation,
};
pub use snapshots::SnapshotReadError;
pub use workspaces::{
    PendingWorkspaceDestroy, Tombstone, WorkspaceAdmission, WorkspaceDestroyReservation,
    WorkspaceObservationWrite,
};

pub const DEFAULT_OPERATION_SUBJECT_MAX_ROWS: u64 = OPERATION_LEDGER_SUBJECT_MAX_ROWS;

pub const DEFAULT_OPERATION_SUBJECT_MAX_BYTES: u64 = OPERATION_LEDGER_SUBJECT_MAX_BYTES;

pub const DEFAULT_OPERATION_GLOBAL_MAX_ROWS: u64 = OPERATION_LEDGER_GLOBAL_MAX_ROWS;

pub const DEFAULT_OPERATION_GLOBAL_MAX_BYTES: u64 = OPERATION_LEDGER_GLOBAL_MAX_BYTES;

pub const DEFAULT_OPERATION_MAX_ROW_BYTES: u64 = 1024 * 1024;

pub const DEFAULT_OPERATION_TERMINAL_HEADROOM_BYTES: u64 = 512 * 1024;

pub const DEFAULT_SNAPSHOT_MAX_WORKSPACES: u64 = 1_024;

pub const DEFAULT_SNAPSHOT_MAX_EXECS: u64 = 2_048;

pub const DEFAULT_SNAPSHOT_MAX_PROVENANCE_EVENTS: u64 = 1_024;

pub const DEFAULT_SNAPSHOT_PRUNE_BATCH_SIZE: u64 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreConfig {
    pub event_retention: u64,
    pub operation_subject_max_rows: u64,
    pub operation_subject_max_bytes: u64,
    pub operation_global_max_rows: u64,
    pub operation_global_max_bytes: u64,
    pub operation_max_row_bytes: u64,
    pub operation_terminal_headroom_bytes: u64,
    pub snapshot_max_workspaces: u64,
    pub snapshot_max_execs: u64,
    pub snapshot_max_provenance_events: u64,
    pub snapshot_prune_batch_size: u64,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            event_retention: 10_000,
            operation_subject_max_rows: DEFAULT_OPERATION_SUBJECT_MAX_ROWS,
            operation_subject_max_bytes: DEFAULT_OPERATION_SUBJECT_MAX_BYTES,
            operation_global_max_rows: DEFAULT_OPERATION_GLOBAL_MAX_ROWS,
            operation_global_max_bytes: DEFAULT_OPERATION_GLOBAL_MAX_BYTES,
            operation_max_row_bytes: DEFAULT_OPERATION_MAX_ROW_BYTES,
            operation_terminal_headroom_bytes: DEFAULT_OPERATION_TERMINAL_HEADROOM_BYTES,
            snapshot_max_workspaces: DEFAULT_SNAPSHOT_MAX_WORKSPACES,
            snapshot_max_execs: DEFAULT_SNAPSHOT_MAX_EXECS,
            snapshot_max_provenance_events: DEFAULT_SNAPSHOT_MAX_PROVENANCE_EVENTS,
            snapshot_prune_batch_size: DEFAULT_SNAPSHOT_PRUNE_BATCH_SIZE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Scope {
    pub deployment: String,
    pub subject: String,
}

pub struct Store {
    connection: Mutex<Connection>,
    event_retention: u64,
    config: StoreConfig,
    effect_sink: RwLock<Option<std::sync::Arc<dyn CommitEffectSink>>>,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("operation {0} is not in an accepted state")]
    NotAccepted(String),
    #[error("stored JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("state database failure: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("stored timestamp is invalid: {0}")]
    Time(#[from] chrono::ParseError),
    #[error("stored response status is outside the HTTP range")]
    StatusRange,
    #[error("lease is absent")]
    LeaseAbsent,
    #[error("lease is expired")]
    LeaseExpired,
    #[error("lease clock is unavailable")]
    LeaseClockUnavailable,
    #[error("lease authority does not match its operation")]
    LeaseAuthorityMismatch,
    #[error("workspace is not ready for this operation")]
    WorkspaceFrozen,
    #[error("stored integer is outside the supported range")]
    IntegerRange,
    #[error("event retention must be nonzero")]
    InvalidEventRetention,
    #[error("operation ledger configuration values must be nonzero and internally consistent")]
    InvalidStoreConfig,
    #[error("operation ledger capacity is exhausted: {0:?}")]
    OperationCapacity(OperationCapacity),
    #[error("configured operation ledger capacity is below durable occupancy: {0:?}")]
    OperationOccupancy(OperationCapacity),
    #[error("operation {0} terminal representation exceeds its reserved headroom")]
    OperationTerminalHeadroom(String),
    #[error("configured current-resource capacity is below durable occupancy: {0:?}")]
    ResourceOccupancy(ResourceCapacity),
    #[error("legacy store exceeds the bounded automatic migration limit; run an offline migration")]
    OfflineMigrationRequired,
    #[error("snapshot retention or materialization limit is exhausted")]
    SnapshotLimit,
    #[error("event does not match the closed Substrate event union")]
    InvalidEventShape,
    #[error("stored session authority does not match its closed shape")]
    InvalidSessionAuthority,
}

fn to_u64(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::IntegerRange)
}

fn to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::IntegerRange)
}
