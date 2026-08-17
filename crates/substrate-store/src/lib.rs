#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc)] // The crate is an internal persistence boundary.

use std::path::Path;

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::Serialize;
use serde_json::{Value, json};
use substrate_wire::{
    ErrorClass, ErrorDetail, Event, EventCause, EventControl, EventPage, Exec, ExecAbsence,
    ExecKind, ExecState, LeaseObservation, LeaseState, OPERATION_LEDGER_GLOBAL_MAX_BYTES,
    OPERATION_LEDGER_GLOBAL_MAX_ROWS, OPERATION_LEDGER_SUBJECT_MAX_BYTES,
    OPERATION_LEDGER_SUBJECT_MAX_ROWS, OperationOutcome, OperationRecord, OperationState,
    PipeSession, SessionAbsence, SessionAttachmentState, SessionKind, SessionState,
    SnapshotHistory, SnapshotItem, SnapshotItemKind, SnapshotMetadata, SnapshotPage,
    SnapshotPartitions, Workspace, WorkspaceState,
};
use thiserror::Error;

const MAX_ACTIVE_SNAPSHOTS_PER_SCOPE: i64 = 64;
const MAX_SNAPSHOT_ITEMS: usize = 4_096;
const MAX_EXPIRED_SNAPSHOT_MARKERS_PER_SCOPE: i64 = 1_024;
const MAX_AUTOMATIC_MIGRATION_ROWS: u64 = 4_096;
const WORKSPACE_CLEANUP_INITIAL_BACKOFF_MS: i64 = 250;
const WORKSPACE_CLEANUP_MAX_BACKOFF_MS: i64 = 30_000;
const LEASE_SWEEPER_ACTOR: &str = "lease-sweeper";
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitEffect {
    pub scope: Scope,
    pub source_scope: String,
    pub generation: u64,
    pub through_seq: u64,
}

pub trait CommitEffectSink: Send + Sync {
    fn committed(&self, effects: &[CommitEffect]);
}

#[derive(Debug, Clone)]
pub struct NewOperation {
    pub scope: Scope,
    pub operation: String,
    pub operation_kind: String,
    pub request_hash: String,
    pub accepted_at: String,
    pub capability_snapshot: Option<String>,
    pub actor: String,
    pub principal: Option<String>,
    pub resource: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredAnswer {
    pub status: u16,
    pub outcome: OperationOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reservation {
    Accepted,
    Replay(StoredAnswer),
    Pending(OperationRecord),
    Conflict,
    Capacity(OperationCapacity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationCapacity {
    SubjectRows,
    SubjectBytes,
    GlobalRows,
    GlobalBytes,
    RowBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceCapacity {
    Workspaces,
    Execs,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecRetireReservation {
    Existing(Reservation),
    Capacity(OperationCapacity),
    Refused(StoredAnswer),
    Retired(substrate_wire::ExecAbsence),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionRetireReservation {
    Existing(Reservation),
    Capacity(OperationCapacity),
    Refused(StoredAnswer),
    Retired(SessionAbsence),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceAdmission {
    Missing,
    Frozen {
        resource: Workspace,
        newly_frozen: bool,
    },
    Admitted {
        root_name: String,
        resource: Workspace,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceObservationWrite {
    Missing,
    Authoritative(Workspace),
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceDestroyReservation {
    Existing(Reservation),
    Capacity(OperationCapacity),
    Missing,
    Frozen {
        resource: Workspace,
        newly_frozen: bool,
    },
    Refused {
        answer: StoredAnswer,
        newly_frozen: bool,
    },
    Admitted {
        root_name: String,
        resource: Workspace,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredExec {
    pub resource: Exec,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub output_complete: bool,
    pub cgroup: Option<String>,
    pub leader_pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecWrite {
    PersistedExact(StoredExec),
    PersistedTransformed(StoredExec),
    Superseded(StoredExec),
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAttachmentClaim {
    Claimed,
    AlreadyClaimed,
    NotAttachable,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseClock {
    pub wall: DateTime<Utc>,
    pub boot_id: String,
    pub boottime_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLease {
    pub ttl_ms: u64,
    pub clock: LeaseClock,
    pub authorizing_operation: String,
    pub actor: String,
    pub principal: Option<String>,
}

impl NewLease {
    pub fn observation(&self) -> LeaseObservation {
        LeaseObservation {
            ttl_ms: self.ttl_ms,
            renew_by: self.clock.wall
                + chrono::Duration::milliseconds(i64::try_from(self.ttl_ms).unwrap_or(i64::MAX)),
            state: LeaseState::Active,
            clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
            authorizing_operation: self.authorizing_operation.clone(),
            actor: self.actor.clone(),
            principal: self.principal.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseResource {
    Workspace { root_name: String },
    Exec { workspace_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiredLease {
    pub scope: Scope,
    pub id: String,
    pub resource: LeaseResource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWorkspaceDestroy {
    pub scope: Scope,
    pub id: String,
    pub root_name: String,
    pub operation: String,
    pub attempt_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryWorkspace {
    pub scope: Scope,
    pub root_name: String,
    pub resource: Workspace,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryExec {
    pub scope: Scope,
    pub stored: StoredExec,
    pub operation: String,
    pub operation_state: OperationState,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tombstone {
    pub kind: String,
    pub id: String,
    pub deleted_at: DateTime<Utc>,
    pub reason: String,
    pub last_observation: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventCursorError {
    Source,
    Retention { first: u64, last: u64 },
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotReadError {
    Expired,
    Incomplete,
    InvalidCursor,
    NotFound,
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
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_with_config(path, StoreConfig::default())
    }

    pub fn open_with_event_retention(
        path: impl AsRef<Path>,
        event_retention: u64,
    ) -> Result<Self, StoreError> {
        Self::open_with_config(
            path,
            StoreConfig {
                event_retention,
                ..StoreConfig::default()
            },
        )
    }

    #[allow(clippy::too_many_lines)] // One reviewable migration defines the complete local schema.
    pub fn open_with_config(
        path: impl AsRef<Path>,
        config: StoreConfig,
    ) -> Result<Self, StoreError> {
        validate_store_config(config)?;
        let mut connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "busy_timeout", 5_000_u64)?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS operations (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                operation TEXT NOT NULL,
                operation_kind TEXT NOT NULL,
                request_hash TEXT NOT NULL,
                state TEXT NOT NULL CHECK (state IN ('refused','accepted','unknown','terminal')),
                accepted_at TEXT,
                terminal_at TEXT,
                capability_snapshot TEXT,
                actor TEXT NOT NULL,
                principal TEXT,
                resource TEXT,
                outcome_json TEXT,
                response_status INTEGER,
                row_bytes INTEGER NOT NULL DEFAULT 0 CHECK (row_bytes >= 0),
                charged_bytes INTEGER NOT NULL DEFAULT 0 CHECK (charged_bytes >= row_bytes),
                PRIMARY KEY (deployment, subject, operation)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS operations_restart_recovery
                ON operations (deployment, state, accepted_at, subject, operation);
            CREATE INDEX IF NOT EXISTS operations_resource_recovery
                ON operations (deployment, subject, resource, operation_kind, accepted_at);

            CREATE TABLE IF NOT EXISTS operation_ledger_usage (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                row_count INTEGER NOT NULL CHECK (row_count >= 0),
                byte_count INTEGER NOT NULL CHECK (byte_count >= 0),
                PRIMARY KEY (deployment, subject)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS operation_ledger_usage_rows
                ON operation_ledger_usage (row_count);
            CREATE INDEX IF NOT EXISTS operation_ledger_usage_bytes
                ON operation_ledger_usage (byte_count);

            CREATE TABLE IF NOT EXISTS operation_ledger_global_usage (
                singleton INTEGER NOT NULL PRIMARY KEY CHECK (singleton = 1),
                row_count INTEGER NOT NULL CHECK (row_count >= 0),
                byte_count INTEGER NOT NULL CHECK (byte_count >= 0)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS store_metadata (
                key TEXT NOT NULL PRIMARY KEY,
                value TEXT NOT NULL
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS workspaces (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                id TEXT NOT NULL,
                root_name TEXT NOT NULL,
                resource_json TEXT NOT NULL,
                PRIMARY KEY (deployment, subject, id),
                UNIQUE (deployment, root_name)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS workspaces_recovery_state
                ON workspaces (deployment, json_extract(resource_json, '$.state'), subject, id);

            CREATE TABLE IF NOT EXISTS workspace_cleanup (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                id TEXT NOT NULL,
                root_name TEXT NOT NULL,
                operation TEXT NOT NULL,
                attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
                progress_batches INTEGER NOT NULL DEFAULT 0 CHECK (progress_batches >= 0),
                removed_items INTEGER NOT NULL DEFAULT 0 CHECK (removed_items >= 0),
                next_attempt_at TEXT NOT NULL,
                last_error TEXT,
                PRIMARY KEY (deployment, subject, id)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS workspace_cleanup_due
                ON workspace_cleanup (deployment, next_attempt_at, subject, id);

            CREATE TABLE IF NOT EXISTS maintenance_cursors (
                deployment TEXT NOT NULL,
                queue TEXT NOT NULL,
                subject TEXT NOT NULL,
                resource_kind TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                PRIMARY KEY (deployment, queue)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS execs (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                resource_json TEXT NOT NULL,
                stdout BLOB NOT NULL DEFAULT X'',
                stderr BLOB NOT NULL DEFAULT X'',
                stdout_truncated INTEGER NOT NULL DEFAULT 0,
                stderr_truncated INTEGER NOT NULL DEFAULT 0,
                output_complete INTEGER NOT NULL DEFAULT 0,
                physically_absent INTEGER NOT NULL DEFAULT 0 CHECK (physically_absent IN (0, 1)),
                cgroup TEXT,
                leader_pid INTEGER,
                PRIMARY KEY (deployment, subject, id)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS execs_recovery_state
                ON execs (
                    deployment,
                    json_extract(resource_json, '$.state'),
                    subject,
                    id
                );

            CREATE TABLE IF NOT EXISTS sessions (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                id TEXT NOT NULL,
                exec_id TEXT NOT NULL,
                resource_json TEXT NOT NULL,
                PRIMARY KEY (deployment, subject, id),
                UNIQUE (deployment, subject, exec_id)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS sessions_recovery_state
                ON sessions (
                    deployment,
                    json_extract(resource_json, '$.state'),
                    subject,
                    id
                );

            CREATE TABLE IF NOT EXISTS stream_meta (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                source_scope TEXT NOT NULL,
                generation INTEGER NOT NULL CHECK (generation > 0),
                next_seq INTEGER NOT NULL CHECK (next_seq > 0),
                PRIMARY KEY (deployment, subject),
                UNIQUE (deployment, source_scope)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS events (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                generation INTEGER NOT NULL,
                seq INTEGER NOT NULL,
                event_json TEXT NOT NULL,
                PRIMARY KEY (deployment, subject, seq)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS events_subject_sequence
                ON events (deployment, subject, seq);

            CREATE TABLE IF NOT EXISTS leases (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                resource_kind TEXT NOT NULL CHECK (resource_kind IN ('workspace','exec')),
                resource_id TEXT NOT NULL,
                ttl_ms INTEGER NOT NULL,
                issued_wall TEXT NOT NULL,
                renew_by_wall TEXT NOT NULL,
                boot_id TEXT NOT NULL,
                issued_boottime_ms INTEGER NOT NULL,
                deadline_boottime_ms INTEGER NOT NULL,
                state TEXT NOT NULL CHECK (state IN ('active','expiring','expired')),
                authorizing_operation TEXT NOT NULL,
                attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
                next_attempt_at TEXT,
                last_error TEXT,
                PRIMARY KEY (deployment, subject, resource_kind, resource_id)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS tombstones (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                resource_kind TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                deleted_at TEXT NOT NULL,
                reason TEXT NOT NULL,
                value_json TEXT NOT NULL,
                PRIMARY KEY (deployment, subject, resource_kind, resource_id)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS snapshots (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                id TEXT NOT NULL,
                source_scope TEXT NOT NULL CHECK (length(source_scope) > 0),
                generation INTEGER NOT NULL,
                through_seq INTEGER NOT NULL,
                item_count INTEGER NOT NULL,
                expires_at TEXT NOT NULL,
                PRIMARY KEY (deployment, subject, id)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS snapshots_maintenance_scan
                ON snapshots (deployment, subject, id, expires_at);

            CREATE TABLE IF NOT EXISTS snapshot_items (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                snapshot_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                item_json TEXT NOT NULL,
                PRIMARY KEY (deployment, subject, snapshot_id, ordinal),
                FOREIGN KEY (deployment, subject, snapshot_id)
                    REFERENCES snapshots (deployment, subject, id) ON DELETE CASCADE
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS expired_snapshots (
                deployment TEXT NOT NULL,
                subject TEXT NOT NULL,
                id TEXT NOT NULL,
                expired_at TEXT NOT NULL,
                PRIMARY KEY (deployment, subject, id)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS expired_snapshots_scope_expiry
                ON expired_snapshots (deployment, subject, expired_at DESC, id DESC);
            ",
        )?;
        migrate_subject_streams(&connection)?;
        migrate_stream_scope_grammar(&connection)?;
        migrate_snapshot_source_scope(&connection)?;
        migrate_lease_authority(&connection)?;
        migrate_exec_physical_absence(&connection)?;
        migrate_workspace_cleanup_progress(&connection)?;
        migrate_operation_ledger_accounting(&mut connection, config)?;
        Ok(Self {
            connection: Mutex::new(connection),
            event_retention: config.event_retention,
            config,
            effect_sink: RwLock::new(None),
        })
    }

    pub fn set_commit_effect_sink(&self, sink: std::sync::Arc<dyn CommitEffectSink>) {
        *self.effect_sink.write() = Some(sink);
    }

    fn report_committed(&self, effects: &[CommitEffect]) {
        if !effects.is_empty()
            && let Some(sink) = self.effect_sink.read().as_ref()
        {
            sink.committed(effects);
        }
    }

    pub fn event_retention(&self) -> u64 {
        self.event_retention
    }

    pub fn reserve(&self, new: &NewOperation) -> Result<Reservation, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(reservation) = existing_reservation(&transaction, new)? {
            transaction.commit()?;
            return Ok(reservation);
        }
        let event =
            match insert_accepted_operation(&transaction, self.event_retention, self.config, new) {
                Ok(event) => event,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(Reservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(&new.scope, &event)]);
        Ok(Reservation::Accepted)
    }

    pub fn reserve_workspace_create(
        &self,
        new: &NewOperation,
        root_name: &str,
        provisional: &Workspace,
        lease: Option<&NewLease>,
    ) -> Result<Reservation, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(reservation) = existing_reservation(&transaction, new)? {
            transaction.commit()?;
            return Ok(reservation);
        }
        if resource_partition_at_capacity(
            &transaction,
            &new.scope,
            "workspaces",
            self.config.snapshot_max_workspaces,
        )? {
            let (reservation, event) = self.persist_resource_capacity_refusal(
                transaction,
                new,
                ResourceCapacity::Workspaces,
            )?;
            drop(connection);
            if let Some(event) = event {
                self.report_committed(&[commit_effect(&new.scope, &event)]);
            }
            return Ok(reservation);
        }
        let event =
            match insert_accepted_operation(&transaction, self.event_retention, self.config, new) {
                Ok(event) => event,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(Reservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
        upsert_workspace(&transaction, &new.scope, root_name, provisional)?;
        if let Some(lease) = lease {
            upsert_lease(
                &transaction,
                &new.scope,
                "workspace",
                &provisional.id,
                lease,
                &new.operation,
            )?;
        }
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(&new.scope, &event)]);
        Ok(Reservation::Accepted)
    }

    pub fn reserve_exec_start(
        &self,
        new: &NewOperation,
        provisional: &StoredExec,
        lease: Option<&NewLease>,
        workspace_clock: Option<&LeaseClock>,
    ) -> Result<Reservation, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(reservation) = existing_reservation(&transaction, new)? {
            transaction.commit()?;
            return Ok(reservation);
        }
        let workspace_json: Option<String> = transaction
            .query_row(
                "SELECT resource_json FROM workspaces
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![
                    new.scope.deployment,
                    new.scope.subject,
                    provisional.resource.workspace
                ],
                |row| row.get(0),
            )
            .optional()?;
        let Some(workspace_json) = workspace_json else {
            return Err(StoreError::NotAccepted(new.operation.clone()));
        };
        let mut workspace: Workspace = serde_json::from_str(&workspace_json)?;
        let (newly_frozen, frozen_event) = freeze_workspace_lease_if_due(
            &transaction,
            self.event_retention,
            &new.scope,
            &provisional.resource.workspace,
            &mut workspace,
            workspace_clock,
        )?;
        if newly_frozen
            || workspace.state != WorkspaceState::Ready
            || workspace
                .lease
                .as_ref()
                .is_some_and(|lease| lease.state != LeaseState::Active)
        {
            transaction.commit()?;
            drop(connection);
            if let Some(event) = frozen_event {
                self.report_committed(&[commit_effect(&new.scope, &event)]);
            }
            return Err(StoreError::WorkspaceFrozen);
        }
        if resource_partition_at_capacity(
            &transaction,
            &new.scope,
            "execs",
            self.config.snapshot_max_execs,
        )? {
            let (reservation, event) =
                self.persist_resource_capacity_refusal(transaction, new, ResourceCapacity::Execs)?;
            drop(connection);
            if let Some(event) = event {
                self.report_committed(&[commit_effect(&new.scope, &event)]);
            }
            return Ok(reservation);
        }
        let event =
            match insert_accepted_operation(&transaction, self.event_retention, self.config, new) {
                Ok(event) => event,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(Reservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
        upsert_exec(&transaction, &new.scope, provisional)?;
        if let Some(lease) = lease {
            upsert_lease(
                &transaction,
                &new.scope,
                "exec",
                &provisional.resource.id,
                lease,
                &new.operation,
            )?;
        }
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(&new.scope, &event)]);
        Ok(Reservation::Accepted)
    }

    /// Atomically reserves a durable pipe session, its private exec, and the exec lease which is
    /// the sole physical cleanup authority for both resources.
    pub fn reserve_pipe_session_start(
        &self,
        new: &NewOperation,
        provisional_session: &PipeSession,
        provisional_exec: &StoredExec,
        lease: &NewLease,
        workspace_clock: Option<&LeaseClock>,
    ) -> Result<Reservation, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(reservation) = existing_reservation(&transaction, new)? {
            transaction.commit()?;
            return Ok(reservation);
        }
        let workspace_json: Option<String> = transaction
            .query_row(
                "SELECT resource_json FROM workspaces
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![
                    new.scope.deployment,
                    new.scope.subject,
                    provisional_exec.resource.workspace
                ],
                |row| row.get(0),
            )
            .optional()?;
        let Some(workspace_json) = workspace_json else {
            return Err(StoreError::NotAccepted(new.operation.clone()));
        };
        let mut workspace: Workspace = serde_json::from_str(&workspace_json)?;
        let (newly_frozen, frozen_event) = freeze_workspace_lease_if_due(
            &transaction,
            self.event_retention,
            &new.scope,
            &provisional_exec.resource.workspace,
            &mut workspace,
            workspace_clock,
        )?;
        if newly_frozen
            || workspace.state != WorkspaceState::Ready
            || workspace
                .lease
                .as_ref()
                .is_some_and(|lease| lease.state != LeaseState::Active)
        {
            transaction.commit()?;
            drop(connection);
            if let Some(event) = frozen_event {
                self.report_committed(&[commit_effect(&new.scope, &event)]);
            }
            return Err(StoreError::WorkspaceFrozen);
        }
        if resource_partition_at_capacity(
            &transaction,
            &new.scope,
            "execs",
            self.config.snapshot_max_execs,
        )? {
            let (reservation, event) =
                self.persist_resource_capacity_refusal(transaction, new, ResourceCapacity::Execs)?;
            drop(connection);
            if let Some(event) = event {
                self.report_committed(&[commit_effect(&new.scope, &event)]);
            }
            return Ok(reservation);
        }
        let event =
            match insert_accepted_operation(&transaction, self.event_retention, self.config, new) {
                Ok(event) => event,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(Reservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
        upsert_exec(&transaction, &new.scope, provisional_exec)?;
        upsert_session(&transaction, &new.scope, provisional_session)?;
        upsert_lease(
            &transaction,
            &new.scope,
            "exec",
            &provisional_exec.resource.id,
            lease,
            &new.operation,
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(&new.scope, &event)]);
        Ok(Reservation::Accepted)
    }

    fn persist_resource_capacity_refusal(
        &self,
        transaction: rusqlite::Transaction<'_>,
        new: &NewOperation,
        capacity: ResourceCapacity,
    ) -> Result<(Reservation, Option<Event>), StoreError> {
        let (code, message, address) = match capacity {
            ResourceCapacity::Workspaces => (
                "workspace.capacity",
                "Current workspace capacity is exhausted; retire or destroy a workspace before retrying with a new operation id.",
                "workspace",
            ),
            ResourceCapacity::Execs => (
                "exec.capacity",
                "Current exec capacity is exhausted; retire a terminal exec before retrying with a new operation id.",
                "exec",
            ),
        };
        let detail = ErrorDetail {
            class: ErrorClass::Exhausted,
            code: code.to_owned(),
            message: message.to_owned(),
            retriable: false,
            address: Some(address.to_owned()),
            operation: Some(new.operation.clone()),
        };
        let (answer, event) = match insert_refused_operation(
            &transaction,
            self.event_retention,
            self.config,
            new,
            &new.accepted_at,
            507,
            &detail,
        ) {
            Ok(value) => value,
            Err(StoreError::OperationCapacity(capacity)) => {
                transaction.rollback()?;
                return Ok((Reservation::Capacity(capacity), None));
            }
            Err(error) => return Err(error),
        };
        transaction.commit()?;
        Ok((Reservation::Replay(answer), Some(event)))
    }

    pub fn inspect_reservation(
        &self,
        scope: &Scope,
        operation: &str,
        request_hash: &str,
    ) -> Result<Option<Reservation>, StoreError> {
        let connection = self.connection.lock();
        let Some(existing) = load_operation(&connection, scope, operation)? else {
            return Ok(None);
        };
        Ok(Some(if existing.record.request_hash != request_hash {
            Reservation::Conflict
        } else if let Some(answer) = existing.answer {
            Reservation::Replay(answer)
        } else {
            Reservation::Pending(existing.record)
        }))
    }

    #[allow(clippy::too_many_lines)] // Reservation, terminal proof, retirement, and event are atomic.
    pub fn retire_exec(
        &self,
        new: &NewOperation,
        id: &str,
        observed_at: DateTime<Utc>,
    ) -> Result<ExecRetireReservation, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(reservation) = existing_reservation(&transaction, new)? {
            transaction.commit()?;
            return Ok(ExecRetireReservation::Existing(reservation));
        }
        if new.operation_kind != "exec.retire" || new.resource.as_deref() != Some(id) {
            return Err(StoreError::NotAccepted(new.operation.clone()));
        }
        let stored = load_exec(&transaction, &new.scope, id)?;
        let session_owned = load_session_for_exec(&transaction, &new.scope, id)?.is_some();
        let refusal = match stored.as_ref() {
            None => Some((
                404,
                ErrorDetail {
                    class: ErrorClass::Refused,
                    code: "resource.not-found".to_owned(),
                    message: "Exec was not found.".to_owned(),
                    retriable: false,
                    address: Some("exec".to_owned()),
                    operation: Some(new.operation.clone()),
                },
            )),
            Some(_) if session_owned => Some((
                409,
                ErrorDetail {
                    class: ErrorClass::Conflict,
                    code: "exec.session-owned".to_owned(),
                    message: "A session-owned exec must be retired through its session.".to_owned(),
                    retriable: false,
                    address: Some("exec".to_owned()),
                    operation: Some(new.operation.clone()),
                },
            )),
            Some(stored) if !is_terminal_exec_state(stored.resource.state) => Some((
                409,
                ErrorDetail {
                    class: ErrorClass::Conflict,
                    code: "exec.not-terminal".to_owned(),
                    message: "Only a durable terminal exec can be retired.".to_owned(),
                    retriable: false,
                    address: Some("exec".to_owned()),
                    operation: Some(new.operation.clone()),
                },
            )),
            Some(_) => None,
        };
        if let Some((status, detail)) = refusal {
            let (answer, event) = match insert_refused_operation(
                &transaction,
                self.event_retention,
                self.config,
                new,
                &observed_at.to_rfc3339(),
                status,
                &detail,
            ) {
                Ok(value) => value,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(ExecRetireReservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
            transaction.commit()?;
            drop(connection);
            self.report_committed(&[commit_effect(&new.scope, &event)]);
            return Ok(ExecRetireReservation::Refused(answer));
        }
        let accepted =
            match insert_accepted_operation(&transaction, self.event_retention, self.config, new) {
                Ok(event) => event,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(ExecRetireReservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
        let absence = ExecAbsence {
            kind: ExecKind::Exec,
            id: id.to_owned(),
            absent: true,
            observed_at,
        };
        let outcome = OperationOutcome::Success {
            result: serde_json::to_value(&absence)?,
        };
        let changed = transaction.execute(
            "UPDATE operations
             SET state = 'terminal', terminal_at = ?4, resource = ?5, outcome_json = ?6,
                 response_status = 200
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3 AND state = 'accepted'",
            params![
                new.scope.deployment,
                new.scope.subject,
                new.operation,
                observed_at.to_rfc3339(),
                id,
                serde_json::to_string(&outcome)?,
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(new.operation.clone()));
        }
        finalize_operation_accounting(&transaction, self.config, &new.scope, &new.operation)?;
        transaction.execute(
            "DELETE FROM leases WHERE deployment = ?1 AND subject = ?2
             AND resource_kind = 'exec' AND resource_id = ?3",
            params![new.scope.deployment, new.scope.subject, id],
        )?;
        transaction.execute(
            "DELETE FROM execs WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![new.scope.deployment, new.scope.subject, id],
        )?;
        let retired = append_event(
            &transaction,
            self.event_retention,
            &new.scope,
            id,
            "exec",
            "exec.retired",
            &observed_at.to_rfc3339(),
            &new.actor,
            new.principal.as_deref(),
            &new.operation,
            Some(serde_json::to_value(&absence)?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[
            commit_effect(&new.scope, &accepted),
            commit_effect(&new.scope, &retired),
        ]);
        Ok(ExecRetireReservation::Retired(absence))
    }

    #[allow(clippy::too_many_lines)]
    pub fn retire_pipe_session(
        &self,
        new: &NewOperation,
        id: &str,
        observed_at: DateTime<Utc>,
    ) -> Result<SessionRetireReservation, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(reservation) = existing_reservation(&transaction, new)? {
            transaction.commit()?;
            return Ok(SessionRetireReservation::Existing(reservation));
        }
        if new.operation_kind != "session.retire" || new.resource.as_deref() != Some(id) {
            return Err(StoreError::NotAccepted(new.operation.clone()));
        }
        let session = load_session(&transaction, &new.scope, id)?;
        let refusal = match session.as_ref() {
            None => Some((
                404,
                ErrorDetail {
                    class: ErrorClass::Refused,
                    code: "resource.not-found".to_owned(),
                    message: "Session was not found.".to_owned(),
                    retriable: false,
                    address: Some("session".to_owned()),
                    operation: Some(new.operation.clone()),
                },
            )),
            Some(value)
                if !matches!(
                    value.state,
                    SessionState::Exited
                        | SessionState::Cancelled
                        | SessionState::Expired
                        | SessionState::Unknown
                ) =>
            {
                Some((
                    409,
                    ErrorDetail {
                        class: ErrorClass::Conflict,
                        code: "session.not-terminal".to_owned(),
                        message: "Only a durable terminal session can be retired.".to_owned(),
                        retriable: false,
                        address: Some("session".to_owned()),
                        operation: Some(new.operation.clone()),
                    },
                ))
            }
            Some(_) => None,
        };
        if let Some((status, detail)) = refusal {
            let (answer, event) = match insert_refused_operation(
                &transaction,
                self.event_retention,
                self.config,
                new,
                &observed_at.to_rfc3339(),
                status,
                &detail,
            ) {
                Ok(value) => value,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(SessionRetireReservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
            transaction.commit()?;
            drop(connection);
            self.report_committed(&[commit_effect(&new.scope, &event)]);
            return Ok(SessionRetireReservation::Refused(answer));
        }
        let Some(session) = session else {
            return Err(StoreError::NotAccepted(new.operation.clone()));
        };
        let accepted =
            match insert_accepted_operation(&transaction, self.event_retention, self.config, new) {
                Ok(event) => event,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(SessionRetireReservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
        let absence = SessionAbsence {
            kind: SessionKind::Session,
            id: id.to_owned(),
            absent: true,
            observed_at,
        };
        let outcome = OperationOutcome::Success {
            result: serde_json::to_value(&absence)?,
        };
        let changed = transaction.execute(
            "UPDATE operations
             SET state = 'terminal', terminal_at = ?4, resource = ?5, outcome_json = ?6,
                 response_status = 200
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3 AND state = 'accepted'",
            params![
                new.scope.deployment,
                new.scope.subject,
                new.operation,
                observed_at.to_rfc3339(),
                id,
                serde_json::to_string(&outcome)?,
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(new.operation.clone()));
        }
        finalize_operation_accounting(&transaction, self.config, &new.scope, &new.operation)?;
        transaction.execute(
            "DELETE FROM leases WHERE deployment = ?1 AND subject = ?2
             AND resource_kind = 'exec' AND resource_id = ?3",
            params![new.scope.deployment, new.scope.subject, session.exec],
        )?;
        transaction.execute(
            "DELETE FROM execs WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![new.scope.deployment, new.scope.subject, session.exec],
        )?;
        transaction.execute(
            "DELETE FROM sessions WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![new.scope.deployment, new.scope.subject, id],
        )?;
        let retired = append_event(
            &transaction,
            self.event_retention,
            &new.scope,
            id,
            "session",
            "session.retired",
            &observed_at.to_rfc3339(),
            &new.actor,
            new.principal.as_deref(),
            &new.operation,
            Some(serde_json::to_value(&absence)?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[
            commit_effect(&new.scope, &accepted),
            commit_effect(&new.scope, &retired),
        ]);
        Ok(SessionRetireReservation::Retired(absence))
    }

    pub fn record_refusal(
        &self,
        new: &NewOperation,
        terminal_at: &str,
        status: u16,
        error: &ErrorDetail,
    ) -> Result<Reservation, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = load_operation(&transaction, &new.scope, &new.operation)? {
            let result = if existing.record.request_hash != new.request_hash {
                Reservation::Conflict
            } else if let Some(answer) = existing.answer {
                Reservation::Replay(answer)
            } else {
                Reservation::Pending(existing.record)
            };
            transaction.commit()?;
            return Ok(result);
        }
        let outcome = OperationOutcome::Error {
            error: error.clone(),
        };
        transaction.execute(
            "INSERT INTO operations (
                deployment, subject, operation, operation_kind, request_hash, state, accepted_at,
                terminal_at, capability_snapshot, actor, principal, resource, outcome_json,
                response_status
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'refused', NULL, ?6, NULL, ?7, ?8, NULL, ?9, ?10)",
            params![
                new.scope.deployment,
                new.scope.subject,
                new.operation,
                new.operation_kind,
                new.request_hash,
                terminal_at,
                new.actor,
                new.principal,
                serde_json::to_string(&outcome)?,
                i64::from(status),
            ],
        )?;
        if let Err(error) =
            charge_new_operation(&transaction, self.config, &new.scope, &new.operation, false)
        {
            return match error {
                StoreError::OperationCapacity(capacity) => {
                    transaction.rollback()?;
                    Ok(Reservation::Capacity(capacity))
                }
                error => Err(error),
            };
        }
        let event = append_event(
            &transaction,
            self.event_retention,
            &new.scope,
            &new.operation,
            "operation",
            "operation.refused",
            terminal_at,
            &new.actor,
            new.principal.as_deref(),
            &new.operation,
            Some(serde_json::to_value(&outcome)?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(&new.scope, &event)]);
        Ok(Reservation::Replay(StoredAnswer { status, outcome }))
    }

    pub fn complete_success<T: Serialize>(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        resource_id: Option<&str>,
        result: &T,
    ) -> Result<(), StoreError> {
        let value = serde_json::to_value(result)?;
        self.complete(
            scope,
            operation,
            terminal_at,
            status,
            resource_id,
            &OperationOutcome::Success { result: value },
            None,
            None,
            None,
        )
    }

    pub fn complete_workspace(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        root_name: &str,
        workspace: &Workspace,
    ) -> Result<(), StoreError> {
        self.complete_workspace_leased(
            scope,
            operation,
            terminal_at,
            status,
            root_name,
            workspace,
            None,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn complete_workspace_leased(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        root_name: &str,
        workspace: &Workspace,
        lease: Option<&NewLease>,
    ) -> Result<(), StoreError> {
        let outcome = OperationOutcome::Success {
            result: serde_json::to_value(workspace)?,
        };
        self.complete(
            scope,
            operation,
            terminal_at,
            status,
            Some(&workspace.id),
            &outcome,
            Some((root_name, workspace)),
            None,
            lease.map(|value| ("workspace", workspace.id.as_str(), value)),
        )
    }

    pub fn complete_workspace_absence<T: Serialize>(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        workspace_id: &str,
        result: &T,
    ) -> Result<(), StoreError> {
        self.complete_workspace_absence_inner(
            scope,
            operation,
            terminal_at,
            status,
            workspace_id,
            result,
            false,
        )
    }

    pub fn complete_reconciled_workspace_absence<T: Serialize>(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        workspace_id: &str,
        result: &T,
    ) -> Result<(), StoreError> {
        self.complete_workspace_absence_inner(
            scope,
            operation,
            terminal_at,
            status,
            workspace_id,
            result,
            true,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn complete_workspace_absence_inner<T: Serialize>(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        workspace_id: &str,
        result: &T,
        reconciled: bool,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if reconciled {
            let workspace_json: Option<String> = transaction
                .query_row(
                    "SELECT resource_json FROM workspaces
                     WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                    params![scope.deployment, scope.subject, workspace_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(workspace_json) = workspace_json else {
                return Err(StoreError::NotAccepted(operation.to_owned()));
            };
            let workspace: Workspace = serde_json::from_str(&workspace_json)?;
            if workspace.state != WorkspaceState::Destroying {
                return Err(StoreError::NotAccepted(operation.to_owned()));
            }
        }
        let outcome = OperationOutcome::Success {
            result: serde_json::to_value(result)?,
        };
        let state_predicate = if reconciled {
            "state IN ('accepted','unknown') AND operation_kind = 'workspace.destroy'"
        } else {
            "state = 'accepted'"
        };
        let changed = transaction.execute(
            &format!(
                "UPDATE operations
                 SET state = 'terminal', terminal_at = ?4, resource = ?5, outcome_json = ?6,
                     response_status = ?7
                 WHERE deployment = ?1 AND subject = ?2 AND operation = ?3
                   AND {state_predicate}"
            ),
            params![
                scope.deployment,
                scope.subject,
                operation,
                terminal_at,
                workspace_id,
                serde_json::to_string(&outcome)?,
                i64::from(status),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(operation.to_owned()));
        }
        finalize_operation_accounting(&transaction, self.config, scope, operation)?;
        transaction.execute(
            "DELETE FROM workspaces WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, workspace_id],
        )?;
        transaction.execute(
            "DELETE FROM workspace_cleanup
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, workspace_id],
        )?;
        transaction.execute(
            "DELETE FROM leases WHERE deployment = ?1 AND subject = ?2
             AND resource_kind = 'workspace' AND resource_id = ?3",
            params![scope.deployment, scope.subject, workspace_id],
        )?;
        insert_tombstone(
            &transaction,
            scope,
            "workspace",
            workspace_id,
            terminal_at,
            "destroyed",
            &serde_json::to_value(result)?,
        )?;
        let operation_row = operation_identity(&transaction, scope, operation)?;
        let event = append_event(
            &transaction,
            self.event_retention,
            scope,
            workspace_id,
            "workspace",
            "workspace.destroyed",
            terminal_at,
            &operation_row.0,
            operation_row.1.as_deref(),
            operation,
            Some(serde_json::to_value(result)?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete_exec(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        resource: &Exec,
        stdout: &[u8],
        stderr: &[u8],
        stdout_truncated: bool,
        stderr_truncated: bool,
        output_complete: bool,
        cgroup: Option<&str>,
        leader_pid: Option<u32>,
    ) -> Result<ExecWrite, StoreError> {
        self.complete_exec_leased(
            scope,
            operation,
            terminal_at,
            status,
            resource,
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
            output_complete,
            cgroup,
            leader_pid,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete_exec_leased(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        resource: &Exec,
        stdout: &[u8],
        stderr: &[u8],
        stdout_truncated: bool,
        stderr_truncated: bool,
        output_complete: bool,
        cgroup: Option<&str>,
        leader_pid: Option<u32>,
        lease: Option<&NewLease>,
    ) -> Result<ExecWrite, StoreError> {
        let proposed = StoredExec {
            resource: resource.clone(),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
            stdout_truncated,
            stderr_truncated,
            output_complete,
            cgroup: cgroup.map(ToOwned::to_owned),
            leader_pid,
        };
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = load_exec(&transaction, scope, &resource.id)?;
        let Some(previous) = previous else {
            transaction.commit()?;
            return Ok(ExecWrite::Retired);
        };
        let (authoritative, exact) =
            if is_terminal_exec_state(previous.resource.state) && previous != proposed {
                (previous, false)
            } else {
                (proposed.clone(), true)
            };
        let outcome = OperationOutcome::Success {
            result: serde_json::to_value(&authoritative.resource)?,
        };
        let changed = transaction.execute(
            "UPDATE operations
             SET state = 'terminal', terminal_at = ?4, resource = ?5, outcome_json = ?6,
                 response_status = ?7
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3
               AND state IN ('accepted','unknown')",
            params![
                scope.deployment,
                scope.subject,
                operation,
                terminal_at,
                authoritative.resource.id,
                serde_json::to_string(&outcome)?,
                i64::from(status),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(operation.to_owned()));
        }
        finalize_operation_accounting(&transaction, self.config, scope, operation)?;
        if exact {
            upsert_exec(&transaction, scope, &authoritative)?;
            if let Some(lease) = lease {
                upsert_lease(
                    &transaction,
                    scope,
                    "exec",
                    &authoritative.resource.id,
                    lease,
                    operation,
                )?;
            }
        }
        let (actor, principal, operation_kind) =
            operation_identity_full(&transaction, scope, operation)?;
        let event = append_event(
            &transaction,
            self.event_retention,
            scope,
            &authoritative.resource.id,
            "exec",
            terminal_transition(&operation_kind, &outcome),
            terminal_at,
            &actor,
            principal.as_deref(),
            operation,
            Some(serde_json::to_value(&authoritative.resource)?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(if exact {
            ExecWrite::PersistedExact(authoritative)
        } else {
            ExecWrite::Superseded(authoritative)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete_pipe_session_start(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        session: &PipeSession,
        exec: &StoredExec,
        lease: &NewLease,
    ) -> Result<(PipeSession, StoredExec), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load_exec(&transaction, scope, &exec.resource.id)?.is_none()
            || load_session(&transaction, scope, &session.id)?.is_none()
        {
            transaction.commit()?;
            return Err(StoreError::NotAccepted(operation.to_owned()));
        }
        let outcome = OperationOutcome::Success {
            result: serde_json::to_value(session)?,
        };
        let changed = transaction.execute(
            "UPDATE operations
             SET state = 'terminal', terminal_at = ?4, resource = ?5, outcome_json = ?6,
                 response_status = ?7
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3
               AND state IN ('accepted','unknown')",
            params![
                scope.deployment,
                scope.subject,
                operation,
                terminal_at,
                session.id,
                serde_json::to_string(&outcome)?,
                i64::from(status),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(operation.to_owned()));
        }
        finalize_operation_accounting(&transaction, self.config, scope, operation)?;
        upsert_exec(&transaction, scope, exec)?;
        upsert_session(&transaction, scope, session)?;
        upsert_lease(
            &transaction,
            scope,
            "exec",
            &exec.resource.id,
            lease,
            operation,
        )?;
        let (actor, principal, _) = operation_identity_full(&transaction, scope, operation)?;
        let event = append_event(
            &transaction,
            self.event_retention,
            scope,
            &session.id,
            "session",
            "session.ready",
            terminal_at,
            &actor,
            principal.as_deref(),
            operation,
            Some(serde_json::to_value(session)?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok((session.clone(), exec.clone()))
    }

    pub fn complete_pipe_session_observation(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        session_id: &str,
        exec: &StoredExec,
    ) -> Result<PipeSession, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(previous_exec) = load_exec(&transaction, scope, &exec.resource.id)? else {
            return Err(StoreError::NotAccepted(operation.to_owned()));
        };
        let authoritative = if is_terminal_exec_state(previous_exec.resource.state) {
            previous_exec
        } else {
            exec.clone()
        };
        upsert_exec(&transaction, scope, &authoritative)?;
        let _projection = project_session_from_exec(&transaction, scope, &authoritative.resource)?;
        let session = load_session(&transaction, scope, session_id)?
            .ok_or_else(|| StoreError::NotAccepted(operation.to_owned()))?;
        let outcome = OperationOutcome::Success {
            result: serde_json::to_value(&session)?,
        };
        let changed = transaction.execute(
            "UPDATE operations
             SET state = 'terminal', terminal_at = ?4, resource = ?5, outcome_json = ?6,
                 response_status = ?7
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3 AND state = 'accepted'",
            params![
                scope.deployment,
                scope.subject,
                operation,
                terminal_at,
                session_id,
                serde_json::to_string(&outcome)?,
                i64::from(status),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(operation.to_owned()));
        }
        finalize_operation_accounting(&transaction, self.config, scope, operation)?;
        let (actor, principal) = operation_identity(&transaction, scope, operation)?;
        let event = append_event(
            &transaction,
            self.event_retention,
            scope,
            session_id,
            "session",
            session_transition(session.state),
            terminal_at,
            &actor,
            principal.as_deref(),
            operation,
            Some(serde_json::to_value(&session)?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(session)
    }

    pub fn complete_error(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        resource_id: Option<&str>,
        error: &ErrorDetail,
    ) -> Result<(), StoreError> {
        self.complete(
            scope,
            operation,
            terminal_at,
            status,
            resource_id,
            &OperationOutcome::Error {
                error: error.clone(),
            },
            None,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // One atomic operation/resource/error terminal boundary.
    pub fn complete_dispatch_absence(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        resource_kind: &str,
        resource_id: &str,
        error: &ErrorDetail,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let event = complete_operation_error_transaction(
            &transaction,
            self.event_retention,
            self.config,
            scope,
            operation,
            terminal_at,
            status,
            Some(resource_id),
            error,
        )?;
        let table = match resource_kind {
            "workspace" => "workspaces",
            "exec" => "execs",
            _ => return Err(StoreError::NotAccepted(operation.to_owned())),
        };
        transaction.execute(
            &format!(
                "DELETE FROM {table}
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3"
            ),
            params![scope.deployment, scope.subject, resource_id],
        )?;
        transaction.execute(
            "DELETE FROM leases WHERE deployment = ?1 AND subject = ?2
             AND resource_kind = ?3 AND resource_id = ?4",
            params![scope.deployment, scope.subject, resource_kind, resource_id],
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete_pipe_session_dispatch_absence(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        session_id: &str,
        exec_id: &str,
        error: &ErrorDetail,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let event = complete_operation_error_transaction(
            &transaction,
            self.event_retention,
            self.config,
            scope,
            operation,
            terminal_at,
            status,
            Some(session_id),
            error,
        )?;
        transaction.execute(
            "DELETE FROM sessions WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, session_id],
        )?;
        transaction.execute(
            "DELETE FROM execs WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, exec_id],
        )?;
        transaction.execute(
            "DELETE FROM leases WHERE deployment = ?1 AND subject = ?2
             AND resource_kind = 'exec' AND resource_id = ?3",
            params![scope.deployment, scope.subject, exec_id],
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(())
    }

    pub fn mark_dispatch_unknown(
        &self,
        scope: &Scope,
        operation: &str,
        observed_at: &str,
        resource_kind: &str,
        resource_id: &str,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE operations SET state = 'unknown'
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3 AND state = 'accepted'",
            params![scope.deployment, scope.subject, operation],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(operation.to_owned()));
        }
        refresh_nonterminal_operation_accounting(&transaction, self.config, scope, operation)?;
        if resource_kind == "exec" {
            let json: String = transaction.query_row(
                "SELECT resource_json FROM execs
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![scope.deployment, scope.subject, resource_id],
                |row| row.get(0),
            )?;
            let mut resource: Exec = serde_json::from_str(&json)?;
            resource.state = ExecState::Unknown;
            resource.observed_at = observed_at.parse()?;
            transaction.execute(
                "UPDATE execs SET resource_json = ?4
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![
                    scope.deployment,
                    scope.subject,
                    resource_id,
                    serde_json::to_string(&resource)?
                ],
            )?;
        }
        let (actor, principal) = operation_identity(&transaction, scope, operation)?;
        let event = append_event(
            &transaction,
            self.event_retention,
            scope,
            resource_id,
            resource_kind,
            "operation.unknown",
            observed_at,
            &actor,
            principal.as_deref(),
            operation,
            Some(json!({ "state": "unknown", "reason": "dispatch-outcome-unproven" })),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(())
    }

    pub fn mark_pipe_session_dispatch_unknown(
        &self,
        scope: &Scope,
        operation: &str,
        observed_at: DateTime<Utc>,
        session_id: &str,
        exec_id: &str,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE operations SET state = 'unknown'
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3 AND state = 'accepted'",
            params![scope.deployment, scope.subject, operation],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(operation.to_owned()));
        }
        refresh_nonterminal_operation_accounting(&transaction, self.config, scope, operation)?;
        let mut exec = load_exec(&transaction, scope, exec_id)?
            .ok_or_else(|| StoreError::NotAccepted(operation.to_owned()))?;
        exec.resource.state = ExecState::Unknown;
        exec.resource.observed_at = observed_at;
        exec.output_complete = true;
        upsert_exec(&transaction, scope, &exec)?;
        let mut session = load_session(&transaction, scope, session_id)?
            .ok_or_else(|| StoreError::NotAccepted(operation.to_owned()))?;
        session.state = SessionState::Unknown;
        session.attachment = SessionAttachmentState::Uncertain;
        session.observed_at = observed_at;
        upsert_session(&transaction, scope, &session)?;
        let (actor, principal) = operation_identity(&transaction, scope, operation)?;
        let event = append_event(
            &transaction,
            self.event_retention,
            scope,
            session_id,
            "session",
            "session.unknown",
            &observed_at.to_rfc3339(),
            &actor,
            principal.as_deref(),
            operation,
            Some(serde_json::to_value(&session)?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn complete(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        resource_id: Option<&str>,
        outcome: &OperationOutcome,
        workspace: Option<(&str, &Workspace)>,
        exec: Option<&StoredExec>,
        lease: Option<(&str, &str, &NewLease)>,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE operations
             SET state = 'terminal', terminal_at = ?4, resource = ?5, outcome_json = ?6,
                 response_status = ?7
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3
               AND state IN ('accepted','unknown')",
            params![
                scope.deployment,
                scope.subject,
                operation,
                terminal_at,
                resource_id,
                serde_json::to_string(outcome)?,
                i64::from(status),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(operation.to_owned()));
        }
        finalize_operation_accounting(&transaction, self.config, scope, operation)?;
        if let Some((root_name, resource)) = workspace {
            upsert_workspace(&transaction, scope, root_name, resource)?;
        }
        if let Some(resource) = exec {
            upsert_exec(&transaction, scope, resource)?;
        }
        if let Some((kind, id, lease)) = lease {
            upsert_lease(&transaction, scope, kind, id, lease, operation)?;
        }
        let (actor, principal, operation_kind) =
            operation_identity_full(&transaction, scope, operation)?;
        let transition = terminal_transition(&operation_kind, outcome);
        let event = append_event(
            &transaction,
            self.event_retention,
            scope,
            resource_id.unwrap_or(operation),
            operation_resource_kind(&operation_kind),
            transition,
            terminal_at,
            &actor,
            principal.as_deref(),
            operation,
            Some(match outcome {
                OperationOutcome::Success { result } => result.clone(),
                OperationOutcome::Error { .. } => serde_json::to_value(outcome)?,
            }),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(())
    }

    #[cfg(test)]
    fn put_workspace(
        &self,
        scope: &Scope,
        root_name: &str,
        workspace: &Workspace,
    ) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        upsert_workspace(&connection, scope, root_name, workspace)
    }

    pub fn merge_workspace_observation(
        &self,
        scope: &Scope,
        root_name: &str,
        observed: &Workspace,
    ) -> Result<WorkspaceObservationWrite, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = transaction
            .query_row(
                "SELECT root_name, resource_json FROM workspaces
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![scope.deployment, scope.subject, observed.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((durable_root, json)) = row else {
            transaction.commit()?;
            return Ok(WorkspaceObservationWrite::Missing);
        };
        if durable_root != root_name {
            transaction.commit()?;
            return Ok(WorkspaceObservationWrite::Missing);
        }
        let mut durable: Workspace = serde_json::from_str(&json)?;
        let frozen_lease = durable
            .lease
            .as_ref()
            .is_some_and(|lease| lease.state != LeaseState::Active);
        if durable.state == WorkspaceState::Ready && !frozen_lease {
            // The host observation proves only that the predeclared root is present. Lifecycle,
            // labels, and lease authority remain store-owned and cannot be replaced by a stale
            // observation captured before a concurrent freeze.
            durable.observed_at = observed.observed_at;
            transaction.execute(
                "UPDATE workspaces SET resource_json = ?4
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![
                    scope.deployment,
                    scope.subject,
                    observed.id,
                    serde_json::to_string(&durable)?,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(WorkspaceObservationWrite::Authoritative(durable))
    }

    pub fn admit_workspace(
        &self,
        scope: &Scope,
        id: &str,
        clock: Option<&LeaseClock>,
    ) -> Result<WorkspaceAdmission, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = transaction
            .query_row(
                "SELECT root_name, resource_json FROM workspaces
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![scope.deployment, scope.subject, id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((root_name, json)) = row else {
            transaction.commit()?;
            return Ok(WorkspaceAdmission::Missing);
        };
        let mut resource: Workspace = serde_json::from_str(&json)?;
        let (newly_frozen, event) = freeze_workspace_lease_if_due(
            &transaction,
            self.event_retention,
            scope,
            id,
            &mut resource,
            clock,
        )?;
        let frozen = resource.state != WorkspaceState::Ready
            || resource
                .lease
                .as_ref()
                .is_some_and(|lease| lease.state != LeaseState::Active);
        transaction.commit()?;
        drop(connection);
        if let Some(event) = event {
            self.report_committed(&[commit_effect(scope, &event)]);
        }
        Ok(if frozen {
            WorkspaceAdmission::Frozen {
                resource,
                newly_frozen,
            }
        } else {
            WorkspaceAdmission::Admitted {
                root_name,
                resource,
            }
        })
    }

    #[cfg(test)]
    fn mark_workspace_destroying(
        &self,
        scope: &Scope,
        id: &str,
        observed_at: DateTime<Utc>,
    ) -> Result<Option<(String, Workspace)>, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if workspace_has_nonterminal_execs(&transaction, scope, id)? {
            transaction.commit()?;
            return Ok(None);
        }
        let row = transaction
            .query_row(
                "SELECT root_name, resource_json FROM workspaces
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![scope.deployment, scope.subject, id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((root_name, json)) = row else {
            transaction.commit()?;
            return Ok(None);
        };
        let mut resource: Workspace = serde_json::from_str(&json)?;
        if resource.state != WorkspaceState::Ready {
            transaction.commit()?;
            return Ok(None);
        }
        resource.state = WorkspaceState::Destroying;
        resource.observed_at = observed_at;
        transaction.execute(
            "UPDATE workspaces SET resource_json = ?4
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![
                scope.deployment,
                scope.subject,
                id,
                serde_json::to_string(&resource)?
            ],
        )?;
        let operation = transaction
            .query_row(
                "SELECT operation FROM operations
                 WHERE deployment = ?1 AND subject = ?2 AND resource = ?3
                   AND operation_kind = 'workspace.destroy'
                   AND state IN ('accepted','unknown')
                 ORDER BY accepted_at, operation LIMIT 1",
                params![scope.deployment, scope.subject, id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(operation) = operation {
            transaction.execute(
                "INSERT INTO workspace_cleanup (
                    deployment, subject, id, root_name, operation, attempt_count,
                    next_attempt_at, last_error
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, NULL)
                 ON CONFLICT (deployment, subject, id) DO NOTHING",
                params![
                    scope.deployment,
                    scope.subject,
                    id,
                    root_name,
                    operation,
                    observed_at.to_rfc3339(),
                ],
            )?;
        }
        transaction.commit()?;
        Ok(Some((root_name, resource)))
    }

    #[allow(clippy::too_many_lines)] // Admission, refusal, and cleanup ownership commit atomically.
    pub fn reserve_workspace_destroy(
        &self,
        new: &NewOperation,
        clock: Option<&LeaseClock>,
    ) -> Result<WorkspaceDestroyReservation, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(reservation) = existing_reservation(&transaction, new)? {
            transaction.commit()?;
            return Ok(WorkspaceDestroyReservation::Existing(reservation));
        }
        let Some(id) = new.resource.as_deref() else {
            return Err(StoreError::NotAccepted(new.operation.clone()));
        };
        let row = transaction
            .query_row(
                "SELECT root_name, resource_json FROM workspaces
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![new.scope.deployment, new.scope.subject, id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((root_name, json)) = row else {
            transaction.commit()?;
            return Ok(WorkspaceDestroyReservation::Missing);
        };
        let mut resource: Workspace = serde_json::from_str(&json)?;
        let (newly_frozen, lease_event) = freeze_workspace_lease_if_due(
            &transaction,
            self.event_retention,
            &new.scope,
            id,
            &mut resource,
            clock,
        )?;
        let frozen = resource.state != WorkspaceState::Ready
            || resource
                .lease
                .as_ref()
                .is_some_and(|lease| lease.state != LeaseState::Active);
        if frozen {
            let detail = ErrorDetail {
                class: ErrorClass::Conflict,
                code: "workspace.not-ready".to_owned(),
                message: "Workspace is not ready for destruction.".to_owned(),
                retriable: false,
                address: Some("workspace".to_owned()),
                operation: Some(new.operation.clone()),
            };
            let answer = match insert_refused_operation(
                &transaction,
                self.event_retention,
                self.config,
                new,
                &new.accepted_at,
                409,
                &detail,
            ) {
                Ok(answer) => answer,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(WorkspaceDestroyReservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
            transaction.commit()?;
            drop(connection);
            let mut effects = Vec::new();
            if let Some(event) = lease_event {
                effects.push(commit_effect(&new.scope, &event));
            }
            effects.push(commit_effect(&new.scope, &answer.1));
            self.report_committed(&effects);
            return Ok(WorkspaceDestroyReservation::Refused {
                answer: answer.0,
                newly_frozen,
            });
        }
        if workspace_has_nonterminal_execs(&transaction, &new.scope, id)? {
            let detail = ErrorDetail {
                class: ErrorClass::Conflict,
                code: "workspace.execs-active".to_owned(),
                message: "Workspace has nonterminal execs.".to_owned(),
                retriable: false,
                address: Some("workspace".to_owned()),
                operation: Some(new.operation.clone()),
            };
            let (answer, event) = match insert_refused_operation(
                &transaction,
                self.event_retention,
                self.config,
                new,
                &new.accepted_at,
                409,
                &detail,
            ) {
                Ok(answer) => answer,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(WorkspaceDestroyReservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
            transaction.commit()?;
            drop(connection);
            self.report_committed(&[commit_effect(&new.scope, &event)]);
            return Ok(WorkspaceDestroyReservation::Refused {
                answer,
                newly_frozen: false,
            });
        }
        let accepted_event =
            match insert_accepted_operation(&transaction, self.event_retention, self.config, new) {
                Ok(event) => event,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(WorkspaceDestroyReservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
        resource.state = WorkspaceState::Destroying;
        resource.observed_at = new.accepted_at.parse()?;
        transaction.execute(
            "UPDATE workspaces SET resource_json = ?4
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![
                new.scope.deployment,
                new.scope.subject,
                id,
                serde_json::to_string(&resource)?
            ],
        )?;
        transaction.execute(
            "INSERT INTO workspace_cleanup (
                deployment, subject, id, root_name, operation, attempt_count, next_attempt_at,
                last_error
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, NULL)
             ON CONFLICT (deployment, subject, id) DO UPDATE SET
                root_name = excluded.root_name,
                operation = excluded.operation,
                attempt_count = 0,
                next_attempt_at = excluded.next_attempt_at,
                last_error = NULL",
            params![
                new.scope.deployment,
                new.scope.subject,
                id,
                root_name,
                new.operation,
                resource.observed_at.to_rfc3339(),
            ],
        )?;
        transaction.commit()?;
        drop(connection);
        let mut effects = Vec::with_capacity(2);
        if let Some(event) = lease_event {
            effects.push(commit_effect(&new.scope, &event));
        }
        effects.push(commit_effect(&new.scope, &accepted_event));
        self.report_committed(&effects);
        Ok(WorkspaceDestroyReservation::Admitted {
            root_name,
            resource,
        })
    }

    pub fn workspace(
        &self,
        scope: &Scope,
        id: &str,
    ) -> Result<Option<(String, Workspace)>, StoreError> {
        let connection = self.connection.lock();
        connection
            .query_row(
                "SELECT root_name, resource_json FROM workspaces
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![scope.deployment, scope.subject, id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .map(|(root_name, json)| Ok((root_name, serde_json::from_str(&json)?)))
            .transpose()
    }

    pub fn due_destroying_workspaces(
        &self,
        deployment: &str,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<PendingWorkspaceDestroy>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT c.subject, c.id, c.root_name, w.resource_json, c.operation, c.attempt_count
             FROM workspace_cleanup AS c
             JOIN workspaces AS w
               ON w.deployment = c.deployment AND w.subject = c.subject AND w.id = c.id
             JOIN operations AS o
               ON o.deployment = c.deployment AND o.subject = c.subject
              AND o.operation = c.operation
             WHERE c.deployment = ?1 AND c.next_attempt_at <= ?2
               AND o.operation_kind = 'workspace.destroy'
               AND o.state IN ('accepted','unknown')
             ORDER BY c.next_attempt_at, c.subject, c.id
             LIMIT ?3",
        )?;
        let rows = statement
            .query_map(
                params![deployment, now.to_rfc3339(), to_i64(limit as u64)?],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let mut result = Vec::new();
        for (subject, id, root_name, resource_json, operation, attempt_count) in rows {
            let resource: Workspace = serde_json::from_str(&resource_json)?;
            if resource.state != WorkspaceState::Destroying {
                continue;
            }
            result.push(PendingWorkspaceDestroy {
                scope: Scope {
                    deployment: deployment.to_owned(),
                    subject,
                },
                id,
                root_name,
                operation,
                attempt_count: u32::try_from(attempt_count)
                    .map_err(|_| StoreError::IntegerRange)?,
            });
        }
        Ok(result)
    }

    pub fn record_workspace_cleanup_failure(
        &self,
        pending: &PendingWorkspaceDestroy,
        observed_at: DateTime<Utc>,
        code: &str,
    ) -> Result<DateTime<Utc>, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next_attempt = pending.attempt_count.saturating_add(1);
        let shift = next_attempt.saturating_sub(1).min(30);
        let multiplier = 1_i64.checked_shl(shift).unwrap_or(i64::MAX);
        let delay_ms = WORKSPACE_CLEANUP_INITIAL_BACKOFF_MS
            .saturating_mul(multiplier)
            .min(WORKSPACE_CLEANUP_MAX_BACKOFF_MS);
        let next_attempt_at = observed_at + chrono::Duration::milliseconds(delay_ms);
        let changed = transaction.execute(
            "UPDATE workspace_cleanup
             SET attempt_count = ?4, next_attempt_at = ?5, last_error = ?6
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3 AND operation = ?7",
            params![
                pending.scope.deployment,
                pending.scope.subject,
                pending.id,
                i64::from(next_attempt),
                next_attempt_at.to_rfc3339(),
                code,
                pending.operation,
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(pending.operation.clone()));
        }
        let (actor, principal) =
            operation_identity(&transaction, &pending.scope, &pending.operation)?;
        let event = append_event(
            &transaction,
            self.event_retention,
            &pending.scope,
            &pending.id,
            "workspace",
            "workspace.cleanup-failed",
            &observed_at.to_rfc3339(),
            &actor,
            principal.as_deref(),
            &pending.operation,
            Some(serde_json::to_value(ErrorDetail {
                class: ErrorClass::Failed,
                code: code.to_owned(),
                message: "Workspace cleanup failed; the daemon will retry with bounded backoff."
                    .to_owned(),
                retriable: true,
                address: Some(pending.id.clone()),
                operation: Some(pending.operation.clone()),
            })?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(&pending.scope, &event)]);
        Ok(next_attempt_at)
    }

    pub fn record_workspace_cleanup_progress(
        &self,
        pending: &PendingWorkspaceDestroy,
        observed_at: DateTime<Utc>,
        removed_items: u64,
    ) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        let changed = connection.execute(
            "UPDATE workspace_cleanup
             SET progress_batches = progress_batches + 1,
                 removed_items = removed_items + ?4,
                 next_attempt_at = ?5,
                 last_error = NULL
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3 AND operation = ?6",
            params![
                pending.scope.deployment,
                pending.scope.subject,
                pending.id,
                to_i64(removed_items)?,
                observed_at.to_rfc3339(),
                pending.operation,
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(pending.operation.clone()));
        }
        Ok(())
    }

    #[cfg(test)]
    fn remove_workspace(&self, scope: &Scope, id: &str) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        connection.execute(
            "DELETE FROM workspaces WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, id],
        )?;
        Ok(())
    }

    pub fn put_exec(&self, scope: &Scope, resource: &StoredExec) -> Result<ExecWrite, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous_stored = load_exec(&transaction, scope, &resource.resource.id)?;
        if previous_stored.is_none() {
            transaction.commit()?;
            return Ok(ExecWrite::Retired);
        }
        let mut resource = resource.clone();
        let mut transformed = false;
        if resource.resource.lease.is_none()
            && let Some(previous_lease) = previous_stored
                .as_ref()
                .and_then(|previous| previous.resource.lease.clone())
        {
            resource.resource.lease = Some(previous_lease);
            transformed = true;
        }
        if previous_stored.as_ref() == Some(&resource) {
            transaction.commit()?;
            return Ok(if transformed {
                ExecWrite::PersistedTransformed(resource)
            } else {
                ExecWrite::PersistedExact(resource)
            });
        }
        if let Some(previous) = previous_stored.as_ref()
            && is_terminal_exec_state(previous.resource.state)
        {
            transaction.commit()?;
            return Ok(ExecWrite::Superseded(previous.clone()));
        }
        let previous = previous_stored.map(|stored| stored.resource);
        upsert_exec(&transaction, scope, &resource)?;
        let projected_session = project_session_from_exec(&transaction, scope, &resource.resource)?;
        let mut effects = Vec::new();
        if previous.as_ref().map(|value| value.state) != Some(resource.resource.state)
            && let Some((operation, actor, principal)) =
                resource_operation_identity(&transaction, scope, &resource.resource.id)?
        {
            let transition = match resource.resource.state {
                ExecState::Accepted => "exec.accepted",
                ExecState::Running => "exec.running",
                ExecState::Exited => "exec.exited",
                ExecState::Cancelled => "exec.cancelled",
                ExecState::Expired => "exec.lease-expired",
                ExecState::Unknown => "exec.unknown",
            };
            let event = append_event(
                &transaction,
                self.event_retention,
                scope,
                &resource.resource.id,
                "exec",
                transition,
                &resource.resource.observed_at.to_rfc3339(),
                &actor,
                principal.as_deref(),
                &operation,
                Some(serde_json::to_value(&resource.resource)?),
            )?;
            effects.push(commit_effect(scope, &event));
        }
        if let Some((session, previous_state)) = projected_session
            && previous_state != session.state
        {
            let transition = session_transition(session.state);
            let operation = session.lease.authorizing_operation.clone();
            let (actor, principal) = operation_identity(&transaction, scope, &operation)?;
            let event = append_event(
                &transaction,
                self.event_retention,
                scope,
                &session.id,
                "session",
                transition,
                &session.observed_at.to_rfc3339(),
                &actor,
                principal.as_deref(),
                &operation,
                Some(serde_json::to_value(&session)?),
            )?;
            effects.push(commit_effect(scope, &event));
        }
        transaction.commit()?;
        drop(connection);
        self.report_committed(&effects);
        Ok(if transformed {
            ExecWrite::PersistedTransformed(resource)
        } else {
            ExecWrite::PersistedExact(resource)
        })
    }

    pub fn exec(&self, scope: &Scope, id: &str) -> Result<Option<StoredExec>, StoreError> {
        let connection = self.connection.lock();
        load_exec(&connection, scope, id)
    }

    pub fn session(&self, scope: &Scope, id: &str) -> Result<Option<PipeSession>, StoreError> {
        let connection = self.connection.lock();
        load_session(&connection, scope, id)
    }

    pub fn session_for_exec(
        &self,
        scope: &Scope,
        exec_id: &str,
    ) -> Result<Option<PipeSession>, StoreError> {
        let connection = self.connection.lock();
        load_session_for_exec(&connection, scope, exec_id)
    }

    /// Consumes the one durable attachment right before the WebSocket upgrade. A failed upgrade or
    /// a lost attachment is therefore terminally contained instead of becoming reconnectable.
    pub fn claim_pipe_session_attachment(
        &self,
        scope: &Scope,
        id: &str,
        observed_at: DateTime<Utc>,
    ) -> Result<SessionAttachmentClaim, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut session) = load_session(&transaction, scope, id)? else {
            transaction.commit()?;
            return Ok(SessionAttachmentClaim::Missing);
        };
        if matches!(
            session.attachment,
            SessionAttachmentState::Attached
                | SessionAttachmentState::Consumed
                | SessionAttachmentState::Uncertain
        ) {
            transaction.commit()?;
            return Ok(SessionAttachmentClaim::AlreadyClaimed);
        }
        if session.state != SessionState::Ready
            || session.attachment != SessionAttachmentState::Available
            || session.lease.state != LeaseState::Active
        {
            transaction.commit()?;
            return Ok(SessionAttachmentClaim::NotAttachable);
        }
        session.state = SessionState::Attached;
        session.attachment = SessionAttachmentState::Attached;
        session.observed_at = observed_at;
        upsert_session(&transaction, scope, &session)?;
        let operation = session.lease.authorizing_operation.clone();
        let (actor, principal) = operation_identity(&transaction, scope, &operation)?;
        let event = append_event(
            &transaction,
            self.event_retention,
            scope,
            id,
            "session",
            "session.attached",
            &observed_at.to_rfc3339(),
            &actor,
            principal.as_deref(),
            &operation,
            Some(serde_json::to_value(&session)?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(SessionAttachmentClaim::Claimed)
    }

    pub fn scopes_for_exec(&self, deployment: &str, id: &str) -> Result<Vec<Scope>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT subject FROM execs WHERE deployment = ?1 AND id = ?2 ORDER BY subject",
        )?;
        statement
            .query_map(params![deployment, id], |row| row.get::<_, String>(0))?
            .map(|subject| {
                Ok(Scope {
                    deployment: deployment.to_owned(),
                    subject: subject?,
                })
            })
            .collect()
    }

    pub fn workspace_has_nonterminal_execs(
        &self,
        scope: &Scope,
        workspace_id: &str,
    ) -> Result<bool, StoreError> {
        let connection = self.connection.lock();
        workspace_has_nonterminal_execs(&connection, scope, workspace_id)
    }

    pub fn execs_for_workspace(
        &self,
        scope: &Scope,
        workspace_id: &str,
    ) -> Result<Vec<StoredExec>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT id FROM execs
             WHERE deployment = ?1 AND subject = ?2 AND workspace_id = ?3
               AND physically_absent = 0 ORDER BY id",
        )?;
        let ids = statement
            .query_map(
                params![scope.deployment, scope.subject, workspace_id],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        ids.into_iter()
            .filter_map(|id| match load_exec(&connection, scope, &id) {
                Ok(Some(value)) => Some(Ok(value)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub fn operation(
        &self,
        scope: &Scope,
        operation: &str,
    ) -> Result<Option<OperationRecord>, StoreError> {
        let connection = self.connection.lock();
        Ok(load_operation(&connection, scope, operation)?.map(|value| value.record))
    }

    pub fn stream_position(&self, scope: &Scope) -> Result<(String, u64, u64), StoreError> {
        let connection = self.connection.lock();
        ensure_stream(&connection, scope)?;
        stream_position(&connection, scope)
    }

    pub fn events(
        &self,
        scope: &Scope,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<Result<EventPage, EventCursorError>, StoreError> {
        let connection = self.connection.lock();
        ensure_stream(&connection, scope)?;
        let (source_scope, generation, through_seq) = stream_position(&connection, scope)?;
        let first_retained = connection
            .query_row(
                "SELECT MIN(seq) FROM events WHERE deployment = ?1 AND subject = ?2",
                params![scope.deployment, scope.subject],
                |row| row.get::<_, Option<i64>>(0),
            )?
            .map(to_u64)
            .transpose()?;
        let start = match cursor {
            None => first_retained.unwrap_or(1).saturating_sub(1),
            Some(value) => match parse_event_cursor(value) {
                Some((cursor_scope, cursor_generation, sequence))
                    if cursor_scope == source_scope && cursor_generation == generation =>
                {
                    sequence
                }
                Some(_) => {
                    return Ok(Err(EventCursorError::Source));
                }
                None => return Ok(Err(EventCursorError::Invalid)),
            },
        };
        if start > through_seq {
            return Ok(Err(EventCursorError::Invalid));
        }
        if let Some(first) = first_retained
            && start.saturating_add(1) < first
        {
            return Ok(Err(EventCursorError::Retention {
                first,
                last: through_seq,
            }));
        }
        let mut statement = connection.prepare(
            "SELECT event_json FROM events
             WHERE deployment = ?1 AND subject = ?2 AND seq > ?3
             ORDER BY seq LIMIT ?4",
        )?;
        let items = statement
            .query_map(
                params![
                    scope.deployment,
                    scope.subject,
                    to_i64(start)?,
                    i64::from(limit)
                ],
                |row| row.get::<_, String>(0),
            )?
            .map(|row| -> Result<Event, StoreError> {
                let mut event: Event = serde_json::from_str(&row?)?;
                event.source_scope.clone_from(&source_scope);
                Ok(event)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let next_seq = if items.len() < usize::try_from(limit).unwrap_or(usize::MAX) {
            through_seq
        } else {
            items.last().map_or(start, |event| event.seq)
        };
        let next_cursor = event_cursor(&source_scope, generation, next_seq);
        Ok(Ok(EventPage {
            source_scope,
            generation,
            items,
            next_cursor,
            through_seq,
            first_retained_seq: first_retained,
        }))
    }

    pub fn reset_stream_generation(&self, scope: &Scope) -> Result<u64, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_stream(&transaction, scope)?;
        let (_, current, _) = stream_position(&transaction, scope)?;
        let replacement = current.checked_add(1).ok_or(StoreError::IntegerRange)?;
        transaction.execute(
            "DELETE FROM events WHERE deployment = ?1 AND subject = ?2",
            params![scope.deployment, scope.subject],
        )?;
        transaction.execute(
            "UPDATE stream_meta SET generation = ?3, next_seq = 1
             WHERE deployment = ?1 AND subject = ?2",
            params![scope.deployment, scope.subject, to_i64(replacement)?],
        )?;
        transaction.commit()?;
        Ok(replacement)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn complete_snapshot(
        &self,
        scope: &Scope,
        actor: &str,
        principal: Option<&str>,
        observed_at: DateTime<Utc>,
        snapshot_id: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<SnapshotMetadata, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        prune_expired_snapshots_for_scope_transaction(
            &transaction,
            scope,
            observed_at,
            self.config.snapshot_prune_batch_size,
        )?;
        let active_snapshots = transaction.query_row(
            "SELECT COUNT(*) FROM snapshots WHERE deployment = ?1 AND subject = ?2",
            params![scope.deployment, scope.subject],
            |row| row.get::<_, i64>(0),
        )?;
        if active_snapshots >= MAX_ACTIVE_SNAPSHOTS_PER_SCOPE {
            let event = append_snapshot_refusal_event(
                &transaction,
                self.event_retention,
                scope,
                actor,
                principal,
                observed_at,
            )?;
            transaction.commit()?;
            drop(connection);
            self.report_committed(&[commit_effect(scope, &event)]);
            return Err(StoreError::SnapshotLimit);
        }
        ensure_stream(&transaction, scope)?;
        let (source_scope, generation, current_seq) = stream_position(&transaction, scope)?;
        let through_seq = current_seq.checked_add(1).ok_or(StoreError::IntegerRange)?;
        let workspaces = collect_snapshot_partition(
            &transaction,
            scope,
            "workspaces",
            SnapshotItemKind::Workspace,
            "workspace",
            i64::try_from(self.config.snapshot_max_workspaces.saturating_add(1))
                .map_err(|_| StoreError::IntegerRange)?,
        )?;
        if workspaces.len()
            > usize::try_from(self.config.snapshot_max_workspaces).unwrap_or(usize::MAX)
        {
            let event = append_snapshot_refusal_event(
                &transaction,
                self.event_retention,
                scope,
                actor,
                principal,
                observed_at,
            )?;
            transaction.commit()?;
            drop(connection);
            self.report_committed(&[commit_effect(scope, &event)]);
            return Err(StoreError::SnapshotLimit);
        }
        let remaining_after_workspaces = MAX_SNAPSHOT_ITEMS.checked_sub(workspaces.len());
        let Some(remaining_after_workspaces) = remaining_after_workspaces else {
            let event = append_snapshot_refusal_event(
                &transaction,
                self.event_retention,
                scope,
                actor,
                principal,
                observed_at,
            )?;
            transaction.commit()?;
            drop(connection);
            self.report_committed(&[commit_effect(scope, &event)]);
            return Err(StoreError::SnapshotLimit);
        };
        let exec_limit = i64::try_from(self.config.snapshot_max_execs.saturating_add(1))
            .map_err(|_| StoreError::IntegerRange)?;
        let execs = collect_snapshot_partition(
            &transaction,
            scope,
            "execs",
            SnapshotItemKind::Exec,
            "exec",
            exec_limit,
        )?;
        if execs.len() > remaining_after_workspaces
            || execs.len() > usize::try_from(self.config.snapshot_max_execs).unwrap_or(usize::MAX)
        {
            let event = append_snapshot_refusal_event(
                &transaction,
                self.event_retention,
                scope,
                actor,
                principal,
                observed_at,
            )?;
            transaction.commit()?;
            drop(connection);
            self.report_committed(&[commit_effect(scope, &event)]);
            return Err(StoreError::SnapshotLimit);
        }
        let remaining = remaining_after_workspaces - execs.len();
        let retained_before: u64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM events
             WHERE deployment = ?1 AND subject = ?2 AND seq <= ?3",
                params![scope.deployment, scope.subject, to_i64(current_seq)?],
                |row| row.get::<_, i64>(0),
            )
            .and_then(|value| {
                u64::try_from(value).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })
            })?;
        let retained_before = retained_before.min(self.event_retention);
        // The barrier itself occupies one retained event slot. Compute provenance from the
        // history that will still exist after appending it, so a full/noisy journal cannot make
        // snapshot bootstrap roll back merely because the barrier evicts the oldest row.
        let retained_after_barrier = retained_before.min(self.event_retention.saturating_sub(1));
        let provenance_budget = remaining
            .min(usize::try_from(self.config.snapshot_max_provenance_events).unwrap_or(usize::MAX));
        let provenance_count = usize::try_from(retained_after_barrier)
            .unwrap_or(usize::MAX)
            .min(provenance_budget);
        let item_count = workspaces
            .len()
            .saturating_add(execs.len())
            .saturating_add(provenance_count);
        let history_through_seq = if provenance_count == 0 {
            0
        } else {
            current_seq
        };
        let first_history_seq = if provenance_count == 0 {
            None
        } else {
            Some(
                history_through_seq
                    .checked_sub(
                        u64::try_from(provenance_count).map_err(|_| StoreError::IntegerRange)?,
                    )
                    .and_then(|value| value.checked_add(1))
                    .ok_or(StoreError::IntegerRange)?,
            )
        };
        let history = SnapshotHistory {
            first_seq: first_history_seq,
            through_seq: history_through_seq,
            item_count: u64::try_from(provenance_count).map_err(|_| StoreError::IntegerRange)?,
            truncated: u64::try_from(provenance_count).map_err(|_| StoreError::IntegerRange)?
                < retained_before,
        };
        let metadata = SnapshotMetadata {
            id: snapshot_id.to_owned(),
            source_scope: source_scope.clone(),
            generation,
            through_seq,
            resume_cursor: event_cursor(&source_scope, generation, through_seq),
            item_count: u64::try_from(item_count).map_err(|_| StoreError::IntegerRange)?,
            partitions: SnapshotPartitions {
                workspaces: u64::try_from(workspaces.len())
                    .map_err(|_| StoreError::IntegerRange)?,
                execs: u64::try_from(execs.len()).map_err(|_| StoreError::IntegerRange)?,
                provenance_events: u64::try_from(provenance_count)
                    .map_err(|_| StoreError::IntegerRange)?,
            },
            history,
            expires_at,
        };
        let created = append_control_event(
            &transaction,
            self.event_retention,
            scope,
            snapshot_id,
            "snapshot",
            "snapshot.created",
            &observed_at.to_rfc3339(),
            actor,
            principal,
            serde_json::to_value(&metadata)?,
        )?;
        if created.seq != through_seq {
            return Err(StoreError::IntegerRange);
        }
        let provenance =
            collect_snapshot_provenance(&transaction, scope, current_seq, provenance_count)?;
        if provenance.len() != provenance_count {
            return Err(StoreError::SnapshotLimit);
        }
        transaction.execute(
            "INSERT INTO snapshots (
                deployment, subject, id, source_scope, generation, through_seq, item_count,
                expires_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                scope.deployment,
                scope.subject,
                snapshot_id,
                source_scope,
                to_i64(generation)?,
                to_i64(through_seq)?,
                to_i64(u64::try_from(item_count).map_err(|_| StoreError::IntegerRange)?)?,
                expires_at.to_rfc3339(),
            ],
        )?;
        let mut materialized = workspaces;
        materialized.extend(execs);
        materialized.extend(provenance);
        for (index, item) in materialized.iter_mut().enumerate() {
            item.ordinal = u64::try_from(index + 1).map_err(|_| StoreError::IntegerRange)?;
            transaction.execute(
                "INSERT INTO snapshot_items (
                    deployment, subject, snapshot_id, ordinal, item_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    scope.deployment,
                    scope.subject,
                    snapshot_id,
                    to_i64(item.ordinal)?,
                    serde_json::to_string(item)?,
                ],
            )?;
        }
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &created)]);
        Ok(metadata)
    }

    #[allow(clippy::too_many_lines)] // A snapshot read validates materialization integrity atomically.
    pub fn snapshot_page(
        &self,
        scope: &Scope,
        snapshot_id: &str,
        cursor: Option<&str>,
        limit: u32,
        now: DateTime<Utc>,
    ) -> Result<Result<SnapshotPage, SnapshotReadError>, StoreError> {
        let connection = self.connection.lock();
        let metadata = connection
            .query_row(
                "SELECT generation, through_seq, item_count, expires_at FROM snapshots
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![scope.deployment, scope.subject, snapshot_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((generation, through_seq, item_count, expires_at)) = metadata else {
            let expired = connection
                .query_row(
                    "SELECT 1 FROM expired_snapshots
                     WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                    params![scope.deployment, scope.subject, snapshot_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            return Ok(Err(if expired {
                SnapshotReadError::Expired
            } else {
                SnapshotReadError::NotFound
            }));
        };
        let expires_at: DateTime<Utc> = expires_at.parse()?;
        if now >= expires_at {
            return Ok(Err(SnapshotReadError::Expired));
        }
        let (actual_count, first_ordinal, last_ordinal, ordinal_sum) = connection.query_row(
            "SELECT COUNT(*), MIN(ordinal), MAX(ordinal), COALESCE(SUM(ordinal), 0)
             FROM snapshot_items
             WHERE deployment = ?1 AND subject = ?2 AND snapshot_id = ?3",
            params![scope.deployment, scope.subject, snapshot_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;
        let expected_sum = item_count.saturating_mul(item_count.saturating_add(1)) / 2;
        if actual_count != item_count
            || (item_count == 0 && (first_ordinal.is_some() || last_ordinal.is_some()))
            || (item_count > 0
                && (first_ordinal != Some(1)
                    || last_ordinal != Some(item_count)
                    || ordinal_sum != expected_sum))
        {
            return Ok(Err(SnapshotReadError::Incomplete));
        }
        if limit == 0 {
            return Ok(Err(SnapshotReadError::InvalidCursor));
        }
        let start = match cursor {
            None => 0,
            Some(value) => match parse_snapshot_cursor(value, snapshot_id) {
                Some(value) if value > 0 && value < to_u64(item_count)? => value,
                None | Some(_) => return Ok(Err(SnapshotReadError::InvalidCursor)),
            },
        };
        let mut statement = connection.prepare(
            "SELECT item_json FROM snapshot_items
             WHERE deployment = ?1 AND subject = ?2 AND snapshot_id = ?3 AND ordinal > ?4
             ORDER BY ordinal LIMIT ?5",
        )?;
        let mut items = statement
            .query_map(
                params![
                    scope.deployment,
                    scope.subject,
                    snapshot_id,
                    to_i64(start)?,
                    i64::from(limit.saturating_add(1))
                ],
                |row| row.get::<_, String>(0),
            )?
            .map(|row| -> Result<SnapshotItem, StoreError> { Ok(serde_json::from_str(&row?)?) })
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = items.len() > usize::try_from(limit).unwrap_or(usize::MAX);
        if has_more {
            items.pop();
        }
        let mut expected = start.saturating_add(1);
        for item in &items {
            if item.ordinal != expected {
                return Ok(Err(SnapshotReadError::Incomplete));
            }
            expected = expected.saturating_add(1);
        }
        let last = items.last().map_or(start, |item| item.ordinal);
        let item_count = to_u64(item_count)?;
        let complete = !has_more && last == item_count;
        Ok(Ok(SnapshotPage {
            snapshot: snapshot_id.to_owned(),
            generation: to_u64(generation)?,
            through_seq: to_u64(through_seq)?,
            items,
            next_cursor: has_more.then(|| snapshot_cursor(snapshot_id, last)),
            complete,
        }))
    }

    /// Physically removes expired snapshot metadata and its cascade-owned materialized rows.
    /// A bounded marker preserves the contract distinction between expired and never-created IDs.
    pub fn prune_expired_snapshots(
        &self,
        deployment: &str,
        now: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = prune_expired_snapshots_transaction(
            &transaction,
            deployment,
            now,
            self.config.snapshot_prune_batch_size,
        )?;
        transaction.commit()?;
        Ok(removed)
    }

    pub fn renew_workspace_lease(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        id: &str,
        lease: &NewLease,
    ) -> Result<Workspace, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (root_name, json): (String, String) = transaction.query_row(
            "SELECT root_name, resource_json FROM workspaces
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let mut resource: Workspace = serde_json::from_str(&json)?;
        if resource.state != WorkspaceState::Ready {
            return Err(StoreError::WorkspaceFrozen);
        }
        let (newly_frozen, frozen_event) = freeze_workspace_lease_if_due(
            &transaction,
            self.event_retention,
            scope,
            id,
            &mut resource,
            Some(&lease.clock),
        )?;
        if newly_frozen
            || resource
                .lease
                .as_ref()
                .is_some_and(|current| current.state != LeaseState::Active)
        {
            transaction.commit()?;
            drop(connection);
            if let Some(event) = frozen_event {
                self.report_committed(&[commit_effect(scope, &event)]);
            }
            return Err(StoreError::LeaseExpired);
        }
        ensure_lease_renewable(&transaction, scope, "workspace", id, &lease.clock)?;
        resource.lease = Some(lease.observation());
        upsert_workspace(&transaction, scope, &root_name, &resource)?;
        upsert_lease(&transaction, scope, "workspace", id, lease, operation)?;
        let event = complete_lease_operation(
            &transaction,
            self.event_retention,
            self.config,
            scope,
            operation,
            terminal_at,
            status,
            id,
            "workspace",
            "workspace.lease-renewed",
            &resource,
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(resource)
    }

    pub fn renew_exec_lease(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        id: &str,
        lease: &NewLease,
    ) -> Result<Exec, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_lease_renewable(&transaction, scope, "exec", id, &lease.clock)?;
        let json: String = transaction.query_row(
            "SELECT resource_json FROM execs
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, id],
            |row| row.get(0),
        )?;
        let mut resource: Exec = serde_json::from_str(&json)?;
        resource.lease = Some(lease.observation());
        transaction.execute(
            "UPDATE execs SET resource_json = ?4
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![
                scope.deployment,
                scope.subject,
                id,
                serde_json::to_string(&resource)?
            ],
        )?;
        upsert_lease(&transaction, scope, "exec", id, lease, operation)?;
        let event = complete_lease_operation(
            &transaction,
            self.event_retention,
            self.config,
            scope,
            operation,
            terminal_at,
            status,
            id,
            "exec",
            "exec.lease-renewed",
            &resource,
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(resource)
    }

    pub fn renew_pipe_session_lease(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: &str,
        status: u16,
        session_id: &str,
        lease: &NewLease,
    ) -> Result<PipeSession, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut session = load_session(&transaction, scope, session_id)?
            .ok_or_else(|| StoreError::NotAccepted(session_id.to_owned()))?;
        ensure_lease_renewable(&transaction, scope, "exec", &session.exec, &lease.clock)?;
        let mut exec = load_exec(&transaction, scope, &session.exec)?
            .ok_or_else(|| StoreError::NotAccepted(session.exec.clone()))?;
        exec.resource.lease = Some(lease.observation());
        session.lease = lease.observation();
        session.observed_at = lease.clock.wall;
        upsert_exec(&transaction, scope, &exec)?;
        upsert_session(&transaction, scope, &session)?;
        upsert_lease(&transaction, scope, "exec", &session.exec, lease, operation)?;
        let event = complete_lease_operation(
            &transaction,
            self.event_retention,
            self.config,
            scope,
            operation,
            terminal_at,
            status,
            session_id,
            "session",
            "session.lease-renewed",
            &session,
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(session)
    }

    #[allow(clippy::too_many_lines)] // One bounded transaction advances the durable fair cursor.
    pub fn lease_cleanup_candidates(
        &self,
        deployment: &str,
        clock: &LeaseClock,
        limit: usize,
    ) -> Result<Vec<ExpiredLease>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cursor = transaction
            .query_row(
                "SELECT subject, resource_kind, resource_id FROM maintenance_cursors
                 WHERE deployment = ?1 AND queue = 'lease-cleanup'",
                params![deployment],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .unwrap_or_default();
        let scan_limit = limit.saturating_mul(4).max(limit);
        let mut statement = transaction.prepare(
            "SELECT l.subject, l.resource_kind, l.resource_id, l.issued_wall, l.renew_by_wall,
                    l.boot_id, l.issued_boottime_ms, l.deadline_boottime_ms, l.state,
                    w.root_name, e.workspace_id
             FROM leases AS l
             LEFT JOIN workspaces AS w
               ON w.deployment = l.deployment AND w.subject = l.subject AND w.id = l.resource_id
              AND l.resource_kind = 'workspace'
             LEFT JOIN execs AS e
               ON e.deployment = l.deployment AND e.subject = l.subject AND e.id = l.resource_id
              AND l.resource_kind = 'exec'
             WHERE l.deployment = ?1 AND l.state IN ('active','expiring')
               AND (l.next_attempt_at IS NULL OR l.next_attempt_at <= ?2)
             ORDER BY CASE WHEN l.subject > ?3
                                  OR (l.subject = ?3 AND l.resource_kind > ?4)
                                  OR (l.subject = ?3 AND l.resource_kind = ?4
                                      AND l.resource_id > ?5)
                                THEN 0 ELSE 1 END,
                      l.subject, l.resource_kind, l.resource_id
             LIMIT ?6",
        )?;
        let rows = statement
            .query_map(
                params![
                    deployment,
                    clock.wall.to_rfc3339(),
                    cursor.0,
                    cursor.1,
                    cursor.2,
                    to_i64(u64::try_from(scan_limit).map_err(|_| StoreError::IntegerRange)?)?
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<String>>(10)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let mut candidates = Vec::new();
        let mut last_examined = None;
        for (
            subject,
            kind,
            id,
            issued_wall,
            renew_by,
            boot_id,
            issued_boot,
            deadline_boot,
            state,
            root_name,
            workspace_id,
        ) in rows
        {
            last_examined = Some((subject.clone(), kind.clone(), id.clone()));
            if state != "expiring"
                && !lease_due(
                    clock,
                    &boot_id,
                    &issued_wall.parse()?,
                    to_u64(issued_boot)?,
                    &renew_by.parse()?,
                    to_u64(deadline_boot)?,
                )
            {
                continue;
            }
            let resource = if kind == "workspace" {
                LeaseResource::Workspace {
                    root_name: root_name.unwrap_or_else(|| id.clone()),
                }
            } else if let Some(workspace_id) = workspace_id {
                LeaseResource::Exec { workspace_id }
            } else {
                continue;
            };
            candidates.push(ExpiredLease {
                scope: Scope {
                    deployment: deployment.to_owned(),
                    subject,
                },
                id,
                resource,
            });
            if candidates.len() == limit {
                break;
            }
        }
        if let Some((subject, resource_kind, resource_id)) = last_examined {
            transaction.execute(
                "INSERT INTO maintenance_cursors (
                    deployment, queue, subject, resource_kind, resource_id
                 ) VALUES (?1, 'lease-cleanup', ?2, ?3, ?4)
                 ON CONFLICT (deployment, queue) DO UPDATE SET
                    subject = excluded.subject,
                    resource_kind = excluded.resource_kind,
                    resource_id = excluded.resource_id",
                params![deployment, subject, resource_kind, resource_id],
            )?;
        }
        transaction.commit()?;
        Ok(candidates)
    }

    #[allow(clippy::too_many_lines)]
    pub fn claim_expired_lease(
        &self,
        candidate: &ExpiredLease,
        clock: &LeaseClock,
    ) -> Result<Option<ExpiredLease>, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let kind = match candidate.resource {
            LeaseResource::Workspace { .. } => "workspace",
            LeaseResource::Exec { .. } => "exec",
        };
        let row = transaction
            .query_row(
                "SELECT issued_wall, renew_by_wall, boot_id, issued_boottime_ms,
                        deadline_boottime_ms, state
                 FROM leases WHERE deployment = ?1 AND subject = ?2
                   AND resource_kind = ?3 AND resource_id = ?4",
                params![
                    candidate.scope.deployment,
                    candidate.scope.subject,
                    kind,
                    candidate.id
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((issued_wall, renew_by, boot_id, issued_boot, deadline_boot, state)) = row else {
            transaction.commit()?;
            return Ok(None);
        };
        if state == "expired"
            || (state == "active"
                && !lease_due(
                    clock,
                    &boot_id,
                    &issued_wall.parse()?,
                    to_u64(issued_boot)?,
                    &renew_by.parse()?,
                    to_u64(deadline_boot)?,
                ))
        {
            transaction.commit()?;
            return Ok(None);
        }
        let mut event = None;
        match &candidate.resource {
            LeaseResource::Workspace { .. } => {
                let json: Option<String> = transaction
                    .query_row(
                        "SELECT resource_json FROM workspaces
                         WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                        params![
                            candidate.scope.deployment,
                            candidate.scope.subject,
                            candidate.id
                        ],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(json) = json {
                    let mut workspace: Workspace = serde_json::from_str(&json)?;
                    let (_, frozen_event) = freeze_workspace_lease_if_due(
                        &transaction,
                        self.event_retention,
                        &candidate.scope,
                        &candidate.id,
                        &mut workspace,
                        Some(clock),
                    )?;
                    event = frozen_event;
                }
            }
            LeaseResource::Exec { .. } => {
                if state == "active" {
                    transaction.execute(
                        "UPDATE leases SET state = 'expiring'
                         WHERE deployment = ?1 AND subject = ?2 AND resource_kind = 'exec'
                           AND resource_id = ?3 AND state = 'active'",
                        params![
                            candidate.scope.deployment,
                            candidate.scope.subject,
                            candidate.id
                        ],
                    )?;
                    if let Some(mut stored) =
                        load_exec(&transaction, &candidate.scope, &candidate.id)?
                    {
                        if let Some(lease) = stored.resource.lease.as_mut() {
                            lease.state = LeaseState::Expiring;
                        }
                        upsert_exec(&transaction, &candidate.scope, &stored)?;
                        let _projected = project_session_from_exec(
                            &transaction,
                            &candidate.scope,
                            &stored.resource,
                        )?;
                        let operation = lease_authorizing_operation(
                            &transaction,
                            &candidate.scope,
                            "exec",
                            &candidate.id,
                        )?;
                        let (_, principal) =
                            operation_identity(&transaction, &candidate.scope, &operation)?;
                        event = Some(append_event(
                            &transaction,
                            self.event_retention,
                            &candidate.scope,
                            &candidate.id,
                            "exec",
                            "exec.lease-expiring",
                            &clock.wall.to_rfc3339(),
                            LEASE_SWEEPER_ACTOR,
                            principal.as_deref(),
                            &operation,
                            Some(serde_json::to_value(&stored.resource)?),
                        )?);
                    }
                }
            }
        }
        transaction.commit()?;
        drop(connection);
        if let Some(event) = event {
            self.report_committed(&[commit_effect(&candidate.scope, &event)]);
        }
        Ok(Some(candidate.clone()))
    }

    pub fn claim_expired_leases(
        &self,
        deployment: &str,
        clock: &LeaseClock,
    ) -> Result<Vec<ExpiredLease>, StoreError> {
        let mut expired = Vec::new();
        for candidate in self.lease_cleanup_candidates(deployment, clock, 64)? {
            if let Some(claimed) = self.claim_expired_lease(&candidate, clock)? {
                expired.push(claimed);
            }
        }
        Ok(expired)
    }

    pub fn complete_exec_lease_expiry(
        &self,
        expired: &ExpiredLease,
        observed_at: DateTime<Utc>,
        observation: Option<&StoredExec>,
    ) -> Result<ExecWrite, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let json: Option<String> = transaction
            .query_row(
                "SELECT resource_json FROM execs
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![expired.scope.deployment, expired.scope.subject, expired.id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(json) = json {
            let operation =
                lease_authorizing_operation(&transaction, &expired.scope, "exec", &expired.id)?;
            let (_, principal) = operation_identity(&transaction, &expired.scope, &operation)?;
            let previous: Exec = serde_json::from_str(&json)?;
            let previous_terminal = is_terminal_exec_state(previous.state);
            let durable = load_exec(&transaction, &expired.scope, &expired.id)?
                .ok_or_else(|| StoreError::NotAccepted(expired.id.clone()))?;
            let mut stored = if previous_terminal {
                durable
            } else {
                observation.cloned().unwrap_or(durable)
            };
            if !previous_terminal {
                stored.resource.state = ExecState::Expired;
                stored.resource.observed_at = observed_at;
                stored.output_complete = true;
            }
            if let Some(lease) = stored.resource.lease.as_mut() {
                lease.state = LeaseState::Expired;
            }
            upsert_exec(&transaction, &expired.scope, &stored)?;
            let projected_session =
                project_session_from_exec(&transaction, &expired.scope, &stored.resource)?;
            transaction.execute(
                "UPDATE leases SET state = 'expired'
                 WHERE deployment = ?1 AND subject = ?2 AND resource_kind = 'exec'
                   AND resource_id = ?3",
                params![expired.scope.deployment, expired.scope.subject, expired.id],
            )?;
            let event = append_event(
                &transaction,
                self.event_retention,
                &expired.scope,
                &expired.id,
                "exec",
                "exec.lease-expired",
                &observed_at.to_rfc3339(),
                LEASE_SWEEPER_ACTOR,
                principal.as_deref(),
                &operation,
                Some(serde_json::to_value(&stored.resource)?),
            )?;
            let session_event = if let Some((session, previous_state)) = projected_session
                && previous_state != session.state
            {
                Some(append_event(
                    &transaction,
                    self.event_retention,
                    &expired.scope,
                    &session.id,
                    "session",
                    session_transition(session.state),
                    &observed_at.to_rfc3339(),
                    LEASE_SWEEPER_ACTOR,
                    principal.as_deref(),
                    &operation,
                    Some(serde_json::to_value(&session)?),
                )?)
            } else {
                None
            };
            transaction.commit()?;
            drop(connection);
            let mut effects = vec![commit_effect(&expired.scope, &event)];
            if let Some(event) = session_event {
                effects.push(commit_effect(&expired.scope, &event));
            }
            self.report_committed(&effects);
            return Ok(if previous_terminal {
                ExecWrite::Superseded(stored)
            } else {
                ExecWrite::PersistedTransformed(stored)
            });
        }
        transaction.commit()?;
        Ok(ExecWrite::Retired)
    }

    pub fn complete_workspace_lease_expiry(
        &self,
        expired: &ExpiredLease,
        observed_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let json: Option<String> = transaction
            .query_row(
                "SELECT resource_json FROM workspaces
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![expired.scope.deployment, expired.scope.subject, expired.id],
                |row| row.get(0),
            )
            .optional()?;
        let mut effect = None;
        if let Some(json) = json {
            let operation = lease_authorizing_operation(
                &transaction,
                &expired.scope,
                "workspace",
                &expired.id,
            )?;
            let (_, principal) = operation_identity(&transaction, &expired.scope, &operation)?;
            let mut resource: Workspace = serde_json::from_str(&json)?;
            resource.state = WorkspaceState::Expired;
            resource.observed_at = observed_at;
            if let Some(lease) = resource.lease.as_mut() {
                lease.state = LeaseState::Expired;
            }
            let value = serde_json::to_value(&resource)?;
            transaction.execute(
                "DELETE FROM workspaces WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![expired.scope.deployment, expired.scope.subject, expired.id],
            )?;
            transaction.execute(
                "UPDATE leases SET state = 'expired'
                 WHERE deployment = ?1 AND subject = ?2 AND resource_kind = 'workspace'
                   AND resource_id = ?3",
                params![expired.scope.deployment, expired.scope.subject, expired.id],
            )?;
            insert_tombstone(
                &transaction,
                &expired.scope,
                "workspace",
                &expired.id,
                &observed_at.to_rfc3339(),
                "lease-expired",
                &value,
            )?;
            let event = append_event(
                &transaction,
                self.event_retention,
                &expired.scope,
                &expired.id,
                "workspace",
                "workspace.lease-expired",
                &observed_at.to_rfc3339(),
                LEASE_SWEEPER_ACTOR,
                principal.as_deref(),
                &operation,
                Some(value),
            )?;
            effect = Some(commit_effect(&expired.scope, &event));
        }
        transaction.commit()?;
        drop(connection);
        if let Some(effect) = effect {
            self.report_committed(&[effect]);
        }
        Ok(())
    }

    pub fn record_lease_cleanup_failure(
        &self,
        expired: &ExpiredLease,
        observed_at: DateTime<Utc>,
        code: &str,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let kind = match expired.resource {
            LeaseResource::Workspace { .. } => "workspace",
            LeaseResource::Exec { .. } => "exec",
        };
        let attempt_count: i64 = transaction.query_row(
            "SELECT attempt_count FROM leases
             WHERE deployment = ?1 AND subject = ?2 AND resource_kind = ?3 AND resource_id = ?4",
            params![
                expired.scope.deployment,
                expired.scope.subject,
                kind,
                expired.id
            ],
            |row| row.get(0),
        )?;
        let next_attempt = u32::try_from(attempt_count)
            .map_err(|_| StoreError::IntegerRange)?
            .saturating_add(1);
        let shift = next_attempt.saturating_sub(1).min(30);
        let multiplier = 1_i64.checked_shl(shift).unwrap_or(i64::MAX);
        let delay_ms = WORKSPACE_CLEANUP_INITIAL_BACKOFF_MS
            .saturating_mul(multiplier)
            .min(WORKSPACE_CLEANUP_MAX_BACKOFF_MS);
        let next_attempt_at = observed_at + chrono::Duration::milliseconds(delay_ms);
        transaction.execute(
            "UPDATE leases
             SET attempt_count = ?5, next_attempt_at = ?6, last_error = ?7
             WHERE deployment = ?1 AND subject = ?2 AND resource_kind = ?3 AND resource_id = ?4",
            params![
                expired.scope.deployment,
                expired.scope.subject,
                kind,
                expired.id,
                i64::from(next_attempt),
                next_attempt_at.to_rfc3339(),
                code,
            ],
        )?;
        let operation: String = transaction.query_row(
            "SELECT authorizing_operation FROM leases
             WHERE deployment = ?1 AND subject = ?2 AND resource_kind = ?3 AND resource_id = ?4",
            params![
                expired.scope.deployment,
                expired.scope.subject,
                kind,
                expired.id
            ],
            |row| row.get(0),
        )?;
        let (_, principal) = operation_identity(&transaction, &expired.scope, &operation)?;
        let event = append_event(
            &transaction,
            self.event_retention,
            &expired.scope,
            &expired.id,
            kind,
            &format!("{kind}.cleanup-failed"),
            &observed_at.to_rfc3339(),
            LEASE_SWEEPER_ACTOR,
            principal.as_deref(),
            &operation,
            Some(serde_json::to_value(ErrorDetail {
                class: ErrorClass::Failed,
                code: code.to_owned(),
                message: format!(
                    "{kind} lease cleanup failed; the daemon will retry with bounded backoff."
                ),
                retriable: true,
                address: Some(expired.id.clone()),
                operation: Some(operation.clone()),
            })?),
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(&expired.scope, &event)]);
        Ok(())
    }

    pub fn record_lease_cleanup_progress(
        &self,
        expired: &ExpiredLease,
        observed_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        let kind = match expired.resource {
            LeaseResource::Workspace { .. } => "workspace",
            LeaseResource::Exec { .. } => "exec",
        };
        let changed = connection.execute(
            "UPDATE leases SET next_attempt_at = ?5, last_error = NULL
             WHERE deployment = ?1 AND subject = ?2 AND resource_kind = ?3
               AND resource_id = ?4 AND state = 'expiring'",
            params![
                expired.scope.deployment,
                expired.scope.subject,
                kind,
                expired.id,
                observed_at.to_rfc3339(),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(expired.id.clone()));
        }
        Ok(())
    }

    pub fn recovery_workspaces(
        &self,
        deployment: &str,
        accepted_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<RecoveryWorkspace>, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cursor = maintenance_cursor(&transaction, deployment, "recovery-workspace")?;
        let scan_limit = limit.saturating_mul(4);
        let rows = scan_workspace_recovery_rows(
            &transaction,
            deployment,
            &cursor.1,
            &cursor.2,
            scan_limit,
        )?;
        let mut candidates = Vec::new();
        for (subject, id, root_name, resource_json) in &rows {
            let operation = transaction
                .query_row(
                    "SELECT operation FROM operations
                     WHERE deployment = ?1 AND subject = ?2 AND resource = ?3
                       AND operation_kind = 'workspace.create'
                       AND (
                           state = 'unknown'
                           OR (state = 'accepted' AND accepted_at < ?4)
                       )
                     ORDER BY accepted_at DESC, operation DESC LIMIT 1",
                    params![deployment, subject, id, accepted_before.to_rfc3339()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(operation) = operation {
                candidates.push(RecoveryWorkspace {
                    scope: Scope {
                        deployment: deployment.to_owned(),
                        subject: subject.clone(),
                    },
                    root_name: root_name.clone(),
                    resource: serde_json::from_str(resource_json)?,
                    operation,
                });
                if candidates.len() == limit {
                    break;
                }
            }
        }
        if let Some((subject, id, _, _)) = rows.last() {
            set_maintenance_cursor(
                &transaction,
                deployment,
                "recovery-workspace",
                subject,
                "workspace",
                id,
            )?;
        }
        transaction.commit()?;
        Ok(candidates)
    }

    pub fn recovery_execs(
        &self,
        deployment: &str,
        accepted_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<RecoveryExec>, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cursor = maintenance_cursor(&transaction, deployment, "recovery-exec")?;
        let scan_limit = limit.saturating_mul(4);
        let keys =
            scan_exec_recovery_keys(&transaction, deployment, &cursor.1, &cursor.2, scan_limit)?;
        let mut candidates = Vec::new();
        for (subject, id) in &keys {
            let operation = transaction
                .query_row(
                    "SELECT o.operation, o.state FROM operations o
                     LEFT JOIN sessions s
                       ON s.deployment = o.deployment AND s.subject = o.subject
                      AND s.id = o.resource
                     WHERE o.deployment = ?1 AND o.subject = ?2
                       AND (
                         (o.resource = ?3 AND o.operation_kind = 'exec.start')
                         OR (s.exec_id = ?3 AND o.operation_kind = 'session.start')
                       )
                       AND (
                           o.state IN ('unknown','terminal')
                           OR (o.state = 'accepted' AND o.accepted_at < ?4)
                       )
                     ORDER BY o.accepted_at DESC, o.operation DESC LIMIT 1",
                    params![deployment, subject, id, accepted_before.to_rfc3339()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            let Some((operation, operation_state)) = operation else {
                continue;
            };
            let scope = Scope {
                deployment: deployment.to_owned(),
                subject: subject.clone(),
            };
            let Some(stored) = load_exec(&transaction, &scope, id)? else {
                continue;
            };
            candidates.push(RecoveryExec {
                scope,
                stored,
                operation,
                operation_state: parse_operation_state(&operation_state)?,
            });
            if candidates.len() == limit {
                break;
            }
        }
        if let Some((subject, id)) = keys.last() {
            set_maintenance_cursor(
                &transaction,
                deployment,
                "recovery-exec",
                subject,
                "exec",
                id,
            )?;
        }
        transaction.commit()?;
        Ok(candidates)
    }

    pub fn mark_exec_physically_absent(
        &self,
        candidate: &RecoveryExec,
        observed_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut stored) = load_exec(
            &transaction,
            &candidate.scope,
            &candidate.stored.resource.id,
        )?
        else {
            transaction.commit()?;
            return Ok(());
        };
        if !matches!(
            stored.resource.state,
            ExecState::Accepted | ExecState::Running | ExecState::Unknown
        ) {
            transaction.commit()?;
            return Ok(());
        }
        stored.resource.state = ExecState::Unknown;
        stored.resource.observed_at = observed_at;
        stored.output_complete = true;
        transaction.execute(
            "UPDATE execs SET resource_json = ?4, output_complete = 1, physically_absent = 1
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![
                candidate.scope.deployment,
                candidate.scope.subject,
                candidate.stored.resource.id,
                serde_json::to_string(&stored.resource)?,
            ],
        )?;
        let projected_session =
            project_session_from_exec(&transaction, &candidate.scope, &stored.resource)?;
        let (actor, principal) =
            operation_identity(&transaction, &candidate.scope, &candidate.operation)?;
        let event = append_event(
            &transaction,
            self.event_retention,
            &candidate.scope,
            &candidate.stored.resource.id,
            "exec",
            "exec.unknown",
            &observed_at.to_rfc3339(),
            &actor,
            principal.as_deref(),
            &candidate.operation,
            Some(serde_json::to_value(&stored.resource)?),
        )?;
        let session_event = if let Some((session, previous_state)) = projected_session
            && previous_state != session.state
        {
            Some(append_event(
                &transaction,
                self.event_retention,
                &candidate.scope,
                &session.id,
                "session",
                "session.unknown",
                &observed_at.to_rfc3339(),
                &actor,
                principal.as_deref(),
                &candidate.operation,
                Some(serde_json::to_value(&session)?),
            )?)
        } else {
            None
        };
        transaction.commit()?;
        drop(connection);
        let mut effects = vec![commit_effect(&candidate.scope, &event)];
        if let Some(event) = session_event {
            effects.push(commit_effect(&candidate.scope, &event));
        }
        self.report_committed(&effects);
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn reconcile_after_restart(
        &self,
        deployment: &str,
        accepted_before: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        limit: usize,
    ) -> Result<usize, StoreError> {
        if limit == 0 {
            return Ok(0);
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let accepted_before = accepted_before.to_rfc3339();
        let observed_at = observed_at.to_rfc3339();
        let operation_limit = limit.div_ceil(2);
        let exec_limit = limit / 2;
        let accepted = {
            let mut statement = transaction.prepare(
                "SELECT subject, operation, resource, operation_kind, actor, principal
                 FROM operations
                 WHERE deployment = ?1 AND state = 'accepted' AND accepted_at < ?2
                 ORDER BY accepted_at, subject, operation LIMIT ?3",
            )?;
            statement
                .query_map(
                    params![
                        deployment,
                        accepted_before,
                        to_i64(
                            u64::try_from(operation_limit).map_err(|_| StoreError::IntegerRange)?
                        )?
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut effects = Vec::new();
        for (subject, operation, resource, kind, actor, principal) in &accepted {
            let scope = Scope {
                deployment: deployment.to_owned(),
                subject: subject.clone(),
            };
            transaction.execute(
                "UPDATE operations SET state = 'unknown'
                 WHERE deployment = ?1 AND subject = ?2 AND operation = ?3 AND state = 'accepted'",
                params![deployment, subject, operation],
            )?;
            refresh_nonterminal_operation_accounting(&transaction, self.config, &scope, operation)?;
            let event = append_event(
                &transaction,
                self.event_retention,
                &scope,
                resource.as_deref().unwrap_or(operation),
                operation_resource_kind(kind),
                "operation.unknown",
                &observed_at,
                actor,
                principal.as_deref(),
                operation,
                Some(json!({ "state": "unknown", "reason": "daemon-restart" })),
            )?;
            effects.push(commit_effect(&scope, &event));
        }
        let mut recovered = accepted.len();
        let cursor = transaction
            .query_row(
                "SELECT resource_kind, subject, resource_id FROM maintenance_cursors
                 WHERE deployment = ?1 AND queue = 'restart-exec-reconcile'",
                params![deployment],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .unwrap_or_default();
        let scan_limit = exec_limit.saturating_mul(4);
        let rows = scan_restart_exec_rows(
            &transaction,
            deployment,
            (&cursor.0, &cursor.1, &cursor.2),
            scan_limit,
        )?;
        for (state, subject, id, json) in &rows {
            let operation = transaction
                .query_row(
                    "SELECT o.operation, o.actor, o.principal FROM operations o
                     LEFT JOIN sessions s
                       ON s.deployment = o.deployment AND s.subject = o.subject
                      AND s.id = o.resource
                     WHERE o.deployment = ?1 AND o.subject = ?2
                       AND (
                         (o.resource = ?3 AND o.operation_kind = 'exec.start')
                         OR (s.exec_id = ?3 AND o.operation_kind = 'session.start')
                       )
                       AND o.accepted_at < ?4
                     ORDER BY o.accepted_at DESC, o.operation DESC LIMIT 1",
                    params![deployment, subject, id, accepted_before],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((operation, actor, principal)) = operation else {
                continue;
            };
            let mut resource: Exec = serde_json::from_str(json)?;
            debug_assert!(matches!(state.as_str(), "accepted" | "running"));
            resource.state = substrate_wire::ExecState::Unknown;
            resource.observed_at = observed_at.parse()?;
            transaction.execute(
                "UPDATE execs SET resource_json = ?4, output_complete = 1
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![deployment, subject, id, serde_json::to_string(&resource)?],
            )?;
            let scope = Scope {
                deployment: deployment.to_owned(),
                subject: subject.clone(),
            };
            let projected_session = project_session_from_exec(&transaction, &scope, &resource)?;
            let event = append_event(
                &transaction,
                self.event_retention,
                &scope,
                id,
                "exec",
                "exec.unknown",
                &observed_at,
                &actor,
                principal.as_deref(),
                &operation,
                Some(serde_json::to_value(&resource)?),
            )?;
            effects.push(commit_effect(&scope, &event));
            if let Some((session, previous_state)) = projected_session
                && previous_state != session.state
            {
                let session_event = append_event(
                    &transaction,
                    self.event_retention,
                    &scope,
                    &session.id,
                    "session",
                    "session.unknown",
                    &observed_at,
                    &actor,
                    principal.as_deref(),
                    &operation,
                    Some(serde_json::to_value(&session)?),
                )?;
                effects.push(commit_effect(&scope, &session_event));
            }
            recovered = recovered.saturating_add(1);
            if recovered.saturating_sub(accepted.len()) == exec_limit {
                break;
            }
        }
        if let Some((state, subject, id, _)) = rows.last() {
            transaction.execute(
                "INSERT INTO maintenance_cursors (
                    deployment, queue, subject, resource_kind, resource_id
                 ) VALUES (?1, 'restart-exec-reconcile', ?2, ?3, ?4)
                 ON CONFLICT (deployment, queue) DO UPDATE SET
                    subject = excluded.subject,
                    resource_kind = excluded.resource_kind,
                    resource_id = excluded.resource_id",
                params![deployment, subject, state, id],
            )?;
        }
        transaction.commit()?;
        drop(connection);
        self.report_committed(&effects);
        Ok(recovered)
    }
}

fn scan_restart_exec_rows(
    transaction: &rusqlite::Transaction<'_>,
    deployment: &str,
    cursor: (&str, &str, &str),
    limit: usize,
) -> Result<Vec<(String, String, String, String)>, StoreError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let limit = u64::try_from(limit).map_err(|_| StoreError::IntegerRange)?;
    let mut rows = {
        let mut statement = transaction.prepare(
            "SELECT json_extract(resource_json, '$.state'), subject, id, resource_json
             FROM execs
             WHERE deployment = ?1
               AND json_extract(resource_json, '$.state') IN ('accepted','running')
               AND (json_extract(resource_json, '$.state'), subject, id) > (?2, ?3, ?4)
             ORDER BY json_extract(resource_json, '$.state'), subject, id LIMIT ?5",
        )?;
        statement
            .query_map(
                params![deployment, cursor.0, cursor.1, cursor.2, to_i64(limit)?],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    let remaining = limit.saturating_sub(u64::try_from(rows.len()).unwrap_or(u64::MAX));
    if remaining > 0 && (!cursor.0.is_empty() || !cursor.1.is_empty() || !cursor.2.is_empty()) {
        let wrapped = {
            let mut statement = transaction.prepare(
                "SELECT json_extract(resource_json, '$.state'), subject, id, resource_json
                 FROM execs
                 WHERE deployment = ?1
                   AND json_extract(resource_json, '$.state') IN ('accepted','running')
                   AND (json_extract(resource_json, '$.state'), subject, id) <= (?2, ?3, ?4)
                 ORDER BY json_extract(resource_json, '$.state'), subject, id LIMIT ?5",
            )?;
            statement
                .query_map(
                    params![deployment, cursor.0, cursor.1, cursor.2, to_i64(remaining)?],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        rows.extend(wrapped);
    }
    Ok(rows)
}

fn maintenance_cursor(
    connection: &Connection,
    deployment: &str,
    queue: &str,
) -> Result<(String, String, String), StoreError> {
    Ok(connection
        .query_row(
            "SELECT resource_kind, subject, resource_id FROM maintenance_cursors
             WHERE deployment = ?1 AND queue = ?2",
            params![deployment, queue],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .unwrap_or_default())
}

fn set_maintenance_cursor(
    connection: &Connection,
    deployment: &str,
    queue: &str,
    subject: &str,
    resource_kind: &str,
    resource_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO maintenance_cursors (
            deployment, queue, subject, resource_kind, resource_id
         ) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (deployment, queue) DO UPDATE SET
            subject = excluded.subject,
            resource_kind = excluded.resource_kind,
            resource_id = excluded.resource_id",
        params![deployment, queue, subject, resource_kind, resource_id],
    )?;
    Ok(())
}

fn scan_workspace_recovery_rows(
    connection: &Connection,
    deployment: &str,
    cursor_subject: &str,
    cursor_id: &str,
    limit: usize,
) -> Result<Vec<(String, String, String, String)>, StoreError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let limit = u64::try_from(limit).map_err(|_| StoreError::IntegerRange)?;
    let mut rows = {
        let mut statement = connection.prepare(
            "SELECT subject, id, root_name, resource_json FROM workspaces
             WHERE deployment = ?1 AND json_extract(resource_json, '$.state') = 'unknown'
               AND (subject, id) > (?2, ?3)
             ORDER BY subject, id LIMIT ?4",
        )?;
        statement
            .query_map(
                params![deployment, cursor_subject, cursor_id, to_i64(limit)?],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    let remaining = limit.saturating_sub(u64::try_from(rows.len()).unwrap_or(u64::MAX));
    if remaining > 0 && (!cursor_subject.is_empty() || !cursor_id.is_empty()) {
        let wrapped = {
            let mut statement = connection.prepare(
                "SELECT subject, id, root_name, resource_json FROM workspaces
                 WHERE deployment = ?1 AND json_extract(resource_json, '$.state') = 'unknown'
                   AND (subject, id) <= (?2, ?3)
                 ORDER BY subject, id LIMIT ?4",
            )?;
            statement
                .query_map(
                    params![deployment, cursor_subject, cursor_id, to_i64(remaining)?],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        rows.extend(wrapped);
    }
    Ok(rows)
}

fn scan_exec_recovery_keys(
    connection: &Connection,
    deployment: &str,
    cursor_subject: &str,
    cursor_id: &str,
    limit: usize,
) -> Result<Vec<(String, String)>, StoreError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let limit = u64::try_from(limit).map_err(|_| StoreError::IntegerRange)?;
    let mut rows = {
        let mut statement = connection.prepare(
            "SELECT subject, id FROM execs
             WHERE deployment = ?1 AND json_extract(resource_json, '$.state') = 'unknown'
               AND physically_absent = 0 AND (subject, id) > (?2, ?3)
             ORDER BY subject, id LIMIT ?4",
        )?;
        statement
            .query_map(
                params![deployment, cursor_subject, cursor_id, to_i64(limit)?],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    let remaining = limit.saturating_sub(u64::try_from(rows.len()).unwrap_or(u64::MAX));
    if remaining > 0 && (!cursor_subject.is_empty() || !cursor_id.is_empty()) {
        let wrapped = {
            let mut statement = connection.prepare(
                "SELECT subject, id FROM execs
                 WHERE deployment = ?1 AND json_extract(resource_json, '$.state') = 'unknown'
                   AND physically_absent = 0 AND (subject, id) <= (?2, ?3)
                 ORDER BY subject, id LIMIT ?4",
            )?;
            statement
                .query_map(
                    params![deployment, cursor_subject, cursor_id, to_i64(remaining)?],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        rows.extend(wrapped);
    }
    Ok(rows)
}

fn parse_operation_state(value: &str) -> Result<OperationState, StoreError> {
    match value {
        "refused" => Ok(OperationState::Refused),
        "accepted" => Ok(OperationState::Accepted),
        "unknown" => Ok(OperationState::Unknown),
        "terminal" => Ok(OperationState::Terminal),
        _ => Err(StoreError::NotAccepted(value.to_owned())),
    }
}

fn migrate_subject_streams(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection.prepare("PRAGMA table_info(stream_meta)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if columns.iter().any(|column| column == "subject") {
        return Ok(());
    }
    connection.execute_batch(
        "
        BEGIN IMMEDIATE;
        DROP INDEX IF EXISTS events_subject_sequence;
        DROP TABLE events;
        DROP TABLE stream_meta;
        CREATE TABLE stream_meta (
            deployment TEXT NOT NULL,
            subject TEXT NOT NULL,
            source_scope TEXT NOT NULL,
            generation INTEGER NOT NULL CHECK (generation > 0),
            next_seq INTEGER NOT NULL CHECK (next_seq > 0),
            PRIMARY KEY (deployment, subject),
            UNIQUE (deployment, source_scope)
        ) WITHOUT ROWID;
        CREATE TABLE events (
            deployment TEXT NOT NULL,
            subject TEXT NOT NULL,
            generation INTEGER NOT NULL,
            seq INTEGER NOT NULL,
            event_json TEXT NOT NULL,
            PRIMARY KEY (deployment, subject, seq)
        ) WITHOUT ROWID;
        CREATE INDEX events_subject_sequence ON events (deployment, subject, seq);
        DELETE FROM snapshot_items;
        DELETE FROM snapshots;
        COMMIT;
        ",
    )?;
    Ok(())
}

fn migrate_stream_scope_grammar(connection: &Connection) -> Result<(), StoreError> {
    let invalid: i64 = connection.query_row(
        "SELECT COUNT(*) FROM stream_meta
         WHERE substr(source_scope, 1, 6) != 'scope_'
            OR length(source_scope) <= 6
            OR substr(source_scope, 7) GLOB '*[^A-Za-z0-9_-]*'",
        [],
        |row| row.get(0),
    )?;
    if invalid == 0 {
        return Ok(());
    }
    // Invalid legacy source identities cannot produce contract-valid cursors. Rotate only those
    // streams to a fresh epoch and invalidate their historical events/snapshots atomically.
    connection.execute_batch(
        "
        BEGIN IMMEDIATE;
        DELETE FROM events
         WHERE (deployment, subject) IN (
             SELECT deployment, subject FROM stream_meta
              WHERE substr(source_scope, 1, 6) != 'scope_'
                 OR length(source_scope) <= 6
                 OR substr(source_scope, 7) GLOB '*[^A-Za-z0-9_-]*'
         );
        DELETE FROM snapshots
         WHERE (deployment, subject) IN (
             SELECT deployment, subject FROM stream_meta
              WHERE substr(source_scope, 1, 6) != 'scope_'
                 OR length(source_scope) <= 6
                 OR substr(source_scope, 7) GLOB '*[^A-Za-z0-9_-]*'
         );
        UPDATE stream_meta
           SET source_scope = 'scope_' || lower(hex(randomblob(16))),
               generation = generation + 1,
               next_seq = 1
         WHERE substr(source_scope, 1, 6) != 'scope_'
            OR length(source_scope) <= 6
            OR substr(source_scope, 7) GLOB '*[^A-Za-z0-9_-]*';
        COMMIT;
        ",
    )?;
    Ok(())
}

fn migrate_snapshot_source_scope(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection.prepare("PRAGMA table_info(snapshots)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if columns.iter().any(|column| column == "source_scope") {
        return Ok(());
    }
    connection.execute_batch(
        "
        BEGIN IMMEDIATE;
        ALTER TABLE snapshots ADD COLUMN source_scope TEXT NOT NULL DEFAULT '';
        UPDATE snapshots
           SET source_scope = COALESCE((
               SELECT stream_meta.source_scope
                 FROM stream_meta
                WHERE stream_meta.deployment = snapshots.deployment
                  AND stream_meta.subject = snapshots.subject
           ), '');
        DELETE FROM snapshots WHERE source_scope = '';
        COMMIT;
        ",
    )?;
    Ok(())
}

fn migrate_lease_authority(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection.prepare("PRAGMA table_info(leases)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if columns
        .iter()
        .any(|column| column == "authorizing_operation")
    {
        connection.execute(
            "DELETE FROM leases
             WHERE trim(authorizing_operation) = ''
                OR authorizing_operation = 'legacy-lease-authority-unavailable'",
            [],
        )?;
        return Ok(());
    }
    connection.execute_batch(
        "
        BEGIN IMMEDIATE;
        ALTER TABLE leases ADD COLUMN authorizing_operation TEXT NOT NULL DEFAULT '';
        ALTER TABLE leases ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE leases ADD COLUMN next_attempt_at TEXT;
        ALTER TABLE leases ADD COLUMN last_error TEXT;
        UPDATE leases
           SET authorizing_operation = COALESCE((
               SELECT operations.operation FROM operations
                WHERE operations.deployment = leases.deployment
                  AND operations.subject = leases.subject
                  AND operations.resource = leases.resource_id
                  AND operations.operation_kind LIKE leases.resource_kind || '.%'
                ORDER BY COALESCE(operations.terminal_at, operations.accepted_at) DESC,
                         operations.operation DESC LIMIT 1
           ), '');
        DELETE FROM leases WHERE trim(authorizing_operation) = '';
        COMMIT;
        ",
    )?;
    Ok(())
}

fn migrate_exec_physical_absence(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection.prepare("PRAGMA table_info(execs)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if !columns.iter().any(|column| column == "physically_absent") {
        connection.execute(
            "ALTER TABLE execs ADD COLUMN physically_absent INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    connection.execute(
        "CREATE INDEX IF NOT EXISTS execs_recovery_absence
         ON execs (
            deployment,
            json_extract(resource_json, '$.state'),
            physically_absent,
            subject,
            id
         )",
        [],
    )?;
    Ok(())
}

fn migrate_workspace_cleanup_progress(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection.prepare("PRAGMA table_info(workspace_cleanup)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if !columns.iter().any(|column| column == "progress_batches") {
        connection.execute(
            "ALTER TABLE workspace_cleanup
             ADD COLUMN progress_batches INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !columns.iter().any(|column| column == "removed_items") {
        connection.execute(
            "ALTER TABLE workspace_cleanup
             ADD COLUMN removed_items INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

fn validate_store_config(config: StoreConfig) -> Result<(), StoreError> {
    if config.event_retention == 0 {
        return Err(StoreError::InvalidEventRetention);
    }
    if config.operation_subject_max_rows == 0
        || config.operation_subject_max_bytes == 0
        || config.operation_global_max_rows == 0
        || config.operation_global_max_bytes == 0
        || config.operation_max_row_bytes == 0
        || config.operation_terminal_headroom_bytes == 0
        || config.operation_subject_max_rows > config.operation_global_max_rows
        || config.operation_subject_max_bytes > config.operation_global_max_bytes
        || config.operation_max_row_bytes > config.operation_subject_max_bytes
        || config.operation_max_row_bytes > config.operation_global_max_bytes
        || config.operation_terminal_headroom_bytes > config.operation_max_row_bytes
        || config.snapshot_max_workspaces == 0
        || config.snapshot_max_execs == 0
        || config.snapshot_max_provenance_events == 0
        || config.snapshot_prune_batch_size == 0
        || config
            .snapshot_max_workspaces
            .saturating_add(config.snapshot_max_execs)
            .saturating_add(config.snapshot_max_provenance_events)
            > u64::try_from(MAX_SNAPSHOT_ITEMS).map_err(|_| StoreError::IntegerRange)?
    {
        return Err(StoreError::InvalidStoreConfig);
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // One bounded, atomic legacy-accounting migration.
fn migrate_operation_ledger_accounting(
    connection: &mut Connection,
    config: StoreConfig,
) -> Result<(), StoreError> {
    let mut statement = connection.prepare("PRAGMA table_info(operations)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if !columns.iter().any(|column| column == "row_bytes") {
        connection.execute(
            "ALTER TABLE operations ADD COLUMN row_bytes INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !columns.iter().any(|column| column == "charged_bytes") {
        connection.execute(
            "ALTER TABLE operations ADD COLUMN charged_bytes INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }

    let fingerprint = operation_accounting_fingerprint(config);
    let stored_fingerprint = connection
        .query_row(
            "SELECT value FROM store_metadata WHERE key = 'operation-accounting-v1'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(stored_fingerprint) = stored_fingerprint {
        if stored_fingerprint != fingerprint {
            validate_durable_operation_occupancy(connection, config)?;
            return Err(StoreError::InvalidStoreConfig);
        }
        validate_durable_operation_occupancy(connection, config)?;
        return Ok(());
    }
    for table in ["operations", "workspaces", "execs"] {
        if bounded_table_exceeds(connection, table, MAX_AUTOMATIC_MIGRATION_ROWS)? {
            return Err(StoreError::OfflineMigrationRequired);
        }
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let operations = {
        let mut statement = transaction.prepare(
            "SELECT deployment, subject, operation, state FROM operations
             ORDER BY deployment, subject, operation",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (deployment, subject, operation, state) in operations {
        let scope = Scope {
            deployment,
            subject,
        };
        let row_bytes = operation_row_bytes(&transaction, &scope, &operation)?;
        let charged_bytes = if matches!(state.as_str(), "accepted" | "unknown") {
            row_bytes
                .checked_add(config.operation_terminal_headroom_bytes)
                .ok_or(StoreError::IntegerRange)?
        } else {
            row_bytes
        };
        if row_bytes > config.operation_max_row_bytes
            || charged_bytes > config.operation_max_row_bytes
        {
            return Err(StoreError::OperationOccupancy(OperationCapacity::RowBytes));
        }
        transaction.execute(
            "UPDATE operations SET row_bytes = ?4, charged_bytes = ?5
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
            params![
                scope.deployment,
                scope.subject,
                operation,
                to_i64(row_bytes)?,
                to_i64(charged_bytes)?,
            ],
        )?;
    }
    transaction.execute("DELETE FROM operation_ledger_usage", [])?;
    transaction.execute(
        "INSERT INTO operation_ledger_usage (
            deployment, subject, row_count, byte_count
         )
         SELECT deployment, subject, COUNT(*), SUM(charged_bytes)
         FROM operations GROUP BY deployment, subject",
        [],
    )?;
    transaction.execute("DELETE FROM operation_ledger_global_usage", [])?;
    transaction.execute(
        "INSERT INTO operation_ledger_global_usage (singleton, row_count, byte_count)
         SELECT 1, COUNT(*), COALESCE(SUM(charged_bytes), 0) FROM operations",
        [],
    )?;
    validate_durable_operation_occupancy(&transaction, config)?;
    validate_durable_resource_occupancy(&transaction, config)?;
    transaction.execute(
        "INSERT INTO store_metadata (key, value)
         VALUES ('operation-accounting-v1', ?1)",
        params![fingerprint],
    )?;
    transaction.commit()?;
    Ok(())
}

fn operation_accounting_fingerprint(config: StoreConfig) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}",
        config.operation_subject_max_rows,
        config.operation_subject_max_bytes,
        config.operation_global_max_rows,
        config.operation_global_max_bytes,
        config.operation_max_row_bytes,
        config.operation_terminal_headroom_bytes,
        config.snapshot_max_workspaces,
        config.snapshot_max_execs,
        config.snapshot_max_provenance_events,
    )
}

fn bounded_table_exceeds(
    connection: &Connection,
    table: &str,
    limit: u64,
) -> Result<bool, StoreError> {
    if !matches!(table, "operations" | "workspaces" | "execs") {
        return Err(StoreError::InvalidStoreConfig);
    }
    let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} LIMIT 1 OFFSET ?1)");
    connection
        .query_row(&sql, params![to_i64(limit)?], |row| row.get::<_, bool>(0))
        .map_err(StoreError::from)
}

fn validate_durable_resource_occupancy(
    connection: &Connection,
    config: StoreConfig,
) -> Result<(), StoreError> {
    for (table, cap, capacity) in [
        (
            "workspaces",
            config.snapshot_max_workspaces,
            ResourceCapacity::Workspaces,
        ),
        ("execs", config.snapshot_max_execs, ResourceCapacity::Execs),
    ] {
        let count: i64 = connection.query_row(
            &format!(
                "SELECT COALESCE(MAX(resource_count), 0) FROM (
                    SELECT COUNT(*) AS resource_count FROM {table}
                    GROUP BY deployment, subject
                 )"
            ),
            [],
            |row| row.get(0),
        )?;
        if to_u64(count)? > cap {
            return Err(StoreError::ResourceOccupancy(capacity));
        }
    }
    Ok(())
}

fn resource_partition_at_capacity(
    connection: &Connection,
    scope: &Scope,
    table: &str,
    capacity: u64,
) -> Result<bool, StoreError> {
    if !matches!(table, "workspaces" | "execs") {
        return Err(StoreError::SnapshotLimit);
    }
    let exists: i64 = connection.query_row(
        &format!(
            "SELECT EXISTS(
                SELECT 1 FROM {table}
                 WHERE deployment = ?1 AND subject = ?2
                 ORDER BY id LIMIT 1 OFFSET ?3
             )"
        ),
        params![
            scope.deployment,
            scope.subject,
            to_i64(capacity.saturating_sub(1))?
        ],
        |row| row.get(0),
    )?;
    Ok(exists != 0)
}

fn validate_durable_operation_occupancy(
    connection: &Connection,
    config: StoreConfig,
) -> Result<(), StoreError> {
    let subject_rows_exceeded: bool = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM operation_ledger_usage WHERE row_count > ?1 LIMIT 1
         )",
        params![to_i64(config.operation_subject_max_rows)?],
        |row| row.get(0),
    )?;
    if subject_rows_exceeded {
        return Err(StoreError::OperationOccupancy(
            OperationCapacity::SubjectRows,
        ));
    }
    let subject_bytes_exceeded: bool = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM operation_ledger_usage WHERE byte_count > ?1 LIMIT 1
         )",
        params![to_i64(config.operation_subject_max_bytes)?],
        |row| row.get(0),
    )?;
    if subject_bytes_exceeded {
        return Err(StoreError::OperationOccupancy(
            OperationCapacity::SubjectBytes,
        ));
    }
    let (rows, bytes) = connection.query_row(
        "SELECT row_count, byte_count FROM operation_ledger_global_usage WHERE singleton = 1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    if to_u64(rows)? > config.operation_global_max_rows {
        return Err(StoreError::OperationOccupancy(
            OperationCapacity::GlobalRows,
        ));
    }
    if to_u64(bytes)? > config.operation_global_max_bytes {
        return Err(StoreError::OperationOccupancy(
            OperationCapacity::GlobalBytes,
        ));
    }
    Ok(())
}

fn operation_row_bytes(
    connection: &Connection,
    scope: &Scope,
    operation: &str,
) -> Result<u64, StoreError> {
    let bytes: i64 = connection.query_row(
        "SELECT
            length(CAST(deployment AS BLOB)) + length(CAST(subject AS BLOB))
          + length(CAST(operation AS BLOB)) + length(CAST(operation_kind AS BLOB))
          + length(CAST(request_hash AS BLOB)) + length(CAST(state AS BLOB))
          + COALESCE(length(CAST(accepted_at AS BLOB)), 0)
          + COALESCE(length(CAST(terminal_at AS BLOB)), 0)
          + COALESCE(length(CAST(capability_snapshot AS BLOB)), 0)
          + length(CAST(actor AS BLOB)) + COALESCE(length(CAST(principal AS BLOB)), 0)
          + COALESCE(length(CAST(resource AS BLOB)), 0)
          + COALESCE(length(CAST(outcome_json AS BLOB)), 0)
          + CASE WHEN response_status IS NULL THEN 0 ELSE 8 END
         FROM operations
         WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
        params![scope.deployment, scope.subject, operation],
        |row| row.get(0),
    )?;
    to_u64(bytes)
}

fn charge_new_operation(
    connection: &Connection,
    config: StoreConfig,
    scope: &Scope,
    operation: &str,
    reserve_terminal: bool,
) -> Result<(), StoreError> {
    let row_bytes = operation_row_bytes(connection, scope, operation)?;
    let charged_bytes = if reserve_terminal {
        row_bytes
            .checked_add(config.operation_terminal_headroom_bytes)
            .ok_or(StoreError::IntegerRange)?
    } else {
        row_bytes
    };
    if row_bytes > config.operation_max_row_bytes || charged_bytes > config.operation_max_row_bytes
    {
        return Err(StoreError::OperationCapacity(OperationCapacity::RowBytes));
    }
    let (subject_rows, subject_bytes) = connection
        .query_row(
            "SELECT row_count, byte_count FROM operation_ledger_usage
             WHERE deployment = ?1 AND subject = ?2",
            params![scope.deployment, scope.subject],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .unwrap_or_default();
    let (global_rows, global_bytes): (i64, i64) = connection.query_row(
        "SELECT row_count, byte_count FROM operation_ledger_global_usage WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let next_subject_rows = to_u64(subject_rows)?.saturating_add(1);
    let next_subject_bytes = to_u64(subject_bytes)?
        .checked_add(charged_bytes)
        .ok_or(StoreError::IntegerRange)?;
    let next_global_rows = to_u64(global_rows)?.saturating_add(1);
    let next_global_bytes = to_u64(global_bytes)?
        .checked_add(charged_bytes)
        .ok_or(StoreError::IntegerRange)?;
    let capacity = if next_subject_rows > config.operation_subject_max_rows {
        Some(OperationCapacity::SubjectRows)
    } else if next_subject_bytes > config.operation_subject_max_bytes {
        Some(OperationCapacity::SubjectBytes)
    } else if next_global_rows > config.operation_global_max_rows {
        Some(OperationCapacity::GlobalRows)
    } else if next_global_bytes > config.operation_global_max_bytes {
        Some(OperationCapacity::GlobalBytes)
    } else {
        None
    };
    if let Some(capacity) = capacity {
        return Err(StoreError::OperationCapacity(capacity));
    }
    connection.execute(
        "UPDATE operations SET row_bytes = ?4, charged_bytes = ?5
         WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
        params![
            scope.deployment,
            scope.subject,
            operation,
            to_i64(row_bytes)?,
            to_i64(charged_bytes)?,
        ],
    )?;
    connection.execute(
        "INSERT INTO operation_ledger_usage (deployment, subject, row_count, byte_count)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT (deployment, subject) DO UPDATE SET
            row_count = row_count + 1,
            byte_count = byte_count + excluded.byte_count",
        params![scope.deployment, scope.subject, to_i64(charged_bytes)?],
    )?;
    connection.execute(
        "UPDATE operation_ledger_global_usage
         SET row_count = row_count + 1, byte_count = byte_count + ?1
         WHERE singleton = 1",
        params![to_i64(charged_bytes)?],
    )?;
    Ok(())
}

fn finalize_operation_accounting(
    connection: &Connection,
    config: StoreConfig,
    scope: &Scope,
    operation: &str,
) -> Result<(), StoreError> {
    let charged: i64 = connection.query_row(
        "SELECT charged_bytes FROM operations
         WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
        params![scope.deployment, scope.subject, operation],
        |row| row.get(0),
    )?;
    let charged = to_u64(charged)?;
    let row_bytes = operation_row_bytes(connection, scope, operation)?;
    if row_bytes > config.operation_max_row_bytes || row_bytes > charged {
        return Err(StoreError::OperationTerminalHeadroom(operation.to_owned()));
    }
    let released = charged - row_bytes;
    connection.execute(
        "UPDATE operations SET row_bytes = ?4, charged_bytes = ?4
         WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
        params![
            scope.deployment,
            scope.subject,
            operation,
            to_i64(row_bytes)?
        ],
    )?;
    connection.execute(
        "UPDATE operation_ledger_usage SET byte_count = byte_count - ?3
         WHERE deployment = ?1 AND subject = ?2",
        params![scope.deployment, scope.subject, to_i64(released)?],
    )?;
    connection.execute(
        "UPDATE operation_ledger_global_usage SET byte_count = byte_count - ?1
         WHERE singleton = 1",
        params![to_i64(released)?],
    )?;
    Ok(())
}

fn refresh_nonterminal_operation_accounting(
    connection: &Connection,
    config: StoreConfig,
    scope: &Scope,
    operation: &str,
) -> Result<(), StoreError> {
    let charged: i64 = connection.query_row(
        "SELECT charged_bytes FROM operations
         WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
        params![scope.deployment, scope.subject, operation],
        |row| row.get(0),
    )?;
    let row_bytes = operation_row_bytes(connection, scope, operation)?;
    if row_bytes > config.operation_max_row_bytes || row_bytes > to_u64(charged)? {
        return Err(StoreError::OperationTerminalHeadroom(operation.to_owned()));
    }
    connection.execute(
        "UPDATE operations SET row_bytes = ?4
         WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
        params![
            scope.deployment,
            scope.subject,
            operation,
            to_i64(row_bytes)?
        ],
    )?;
    Ok(())
}

fn existing_reservation(
    connection: &Connection,
    new: &NewOperation,
) -> Result<Option<Reservation>, StoreError> {
    let Some(existing) = load_operation(connection, &new.scope, &new.operation)? else {
        return Ok(None);
    };
    Ok(Some(if existing.record.request_hash != new.request_hash {
        Reservation::Conflict
    } else if let Some(answer) = existing.answer {
        Reservation::Replay(answer)
    } else {
        Reservation::Pending(existing.record)
    }))
}

fn insert_accepted_operation(
    connection: &Connection,
    retention: u64,
    config: StoreConfig,
    new: &NewOperation,
) -> Result<Event, StoreError> {
    connection.execute(
        "INSERT INTO operations (
            deployment, subject, operation, operation_kind, request_hash, state, accepted_at,
            capability_snapshot, actor, principal, resource
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'accepted', ?6, ?7, ?8, ?9, ?10)",
        params![
            new.scope.deployment,
            new.scope.subject,
            new.operation,
            new.operation_kind,
            new.request_hash,
            new.accepted_at,
            new.capability_snapshot,
            new.actor,
            new.principal,
            new.resource,
        ],
    )?;
    charge_new_operation(connection, config, &new.scope, &new.operation, true)?;
    append_event(
        connection,
        retention,
        &new.scope,
        new.resource.as_deref().unwrap_or(new.operation.as_str()),
        operation_resource_kind(&new.operation_kind),
        "operation.accepted",
        &new.accepted_at,
        &new.actor,
        new.principal.as_deref(),
        &new.operation,
        Some(json!({
            "operation_kind": new.operation_kind,
            "state": "accepted"
        })),
    )
}

fn insert_refused_operation(
    connection: &Connection,
    retention: u64,
    config: StoreConfig,
    new: &NewOperation,
    terminal_at: &str,
    status: u16,
    error: &ErrorDetail,
) -> Result<(StoredAnswer, Event), StoreError> {
    let outcome = OperationOutcome::Error {
        error: error.clone(),
    };
    connection.execute(
        "INSERT INTO operations (
            deployment, subject, operation, operation_kind, request_hash, state, accepted_at,
            terminal_at, capability_snapshot, actor, principal, resource, outcome_json,
            response_status
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'refused', NULL, ?6, NULL, ?7, ?8, ?9, ?10, ?11)",
        params![
            new.scope.deployment,
            new.scope.subject,
            new.operation,
            new.operation_kind,
            new.request_hash,
            terminal_at,
            new.actor,
            new.principal,
            new.resource,
            serde_json::to_string(&outcome)?,
            i64::from(status),
        ],
    )?;
    charge_new_operation(connection, config, &new.scope, &new.operation, false)?;
    let event = append_event(
        connection,
        retention,
        &new.scope,
        new.resource.as_deref().unwrap_or(&new.operation),
        operation_resource_kind(&new.operation_kind),
        "operation.refused",
        terminal_at,
        &new.actor,
        new.principal.as_deref(),
        &new.operation,
        Some(serde_json::to_value(&outcome)?),
    )?;
    Ok((StoredAnswer { status, outcome }, event))
}

fn is_terminal_exec_state(state: ExecState) -> bool {
    matches!(
        state,
        ExecState::Exited | ExecState::Cancelled | ExecState::Expired
    )
}

fn to_u64(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::IntegerRange)
}

fn to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::IntegerRange)
}

fn ensure_stream(connection: &Connection, scope: &Scope) -> Result<(), StoreError> {
    if connection
        .query_row(
            "SELECT 1 FROM stream_meta WHERE deployment = ?1 AND subject = ?2",
            params![scope.deployment, scope.subject],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        return Ok(());
    }
    for _ in 0..8 {
        connection.execute(
            "INSERT OR IGNORE INTO stream_meta (
                deployment, subject, source_scope, generation, next_seq
             ) VALUES (
                ?1, ?2, 'scope_' || lower(hex(randomblob(16))),
                ((random() & 9223372036854775807) % 9007199254740990) + 1, 1
             )",
            params![scope.deployment, scope.subject],
        )?;
        if connection
            .query_row(
                "SELECT 1 FROM stream_meta WHERE deployment = ?1 AND subject = ?2",
                params![scope.deployment, scope.subject],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            return Ok(());
        }
    }
    Err(StoreError::IntegerRange)
}

fn stream_position(
    connection: &Connection,
    scope: &Scope,
) -> Result<(String, u64, u64), StoreError> {
    let (source_scope, generation, next_seq): (String, i64, i64) = connection.query_row(
        "SELECT source_scope, generation, next_seq FROM stream_meta
         WHERE deployment = ?1 AND subject = ?2",
        params![scope.deployment, scope.subject],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok((
        source_scope,
        to_u64(generation)?,
        to_u64(next_seq)?.saturating_sub(1),
    ))
}

#[allow(clippy::too_many_arguments)]
fn append_event(
    connection: &Connection,
    retention: u64,
    scope: &Scope,
    resource: &str,
    resource_kind: &str,
    transition: &str,
    observed_at: &str,
    actor: &str,
    principal: Option<&str>,
    operation: &str,
    observation: Option<Value>,
) -> Result<Event, StoreError> {
    let operation_transition = transition.starts_with("operation.");
    let observation = if operation_transition {
        serde_json::to_value(
            load_operation(connection, scope, operation)?
                .ok_or_else(|| StoreError::NotAccepted(operation.to_owned()))?
                .record,
        )?
    } else {
        observation.ok_or_else(|| {
            StoreError::NotAccepted(format!("event {transition} is missing its observation"))
        })?
    };
    append_event_with_cause(
        connection,
        retention,
        scope,
        if operation_transition {
            operation
        } else {
            resource
        },
        if operation_transition {
            "operation"
        } else {
            resource_kind
        },
        transition,
        observed_at,
        actor,
        principal,
        EventCause::Operation {
            operation: operation.to_owned(),
        },
        observation,
    )
}

#[allow(clippy::too_many_arguments)]
fn append_control_event(
    connection: &Connection,
    retention: u64,
    scope: &Scope,
    resource: &str,
    resource_kind: &str,
    transition: &str,
    observed_at: &str,
    actor: &str,
    principal: Option<&str>,
    observation: Value,
) -> Result<Event, StoreError> {
    append_event_with_cause(
        connection,
        retention,
        scope,
        resource,
        resource_kind,
        transition,
        observed_at,
        actor,
        principal,
        EventCause::Control {
            control: EventControl::ReconciliationSnapshotCreate,
        },
        observation,
    )
}

#[allow(clippy::too_many_arguments)]
fn append_event_with_cause(
    connection: &Connection,
    retention: u64,
    scope: &Scope,
    resource: &str,
    resource_kind: &str,
    transition: &str,
    observed_at: &str,
    actor: &str,
    principal: Option<&str>,
    cause: EventCause,
    observation: Value,
) -> Result<Event, StoreError> {
    ensure_stream(connection, scope)?;
    let (source_scope, generation, current) = stream_position(connection, scope)?;
    let sequence = current.checked_add(1).ok_or(StoreError::IntegerRange)?;
    let event = Event {
        source_scope,
        generation,
        seq: sequence,
        resource: resource.to_owned(),
        resource_kind: resource_kind.to_owned(),
        transition: transition.to_owned(),
        observed_at: observed_at.parse()?,
        actor: actor.to_owned(),
        principal: principal.map(ToOwned::to_owned),
        cause,
        observation,
    };
    connection.execute(
        "INSERT INTO events (deployment, subject, generation, seq, event_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            scope.deployment,
            scope.subject,
            to_i64(generation)?,
            to_i64(sequence)?,
            serde_json::to_string(&event)?
        ],
    )?;
    connection.execute(
        "UPDATE stream_meta SET next_seq = ?3 WHERE deployment = ?1 AND subject = ?2",
        params![
            scope.deployment,
            scope.subject,
            to_i64(sequence.checked_add(1).ok_or(StoreError::IntegerRange)?)?
        ],
    )?;
    let delete_through = sequence.saturating_sub(retention);
    if delete_through > 0 {
        connection.execute(
            "DELETE FROM events WHERE deployment = ?1 AND subject = ?2 AND seq <= ?3",
            params![scope.deployment, scope.subject, to_i64(delete_through)?],
        )?;
    }
    Ok(event)
}

fn commit_effect(scope: &Scope, event: &Event) -> CommitEffect {
    CommitEffect {
        scope: scope.clone(),
        source_scope: event.source_scope.clone(),
        generation: event.generation,
        through_seq: event.seq,
    }
}

fn event_cursor(source_scope: &str, generation: u64, sequence: u64) -> String {
    format!("ev2.{source_scope}.{generation}.{sequence}")
}

fn parse_event_cursor(value: &str) -> Option<(&str, u64, u64)> {
    let mut parts = value.strip_prefix("ev2.")?.split('.');
    let source_scope = parts.next()?;
    let generation = parse_canonical_u64(parts.next()?)?;
    let sequence = parse_canonical_u64(parts.next()?)?;
    (parts.next().is_none() && !source_scope.is_empty()).then_some((
        source_scope,
        generation,
        sequence,
    ))
}

fn snapshot_cursor(snapshot: &str, ordinal: u64) -> String {
    format!("sp2.{snapshot}.{ordinal}")
}

fn parse_canonical_u64(value: &str) -> Option<u64> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return None;
    }
    let parsed: u64 = value.parse().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

fn prune_expired_snapshots_transaction(
    transaction: &rusqlite::Transaction<'_>,
    deployment: &str,
    now: DateTime<Utc>,
    batch_size: u64,
) -> Result<u64, StoreError> {
    let cursor = transaction
        .query_row(
            "SELECT subject, resource_id FROM maintenance_cursors
             WHERE deployment = ?1 AND queue = 'snapshot-prune'",
            params![deployment],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .unwrap_or_default();
    let rows =
        scan_snapshot_maintenance_rows(transaction, deployment, &cursor.0, &cursor.1, batch_size)?;
    let now = now.to_rfc3339();
    let mut removed = 0_u64;
    for (subject, id, expires_at) in &rows {
        if expires_at <= &now {
            mark_and_delete_expired_snapshot(transaction, deployment, subject, id, expires_at)?;
            prune_expired_snapshot_markers_for_scope(
                transaction,
                &Scope {
                    deployment: deployment.to_owned(),
                    subject: subject.clone(),
                },
                batch_size,
            )?;
            removed = removed.saturating_add(1);
        }
    }
    if let Some((subject, id, _)) = rows.last() {
        transaction.execute(
            "INSERT INTO maintenance_cursors (
                deployment, queue, subject, resource_kind, resource_id
             ) VALUES (?1, 'snapshot-prune', ?2, 'snapshot', ?3)
             ON CONFLICT (deployment, queue) DO UPDATE SET
                subject = excluded.subject,
                resource_kind = excluded.resource_kind,
                resource_id = excluded.resource_id",
            params![deployment, subject, id],
        )?;
    }
    prune_one_expired_snapshot_marker_scope(transaction, deployment, batch_size)?;
    Ok(removed)
}

fn scan_snapshot_maintenance_rows(
    transaction: &rusqlite::Transaction<'_>,
    deployment: &str,
    cursor_subject: &str,
    cursor_id: &str,
    limit: u64,
) -> Result<Vec<(String, String, String)>, StoreError> {
    let mut rows = {
        let mut statement = transaction.prepare(
            "SELECT subject, id, expires_at FROM snapshots
             WHERE deployment = ?1 AND (subject, id) > (?2, ?3)
             ORDER BY subject, id LIMIT ?4",
        )?;
        statement
            .query_map(
                params![deployment, cursor_subject, cursor_id, to_i64(limit)?],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    let remaining = limit.saturating_sub(u64::try_from(rows.len()).unwrap_or(u64::MAX));
    if remaining > 0 && (!cursor_subject.is_empty() || !cursor_id.is_empty()) {
        let wrapped = {
            let mut statement = transaction.prepare(
                "SELECT subject, id, expires_at FROM snapshots
                 WHERE deployment = ?1 AND (subject, id) <= (?2, ?3)
                 ORDER BY subject, id LIMIT ?4",
            )?;
            statement
                .query_map(
                    params![deployment, cursor_subject, cursor_id, to_i64(remaining)?],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        rows.extend(wrapped);
    }
    Ok(rows)
}

fn prune_one_expired_snapshot_marker_scope(
    transaction: &rusqlite::Transaction<'_>,
    deployment: &str,
    batch_size: u64,
) -> Result<(), StoreError> {
    let cursor = transaction
        .query_row(
            "SELECT subject, resource_id FROM maintenance_cursors
             WHERE deployment = ?1 AND queue = 'snapshot-marker-prune'",
            params![deployment],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .unwrap_or_default();
    let next = transaction
        .query_row(
            "SELECT subject, id FROM expired_snapshots
             WHERE deployment = ?1 AND (subject, id) > (?2, ?3)
             ORDER BY subject, id LIMIT 1",
            params![deployment, cursor.0, cursor.1],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let next = match next {
        Some(next) => Some(next),
        None => transaction
            .query_row(
                "SELECT subject, id FROM expired_snapshots
                 WHERE deployment = ?1 ORDER BY subject, id LIMIT 1",
                params![deployment],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?,
    };
    let Some((subject, id)) = next else {
        return Ok(());
    };
    prune_expired_snapshot_markers_for_scope(
        transaction,
        &Scope {
            deployment: deployment.to_owned(),
            subject: subject.clone(),
        },
        batch_size,
    )?;
    transaction.execute(
        "INSERT INTO maintenance_cursors (
            deployment, queue, subject, resource_kind, resource_id
         ) VALUES (?1, 'snapshot-marker-prune', ?2, 'snapshot', ?3)
         ON CONFLICT (deployment, queue) DO UPDATE SET
            subject = excluded.subject,
            resource_kind = excluded.resource_kind,
            resource_id = excluded.resource_id",
        params![deployment, subject, id],
    )?;
    Ok(())
}

fn prune_expired_snapshots_for_scope_transaction(
    transaction: &rusqlite::Transaction<'_>,
    scope: &Scope,
    now: DateTime<Utc>,
    batch_size: u64,
) -> Result<u64, StoreError> {
    let rows = {
        let mut statement = transaction.prepare(
            "SELECT id, expires_at FROM snapshots
             WHERE deployment = ?1 AND subject = ?2 AND expires_at <= ?3
             ORDER BY expires_at, id LIMIT ?4",
        )?;
        statement
            .query_map(
                params![
                    scope.deployment,
                    scope.subject,
                    now.to_rfc3339(),
                    to_i64(batch_size)?
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (id, expires_at) in &rows {
        mark_and_delete_expired_snapshot(
            transaction,
            &scope.deployment,
            &scope.subject,
            id,
            expires_at,
        )?;
    }
    prune_expired_snapshot_markers_for_scope(transaction, scope, batch_size)?;
    u64::try_from(rows.len()).map_err(|_| StoreError::IntegerRange)
}

fn mark_and_delete_expired_snapshot(
    transaction: &rusqlite::Transaction<'_>,
    deployment: &str,
    subject: &str,
    id: &str,
    expires_at: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO expired_snapshots (deployment, subject, id, expired_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (deployment, subject, id) DO UPDATE SET expired_at = excluded.expired_at",
        params![deployment, subject, id, expires_at],
    )?;
    transaction.execute(
        "DELETE FROM snapshots WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
        params![deployment, subject, id],
    )?;
    Ok(())
}

fn prune_expired_snapshot_markers_for_scope(
    transaction: &rusqlite::Transaction<'_>,
    scope: &Scope,
    batch_size: u64,
) -> Result<(), StoreError> {
    transaction.execute(
        "DELETE FROM expired_snapshots
         WHERE deployment = ?1 AND subject = ?2 AND id IN (
             SELECT id FROM expired_snapshots
             WHERE deployment = ?1 AND subject = ?2
             ORDER BY expired_at DESC, id DESC LIMIT ?3 OFFSET ?4
         )",
        params![
            scope.deployment,
            scope.subject,
            to_i64(batch_size)?,
            MAX_EXPIRED_SNAPSHOT_MARKERS_PER_SCOPE,
        ],
    )?;
    Ok(())
}

fn parse_snapshot_cursor(value: &str, snapshot: &str) -> Option<u64> {
    parse_canonical_u64(value.strip_prefix(&format!("sp2.{snapshot}."))?)
}

fn operation_resource_kind(operation_kind: &str) -> &str {
    if operation_kind.starts_with("workspace.") {
        "workspace"
    } else if operation_kind.starts_with("exec.") {
        "exec"
    } else if operation_kind.starts_with("session.") {
        "session"
    } else if operation_kind.starts_with("reconciliation.") {
        "snapshot"
    } else {
        "operation"
    }
}

fn terminal_transition(operation_kind: &str, outcome: &OperationOutcome) -> &'static str {
    if matches!(outcome, OperationOutcome::Error { .. }) {
        return "operation.failed";
    }
    match operation_kind {
        "workspace.create" => "workspace.created",
        "workspace.file.write" => "workspace.file-written",
        "workspace.file.delete" => "workspace.file-deleted",
        "workspace.destroy" => "workspace.destroyed",
        "workspace.lease.renew" => "workspace.lease-renewed",
        "exec.start" | "exec.pipe.start" => "exec.observed",
        "exec.signal" => "exec.cancelled",
        "exec.lease.renew" => "exec.lease-renewed",
        "session.start" => "session.ready",
        "session.signal" => "session.cancelled",
        "session.lease.renew" => "session.lease-renewed",
        "session.retire" => "session.retired",
        "reconciliation.snapshot.create" => "snapshot.created",
        _ => "operation.terminal",
    }
}

fn append_snapshot_refusal_event(
    connection: &Connection,
    retention: u64,
    scope: &Scope,
    actor: &str,
    principal: Option<&str>,
    observed_at: DateTime<Utc>,
) -> Result<Event, StoreError> {
    let detail = ErrorDetail {
        class: ErrorClass::Exhausted,
        code: "snapshot.materialization-limit".to_owned(),
        message: "Snapshot materialization exceeds the bounded item limit.".to_owned(),
        retriable: false,
        address: Some("snapshot".to_owned()),
        operation: None,
    };
    append_control_event(
        connection,
        retention,
        scope,
        "reconciliation.snapshot.create",
        "snapshot",
        "snapshot.refused",
        &observed_at.to_rfc3339(),
        actor,
        principal,
        serde_json::to_value(detail)?,
    )
}

fn operation_identity(
    connection: &Connection,
    scope: &Scope,
    operation: &str,
) -> Result<(String, Option<String>), StoreError> {
    connection
        .query_row(
            "SELECT actor, principal FROM operations
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
            params![scope.deployment, scope.subject, operation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(StoreError::from)
}

fn operation_identity_full(
    connection: &Connection,
    scope: &Scope,
    operation: &str,
) -> Result<(String, Option<String>, String), StoreError> {
    connection
        .query_row(
            "SELECT actor, principal, operation_kind FROM operations
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
            params![scope.deployment, scope.subject, operation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(StoreError::from)
}

fn resource_operation_identity(
    connection: &Connection,
    scope: &Scope,
    resource: &str,
) -> Result<Option<(String, String, Option<String>)>, StoreError> {
    connection
        .query_row(
            "SELECT operation, actor, principal FROM (
                SELECT operation, actor, principal, accepted_at FROM operations
                WHERE deployment = ?1 AND subject = ?2 AND resource = ?3
                UNION ALL
                SELECT o.operation, o.actor, o.principal, o.accepted_at
                FROM sessions s JOIN operations o
                  ON o.deployment = s.deployment AND o.subject = s.subject
                 AND o.resource = s.id AND o.operation_kind = 'session.start'
                WHERE s.deployment = ?1 AND s.subject = ?2 AND s.exec_id = ?3
             ) ORDER BY accepted_at LIMIT 1",
            params![scope.deployment, scope.subject, resource],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(StoreError::from)
}

fn insert_tombstone(
    connection: &Connection,
    scope: &Scope,
    kind: &str,
    id: &str,
    deleted_at: &str,
    reason: &str,
    value: &Value,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO tombstones (
            deployment, subject, resource_kind, resource_id, deleted_at, reason, value_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (deployment, subject, resource_kind, resource_id) DO UPDATE SET
            deleted_at = excluded.deleted_at,
            reason = excluded.reason,
            value_json = excluded.value_json",
        params![
            scope.deployment,
            scope.subject,
            kind,
            id,
            deleted_at,
            reason,
            serde_json::to_string(value)?
        ],
    )?;
    Ok(())
}

fn upsert_lease(
    connection: &Connection,
    scope: &Scope,
    kind: &str,
    id: &str,
    lease: &NewLease,
    authorizing_operation: &str,
) -> Result<(), StoreError> {
    if lease.authorizing_operation != authorizing_operation {
        return Err(StoreError::LeaseAuthorityMismatch);
    }
    let renew_by = lease.clock.wall
        + chrono::Duration::milliseconds(
            i64::try_from(lease.ttl_ms).map_err(|_| StoreError::IntegerRange)?,
        );
    let deadline = lease
        .clock
        .boottime_ms
        .checked_add(lease.ttl_ms)
        .ok_or(StoreError::IntegerRange)?;
    connection.execute(
        "INSERT INTO leases (
            deployment, subject, resource_kind, resource_id, ttl_ms, issued_wall, renew_by_wall,
            boot_id, issued_boottime_ms, deadline_boottime_ms, state, authorizing_operation,
            attempt_count, next_attempt_at, last_error
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'active', ?11, 0, NULL, NULL)
         ON CONFLICT (deployment, subject, resource_kind, resource_id) DO UPDATE SET
            ttl_ms = excluded.ttl_ms,
            issued_wall = excluded.issued_wall,
            renew_by_wall = excluded.renew_by_wall,
            boot_id = excluded.boot_id,
            issued_boottime_ms = excluded.issued_boottime_ms,
            deadline_boottime_ms = excluded.deadline_boottime_ms,
            state = 'active',
            authorizing_operation = excluded.authorizing_operation,
            attempt_count = 0,
            next_attempt_at = NULL,
            last_error = NULL",
        params![
            scope.deployment,
            scope.subject,
            kind,
            id,
            to_i64(lease.ttl_ms)?,
            lease.clock.wall.to_rfc3339(),
            renew_by.to_rfc3339(),
            lease.clock.boot_id,
            to_i64(lease.clock.boottime_ms)?,
            to_i64(deadline)?,
            authorizing_operation,
        ],
    )?;
    Ok(())
}

fn lease_due(
    clock: &LeaseClock,
    boot_id: &str,
    issued_wall: &DateTime<Utc>,
    issued_boottime_ms: u64,
    renew_by: &DateTime<Utc>,
    deadline_boottime_ms: u64,
) -> bool {
    if boot_id.is_empty()
        || clock.boot_id != boot_id
        || clock.boottime_ms < issued_boottime_ms
        || clock.boottime_ms >= deadline_boottime_ms
    {
        return true;
    }
    let elapsed = clock.boottime_ms - issued_boottime_ms;
    let expected_wall =
        *issued_wall + chrono::Duration::milliseconds(i64::try_from(elapsed).unwrap_or(i64::MAX));
    let skew = (clock.wall - expected_wall)
        .num_milliseconds()
        .unsigned_abs();
    skew > substrate_wire::LEASE_CLOCK_TOLERANCE_MS
        || clock.wall
            > *renew_by
                + chrono::Duration::milliseconds(
                    i64::try_from(substrate_wire::LEASE_CLOCK_TOLERANCE_MS)
                        .expect("tolerance fits i64"),
                )
}

#[allow(clippy::too_many_lines)] // Projection, freeze, and event authority stay one transaction.
fn freeze_workspace_lease_if_due(
    connection: &Connection,
    retention: u64,
    scope: &Scope,
    id: &str,
    resource: &mut Workspace,
    clock: Option<&LeaseClock>,
) -> Result<(bool, Option<Event>), StoreError> {
    let row = connection
        .query_row(
            "SELECT ttl_ms, issued_wall, renew_by_wall, boot_id, issued_boottime_ms,
                    deadline_boottime_ms, state, authorizing_operation
             FROM leases WHERE deployment = ?1 AND subject = ?2
               AND resource_kind = 'workspace' AND resource_id = ?3",
            params![scope.deployment, scope.subject, id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((
        ttl_ms,
        issued_wall,
        renew_by,
        boot_id,
        issued_boot,
        deadline_boot,
        state,
        authorizing_operation,
    )) = row
    else {
        return Ok((false, None));
    };
    let lease_state = match state.as_str() {
        "active" => LeaseState::Active,
        "expiring" => LeaseState::Expiring,
        "expired" => LeaseState::Expired,
        _ => return Err(StoreError::LeaseExpired),
    };
    let renew_by: DateTime<Utc> = renew_by.parse()?;
    let due = if lease_state == LeaseState::Active {
        let clock = clock.ok_or(StoreError::LeaseClockUnavailable)?;
        lease_due(
            clock,
            &boot_id,
            &issued_wall.parse()?,
            to_u64(issued_boot)?,
            &renew_by,
            to_u64(deadline_boot)?,
        )
    } else {
        false
    };
    let projected_state = if due {
        LeaseState::Expiring
    } else {
        lease_state
    };
    let (actor, principal) = operation_identity(connection, scope, &authorizing_operation)?;
    let projected = LeaseObservation {
        ttl_ms: to_u64(ttl_ms)?,
        renew_by,
        state: projected_state,
        clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
        authorizing_operation,
        actor,
        principal,
    };
    let projection_changed = resource.lease.as_ref() != Some(&projected);
    resource.lease = Some(projected);
    if due {
        connection.execute(
            "UPDATE leases SET state = 'expiring'
             WHERE deployment = ?1 AND subject = ?2 AND resource_kind = 'workspace'
               AND resource_id = ?3 AND state = 'active'",
            params![scope.deployment, scope.subject, id],
        )?;
    }
    if due || projection_changed {
        connection.execute(
            "UPDATE workspaces SET resource_json = ?4
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![
                scope.deployment,
                scope.subject,
                id,
                serde_json::to_string(resource)?
            ],
        )?;
    }
    if !due {
        return Ok((false, None));
    }
    let operation = lease_authorizing_operation(connection, scope, "workspace", id)?;
    let (_, principal) = operation_identity(connection, scope, &operation)?;
    let observed_at = clock.expect("due lease requires clock").wall.to_rfc3339();
    let event = append_event(
        connection,
        retention,
        scope,
        id,
        "workspace",
        "workspace.lease-expiring",
        &observed_at,
        LEASE_SWEEPER_ACTOR,
        principal.as_deref(),
        &operation,
        Some(serde_json::to_value(resource)?),
    )?;
    Ok((true, Some(event)))
}

fn lease_authorizing_operation(
    connection: &Connection,
    scope: &Scope,
    kind: &str,
    id: &str,
) -> Result<String, StoreError> {
    connection
        .query_row(
            "SELECT authorizing_operation FROM leases
             WHERE deployment = ?1 AND subject = ?2
               AND resource_kind = ?3 AND resource_id = ?4",
            params![scope.deployment, scope.subject, kind, id],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
}

fn ensure_lease_renewable(
    connection: &Connection,
    scope: &Scope,
    kind: &str,
    id: &str,
    clock: &LeaseClock,
) -> Result<(), StoreError> {
    let row = connection
        .query_row(
            "SELECT issued_wall, renew_by_wall, boot_id, issued_boottime_ms,
                    deadline_boottime_ms, state
             FROM leases WHERE deployment = ?1 AND subject = ?2
               AND resource_kind = ?3 AND resource_id = ?4",
            params![scope.deployment, scope.subject, kind, id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((issued_wall, renew_by, boot_id, issued_boot, deadline_boot, state)) = row else {
        return Err(StoreError::LeaseAbsent);
    };
    if state != "active"
        || lease_due(
            clock,
            &boot_id,
            &issued_wall.parse()?,
            to_u64(issued_boot)?,
            &renew_by.parse()?,
            to_u64(deadline_boot)?,
        )
    {
        return Err(StoreError::LeaseExpired);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn complete_lease_operation<T: Serialize>(
    connection: &Connection,
    retention: u64,
    config: StoreConfig,
    scope: &Scope,
    operation: &str,
    terminal_at: &str,
    status: u16,
    resource_id: &str,
    resource_kind: &str,
    transition: &str,
    resource: &T,
) -> Result<Event, StoreError> {
    let outcome = OperationOutcome::Success {
        result: serde_json::to_value(resource)?,
    };
    let changed = connection.execute(
        "UPDATE operations
         SET state = 'terminal', terminal_at = ?4, resource = ?5, outcome_json = ?6,
             response_status = ?7
         WHERE deployment = ?1 AND subject = ?2 AND operation = ?3 AND state = 'accepted'",
        params![
            scope.deployment,
            scope.subject,
            operation,
            terminal_at,
            resource_id,
            serde_json::to_string(&outcome)?,
            i64::from(status)
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::NotAccepted(operation.to_owned()));
    }
    finalize_operation_accounting(connection, config, scope, operation)?;
    let (actor, principal) = operation_identity(connection, scope, operation)?;
    append_event(
        connection,
        retention,
        scope,
        resource_id,
        resource_kind,
        transition,
        terminal_at,
        &actor,
        principal.as_deref(),
        operation,
        Some(serde_json::to_value(resource)?),
    )
}

#[allow(clippy::too_many_arguments)]
fn complete_operation_error_transaction(
    connection: &Connection,
    retention: u64,
    config: StoreConfig,
    scope: &Scope,
    operation: &str,
    terminal_at: &str,
    status: u16,
    resource_id: Option<&str>,
    error: &ErrorDetail,
) -> Result<Event, StoreError> {
    let outcome = OperationOutcome::Error {
        error: error.clone(),
    };
    let changed = connection.execute(
        "UPDATE operations
         SET state = 'terminal', terminal_at = ?4, resource = ?5, outcome_json = ?6,
             response_status = ?7
         WHERE deployment = ?1 AND subject = ?2 AND operation = ?3
           AND state IN ('accepted','unknown')",
        params![
            scope.deployment,
            scope.subject,
            operation,
            terminal_at,
            resource_id,
            serde_json::to_string(&outcome)?,
            i64::from(status),
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::NotAccepted(operation.to_owned()));
    }
    finalize_operation_accounting(connection, config, scope, operation)?;
    let (actor, principal, operation_kind) = operation_identity_full(connection, scope, operation)?;
    append_event(
        connection,
        retention,
        scope,
        resource_id.unwrap_or(operation),
        operation_resource_kind(&operation_kind),
        "operation.failed",
        terminal_at,
        &actor,
        principal.as_deref(),
        operation,
        Some(serde_json::to_value(&outcome)?),
    )
}

fn collect_snapshot_partition(
    connection: &Connection,
    scope: &Scope,
    table: &str,
    item_kind: SnapshotItemKind,
    id_prefix: &str,
    limit: i64,
) -> Result<Vec<SnapshotItem>, StoreError> {
    if !matches!(table, "workspaces" | "execs") {
        return Err(StoreError::SnapshotLimit);
    }
    let sql = format!(
        "SELECT id, resource_json FROM {table}
         WHERE deployment = ?1 AND subject = ?2 ORDER BY id LIMIT ?3"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map(params![scope.deployment, scope.subject, limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, value)| {
            Ok(SnapshotItem {
                ordinal: 0,
                kind: item_kind,
                id: format!("{id_prefix}:{id}"),
                value: serde_json::from_str(&value)?,
            })
        })
        .collect()
}

fn collect_snapshot_provenance(
    connection: &Connection,
    scope: &Scope,
    through_seq: u64,
    limit: usize,
) -> Result<Vec<SnapshotItem>, StoreError> {
    let source_scope = stream_position(connection, scope)?.0;
    let mut statement = connection.prepare(
        "SELECT event_json FROM (
             SELECT seq, event_json FROM events
             WHERE deployment = ?1 AND subject = ?2 AND seq <= ?3
             ORDER BY seq DESC LIMIT ?4
         ) ORDER BY seq",
    )?;
    let values = statement
        .query_map(
            params![
                scope.deployment,
                scope.subject,
                to_i64(through_seq)?,
                to_i64(u64::try_from(limit).map_err(|_| StoreError::IntegerRange)?)?,
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    values
        .into_iter()
        .map(|value| {
            let mut event: Event = serde_json::from_str(&value)?;
            event.source_scope.clone_from(&source_scope);
            Ok(SnapshotItem {
                ordinal: 0,
                kind: SnapshotItemKind::ProvenanceEvent,
                id: format!("event:{}:{}", event.generation, event.seq),
                value: serde_json::to_value(event)?,
            })
        })
        .collect()
}

fn workspace_has_nonterminal_execs(
    connection: &Connection,
    scope: &Scope,
    workspace_id: &str,
) -> Result<bool, StoreError> {
    let mut statement = connection.prepare(
        "SELECT resource_json FROM execs
         WHERE deployment = ?1 AND subject = ?2 AND workspace_id = ?3
           AND physically_absent = 0",
    )?;
    let resources = statement
        .query_map(
            params![scope.deployment, scope.subject, workspace_id],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    for json in resources {
        let resource: Exec = serde_json::from_str(&json)?;
        if matches!(
            resource.state,
            ExecState::Accepted | ExecState::Running | ExecState::Unknown
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

struct LoadedOperation {
    record: OperationRecord,
    answer: Option<StoredAnswer>,
}

fn load_exec(
    connection: &Connection,
    scope: &Scope,
    id: &str,
) -> Result<Option<StoredExec>, StoreError> {
    connection
        .query_row(
            "SELECT resource_json, stdout, stderr, stdout_truncated, stderr_truncated,
                    output_complete, cgroup, leader_pid
             FROM execs WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                ))
            },
        )
        .optional()?
        .map(
            |(
                json,
                stdout,
                stderr,
                stdout_truncated,
                stderr_truncated,
                output_complete,
                cgroup,
                leader_pid,
            )| {
                Ok(StoredExec {
                    resource: serde_json::from_str(&json)?,
                    stdout,
                    stderr,
                    stdout_truncated,
                    stderr_truncated,
                    output_complete,
                    cgroup,
                    leader_pid: leader_pid.and_then(|value| u32::try_from(value).ok()),
                })
            },
        )
        .transpose()
}

fn load_session(
    connection: &Connection,
    scope: &Scope,
    id: &str,
) -> Result<Option<PipeSession>, StoreError> {
    connection
        .query_row(
            "SELECT resource_json FROM sessions
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|json| serde_json::from_str(&json).map_err(StoreError::from))
        .transpose()
}

fn load_session_for_exec(
    connection: &Connection,
    scope: &Scope,
    exec_id: &str,
) -> Result<Option<PipeSession>, StoreError> {
    connection
        .query_row(
            "SELECT resource_json FROM sessions
             WHERE deployment = ?1 AND subject = ?2 AND exec_id = ?3",
            params![scope.deployment, scope.subject, exec_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|json| serde_json::from_str(&json).map_err(StoreError::from))
        .transpose()
}

fn load_operation(
    connection: &Connection,
    scope: &Scope,
    operation: &str,
) -> Result<Option<LoadedOperation>, StoreError> {
    let stored = connection
        .query_row(
            "SELECT operation_kind, request_hash, state, accepted_at, terminal_at,
                    capability_snapshot, actor, principal, resource, outcome_json, response_status
             FROM operations WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
            params![scope.deployment, scope.subject, operation],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((
        operation_kind,
        request_hash,
        state,
        accepted_at,
        terminal_at,
        capability_snapshot,
        actor,
        principal,
        resource,
        outcome_json,
        response_status,
    )) = stored
    else {
        return Ok(None);
    };
    let state = match state.as_str() {
        "refused" => OperationState::Refused,
        "accepted" => OperationState::Accepted,
        "unknown" => OperationState::Unknown,
        "terminal" => OperationState::Terminal,
        _ => unreachable!("state constrained by SQLite"),
    };
    let outcome = outcome_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    let answer = match (response_status, outcome.clone()) {
        (Some(status), Some(outcome)) => Some(StoredAnswer {
            status: u16::try_from(status).map_err(|_| StoreError::StatusRange)?,
            outcome,
        }),
        _ => None,
    };
    Ok(Some(LoadedOperation {
        record: OperationRecord {
            operation: operation.to_owned(),
            operation_kind,
            request_hash,
            state,
            accepted_at: accepted_at.as_deref().map(str::parse).transpose()?,
            terminal_at: terminal_at.as_deref().map(str::parse).transpose()?,
            capability_snapshot,
            actor,
            principal,
            resource,
            outcome,
        },
        answer,
    }))
}

fn upsert_workspace(
    connection: &Connection,
    scope: &Scope,
    root_name: &str,
    workspace: &Workspace,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO workspaces (deployment, subject, id, root_name, resource_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (deployment, subject, id) DO UPDATE SET
             root_name = excluded.root_name, resource_json = excluded.resource_json",
        params![
            scope.deployment,
            scope.subject,
            workspace.id,
            root_name,
            serde_json::to_string(workspace)?,
        ],
    )?;
    Ok(())
}

fn upsert_exec(
    connection: &Connection,
    scope: &Scope,
    stored: &StoredExec,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO execs (
            deployment, subject, id, workspace_id, resource_json, stdout, stderr,
            stdout_truncated, stderr_truncated, output_complete, physically_absent, cgroup,
            leader_pid
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12)
         ON CONFLICT (deployment, subject, id) DO UPDATE SET
            resource_json = excluded.resource_json,
            stdout = excluded.stdout,
            stderr = excluded.stderr,
            stdout_truncated = excluded.stdout_truncated,
            stderr_truncated = excluded.stderr_truncated,
            output_complete = excluded.output_complete,
            physically_absent = 0,
            cgroup = excluded.cgroup,
            leader_pid = excluded.leader_pid",
        params![
            scope.deployment,
            scope.subject,
            stored.resource.id,
            stored.resource.workspace,
            serde_json::to_string(&stored.resource)?,
            stored.stdout,
            stored.stderr,
            stored.stdout_truncated,
            stored.stderr_truncated,
            stored.output_complete,
            stored.cgroup,
            stored.leader_pid.map(i64::from),
        ],
    )?;
    Ok(())
}

fn upsert_session(
    connection: &Connection,
    scope: &Scope,
    session: &PipeSession,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO sessions (deployment, subject, id, exec_id, resource_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (deployment, subject, id) DO UPDATE SET
            exec_id = excluded.exec_id,
            resource_json = excluded.resource_json",
        params![
            scope.deployment,
            scope.subject,
            session.id,
            session.exec,
            serde_json::to_string(session)?,
        ],
    )?;
    Ok(())
}

fn project_session_from_exec(
    connection: &Connection,
    scope: &Scope,
    exec: &Exec,
) -> Result<Option<(PipeSession, SessionState)>, StoreError> {
    let Some(mut session) = load_session_for_exec(connection, scope, &exec.id)? else {
        return Ok(None);
    };
    let previous_state = session.state;
    session.observed_at = exec.observed_at;
    session.exit.clone_from(&exec.exit);
    if let Some(lease) = exec.lease.as_ref() {
        session.lease.clone_from(lease);
    }
    session.state = match exec.state {
        ExecState::Accepted => SessionState::Accepted,
        ExecState::Running if session.attachment == SessionAttachmentState::Attached => {
            SessionState::Attached
        }
        ExecState::Running => SessionState::Ready,
        ExecState::Exited => SessionState::Exited,
        ExecState::Cancelled => SessionState::Cancelled,
        ExecState::Expired => SessionState::Expired,
        ExecState::Unknown => SessionState::Unknown,
    };
    if matches!(
        session.state,
        SessionState::Exited | SessionState::Cancelled | SessionState::Expired
    ) {
        session.attachment = SessionAttachmentState::Consumed;
    } else if session.state == SessionState::Unknown {
        session.attachment = SessionAttachmentState::Uncertain;
    }
    upsert_session(connection, scope, &session)?;
    Ok(Some((session, previous_state)))
}

const fn session_transition(state: SessionState) -> &'static str {
    match state {
        SessionState::Accepted => "session.accepted",
        SessionState::Ready => "session.ready",
        SessionState::Attached => "session.attached",
        SessionState::Exited => "session.exited",
        SessionState::Cancelled => "session.cancelled",
        SessionState::Expired => "session.lease-expired",
        SessionState::Unknown => "session.unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex as StdMutex, Weak};

    use chrono::TimeZone as _;
    use rusqlite::params;
    use substrate_wire::{
        ConfinementRequest, ErrorClass, ErrorDetail, EventCause, Exec, ExecKind, ExecState,
        LeaseObservation, LeaseState, NetworkMode, OperationOutcome, OperationState, PipeSession,
        PipeSessionLimits, SandboxProfile, SessionAttachmentState, SessionKind, SessionMode,
        SessionState, SnapshotItemKind, Workspace, WorkspaceKind, WorkspaceState,
    };
    use tempfile::tempdir;

    use super::{
        CommitEffect, CommitEffectSink, EventCursorError, ExecRetireReservation, ExecWrite,
        ExpiredLease, LEASE_SWEEPER_ACTOR, LeaseClock, LeaseResource, NewLease, NewOperation,
        OperationCapacity, Reservation, Scope, SessionAttachmentClaim, SnapshotReadError, Store,
        StoreConfig, StoreError, StoredExec, WorkspaceAdmission, WorkspaceDestroyReservation,
        WorkspaceObservationWrite, event_cursor, lease_due, upsert_exec, upsert_lease,
    };

    #[derive(Default)]
    struct RecordingEffects(StdMutex<Vec<CommitEffect>>);

    impl CommitEffectSink for RecordingEffects {
        fn committed(&self, effects: &[CommitEffect]) {
            self.0
                .lock()
                .expect("effect recorder")
                .extend_from_slice(effects);
        }
    }

    fn attach_effect_recorder(store: &Store) -> Arc<RecordingEffects> {
        let effects = Arc::new(RecordingEffects::default());
        let sink: Arc<dyn CommitEffectSink> = effects.clone();
        store.set_commit_effect_sink(sink);
        effects
    }

    fn clear_effects(effects: &RecordingEffects) {
        effects.0.lock().expect("effect recorder").clear();
    }

    fn assert_one_exact_effect(store: &Store, effects: &RecordingEffects, expected_scope: &Scope) {
        let recorded = effects.0.lock().expect("effect recorder").clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(&recorded[0].scope, expected_scope);
        let (source_scope, generation, through_seq) = store
            .stream_position(expected_scope)
            .expect("stream position after callback");
        assert_eq!(recorded[0].source_scope, source_scope);
        assert_eq!(recorded[0].generation, generation);
        assert_eq!(recorded[0].through_seq, through_seq);
    }

    struct ReentrantEffects {
        store: Weak<Store>,
        calls: StdMutex<usize>,
    }

    impl CommitEffectSink for ReentrantEffects {
        fn committed(&self, effects: &[CommitEffect]) {
            let store = self.store.upgrade().expect("store remains alive");
            for effect in effects {
                let (source_scope, generation, through_seq) = store
                    .stream_position(&effect.scope)
                    .expect("sink can re-enter the store after commit");
                assert_eq!(effect.source_scope, source_scope);
                assert_eq!(effect.generation, generation);
                assert_eq!(effect.through_seq, through_seq);
            }
            *self.calls.lock().expect("reentrant call counter") += 1;
        }
    }

    fn scope(subject: &str) -> Scope {
        Scope {
            deployment: "dep_test".to_owned(),
            subject: subject.to_owned(),
        }
    }

    fn ledger_config(subject_rows: u64, global_rows: u64) -> StoreConfig {
        StoreConfig {
            operation_subject_max_rows: subject_rows,
            operation_global_max_rows: global_rows,
            operation_subject_max_bytes: 8 * 1024 * 1024,
            operation_global_max_bytes: 32 * 1024 * 1024,
            operation_max_row_bytes: 1024 * 1024,
            operation_terminal_headroom_bytes: 512 * 1024,
            ..StoreConfig::default()
        }
    }

    fn operation(subject: &str, hash: &str) -> NewOperation {
        operation_named(
            subject,
            "01JSTORETEST0000000001",
            "workspace.create",
            "ws_reserved",
            hash,
        )
    }

    fn operation_named(
        subject: &str,
        operation: &str,
        kind: &str,
        resource: &str,
        hash: &str,
    ) -> NewOperation {
        NewOperation {
            scope: scope(subject),
            operation: operation.to_owned(),
            operation_kind: kind.to_owned(),
            request_hash: hash.to_owned(),
            accepted_at: "2026-08-13T12:00:00Z".to_owned(),
            capability_snapshot: Some(format!("sha256:{}", "7".repeat(64))),
            actor: "test".to_owned(),
            principal: None,
            resource: Some(resource.to_owned()),
        }
    }

    fn workspace(id: &str) -> Workspace {
        Workspace {
            id: id.to_owned(),
            kind: WorkspaceKind::Workspace,
            labels: BTreeMap::new(),
            observed_at: "2026-08-13T12:00:01Z".parse().expect("time"),
            state: WorkspaceState::Ready,
            lease: None,
        }
    }

    fn exec(id: &str, workspace: &str, state: ExecState) -> StoredExec {
        StoredExec {
            resource: Exec {
                id: id.to_owned(),
                kind: ExecKind::Exec,
                workspace: workspace.to_owned(),
                state,
                observed_at: "2026-08-13T12:00:01Z".parse().expect("time"),
                requested: ConfinementRequest {
                    capability_snapshot: format!("sha256:{}", "7".repeat(64)),
                    network: NetworkMode::None,
                    profile: SandboxProfile::Workspace,
                    required: true,
                },
                applied: None,
                exit: None,
                lease: None,
            },
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            output_complete: false,
            cgroup: None,
            leader_pid: None,
        }
    }

    fn pipe_session(id: &str, exec_id: &str, workspace: &str, lease: &NewLease) -> PipeSession {
        PipeSession {
            id: id.to_owned(),
            kind: SessionKind::Session,
            mode: SessionMode::Pipes,
            exec: exec_id.to_owned(),
            workspace: workspace.to_owned(),
            state: SessionState::Accepted,
            attachment: SessionAttachmentState::Pending,
            observed_at: "2026-08-13T12:00:01Z".parse().expect("time"),
            capability_snapshot: format!("sha256:{}", "7".repeat(64)),
            limits: PipeSessionLimits {
                input_bytes: 1_024,
                frame_bytes: 256,
                queued_frames: 4,
            },
            exit: None,
            lease: lease.observation(),
        }
    }

    fn seed_exec(store: &Store, scope: &Scope, resource: &StoredExec) {
        upsert_exec(&store.connection.lock(), scope, resource).expect("seed exec membership");
    }

    fn lease_at(subject: &str, id: &str, ttl_ms: u64) -> (NewOperation, NewLease) {
        let operation = operation_named(
            subject,
            &format!("lease-authority-{id}"),
            "workspace.create",
            id,
            &"a".repeat(64),
        );
        let lease = NewLease {
            ttl_ms,
            clock: LeaseClock {
                wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap(),
                boot_id: "boot-test".to_owned(),
                boottime_ms: 1_000,
            },
            authorizing_operation: operation.operation.clone(),
            actor: operation.actor.clone(),
            principal: operation.principal.clone(),
        };
        (operation, lease)
    }

    fn seed_leased_workspace(store: &Store, subject: &str, id: &str, ttl_ms: u64) -> NewOperation {
        let (operation, lease) = lease_at(subject, id, ttl_ms);
        store.reserve(&operation).expect("reserve lease authority");
        let mut resource = workspace(id);
        resource.lease = Some(lease.observation());
        store
            .complete_workspace_leased(
                &operation.scope,
                &operation.operation,
                "2026-08-13T12:00:00Z",
                201,
                id,
                &resource,
                Some(&lease),
            )
            .expect("complete leased workspace");
        operation
    }

    fn authorize_exec_lease(store: &Store, subject: &str, id: &str) -> NewOperation {
        let operation = operation_named(
            subject,
            &format!("exec-lease-authority-{id}"),
            "exec.start",
            id,
            &"b".repeat(64),
        );
        let lease = NewLease {
            ttl_ms: 1_000,
            clock: LeaseClock {
                wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap(),
                boot_id: "boot-test".to_owned(),
                boottime_ms: 1_000,
            },
            authorizing_operation: operation.operation.clone(),
            actor: operation.actor.clone(),
            principal: operation.principal.clone(),
        };
        store
            .reserve(&operation)
            .expect("reserve exec lease authority");
        let connection = store.connection.lock();
        upsert_lease(
            &connection,
            &operation.scope,
            "exec",
            id,
            &lease,
            &operation.operation,
        )
        .expect("persist exec lease authority");
        drop(connection);
        operation
    }

    #[test]
    fn commit_effects_report_only_new_events_after_successful_commit() {
        let store = Store::open(":memory:").expect("open store");
        let effects = Arc::new(RecordingEffects::default());
        let sink: Arc<dyn CommitEffectSink> = effects.clone();
        store.set_commit_effect_sink(sink);
        let accepted = operation("local:1000", &"1".repeat(64));

        assert_eq!(
            store.reserve(&accepted).expect("accept"),
            Reservation::Accepted
        );
        let recorded = effects.0.lock().expect("effect recorder").clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].scope, accepted.scope);
        let position = store
            .stream_position(&accepted.scope)
            .expect("stream position");
        assert_eq!(recorded[0].source_scope, position.0);
        assert_eq!(recorded[0].generation, position.1);
        assert_eq!(recorded[0].through_seq, 1);

        assert!(matches!(
            store.reserve(&accepted).expect("replay inspection"),
            Reservation::Pending(_)
        ));
        let mut conflicting = accepted.clone();
        conflicting.request_hash = "2".repeat(64);
        assert_eq!(
            store.reserve(&conflicting).expect("conflict inspection"),
            Reservation::Conflict
        );
        assert_eq!(effects.0.lock().expect("effect recorder").len(), 1);

        let failed = operation_named(
            "local:1000",
            "01JEFFECTROLLBACK000001",
            "exec.start",
            "ex_effect_rollback",
            &"3".repeat(64),
        );
        assert!(matches!(
            store.reserve_exec_start(
                &failed,
                &exec("ex_effect_rollback", "ws_missing", ExecState::Accepted),
                None,
                None,
            ),
            Err(StoreError::NotAccepted(_))
        ));
        assert_eq!(effects.0.lock().expect("effect recorder").len(), 1);

        // This helper appends a terminal event before validating the resource kind. The invalid
        // kind forces the transaction to roll back after append; neither state nor callback may
        // escape the rollback.
        let error = substrate_wire::ErrorDetail {
            class: substrate_wire::ErrorClass::Failed,
            code: "driver.failed".to_owned(),
            message: "failure".to_owned(),
            retriable: false,
            address: Some("resource".to_owned()),
            operation: Some(accepted.operation.clone()),
        };
        assert!(matches!(
            store.complete_dispatch_absence(
                &accepted.scope,
                &accepted.operation,
                "2026-08-13T12:00:02Z",
                500,
                "invalid-kind",
                "ws_reserved",
                &error,
            ),
            Err(StoreError::NotAccepted(_))
        ));
        assert_eq!(effects.0.lock().expect("effect recorder").len(), 1);
        assert_eq!(
            store
                .operation(&accepted.scope, &accepted.operation)
                .expect("operation lookup")
                .expect("accepted operation")
                .state,
            OperationState::Accepted
        );
    }

    #[test]
    fn commit_effect_callback_runs_after_database_lock_release() {
        let store = Arc::new(Store::open(":memory:").expect("open store"));
        let sink = Arc::new(ReentrantEffects {
            store: Arc::downgrade(&store),
            calls: StdMutex::new(0),
        });
        let erased: Arc<dyn CommitEffectSink> = sink.clone();
        store.set_commit_effect_sink(erased);

        assert_eq!(
            store
                .reserve(&operation("local:1000", &"4".repeat(64)))
                .expect("accept operation"),
            Reservation::Accepted
        );
        assert_eq!(*sink.calls.lock().expect("reentrant call counter"), 1);
    }

    #[test]
    fn destroy_terminal_and_conflict_report_exact_post_commit_effects() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_effect_destroy", &workspace("ws_effect_destroy"))
            .expect("seed workspace");
        let effects = attach_effect_recorder(&store);
        let destroy = operation_named(
            "local:1000",
            "01JEFFECTDESTROYTERM001",
            "workspace.destroy",
            "ws_effect_destroy",
            &"1".repeat(64),
        );
        assert!(matches!(
            store
                .reserve_workspace_destroy(&destroy, None)
                .expect("reserve destroy"),
            WorkspaceDestroyReservation::Admitted { .. }
        ));
        clear_effects(&effects);
        store
            .complete_workspace_absence(
                &scope,
                &destroy.operation,
                "2026-08-13T12:00:02Z",
                200,
                "ws_effect_destroy",
                &substrate_wire::WorkspaceAbsence {
                    kind: WorkspaceKind::Workspace,
                    id: "ws_effect_destroy".to_owned(),
                    absent: true,
                    observed_at: "2026-08-13T12:00:02Z".parse().expect("time"),
                },
            )
            .expect("complete destroy");
        assert_one_exact_effect(&store, &effects, &scope);

        store
            .put_workspace(
                &scope,
                "ws_effect_conflict",
                &workspace("ws_effect_conflict"),
            )
            .expect("seed conflict workspace");
        seed_exec(
            &store,
            &scope,
            &exec("ex_effect_active", "ws_effect_conflict", ExecState::Running),
        );
        clear_effects(&effects);
        let conflict = operation_named(
            "local:1000",
            "01JEFFECTDESTROYCONFLICT1",
            "workspace.destroy",
            "ws_effect_conflict",
            &"2".repeat(64),
        );
        assert!(matches!(
            store
                .reserve_workspace_destroy(&conflict, None)
                .expect("reserve conflict"),
            WorkspaceDestroyReservation::Refused { .. }
        ));
        assert_one_exact_effect(&store, &effects, &scope);
    }

    #[test]
    fn observation_terminal_and_lease_claim_failure_report_exact_effects() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_effect_exec", &workspace("ws_effect_exec"))
            .expect("seed workspace");
        let start = operation_named(
            "local:1000",
            "01JEFFECTEXECSTART00001",
            "exec.start",
            "ex_effect_terminal",
            &"3".repeat(64),
        );
        let running = exec("ex_effect_terminal", "ws_effect_exec", ExecState::Running);
        assert_eq!(
            store
                .reserve_exec_start(&start, &running, None, None)
                .expect("reserve exec"),
            Reservation::Accepted
        );
        store
            .complete_exec(
                &scope,
                &start.operation,
                "2026-08-13T12:00:01Z",
                202,
                &running.resource,
                &[],
                &[],
                false,
                false,
                false,
                None,
                None,
            )
            .expect("complete running observation");
        let effects = attach_effect_recorder(&store);
        let mut terminal = running;
        terminal.resource.state = ExecState::Exited;
        terminal.resource.exit = Some(substrate_wire::ExecExit {
            code: Some(0),
            signal: None,
        });
        terminal.output_complete = true;
        assert!(matches!(
            store
                .put_exec(&scope, &terminal)
                .expect("terminal observation"),
            ExecWrite::PersistedExact(_)
        ));
        assert_one_exact_effect(&store, &effects, &scope);

        let lease_authority = seed_leased_workspace(&store, "local:1001", "ws_effect_lease", 1_000);
        let due = LeaseClock {
            wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 1).unwrap(),
            boot_id: "boot-test".to_owned(),
            boottime_ms: 2_000,
        };
        let candidate = store
            .lease_cleanup_candidates("dep_test", &due, 10)
            .expect("lease candidates")
            .into_iter()
            .find(|candidate| candidate.id == "ws_effect_lease")
            .expect("lease candidate");
        clear_effects(&effects);
        let claimed = store
            .claim_expired_lease(&candidate, &due)
            .expect("claim lease")
            .expect("claimed lease");
        assert_one_exact_effect(&store, &effects, &lease_authority.scope);
        clear_effects(&effects);
        store
            .record_lease_cleanup_failure(&claimed, due.wall, "driver.busy")
            .expect("record cleanup failure");
        assert_one_exact_effect(&store, &effects, &lease_authority.scope);
    }

    #[test]
    fn snapshot_limit_reports_one_exact_effect_after_refusal_commit() {
        let store = Store::open_with_config(
            ":memory:",
            StoreConfig {
                snapshot_max_workspaces: 1,
                snapshot_max_execs: 1,
                snapshot_max_provenance_events: 1,
                ..StoreConfig::default()
            },
        )
        .expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_effect_over_a", &workspace("ws_effect_over_a"))
            .expect("seed first workspace");
        store
            .put_workspace(&scope, "ws_effect_over_b", &workspace("ws_effect_over_b"))
            .expect("seed second workspace");
        let effects = attach_effect_recorder(&store);
        assert!(matches!(
            store.complete_snapshot(
                &scope,
                "test",
                None,
                "2026-08-13T12:00:00Z".parse().expect("observed at"),
                "snap_effect_limit",
                "2026-08-13T13:00:00Z".parse().expect("expiry"),
            ),
            Err(StoreError::SnapshotLimit)
        ));
        assert_one_exact_effect(&store, &effects, &scope);
    }

    #[test]
    fn operation_row_quota_checks_existing_identity_before_capacity() {
        let store = Store::open_with_config(":memory:", ledger_config(1, 2)).expect("open store");
        let first = operation_named(
            "local:1000",
            "01JLEDGERROWFIRST000001",
            "workspace.create",
            "ws_ledger_first",
            &"1".repeat(64),
        );
        assert_eq!(store.reserve(&first).expect("first"), Reservation::Accepted);
        assert!(matches!(
            store.reserve(&first).expect("pending replay at capacity"),
            Reservation::Pending(_)
        ));
        let mut conflicting = first.clone();
        conflicting.request_hash = "2".repeat(64);
        assert_eq!(
            store.reserve(&conflicting).expect("conflict at capacity"),
            Reservation::Conflict
        );

        let same_subject = operation_named(
            "local:1000",
            "01JLEDGERROWSECOND00001",
            "workspace.create",
            "ws_ledger_second",
            &"3".repeat(64),
        );
        assert_eq!(
            store.reserve(&same_subject).expect("subject capacity"),
            Reservation::Capacity(OperationCapacity::SubjectRows)
        );
        assert!(
            store
                .operation(&same_subject.scope, &same_subject.operation)
                .expect("capacity lookup")
                .is_none()
        );

        let other_subject = operation_named(
            "local:1001",
            "01JLEDGERROWOTHER000001",
            "workspace.create",
            "ws_ledger_other",
            &"4".repeat(64),
        );
        assert_eq!(
            store.reserve(&other_subject).expect("other subject"),
            Reservation::Accepted
        );
        let global_full = operation_named(
            "local:1002",
            "01JLEDGERROWGLOBAL00001",
            "workspace.create",
            "ws_ledger_global",
            &"5".repeat(64),
        );
        assert_eq!(
            store.reserve(&global_full).expect("global capacity"),
            Reservation::Capacity(OperationCapacity::GlobalRows)
        );
    }

    #[test]
    fn refused_operations_charge_quota_and_replay_at_capacity() {
        let store = Store::open_with_config(":memory:", ledger_config(1, 10)).expect("open store");
        let refused = operation_named(
            "local:1000",
            "01JLEDGERREFUSED000001",
            "workspace.file.write",
            "ws_refused",
            &"6".repeat(64),
        );
        let detail = ErrorDetail {
            class: ErrorClass::Refused,
            code: "request.schema-invalid".to_owned(),
            message: "invalid".to_owned(),
            retriable: false,
            address: Some("input".to_owned()),
            operation: Some(refused.operation.clone()),
        };
        assert!(matches!(
            store
                .record_refusal(&refused, "2026-08-13T12:00:00Z", 422, &detail)
                .expect("record refusal"),
            Reservation::Replay(_)
        ));
        assert!(matches!(
            store
                .record_refusal(&refused, "2026-08-13T12:00:01Z", 422, &detail)
                .expect("replay refusal"),
            Reservation::Replay(_)
        ));
        let second = operation_named(
            "local:1000",
            "01JLEDGERREFUSED000002",
            "workspace.file.write",
            "ws_refused",
            &"7".repeat(64),
        );
        assert_eq!(
            store
                .record_refusal(&second, "2026-08-13T12:00:01Z", 422, &detail)
                .expect("refusal capacity"),
            Reservation::Capacity(OperationCapacity::SubjectRows)
        );
    }

    #[test]
    fn ledger_accounting_is_atomic_across_connections_and_survives_restart() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.db");
        let config = ledger_config(1, 10);
        let first = Store::open_with_config(&path, config).expect("first connection");
        let second = Store::open_with_config(&path, config).expect("second connection");
        let left = operation_named(
            "local:1000",
            "01JLEDGERCONCURRENT0001",
            "workspace.create",
            "ws_concurrent_left",
            &"8".repeat(64),
        );
        let right = operation_named(
            "local:1000",
            "01JLEDGERCONCURRENT0002",
            "workspace.create",
            "ws_concurrent_right",
            &"9".repeat(64),
        );
        let (left_result, right_result) = std::thread::scope(|threads| {
            let left_task = threads.spawn(|| first.reserve(&left).expect("left reservation"));
            let right_task = threads.spawn(|| second.reserve(&right).expect("right reservation"));
            (
                left_task.join().expect("left thread"),
                right_task.join().expect("right thread"),
            )
        });
        assert!(matches!(
            (left_result, right_result),
            (
                Reservation::Accepted,
                Reservation::Capacity(OperationCapacity::SubjectRows)
            ) | (
                Reservation::Capacity(OperationCapacity::SubjectRows),
                Reservation::Accepted
            )
        ));
        drop(first);
        drop(second);

        let reopened = Store::open_with_config(&path, config).expect("reopen at exact cap");
        let usage: (i64, i64) = reopened
            .connection
            .lock()
            .query_row(
                "SELECT row_count, byte_count FROM operation_ledger_usage
                 WHERE deployment = 'dep_test' AND subject = 'local:1000'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("durable usage");
        assert_eq!(usage.0, 1);
        assert!(usage.1 > 0);
    }

    #[test]
    fn startup_fails_closed_when_configured_caps_are_below_occupancy() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.db");
        let store = Store::open_with_config(&path, ledger_config(2, 10)).expect("open store");
        for index in 0..2 {
            let operation = operation_named(
                "local:1000",
                &format!("01JLEDGEROCCUPANCY{index:06}"),
                "workspace.create",
                &format!("ws_occupancy_{index}"),
                &format!("{index}").repeat(64),
            );
            assert_eq!(
                store.reserve(&operation).expect("seed occupancy"),
                Reservation::Accepted
            );
        }
        drop(store);

        assert!(matches!(
            Store::open_with_config(&path, ledger_config(1, 10)),
            Err(StoreError::OperationOccupancy(
                OperationCapacity::SubjectRows
            ))
        ));
        assert!(Store::open_with_config(&path, ledger_config(2, 10)).is_ok());
    }

    #[test]
    fn byte_quota_accepts_exact_boundary_and_refuses_next_row() {
        let directory = tempdir().expect("tempdir");
        let measure_path = directory.path().join("measure.db");
        let operation = operation_named(
            "local:1000",
            "01JLEDGERBYTEBOUNDARY01",
            "workspace.create",
            "ws_byte_boundary_a",
            &"a".repeat(64),
        );
        let measure = Store::open_with_config(&measure_path, ledger_config(10, 10))
            .expect("measurement store");
        assert_eq!(
            measure.reserve(&operation).expect("measure reservation"),
            Reservation::Accepted
        );
        let charged: i64 = measure
            .connection
            .lock()
            .query_row(
                "SELECT charged_bytes FROM operations
                 WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
                params![
                    operation.scope.deployment,
                    operation.scope.subject,
                    operation.operation
                ],
                |row| row.get(0),
            )
            .expect("charged bytes");
        drop(measure);

        let boundary_path = directory.path().join("boundary.db");
        let charged = u64::try_from(charged).expect("positive charge");
        let config = StoreConfig {
            operation_subject_max_rows: 10,
            operation_global_max_rows: 10,
            operation_subject_max_bytes: charged,
            operation_global_max_bytes: charged * 2,
            operation_max_row_bytes: charged,
            ..StoreConfig::default()
        };
        let store = Store::open_with_config(&boundary_path, config).expect("boundary store");
        assert_eq!(
            store.reserve(&operation).expect("exact byte boundary"),
            Reservation::Accepted
        );
        let second = operation_named(
            "local:1000",
            "01JLEDGERBYTEBOUNDARY02",
            "workspace.create",
            "ws_byte_boundary_b",
            &"b".repeat(64),
        );
        assert_eq!(
            store.reserve(&second).expect("byte capacity"),
            Reservation::Capacity(OperationCapacity::SubjectBytes)
        );
    }

    #[test]
    fn global_byte_and_max_row_boundaries_are_enforced() {
        let directory = tempdir().expect("tempdir");
        let measure_path = directory.path().join("measure-global.db");
        let first = operation_named(
            "local:1000",
            "01JLEDGERGLOBALBYTES001",
            "workspace.create",
            "ws_global_bytes_a",
            &"d".repeat(64),
        );
        let measure = Store::open_with_config(&measure_path, ledger_config(10, 10))
            .expect("measurement store");
        assert_eq!(
            measure.reserve(&first).expect("measurement reservation"),
            Reservation::Accepted
        );
        let charged = measure
            .connection
            .lock()
            .query_row(
                "SELECT charged_bytes FROM operations
                 WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
                params![first.scope.deployment, first.scope.subject, first.operation],
                |row| row.get::<_, i64>(0),
            )
            .map(|value| u64::try_from(value).expect("positive charge"))
            .expect("charged bytes");
        drop(measure);

        let global_path = directory.path().join("global.db");
        let global_config = StoreConfig {
            operation_subject_max_rows: 10,
            operation_global_max_rows: 10,
            operation_subject_max_bytes: charged,
            operation_global_max_bytes: charged,
            operation_max_row_bytes: charged,
            ..StoreConfig::default()
        };
        let global = Store::open_with_config(&global_path, global_config).expect("global store");
        assert_eq!(
            global.reserve(&first).expect("global exact boundary"),
            Reservation::Accepted
        );
        let other = operation_named(
            "local:1001",
            "01JLEDGERGLOBALBYTES002",
            "workspace.create",
            "ws_global_bytes_b",
            &"e".repeat(64),
        );
        assert_eq!(
            global.reserve(&other).expect("global byte capacity"),
            Reservation::Capacity(OperationCapacity::GlobalBytes)
        );

        let row_path = directory.path().join("row.db");
        let row_config = StoreConfig {
            operation_subject_max_rows: 10,
            operation_global_max_rows: 10,
            operation_subject_max_bytes: charged * 2,
            operation_global_max_bytes: charged * 2,
            operation_max_row_bytes: charged - 1,
            ..StoreConfig::default()
        };
        let row = Store::open_with_config(&row_path, row_config).expect("row store");
        assert_eq!(
            row.reserve(&first).expect("row byte capacity"),
            Reservation::Capacity(OperationCapacity::RowBytes)
        );
    }

    #[test]
    fn terminal_update_cannot_exceed_reserved_headroom_and_rolls_back() {
        let config = StoreConfig {
            operation_subject_max_rows: 10,
            operation_global_max_rows: 10,
            operation_subject_max_bytes: 16 * 1024,
            operation_global_max_bytes: 32 * 1024,
            operation_max_row_bytes: 4 * 1024,
            operation_terminal_headroom_bytes: 32,
            ..StoreConfig::default()
        };
        let store = Store::open_with_config(":memory:", config).expect("open store");
        let accepted = operation_named(
            "local:1000",
            "01JLEDGERHEADROOM000001",
            "workspace.create",
            "ws_headroom",
            &"c".repeat(64),
        );
        assert_eq!(
            store.reserve(&accepted).expect("accept"),
            Reservation::Accepted
        );
        let before: i64 = store
            .connection
            .lock()
            .query_row(
                "SELECT byte_count FROM operation_ledger_usage
                 WHERE deployment = 'dep_test' AND subject = 'local:1000'",
                [],
                |row| row.get(0),
            )
            .expect("usage before terminal");
        let oversized = "x".repeat(1_024);
        assert!(matches!(
            store.complete_success(
                &accepted.scope,
                &accepted.operation,
                "2026-08-13T12:00:01Z",
                200,
                Some("ws_headroom"),
                &oversized,
            ),
            Err(StoreError::OperationTerminalHeadroom(_))
        ));
        assert_eq!(
            store
                .operation(&accepted.scope, &accepted.operation)
                .expect("operation")
                .expect("accepted operation")
                .state,
            OperationState::Accepted
        );
        let after: i64 = store
            .connection
            .lock()
            .query_row(
                "SELECT byte_count FROM operation_ledger_usage
                 WHERE deployment = 'dep_test' AND subject = 'local:1000'",
                [],
                |row| row.get(0),
            )
            .expect("usage after rollback");
        assert_eq!(after, before);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Proves workspace and exec admission in one transaction story.
    fn provisional_workspace_and_exec_membership_commit_with_acceptance() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        let workspace_operation = operation_named(
            "local:1000",
            "01JPROVISIONALWORKSPACE01",
            "workspace.create",
            "ws_provisional",
            &"1".repeat(64),
        );
        let mut provisional_workspace = workspace("ws_provisional");
        provisional_workspace.state = WorkspaceState::Unknown;
        assert_eq!(
            store
                .reserve_workspace_create(
                    &workspace_operation,
                    "arbitrary-root-name",
                    &provisional_workspace,
                    None,
                )
                .expect("reserve workspace"),
            Reservation::Accepted
        );
        let (root_name, durable_workspace) = store
            .workspace(&scope, "ws_provisional")
            .expect("workspace lookup")
            .expect("provisional workspace");
        assert_eq!(root_name, "arbitrary-root-name");
        assert_eq!(durable_workspace.state, WorkspaceState::Unknown);
        assert_eq!(
            store
                .operation(&scope, &workspace_operation.operation)
                .expect("operation lookup")
                .expect("workspace operation")
                .state,
            OperationState::Accepted
        );

        let mut ready_workspace = durable_workspace;
        ready_workspace.state = WorkspaceState::Ready;
        store
            .complete_workspace(
                &scope,
                &workspace_operation.operation,
                "2026-08-13T12:00:01Z",
                201,
                &root_name,
                &ready_workspace,
            )
            .expect("complete workspace create");
        let exec_operation = operation_named(
            "local:1000",
            "01JPROVISIONALEXEC000001",
            "exec.start",
            "ex_provisional",
            &"2".repeat(64),
        );
        let provisional_exec = exec("ex_provisional", "ws_provisional", ExecState::Accepted);
        assert_eq!(
            store
                .reserve_exec_start(&exec_operation, &provisional_exec, None, None)
                .expect("reserve exec"),
            Reservation::Accepted
        );
        assert_eq!(
            store
                .exec(&scope, "ex_provisional")
                .expect("exec lookup")
                .expect("provisional exec"),
            provisional_exec
        );
        assert!(
            store
                .workspace_has_nonterminal_execs(&scope, "ws_provisional")
                .expect("membership")
        );

        store
            .reconcile_after_restart(
                "dep_test",
                "2026-08-13T12:00:02Z".parse().expect("cutoff"),
                "2026-08-13T12:00:02Z".parse().expect("observed"),
                64,
            )
            .expect("restart reconcile");
        assert_eq!(
            store
                .exec(&scope, "ex_provisional")
                .expect("exec lookup")
                .expect("unknown exec")
                .resource
                .state,
            ExecState::Unknown
        );
        assert!(
            store
                .workspace_has_nonterminal_execs(&scope, "ws_provisional")
                .expect("unknown membership blocks cleanup")
        );
        assert!(
            store
                .mark_workspace_destroying(
                    &scope,
                    "ws_provisional",
                    "2026-08-13T12:00:02Z".parse().expect("time"),
                )
                .expect("destroy admission")
                .is_none()
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One transaction/restart scenario keeps its setup adjacent.
    fn restart_makes_pipe_session_nonattachable_without_redispatching_its_exec() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_pipe_restart", &workspace("ws_pipe_restart"))
            .expect("seed workspace");
        let start = operation_named(
            "local:1000",
            "01JPIPESTORERESTART0001",
            "session.start",
            "ses_restart",
            &"b".repeat(64),
        );
        let lease = NewLease {
            ttl_ms: 60_000,
            clock: LeaseClock {
                wall: "2026-08-13T12:00:00Z".parse().expect("time"),
                boot_id: "boot-test".to_owned(),
                boottime_ms: 1_000,
            },
            authorizing_operation: start.operation.clone(),
            actor: "test".to_owned(),
            principal: None,
        };
        let mut running = exec("ex_pipe_restart", "ws_pipe_restart", ExecState::Accepted);
        running.resource.lease = Some(lease.observation());
        let provisional = pipe_session("ses_restart", "ex_pipe_restart", "ws_pipe_restart", &lease);
        assert_eq!(
            store
                .reserve_pipe_session_start(&start, &provisional, &running, &lease, None)
                .expect("reserve session"),
            Reservation::Accepted
        );
        running.resource.state = ExecState::Running;
        let mut ready = provisional;
        ready.state = SessionState::Ready;
        ready.attachment = SessionAttachmentState::Available;
        store
            .complete_pipe_session_start(
                &scope,
                &start.operation,
                "2026-08-13T12:00:01Z",
                202,
                &ready,
                &running,
                &lease,
            )
            .expect("complete session start");
        assert_eq!(
            store
                .claim_pipe_session_attachment(
                    &scope,
                    "ses_restart",
                    "2026-08-13T12:00:02Z".parse().expect("time"),
                )
                .expect("claim attachment"),
            SessionAttachmentClaim::Claimed
        );
        store
            .reconcile_after_restart(
                "dep_test",
                "2026-08-13T12:00:03Z".parse().expect("cutoff"),
                "2026-08-13T12:00:03Z".parse().expect("observed"),
                64,
            )
            .expect("restart reconcile");
        let session = store
            .session(&scope, "ses_restart")
            .expect("session lookup")
            .expect("durable session");
        assert_eq!(session.state, SessionState::Unknown);
        assert_eq!(session.attachment, SessionAttachmentState::Uncertain);
        assert_eq!(
            store
                .exec(&scope, "ex_pipe_restart")
                .expect("exec lookup")
                .expect("durable exec")
                .resource
                .state,
            ExecState::Unknown
        );
        assert!(
            store
                .recovery_execs(
                    "dep_test",
                    "2026-08-13T12:00:03Z".parse().expect("cutoff"),
                    8,
                )
                .expect("recovery candidates")
                .iter()
                .any(|candidate| candidate.stored.resource.id == "ex_pipe_restart")
        );
        assert_eq!(
            store
                .claim_pipe_session_attachment(
                    &scope,
                    "ses_restart",
                    "2026-08-13T12:00:04Z".parse().expect("time"),
                )
                .expect("repeat claim"),
            SessionAttachmentClaim::AlreadyClaimed
        );
    }

    #[test]
    fn restart_reconciliation_is_batched_and_provisional_membership_is_resolvable() {
        let store = Store::open(":memory:").expect("open store");
        let cutoff = "2026-08-13T12:01:00Z".parse().expect("cutoff");
        for index in 0..3 {
            let operation = operation_named(
                "local:1000",
                &format!("01JBOUNDEDRESTART{index:07}"),
                "workspace.file.write",
                "ws_bounded",
                &format!("{index}").repeat(64),
            );
            assert_eq!(
                store.reserve(&operation).expect("reserve"),
                Reservation::Accepted
            );
        }
        assert_eq!(
            store
                .reconcile_after_restart("dep_test", cutoff, cutoff, 4)
                .expect("bounded recovery"),
            2
        );
        let accepted: i64 = store
            .connection
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE state = 'accepted'",
                [],
                |row| row.get(0),
            )
            .expect("accepted count");
        assert_eq!(accepted, 1);

        let workspace_create = operation_named(
            "local:1001",
            "01JRECOVERYWORKSPACE001",
            "workspace.create",
            "ws_recovery_pending",
            &"a".repeat(64),
        );
        let mut provisional = workspace("ws_recovery_pending");
        provisional.state = WorkspaceState::Unknown;
        assert_eq!(
            store
                .reserve_workspace_create(
                    &workspace_create,
                    "root_recovery_pending",
                    &provisional,
                    None,
                )
                .expect("reserve provisional workspace"),
            Reservation::Accepted
        );
        store
            .reconcile_after_restart("dep_test", cutoff, cutoff, 8)
            .expect("mark provisional operation unknown");
        let candidates = store
            .recovery_workspaces("dep_test", cutoff, 4)
            .expect("workspace recovery candidates");
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.resource.id == "ws_recovery_pending")
            .expect("provisional workspace candidate");
        let error = ErrorDetail {
            class: ErrorClass::Refused,
            code: "resource.not-found".to_owned(),
            message: "Workspace root is positively absent.".to_owned(),
            retriable: false,
            address: Some("workspace".to_owned()),
            operation: Some(candidate.operation.clone()),
        };
        store
            .complete_dispatch_absence(
                &candidate.scope,
                &candidate.operation,
                "2026-08-13T12:01:01Z",
                404,
                "workspace",
                &candidate.resource.id,
                &error,
            )
            .expect("resolve absent provisional workspace");
        assert!(
            store
                .workspace(&candidate.scope, &candidate.resource.id)
                .expect("workspace lookup")
                .is_none()
        );
    }

    #[test]
    fn proven_absent_restart_exec_remains_observable_but_stops_blocking_cleanup() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_absent_exec", &workspace("ws_absent_exec"))
            .expect("seed workspace");
        let operation = operation_named(
            "local:1000",
            "01JRECOVERYEXECABSENT001",
            "exec.start",
            "ex_absent_restart",
            &"b".repeat(64),
        );
        let accepted = exec("ex_absent_restart", "ws_absent_exec", ExecState::Accepted);
        assert_eq!(
            store
                .reserve_exec_start(&operation, &accepted, None, None)
                .expect("reserve exec"),
            Reservation::Accepted
        );
        let mut running = accepted.clone();
        running.resource.state = ExecState::Running;
        store
            .complete_exec(
                &scope,
                &operation.operation,
                "2026-08-13T12:00:01Z",
                202,
                &running.resource,
                &[],
                &[],
                false,
                false,
                false,
                running.cgroup.as_deref(),
                running.leader_pid,
            )
            .expect("complete exec start");
        let cutoff = "2026-08-13T12:01:00Z".parse().expect("cutoff");
        store
            .reconcile_after_restart("dep_test", cutoff, cutoff, 4)
            .expect("restart recovery");
        let candidate = store
            .recovery_execs("dep_test", cutoff, 4)
            .expect("exec candidates")
            .into_iter()
            .find(|candidate| candidate.stored.resource.id == "ex_absent_restart")
            .expect("unknown exec candidate");
        assert_eq!(candidate.operation_state, OperationState::Terminal);
        store
            .mark_exec_physically_absent(&candidate, cutoff)
            .expect("persist absence proof");
        assert!(
            !store
                .workspace_has_nonterminal_execs(&scope, "ws_absent_exec")
                .expect("cleanup admission")
        );
        assert_eq!(
            store
                .exec(&scope, "ex_absent_restart")
                .expect("exec lookup")
                .expect("exec remains observable")
                .resource
                .state,
            ExecState::Unknown
        );
        assert!(
            store
                .recovery_execs("dep_test", cutoff, 4)
                .expect("recovery candidates after proof")
                .is_empty()
        );
    }

    #[test]
    fn resource_admission_capacity_is_durable_and_replay_precedes_cap() {
        let config = StoreConfig {
            snapshot_max_workspaces: 1,
            snapshot_max_execs: 1,
            snapshot_max_provenance_events: 1,
            ..StoreConfig::default()
        };
        let store = Store::open_with_config(":memory:", config).expect("open store");
        let scope = scope("local:1000");
        let first = operation_named(
            "local:1000",
            "01JRESOURCECAPWORKSPACE1",
            "workspace.create",
            "ws_capacity_a",
            &"1".repeat(64),
        );
        assert_eq!(
            store
                .reserve_workspace_create(
                    &first,
                    "ws_capacity_a",
                    &workspace("ws_capacity_a"),
                    None,
                )
                .expect("first workspace"),
            Reservation::Accepted
        );
        assert!(matches!(
            store
                .reserve_workspace_create(
                    &first,
                    "ws_capacity_a",
                    &workspace("ws_capacity_a"),
                    None,
                )
                .expect("existing replay before cap"),
            Reservation::Pending(_)
        ));
        let second = operation_named(
            "local:1000",
            "01JRESOURCECAPWORKSPACE2",
            "workspace.create",
            "ws_capacity_b",
            &"2".repeat(64),
        );
        let Reservation::Replay(answer) = store
            .reserve_workspace_create(&second, "ws_capacity_b", &workspace("ws_capacity_b"), None)
            .expect("capacity refusal")
        else {
            panic!("resource capacity must be durably replayable");
        };
        assert_eq!(answer.status, 507);
        let OperationOutcome::Error { error } = answer.outcome else {
            panic!("capacity is an error outcome");
        };
        assert_eq!(error.code, "workspace.capacity");
        assert!(!error.retriable);
        assert!(
            store
                .workspace(&scope, "ws_capacity_b")
                .expect("workspace lookup")
                .is_none()
        );
        assert_eq!(
            store
                .operation(&scope, &second.operation)
                .expect("operation lookup")
                .expect("durable refusal")
                .state,
            OperationState::Refused
        );
        assert!(matches!(
            store
                .reserve_workspace_create(
                    &second,
                    "ws_capacity_b",
                    &workspace("ws_capacity_b"),
                    None,
                )
                .expect("exact refusal replay"),
            Reservation::Replay(_)
        ));
    }

    #[test]
    fn terminal_exec_retirement_is_atomic_idempotent_and_releases_capacity() {
        let config = StoreConfig {
            snapshot_max_workspaces: 1,
            snapshot_max_execs: 1,
            snapshot_max_provenance_events: 1,
            ..StoreConfig::default()
        };
        let store = Store::open_with_config(":memory:", config).expect("open store");
        let scope = scope("local:1000");
        seed_exec(
            &store,
            &scope,
            &exec("ex_retire", "ws_retire", ExecState::Exited),
        );
        let retire = operation_named(
            "local:1000",
            "01JEXECRETIRE000000001",
            "exec.retire",
            "ex_retire",
            &"3".repeat(64),
        );
        let ExecRetireReservation::Retired(absence) = store
            .retire_exec(
                &retire,
                "ex_retire",
                "2026-08-13T12:00:00Z".parse().expect("observed at"),
            )
            .expect("retire terminal exec")
        else {
            panic!("terminal exec must retire");
        };
        assert!(absence.absent);
        assert!(
            store
                .exec(&scope, "ex_retire")
                .expect("exec lookup")
                .is_none()
        );
        assert_eq!(
            store
                .operation(&scope, &retire.operation)
                .expect("operation lookup")
                .expect("retirement operation")
                .state,
            OperationState::Terminal
        );
        assert!(matches!(
            store
                .retire_exec(
                    &retire,
                    "ex_retire",
                    "2026-08-13T12:00:01Z".parse().expect("observed at"),
                )
                .expect("retirement replay"),
            ExecRetireReservation::Existing(Reservation::Replay(_))
        ));
        let events = store
            .events(&scope, None, 10)
            .expect("events")
            .expect("event page");
        assert_eq!(
            events.items.last().expect("retired event").transition,
            "exec.retired"
        );
    }

    #[test]
    fn late_exec_observation_cannot_resurrect_retired_membership() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        let terminal = exec("ex_retire_race", "ws_retire", ExecState::Exited);
        seed_exec(&store, &scope, &terminal);
        let retire = operation_named(
            "local:1000",
            "01JEXECRETIRERACE00001",
            "exec.retire",
            "ex_retire_race",
            &"4".repeat(64),
        );
        let barrier = std::sync::Barrier::new(2);
        let late = terminal.clone();
        let write = std::thread::scope(|threads| {
            let thread = threads.spawn(|| {
                barrier.wait();
                store.put_exec(&scope, &late).expect("late observation")
            });
            assert!(matches!(
                store
                    .retire_exec(
                        &retire,
                        "ex_retire_race",
                        "2026-08-13T12:00:02Z".parse().expect("observed at"),
                    )
                    .expect("retirement"),
                ExecRetireReservation::Retired(_)
            ));
            barrier.wait();
            thread.join().expect("late writer")
        });
        assert_eq!(write, ExecWrite::Retired);
        assert!(
            store
                .exec(&scope, "ex_retire_race")
                .expect("lookup")
                .is_none()
        );
    }

    #[test]
    fn durable_replay_and_conflict_are_subject_scoped() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.db");
        let store = Store::open(&path).expect("open store");
        let first = operation("local:1000", &"1".repeat(64));
        assert_eq!(
            store.reserve(&first).expect("reserve"),
            Reservation::Accepted
        );
        let workspace = workspace("ws_test");
        store
            .complete_workspace(
                &first.scope,
                &first.operation,
                "2026-08-13T12:00:01Z",
                201,
                "ws_test",
                &workspace,
            )
            .expect("complete");
        let position = store
            .stream_position(&first.scope)
            .expect("stream position");
        drop(store);

        let reopened = Store::open(&path).expect("reopen");
        assert_eq!(
            reopened
                .stream_position(&first.scope)
                .expect("persisted stream"),
            position
        );
        let Reservation::Replay(answer) = reopened.reserve(&first).expect("replay") else {
            panic!("expected replay");
        };
        assert_eq!(answer.status, 201);
        assert!(matches!(answer.outcome, OperationOutcome::Success { .. }));
        assert_eq!(
            reopened
                .reserve(&operation("local:1000", &"2".repeat(64)))
                .expect("conflict"),
            Reservation::Conflict
        );
        assert_eq!(
            reopened
                .reserve(&operation("local:1001", &"2".repeat(64)))
                .expect("other subject"),
            Reservation::Accepted
        );
        assert!(
            reopened
                .operation(&scope("local:1001"), "missing")
                .expect("lookup")
                .is_none()
        );
    }

    #[test]
    fn restart_moves_only_accepted_operations_to_unknown() {
        let store = Store::open(":memory:").expect("open store");
        let accepted = operation("local:1000", &"1".repeat(64));
        store.reserve(&accepted).expect("reserve");
        assert_eq!(
            store
                .reconcile_after_restart(
                    "dep_test",
                    "2026-08-13T12:00:02Z".parse().expect("cutoff"),
                    "2026-08-13T12:00:02Z".parse().expect("observed"),
                    64,
                )
                .expect("reconcile"),
            1
        );
        let reconciled = store
            .operation(&accepted.scope, &accepted.operation)
            .expect("lookup")
            .expect("record");
        assert_eq!(reconciled.state, substrate_wire::OperationState::Unknown);
        assert_eq!(reconciled.resource.as_deref(), Some("ws_reserved"));
        let page = store
            .events(&accepted.scope, None, 100)
            .expect("events")
            .expect("cursor");
        assert_eq!(
            page.items.last().expect("restart event").transition,
            "operation.unknown"
        );
    }

    #[test]
    fn journal_retention_generation_and_duplicate_are_explicit() {
        let store = Store::open_with_event_retention(":memory:", 3).expect("open store");
        let scope = scope("local:1000");
        let first = operation_named(
            "local:1000",
            "01JSTOREJOURNALFIRST001",
            "workspace.create",
            "ws_first",
            &"1".repeat(64),
        );
        store.reserve(&first).expect("reserve first");
        store
            .complete_workspace(
                &scope,
                &first.operation,
                "2026-08-13T12:00:01Z",
                201,
                "ws_first",
                &workspace("ws_first"),
            )
            .expect("complete first");
        let (source_scope, generation, sequence) = store.stream_position(&scope).expect("position");
        assert_eq!(sequence, 2);
        assert!(matches!(
            store.reserve(&first).expect("duplicate"),
            Reservation::Replay(_)
        ));
        assert_eq!(
            store.stream_position(&scope).expect("unchanged").2,
            sequence
        );

        let second = operation_named(
            "local:1000",
            "01JSTOREJOURNALSECOND01",
            "workspace.create",
            "ws_second",
            &"2".repeat(64),
        );
        store.reserve(&second).expect("reserve second");
        store
            .complete_workspace(
                &scope,
                &second.operation,
                "2026-08-13T12:00:02Z",
                201,
                "ws_second",
                &workspace("ws_second"),
            )
            .expect("complete second");
        assert_eq!(store.stream_position(&scope).expect("position").2, 4);
        assert_eq!(
            store
                .events(
                    &scope,
                    Some(&event_cursor(&source_scope, generation, 0)),
                    10
                )
                .expect("gap query"),
            Err(EventCursorError::Retention { first: 2, last: 4 })
        );
        let replacement = store
            .reset_stream_generation(&scope)
            .expect("generation reset");
        assert_ne!(replacement, generation);
        assert_eq!(
            store
                .events(
                    &scope,
                    Some(&event_cursor(&source_scope, generation, 4)),
                    10,
                )
                .expect("old generation"),
            Err(EventCursorError::Source)
        );
    }

    #[test]
    fn event_stream_positions_retention_and_cursors_are_subject_local() {
        let store = Store::open_with_event_retention(":memory:", 3).expect("open store");
        let scope_a = scope("local:1000");
        let scope_b = scope("local:1001");
        let operation_a = operation_named(
            "local:1000",
            "01JSTREAMSUBJECTA000001",
            "workspace.create",
            "ws_a",
            &"a".repeat(64),
        );
        store.reserve(&operation_a).expect("reserve A");
        store
            .complete_workspace(
                &scope_a,
                &operation_a.operation,
                "2026-08-13T12:00:01Z",
                201,
                "ws_a",
                &workspace("ws_a"),
            )
            .expect("complete A");
        let position_a = store.stream_position(&scope_a).expect("A position");
        assert_eq!(position_a.2, 2);

        for index in 0..4 {
            let operation_b = operation_named(
                "local:1001",
                &format!("01JSTREAMSUBJECTB{index:06}"),
                "workspace.create",
                &format!("ws_b_{index}"),
                &format!("{index:x}").repeat(64),
            );
            store.reserve(&operation_b).expect("reserve B");
            store
                .complete_workspace(
                    &scope_b,
                    &operation_b.operation,
                    "2026-08-13T12:00:02Z",
                    201,
                    &format!("ws_b_{index}"),
                    &workspace(&format!("ws_b_{index}")),
                )
                .expect("complete B");
        }

        assert_eq!(
            store.stream_position(&scope_a).expect("unchanged A"),
            position_a
        );
        let page_a = store
            .events(&scope_a, None, 10)
            .expect("A events")
            .expect("A cursor");
        assert_eq!(page_a.items.len(), 2);
        assert_eq!(page_a.through_seq, 2);
        assert_eq!(page_a.first_retained_seq, Some(1));
        assert!(page_a.items.iter().all(|event| {
            event.source_scope == position_a.0 && event.generation == position_a.1
        }));

        let position_b = store.stream_position(&scope_b).expect("B position");
        assert_ne!(position_a.0, position_b.0);
        assert_eq!(
            store
                .events(
                    &scope_a,
                    Some(&event_cursor(&position_b.0, position_b.1, position_b.2)),
                    10,
                )
                .expect("cross-subject cursor"),
            Err(EventCursorError::Source)
        );
    }

    #[test]
    fn deployment_global_stream_schema_migrates_to_fresh_subject_scopes() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("legacy.db");
        let legacy = rusqlite::Connection::open(&path).expect("legacy database");
        legacy
            .execute_batch(
                "
                CREATE TABLE stream_meta (
                    deployment TEXT NOT NULL PRIMARY KEY,
                    generation INTEGER NOT NULL,
                    next_seq INTEGER NOT NULL
                ) WITHOUT ROWID;
                CREATE TABLE events (
                    deployment TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    seq INTEGER NOT NULL,
                    event_json TEXT NOT NULL,
                    PRIMARY KEY (deployment, seq)
                ) WITHOUT ROWID;
                INSERT INTO stream_meta VALUES ('dep_test', 7, 2);
                INSERT INTO events VALUES ('dep_test', 'local:1000', 7, 1, '{}');
                ",
            )
            .expect("legacy schema");
        drop(legacy);

        let store = Store::open(&path).expect("migrated store");
        let a = store
            .stream_position(&scope("local:1000"))
            .expect("A stream");
        let b = store
            .stream_position(&scope("local:1001"))
            .expect("B stream");
        assert_eq!(a.2, 0);
        assert_eq!(b.2, 0);
        assert_ne!(a.0, b.0);
        assert!(
            store
                .events(&scope("local:1000"), None, 10)
                .expect("A events")
                .expect("A cursor")
                .items
                .is_empty()
        );
    }

    #[test]
    fn stream_scope_migration_rotates_every_invalid_suffix_but_preserves_valid_tokens() {
        let connection = rusqlite::Connection::open_in_memory().expect("database");
        connection
            .execute_batch(
                "
                CREATE TABLE stream_meta (
                    deployment TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    source_scope TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    next_seq INTEGER NOT NULL,
                    PRIMARY KEY (deployment, subject),
                    UNIQUE (deployment, source_scope)
                ) WITHOUT ROWID;
                CREATE TABLE events (
                    deployment TEXT NOT NULL, subject TEXT NOT NULL,
                    generation INTEGER NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL
                );
                CREATE TABLE snapshots (
                    deployment TEXT NOT NULL, subject TEXT NOT NULL, id TEXT NOT NULL
                );
                INSERT INTO stream_meta VALUES ('valid', 's', 'scope_Az-09_ok', 1, 9);
                INSERT INTO stream_meta VALUES ('dot', 's', 'scope_a.bad', 1, 9);
                INSERT INTO stream_meta VALUES ('slash', 's', 'scope_a/bad', 1, 9);
                INSERT INTO stream_meta VALUES ('space', 's', 'scope_a bad', 1, 9);
                INSERT INTO stream_meta VALUES ('unicode', 's', 'scope_aä', 1, 9);
                ",
            )
            .expect("legacy fixtures");
        super::migrate_stream_scope_grammar(&connection).expect("migrate scope grammar");
        let valid: (String, i64, i64) = connection
            .query_row(
                "SELECT source_scope, generation, next_seq FROM stream_meta WHERE deployment = 'valid'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("valid row");
        assert_eq!(valid, ("scope_Az-09_ok".to_owned(), 1, 9));
        let mut statement = connection
            .prepare(
                "SELECT source_scope, generation, next_seq FROM stream_meta
                 WHERE deployment != 'valid' ORDER BY deployment",
            )
            .expect("invalid rows");
        for row in statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .expect("query invalid rows")
        {
            let (source_scope, generation, next_seq) = row.expect("invalid row");
            assert!(source_scope.starts_with("scope_"));
            assert!(
                source_scope[6..]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            );
            assert_eq!(generation, 2);
            assert_eq!(next_seq, 1);
        }
    }

    #[test]
    fn legacy_snapshot_source_scope_is_backfilled_from_its_subject_stream() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("legacy-snapshot.db");
        let legacy = rusqlite::Connection::open(&path).expect("legacy database");
        legacy
            .execute_batch(
                "
                CREATE TABLE stream_meta (
                    deployment TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    source_scope TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    next_seq INTEGER NOT NULL,
                    PRIMARY KEY (deployment, subject),
                    UNIQUE (deployment, source_scope)
                ) WITHOUT ROWID;
                CREATE TABLE snapshots (
                    deployment TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    id TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    through_seq INTEGER NOT NULL,
                    item_count INTEGER NOT NULL,
                    expires_at TEXT NOT NULL,
                    PRIMARY KEY (deployment, subject, id)
                ) WITHOUT ROWID;
                INSERT INTO stream_meta VALUES ('dep_test', 'local:1000', 'source-legacy', 3, 8);
                INSERT INTO snapshots VALUES (
                    'dep_test', 'local:1000', 'snap_legacy', 3, 7, 0,
                    '2026-08-13T13:00:00Z'
                );
                ",
            )
            .expect("legacy schema");
        drop(legacy);

        let store = Store::open(&path).expect("migrated store");
        assert_eq!(
            store
                .snapshot_page(
                    &scope("local:1000"),
                    "snap_legacy",
                    None,
                    1,
                    "2026-08-13T12:00:00Z".parse().expect("read time"),
                )
                .expect("snapshot read"),
            Err(SnapshotReadError::NotFound)
        );
        assert!(
            store
                .stream_position(&scope("local:1000"))
                .expect("rotated stream")
                .0
                .starts_with("scope_")
        );
    }

    #[test]
    fn snapshot_is_materialized_stable_and_detects_incomplete_rows() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_before", &workspace("ws_before"))
            .expect("seed workspace");
        let operation = operation_named(
            "local:1000",
            "01JSTORESNAPSHOTCREATE01",
            "reconciliation.snapshot.create",
            "snap_store",
            &"3".repeat(64),
        );
        store.reserve(&operation).expect("reserve snapshot");
        let metadata = store
            .complete_snapshot(
                &scope,
                "test",
                None,
                "2026-08-13T12:00:03Z".parse().expect("observed at"),
                "snap_store",
                "2026-08-13T12:05:00Z".parse().expect("expiry"),
            )
            .expect("materialize snapshot");
        store
            .connection
            .lock()
            .execute(
                "UPDATE stream_meta SET source_scope = 'source-rotated-after-snapshot'
                 WHERE deployment = ?1 AND subject = ?2",
                params![scope.deployment, scope.subject],
            )
            .expect("rotate current stream source after snapshot");
        let first = store
            .snapshot_page(
                &scope,
                "snap_store",
                None,
                1,
                "2026-08-13T12:01:00Z".parse().expect("now"),
            )
            .expect("snapshot read")
            .expect("snapshot available");
        assert_eq!(first.generation, metadata.generation);
        assert_eq!(first.through_seq, metadata.through_seq);
        store
            .put_workspace(&scope, "ws_after", &workspace("ws_after"))
            .expect("concurrent mutation");
        let mut ids = first
            .items
            .iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        let mut cursor = first.next_cursor;
        while let Some(value) = cursor {
            let page = store
                .snapshot_page(
                    &scope,
                    "snap_store",
                    Some(&value),
                    1,
                    "2026-08-13T12:01:00Z".parse().expect("now"),
                )
                .expect("page read")
                .expect("page available");
            ids.extend(page.items.iter().map(|item| item.id.clone()));
            cursor = page.next_cursor;
        }
        assert_eq!(ids.len() as u64, metadata.item_count);
        assert!(ids.iter().any(|id| id == "workspace:ws_before"));
        assert!(!ids.iter().any(|id| id == "workspace:ws_after"));

        store
            .connection
            .lock()
            .execute(
                "DELETE FROM snapshot_items WHERE snapshot_id = 'snap_store' AND ordinal = 1",
                [],
            )
            .expect("corrupt materialization");
        assert_eq!(
            store
                .snapshot_page(
                    &scope,
                    "snap_store",
                    None,
                    1,
                    "2026-08-13T12:01:00Z".parse().expect("now"),
                )
                .expect("incomplete read"),
            Err(SnapshotReadError::Incomplete)
        );
    }

    #[test]
    fn empty_snapshot_is_non_keyed_and_uses_a_control_barrier() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        let observed_at = "2026-08-13T12:00:00Z".parse().expect("observed at");
        let metadata = store
            .complete_snapshot(
                &scope,
                "test-actor",
                Some("test-principal"),
                observed_at,
                "snap_empty",
                "2026-08-13T13:00:00Z".parse().expect("expiry"),
            )
            .expect("empty snapshot");
        assert_eq!(metadata.item_count, 0);
        assert_eq!(metadata.partitions.workspaces, 0);
        assert_eq!(metadata.partitions.execs, 0);
        assert_eq!(metadata.partitions.provenance_events, 0);
        assert_eq!(metadata.history.first_seq, None);
        assert_eq!(metadata.history.through_seq, 0);
        assert_eq!(metadata.history.item_count, 0);
        assert!(!metadata.history.truncated);
        assert_eq!(metadata.through_seq, 1);
        assert!(metadata.source_scope.starts_with("scope_"));
        let connection = store.connection.lock();
        let operation_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
            .expect("operation count");
        assert_eq!(operation_count, 0);
        drop(connection);
        let events = store
            .events(&scope, None, 10)
            .expect("events")
            .expect("event page");
        assert_eq!(events.items.len(), 1);
        let created = &events.items[0];
        assert_eq!(created.seq, metadata.through_seq);
        assert_eq!(created.transition, "snapshot.created");
        assert_eq!(
            created.cause,
            EventCause::Control {
                control: substrate_wire::EventControl::ReconciliationSnapshotCreate
            }
        );
        assert_eq!(
            created.observation,
            serde_json::to_value(&metadata).unwrap()
        );
        let page = store
            .snapshot_page(&scope, "snap_empty", None, 1, observed_at)
            .expect("page")
            .expect("snapshot exists");
        assert!(page.items.is_empty());
        assert!(page.complete);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn snapshot_partitions_history_and_cursors_are_exact() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_partition", &workspace("ws_partition"))
            .expect("seed workspace");
        seed_exec(
            &store,
            &scope,
            &exec("ex_partition", "ws_partition", ExecState::Exited),
        );
        let cause = operation_named(
            "local:1000",
            "01JSNAPSHOTPROVENANCE001",
            "workspace.file.write",
            "ws_partition",
            &"e".repeat(64),
        );
        store.reserve(&cause).expect("accepted event");
        store
            .complete_success(
                &scope,
                &cause.operation,
                "2026-08-13T12:00:01Z",
                200,
                Some("ws_partition"),
                &serde_json::json!({"written": true}),
            )
            .expect("terminal event");
        let metadata = store
            .complete_snapshot(
                &scope,
                "test",
                None,
                "2026-08-13T12:00:02Z".parse().expect("observed at"),
                "snap_partitioned",
                "2026-08-13T13:00:00Z".parse().expect("expiry"),
            )
            .expect("snapshot");
        assert_eq!(metadata.partitions.workspaces, 1);
        assert_eq!(metadata.partitions.execs, 1);
        assert_eq!(metadata.partitions.provenance_events, 2);
        assert_eq!(metadata.item_count, 4);
        assert_eq!(metadata.history.first_seq, Some(1));
        assert_eq!(metadata.history.through_seq, 2);
        assert_eq!(metadata.through_seq, 3);
        assert!(!metadata.history.truncated);

        let first = store
            .snapshot_page(
                &scope,
                "snap_partitioned",
                None,
                2,
                "2026-08-13T12:00:03Z".parse().expect("now"),
            )
            .expect("first page")
            .expect("snapshot");
        assert_eq!(first.items.len(), 2);
        assert!(!first.complete);
        assert_eq!(first.next_cursor.as_deref(), Some("sp2.snap_partitioned.2"));
        let second = store
            .snapshot_page(
                &scope,
                "snap_partitioned",
                first.next_cursor.as_deref(),
                2,
                "2026-08-13T12:00:03Z".parse().expect("now"),
            )
            .expect("second page")
            .expect("snapshot");
        assert_eq!(second.items.len(), 2);
        assert!(second.complete);
        assert!(second.next_cursor.is_none());
        assert_eq!(
            second.items.last().expect("last item").kind,
            SnapshotItemKind::ProvenanceEvent
        );
        for invalid in [
            "sp2.snap_partitioned.0",
            "sp2.snap_partitioned.4",
            "sp2.snap_partitioned.5",
            "sp2.other.2",
            "sp_snap_partitioned_2",
        ] {
            assert_eq!(
                store
                    .snapshot_page(
                        &scope,
                        "snap_partitioned",
                        Some(invalid),
                        2,
                        "2026-08-13T12:00:03Z".parse().expect("now"),
                    )
                    .expect("invalid cursor read"),
                Err(SnapshotReadError::InvalidCursor)
            );
        }
    }

    #[test]
    fn snapshot_barrier_at_full_retention_keeps_bootstrap_available() {
        let store = Store::open_with_config(
            ":memory:",
            StoreConfig {
                event_retention: 2,
                snapshot_max_provenance_events: 2,
                ..StoreConfig::default()
            },
        )
        .expect("open store");
        let scope = scope("local:1000");
        let cause = operation_named(
            "local:1000",
            "01JSNAPSHOTFULLRETENTION1",
            "workspace.file.write",
            "ws_history",
            &"f".repeat(64),
        );
        store.reserve(&cause).expect("accepted event");
        store
            .complete_success(
                &scope,
                &cause.operation,
                "2026-08-13T12:00:01Z",
                200,
                Some("ws_history"),
                &serde_json::json!({"written": true}),
            )
            .expect("terminal event fills retention");

        let metadata = store
            .complete_snapshot(
                &scope,
                "test",
                None,
                "2026-08-13T12:00:02Z".parse().expect("observed at"),
                "snap_full_retention",
                "2026-08-13T13:00:00Z".parse().expect("expiry"),
            )
            .expect("barrier must not make bootstrap unavailable");

        assert_eq!(metadata.through_seq, 3);
        assert_eq!(metadata.partitions.provenance_events, 1);
        assert_eq!(metadata.history.first_seq, Some(2));
        assert_eq!(metadata.history.through_seq, 2);
        assert_eq!(metadata.history.item_count, 1);
        assert!(metadata.history.truncated);
        let page = store
            .snapshot_page(
                &scope,
                "snap_full_retention",
                None,
                2,
                "2026-08-13T12:00:03Z".parse().expect("now"),
            )
            .expect("page")
            .expect("snapshot exists");
        assert!(page.complete);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, format!("event:{}:2", metadata.generation));
    }

    #[test]
    fn snapshot_materialization_limit_commits_only_control_refusal() {
        let config = StoreConfig {
            snapshot_max_workspaces: 1,
            snapshot_max_execs: 1,
            snapshot_max_provenance_events: 1,
            ..StoreConfig::default()
        };
        let store = Store::open_with_config(":memory:", config).expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_over_a", &workspace("ws_over_a"))
            .expect("seed A");
        store
            .put_workspace(&scope, "ws_over_b", &workspace("ws_over_b"))
            .expect("seed B bypassing admission for corruption posture");
        assert!(matches!(
            store.complete_snapshot(
                &scope,
                "test",
                None,
                "2026-08-13T12:00:00Z".parse().expect("observed at"),
                "snap_over",
                "2026-08-13T13:00:00Z".parse().expect("expiry"),
            ),
            Err(StoreError::SnapshotLimit)
        ));
        let connection = store.connection.lock();
        let snapshots: i64 = connection
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .expect("snapshot count");
        let items: i64 = connection
            .query_row("SELECT COUNT(*) FROM snapshot_items", [], |row| row.get(0))
            .expect("item count");
        let operations: i64 = connection
            .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
            .expect("operation count");
        assert_eq!((snapshots, items, operations), (0, 0, 0));
        drop(connection);
        let events = store
            .events(&scope, None, 10)
            .expect("events")
            .expect("event page");
        assert_eq!(events.items.len(), 1);
        assert_eq!(events.items[0].transition, "snapshot.refused");
        assert_eq!(
            events.items[0].observation["code"],
            "snapshot.materialization-limit"
        );
        assert!(matches!(events.items[0].cause, EventCause::Control { .. }));
    }

    #[test]
    fn snapshot_gc_bounds_materialized_rows_and_preserves_expired_posture() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_gc", &workspace("ws_gc"))
            .expect("seed workspace");
        let operation = operation_named(
            "local:1000",
            "01JSTORESNAPSHOTGC00001",
            "reconciliation.snapshot.create",
            "snap_gc",
            &"6".repeat(64),
        );
        store.reserve(&operation).expect("reserve snapshot");
        store
            .complete_snapshot(
                &scope,
                "test",
                None,
                "2026-08-13T12:00:00Z".parse().expect("observed at"),
                "snap_gc",
                "2026-08-13T12:01:00Z".parse().expect("expiry"),
            )
            .expect("materialize snapshot");
        assert_eq!(
            store
                .prune_expired_snapshots(
                    "dep_test",
                    "2026-08-13T12:02:00Z".parse().expect("prune time"),
                )
                .expect("prune"),
            1
        );
        let connection = store.connection.lock();
        let snapshots: i64 = connection
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .expect("snapshot count");
        let items: i64 = connection
            .query_row("SELECT COUNT(*) FROM snapshot_items", [], |row| row.get(0))
            .expect("item count");
        assert_eq!((snapshots, items), (0, 0));
        drop(connection);
        assert_eq!(
            store
                .snapshot_page(
                    &scope,
                    "snap_gc",
                    None,
                    1,
                    "2026-08-13T12:02:00Z".parse().expect("read time"),
                )
                .expect("expired lookup"),
            Err(SnapshotReadError::Expired)
        );
        assert_eq!(
            store
                .snapshot_page(
                    &scope,
                    "snap_never",
                    None,
                    1,
                    "2026-08-13T12:02:00Z".parse().expect("read time"),
                )
                .expect("missing lookup"),
            Err(SnapshotReadError::NotFound)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Multiple reopen boundaries prove cursor continuity.
    fn snapshot_prune_cursor_continues_across_multiple_batches_and_reopen() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.db");
        let config = StoreConfig {
            snapshot_prune_batch_size: 2,
            ..StoreConfig::default()
        };
        let store = Store::open_with_config(&path, config).expect("open store");
        {
            let connection = store.connection.lock();
            for index in 0..6_u32 {
                connection
                    .execute(
                        "INSERT INTO snapshots (
                            deployment, subject, id, source_scope, generation, through_seq,
                            item_count, expires_at
                         ) VALUES (?1, ?2, ?3, ?4, 1, 0, 0, ?5)",
                        params![
                            "dep_test",
                            format!("local:{}", 1_000 + index),
                            format!("snap_prune_{index}"),
                            format!("scope_prune_{index}"),
                            if index < 2 {
                                "2026-08-13T14:00:00+00:00"
                            } else {
                                "2026-08-13T12:00:00+00:00"
                            },
                        ],
                    )
                    .expect("seed snapshot");
            }
        }
        let now = chrono::Utc.with_ymd_and_hms(2026, 8, 13, 13, 0, 0).unwrap();
        assert_eq!(
            store
                .prune_expired_snapshots("dep_test", now)
                .expect("first prune batch"),
            0
        );
        {
            let connection = store.connection.lock();
            let cursor: (String, String) = connection
                .query_row(
                    "SELECT subject, resource_id FROM maintenance_cursors
                     WHERE deployment = 'dep_test' AND queue = 'snapshot-prune'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("first durable cursor");
            assert_eq!(cursor, ("local:1001".to_owned(), "snap_prune_1".to_owned()));
        }
        drop(store);

        let reopened = Store::open_with_config(&path, config).expect("reopen for second batch");
        {
            let connection = reopened.connection.lock();
            let persisted_cursor: (String, String) = connection
                .query_row(
                    "SELECT subject, resource_id FROM maintenance_cursors
                     WHERE deployment = 'dep_test' AND queue = 'snapshot-prune'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("cursor survives first reopen");
            assert_eq!(
                persisted_cursor,
                ("local:1001".to_owned(), "snap_prune_1".to_owned())
            );
        }
        assert_eq!(
            reopened
                .prune_expired_snapshots("dep_test", now)
                .expect("second prune batch"),
            2
        );
        {
            let connection = reopened.connection.lock();
            let cursor: (String, String) = connection
                .query_row(
                    "SELECT subject, resource_id FROM maintenance_cursors
                     WHERE deployment = 'dep_test' AND queue = 'snapshot-prune'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("second durable cursor");
            assert_eq!(cursor, ("local:1003".to_owned(), "snap_prune_3".to_owned()));
        }
        drop(reopened);

        let reopened = Store::open_with_config(&path, config).expect("reopen for final batch");
        {
            let connection = reopened.connection.lock();
            let persisted_cursor: (String, String) = connection
                .query_row(
                    "SELECT subject, resource_id FROM maintenance_cursors
                     WHERE deployment = 'dep_test' AND queue = 'snapshot-prune'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("cursor survives second reopen");
            assert_eq!(
                persisted_cursor,
                ("local:1003".to_owned(), "snap_prune_3".to_owned())
            );
        }
        assert_eq!(
            reopened
                .prune_expired_snapshots("dep_test", now)
                .expect("final prune batch"),
            2
        );
        let connection = reopened.connection.lock();
        let snapshots: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM snapshots WHERE deployment = 'dep_test'",
                [],
                |row| row.get(0),
            )
            .expect("remaining snapshots");
        let cursor: (String, String) = connection
            .query_row(
                "SELECT subject, resource_id FROM maintenance_cursors
                 WHERE deployment = 'dep_test' AND queue = 'snapshot-prune'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("final durable cursor");
        assert_eq!(snapshots, 2);
        assert_eq!(cursor, ("local:1005".to_owned(), "snap_prune_5".to_owned()));
    }

    #[test]
    fn snapshot_active_cap_terminalizes_and_replays_the_exhaustion() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        for index in 0..64_u32 {
            let operation_id = format!("01JSNAPSHOTCAP{index:010}");
            let snapshot_id = format!("snap_cap_{index:03}");
            let operation = operation_named(
                "local:1000",
                &operation_id,
                "reconciliation.snapshot.create",
                &snapshot_id,
                &format!("{index:064x}"),
            );
            store.reserve(&operation).expect("reserve snapshot");
            store
                .complete_snapshot(
                    &scope,
                    "test",
                    None,
                    "2026-08-13T12:00:00Z".parse().expect("observed at"),
                    &snapshot_id,
                    "2026-08-13T13:00:00Z".parse().expect("expiry"),
                )
                .expect("snapshot below active cap");
        }

        let limited = operation_named(
            "local:1000",
            "01JSNAPSHOTCAP0000000064",
            "reconciliation.snapshot.create",
            "snap_cap_064",
            &"a".repeat(64),
        );
        assert_eq!(
            store.reserve(&limited).expect("reserve cap"),
            Reservation::Accepted
        );
        assert!(matches!(
            store.complete_snapshot(
                &scope,
                "test",
                None,
                "2026-08-13T12:00:00Z".parse().expect("observed at"),
                "snap_cap_064",
                "2026-08-13T13:00:00Z".parse().expect("expiry"),
            ),
            Err(StoreError::SnapshotLimit)
        ));
        let record = store
            .operation(&scope, &limited.operation)
            .expect("lookup")
            .expect("limited operation");
        assert_eq!(record.state, OperationState::Accepted);
        assert!(matches!(
            store.reserve(&limited).expect("stable replay"),
            Reservation::Pending(_)
        ));
        let mut changed = limited;
        changed.request_hash = "b".repeat(64);
        assert_eq!(
            store.reserve(&changed).expect("changed input"),
            Reservation::Conflict
        );
    }

    #[test]
    fn snapshot_item_cap_terminalizes_without_partial_materialization() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        for index in 0..4_096_u32 {
            let id = format!("ws_item_{index:04}");
            store
                .put_workspace(&scope, &id, &workspace(&id))
                .expect("seed item");
        }
        let operation = operation_named(
            "local:1000",
            "01JSNAPSHOTITEMCAP00001",
            "reconciliation.snapshot.create",
            "snap_item_cap",
            &"c".repeat(64),
        );
        store.reserve(&operation).expect("reserve snapshot");
        assert!(matches!(
            store.complete_snapshot(
                &scope,
                "test",
                None,
                "2026-08-13T12:00:00Z".parse().expect("observed at"),
                "snap_item_cap",
                "2026-08-13T13:00:00Z".parse().expect("expiry"),
            ),
            Err(StoreError::SnapshotLimit)
        ));
        let connection = store.connection.lock();
        let materialized: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM snapshot_items WHERE snapshot_id = 'snap_item_cap'",
                [],
                |row| row.get(0),
            )
            .expect("materialized count");
        assert_eq!(materialized, 0);
        drop(connection);
        assert_eq!(
            store
                .operation(&scope, &operation.operation)
                .expect("lookup")
                .expect("operation")
                .state,
            OperationState::Accepted
        );
    }

    #[test]
    fn expired_snapshot_markers_are_bounded_per_scope() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        {
            let connection = store.connection.lock();
            for index in 0..1_025_u32 {
                connection
                    .execute(
                        "INSERT INTO expired_snapshots (deployment, subject, id, expired_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![
                            scope.deployment,
                            scope.subject,
                            format!("snap_marker_{index:04}"),
                            format!("2026-08-13T12:{:02}:{:02}Z", (index / 60) % 60, index % 60),
                        ],
                    )
                    .expect("seed marker");
            }
        }
        store
            .prune_expired_snapshots("dep_test", "2026-08-13T13:00:00Z".parse().expect("now"))
            .expect("prune markers");
        let connection = store.connection.lock();
        let retained: i64 = connection
            .query_row("SELECT COUNT(*) FROM expired_snapshots", [], |row| {
                row.get(0)
            })
            .expect("marker count");
        assert_eq!(retained, 1_024);
    }

    #[test]
    fn exec_observation_preserves_lease_and_snapshot_projects_it() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        let mut leased = exec("ex_leased", "ws_lease", ExecState::Running);
        leased.resource.lease = Some(LeaseObservation {
            ttl_ms: 60_000,
            renew_by: "2026-08-13T12:01:00Z".parse().expect("renew by"),
            state: LeaseState::Active,
            clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
            authorizing_operation: "lease-authority-ex_leased".to_owned(),
            actor: "test".to_owned(),
            principal: None,
        });
        seed_exec(&store, &scope, &leased);
        let without_lease = exec("ex_leased", "ws_lease", ExecState::Exited);
        store
            .put_exec(&scope, &without_lease)
            .expect("persist terminal observation");
        assert!(
            store
                .exec(&scope, "ex_leased")
                .expect("lookup")
                .expect("exec")
                .resource
                .lease
                .is_some()
        );

        let operation = operation_named(
            "local:1000",
            "01JSNAPSHOTLEASE0000001",
            "reconciliation.snapshot.create",
            "snap_lease",
            &"d".repeat(64),
        );
        store.reserve(&operation).expect("reserve snapshot");
        store
            .complete_snapshot(
                &scope,
                "test",
                None,
                "2026-08-13T12:00:00Z".parse().expect("observed at"),
                "snap_lease",
                "2026-08-13T13:00:00Z".parse().expect("expiry"),
            )
            .expect("snapshot");
        let page = store
            .snapshot_page(
                &scope,
                "snap_lease",
                None,
                100,
                "2026-08-13T12:01:00Z".parse().expect("now"),
            )
            .expect("snapshot read")
            .expect("snapshot available");
        let projected = page
            .items
            .iter()
            .find(|item| item.kind == SnapshotItemKind::Exec && item.id == "exec:ex_leased")
            .expect("projected exec");
        assert_eq!(projected.value["lease"]["state"], "active");
    }

    #[test]
    fn workspace_observation_merge_never_regresses_store_owned_lifecycle() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        for state in [
            WorkspaceState::Unknown,
            WorkspaceState::Destroying,
            WorkspaceState::Expired,
        ] {
            let id = format!("ws_{state:?}").to_ascii_lowercase();
            let mut durable = workspace(&id);
            durable.state = state;
            durable
                .labels
                .insert("authority".to_owned(), "store".to_owned());
            store
                .put_workspace(&scope, &id, &durable)
                .expect("seed durable workspace");
            let mut observed = workspace(&id);
            observed.observed_at = "2026-08-13T12:01:00Z".parse().expect("time");
            observed
                .labels
                .insert("authority".to_owned(), "driver".to_owned());
            let WorkspaceObservationWrite::Authoritative(authoritative) = store
                .merge_workspace_observation(&scope, &id, &observed)
                .expect("merge observation")
            else {
                panic!("durable workspace must remain authoritative");
            };
            assert_eq!(authoritative.state, state);
            assert_eq!(authoritative.labels["authority"], "store");
            assert_eq!(authoritative.observed_at, durable.observed_at);
        }

        let mut frozen = workspace("ws_expiring");
        frozen.lease = Some(LeaseObservation {
            ttl_ms: 1_000,
            renew_by: "2026-08-13T12:00:01Z".parse().expect("renew by"),
            state: LeaseState::Expiring,
            clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
            authorizing_operation: "lease-authority-ws_expiring".to_owned(),
            actor: "test".to_owned(),
            principal: None,
        });
        store
            .put_workspace(&scope, "ws_expiring", &frozen)
            .expect("seed frozen workspace");
        let observed = workspace("ws_expiring");
        let WorkspaceObservationWrite::Authoritative(authoritative) = store
            .merge_workspace_observation(&scope, "ws_expiring", &observed)
            .expect("merge frozen observation")
        else {
            panic!("frozen workspace must remain authoritative");
        };
        assert_eq!(authoritative.lease, frozen.lease);
    }

    #[test]
    fn due_workspace_admission_freezes_once_and_keeps_real_authorizing_operation() {
        let store = Store::open(":memory:").expect("open store");
        let operation = seed_leased_workspace(&store, "local:1000", "ws_due", 1_000);
        let due = LeaseClock {
            wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 1).unwrap(),
            boot_id: "boot-test".to_owned(),
            boottime_ms: 2_000,
        };

        let WorkspaceAdmission::Frozen {
            resource,
            newly_frozen,
        } = store
            .admit_workspace(&operation.scope, "ws_due", Some(&due))
            .expect("first admission")
        else {
            panic!("due workspace must be frozen");
        };
        assert!(newly_frozen);
        assert_eq!(
            resource.lease.as_ref().expect("lease").state,
            LeaseState::Expiring
        );
        let WorkspaceAdmission::Frozen { newly_frozen, .. } = store
            .admit_workspace(&operation.scope, "ws_due", Some(&due))
            .expect("second admission")
        else {
            panic!("frozen workspace must remain frozen");
        };
        assert!(!newly_frozen);
        let page = store
            .events(&operation.scope, None, 100)
            .expect("events")
            .expect("event page");
        let expiring = page
            .items
            .iter()
            .filter(|event| event.transition == "workspace.lease-expiring")
            .collect::<Vec<_>>();
        assert_eq!(expiring.len(), 1);
        assert_eq!(
            expiring[0].cause,
            EventCause::Operation {
                operation: operation.operation.clone()
            }
        );
        assert_eq!(expiring[0].actor, LEASE_SWEEPER_ACTOR);
    }

    #[test]
    fn due_workspace_rejects_exec_start_without_partial_acceptance() {
        let store = Store::open(":memory:").expect("open store");
        let authority = seed_leased_workspace(&store, "local:1000", "ws_due_exec", 1_000);
        let start = operation_named(
            "local:1000",
            "01JEXECSTARTAFTERDUE001",
            "exec.start",
            "ex_after_due",
            &"c".repeat(64),
        );
        let due = LeaseClock {
            wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 1).unwrap(),
            boot_id: "boot-test".to_owned(),
            boottime_ms: 2_000,
        };
        assert!(matches!(
            store.reserve_exec_start(
                &start,
                &exec("ex_after_due", "ws_due_exec", ExecState::Accepted),
                None,
                Some(&due),
            ),
            Err(StoreError::WorkspaceFrozen)
        ));
        assert!(
            store
                .operation(&start.scope, &start.operation)
                .expect("operation lookup")
                .is_none()
        );
        assert!(
            store
                .exec(&start.scope, "ex_after_due")
                .expect("exec lookup")
                .is_none()
        );
        let page = store
            .events(&authority.scope, None, 100)
            .expect("events")
            .expect("event page");
        assert_eq!(
            page.items
                .iter()
                .filter(|event| event.transition == "workspace.lease-expiring")
                .count(),
            1
        );
    }

    #[test]
    fn workspace_destroy_reservation_and_retry_schedule_are_atomic_and_durable() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.db");
        let store = Store::open(&path).expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_destroy_retry", &workspace("ws_destroy_retry"))
            .expect("seed workspace");
        let destroy = operation_named(
            "local:1000",
            "01JDESTROYRETRYSCHEDULE1",
            "workspace.destroy",
            "ws_destroy_retry",
            &"d".repeat(64),
        );
        let WorkspaceDestroyReservation::Admitted { resource, .. } = store
            .reserve_workspace_destroy(&destroy, None)
            .expect("reserve destroy")
        else {
            panic!("destroy must be admitted");
        };
        assert_eq!(resource.state, WorkspaceState::Destroying);
        assert_eq!(
            store
                .operation(&scope, &destroy.operation)
                .expect("operation")
                .expect("reserved operation")
                .state,
            OperationState::Accepted
        );
        let now = chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap();
        let pending = store
            .due_destroying_workspaces("dep_test", now, 10)
            .expect("due destroy");
        assert_eq!(pending.len(), 1);
        let next = store
            .record_workspace_cleanup_failure(&pending[0], now, "driver.busy")
            .expect("schedule retry");
        assert_eq!(next, now + chrono::Duration::milliseconds(250));
        assert!(
            store
                .due_destroying_workspaces("dep_test", now, 10)
                .expect("backoff window")
                .is_empty()
        );
        drop(store);

        let reopened = Store::open(&path).expect("reopen store");
        let due = reopened
            .due_destroying_workspaces("dep_test", next, 10)
            .expect("persisted retry");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].attempt_count, 1);
        assert_eq!(due[0].operation, destroy.operation);
    }

    #[test]
    fn workspace_cleanup_backoff_reaches_and_remains_at_exact_cap_across_reopen() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.db");
        let store = Store::open(&path).expect("open store");
        let scope = scope("local:1000");
        store
            .put_workspace(&scope, "ws_destroy_cap", &workspace("ws_destroy_cap"))
            .expect("seed workspace");
        let destroy = operation_named(
            "local:1000",
            "01JDESTROYBACKOFFCAP001",
            "workspace.destroy",
            "ws_destroy_cap",
            &"e".repeat(64),
        );
        assert!(matches!(
            store
                .reserve_workspace_destroy(&destroy, None)
                .expect("reserve destroy"),
            WorkspaceDestroyReservation::Admitted { .. }
        ));
        drop(store);

        let expected_delays_ms = [
            250_i64, 500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000,
        ];
        let mut due_at = chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap();
        for (attempt, expected_delay_ms) in expected_delays_ms.into_iter().enumerate() {
            let store = Store::open(&path).expect("reopen before failure");
            let due = store
                .due_destroying_workspaces("dep_test", due_at, 1)
                .expect("due destroy");
            assert_eq!(due.len(), 1);
            assert_eq!(
                due[0].attempt_count,
                u32::try_from(attempt).expect("attempt fits")
            );
            let next = store
                .record_workspace_cleanup_failure(&due[0], due_at, "driver.busy")
                .expect("schedule failure retry");
            assert_eq!(
                next - due_at,
                chrono::Duration::milliseconds(expected_delay_ms)
            );
            assert!(
                store
                    .due_destroying_workspaces(
                        "dep_test",
                        next - chrono::Duration::milliseconds(1),
                        1,
                    )
                    .expect("before exact retry boundary")
                    .is_empty()
            );
            drop(store);

            let reopened = Store::open(&path).expect("reopen after failure");
            let persisted = reopened
                .due_destroying_workspaces("dep_test", next, 1)
                .expect("retry survives reopen");
            assert_eq!(persisted.len(), 1);
            assert_eq!(
                persisted[0].attempt_count,
                u32::try_from(attempt + 1).expect("attempt fits")
            );
            drop(reopened);
            due_at = next;
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // The three durable batches and retained states are one proof.
    fn workspace_cleanup_continues_fairly_across_pending_batches_and_reopen() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.db");
        let store = Store::open(&path).expect("open store");
        let mut destroys = Vec::new();
        for index in 0..5_u32 {
            let subject = format!("local:{}", 1_000 + index);
            let id = format!("ws_destroy_batch_{index}");
            let scope = scope(&subject);
            store
                .put_workspace(&scope, &id, &workspace(&id))
                .expect("seed workspace");
            let destroy = operation_named(
                &subject,
                &format!("01JDESTROYBATCH{index:010}"),
                "workspace.destroy",
                &id,
                &format!("{index:064x}"),
            );
            assert!(matches!(
                store
                    .reserve_workspace_destroy(&destroy, None)
                    .expect("reserve destroy"),
                WorkspaceDestroyReservation::Admitted { .. }
            ));
            destroys.push(destroy);
        }

        let first_clock = chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 1).unwrap();
        let first = store
            .due_destroying_workspaces("dep_test", first_clock, 2)
            .expect("first batch");
        assert_eq!(
            first
                .iter()
                .map(|pending| pending.id.as_str())
                .collect::<Vec<_>>(),
            ["ws_destroy_batch_0", "ws_destroy_batch_1"]
        );
        store
            .record_workspace_cleanup_progress(&first[0], first_clock, 1)
            .expect("first remains pending after progress");
        store
            .record_workspace_cleanup_failure(&first[1], first_clock, "driver.busy")
            .expect("second remains pending after failure");
        drop(store);

        let reopened = Store::open(&path).expect("reopen before second batch");
        let second = reopened
            .due_destroying_workspaces("dep_test", first_clock, 2)
            .expect("second batch");
        assert_eq!(
            second
                .iter()
                .map(|pending| pending.id.as_str())
                .collect::<Vec<_>>(),
            ["ws_destroy_batch_2", "ws_destroy_batch_3"]
        );
        let second_clock = first_clock + chrono::Duration::seconds(1);
        for pending in &second {
            reopened
                .record_workspace_cleanup_progress(pending, second_clock, 1)
                .expect("second batch remains pending");
        }
        drop(reopened);

        let reopened = Store::open(&path).expect("reopen before third batch");
        let third = reopened
            .due_destroying_workspaces("dep_test", second_clock, 2)
            .expect("third batch");
        assert_eq!(third[0].id, "ws_destroy_batch_4");
        assert_eq!(third[1].id, "ws_destroy_batch_0");
        for destroy in &destroys {
            assert_eq!(
                reopened
                    .operation(&destroy.scope, &destroy.operation)
                    .expect("operation lookup")
                    .expect("durable destroy operation")
                    .state,
                OperationState::Accepted
            );
            assert_eq!(
                reopened
                    .workspace(
                        &destroy.scope,
                        destroy.resource.as_deref().expect("workspace id"),
                    )
                    .expect("workspace lookup")
                    .expect("destroying workspace")
                    .1
                    .state,
                WorkspaceState::Destroying
            );
        }
    }

    #[test]
    fn lease_cleanup_backoff_and_fair_cursor_survive_restart() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.db");
        let store = Store::open(&path).expect("open store");
        let first_authority = seed_leased_workspace(&store, "local:1000", "ws_fair_a", 1_000);
        seed_leased_workspace(&store, "local:1001", "ws_fair_b", 1_000);
        seed_leased_workspace(&store, "local:1002", "ws_fair_c", 1_000);
        let due = LeaseClock {
            wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 1).unwrap(),
            boot_id: "boot-test".to_owned(),
            boottime_ms: 2_000,
        };
        let first = store
            .lease_cleanup_candidates("dep_test", &due, 1)
            .expect("first fair batch");
        assert_eq!(first.len(), 1);
        drop(store);

        let reopened = Store::open(&path).expect("reopen store");
        let second = reopened
            .lease_cleanup_candidates("dep_test", &due, 1)
            .expect("second fair batch");
        assert_eq!(second.len(), 1);
        assert_ne!(second[0].scope.subject, first[0].scope.subject);

        let candidate = if first[0].id == "ws_fair_a" {
            first[0].clone()
        } else {
            reopened
                .lease_cleanup_candidates("dep_test", &due, 3)
                .expect("find first candidate")
                .into_iter()
                .find(|candidate| candidate.id == "ws_fair_a")
                .expect("candidate A")
        };
        let claimed = reopened
            .claim_expired_lease(&candidate, &due)
            .expect("claim candidate")
            .expect("claimed candidate");
        reopened
            .record_lease_cleanup_failure(&claimed, due.wall, "driver.busy")
            .expect("persist cleanup failure");
        assert!(
            reopened
                .lease_cleanup_candidates("dep_test", &due, 10)
                .expect("backoff batch")
                .iter()
                .all(|candidate| candidate.id != "ws_fair_a")
        );
        let retry_clock = LeaseClock {
            wall: due.wall + chrono::Duration::milliseconds(250),
            ..due
        };
        assert!(
            reopened
                .lease_cleanup_candidates("dep_test", &retry_clock, 10)
                .expect("retry batch")
                .iter()
                .any(|candidate| candidate.id == "ws_fair_a")
        );
        let page = reopened
            .events(&first_authority.scope, None, 100)
            .expect("events")
            .expect("event page");
        let failure = page
            .items
            .iter()
            .find(|event| event.transition == "workspace.cleanup-failed")
            .expect("cleanup failure event");
        assert_eq!(
            failure.cause,
            EventCause::Operation {
                operation: first_authority.operation.clone()
            }
        );
        assert_eq!(failure.actor, LEASE_SWEEPER_ACTOR);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One test proves renewal plus both boot-clock expiry branches.
    fn lease_renewal_uses_boot_clock_and_changed_boot_expires_conservatively() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        let issued = LeaseClock {
            wall: chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap(),
            boot_id: "boot-a".to_owned(),
            boottime_ms: 1_000,
        };
        let create = operation_named(
            "local:1000",
            "01JSTORELEASECREATE0001",
            "workspace.create",
            "ws_lease",
            &"4".repeat(64),
        );
        let lease = NewLease {
            ttl_ms: 1_000,
            clock: issued.clone(),
            authorizing_operation: create.operation.clone(),
            actor: create.actor.clone(),
            principal: create.principal.clone(),
        };
        store.reserve(&create).expect("reserve create");
        let mut resource = workspace("ws_lease");
        resource.lease = Some(lease.observation());
        store
            .complete_workspace_leased(
                &scope,
                &create.operation,
                "2026-08-13T12:00:00Z",
                201,
                "ws_lease",
                &resource,
                Some(&lease),
            )
            .expect("complete leased create");
        let renewal = operation_named(
            "local:1000",
            "01JSTORELEASERENEW0001",
            "workspace.lease.renew",
            "ws_lease",
            &"5".repeat(64),
        );
        store.reserve(&renewal).expect("reserve renewal");
        let renewed = NewLease {
            ttl_ms: 2_000,
            clock: LeaseClock {
                wall: issued.wall + chrono::Duration::milliseconds(500),
                boot_id: "boot-a".to_owned(),
                boottime_ms: 1_500,
            },
            authorizing_operation: renewal.operation.clone(),
            actor: renewal.actor.clone(),
            principal: renewal.principal.clone(),
        };
        store
            .renew_workspace_lease(
                &scope,
                &renewal.operation,
                "2026-08-13T12:00:00.500Z",
                200,
                "ws_lease",
                &renewed,
            )
            .expect("renew lease");
        assert!(
            store
                .claim_expired_leases(
                    "dep_test",
                    &LeaseClock {
                        wall: issued.wall + chrono::Duration::milliseconds(900),
                        boot_id: "boot-a".to_owned(),
                        boottime_ms: 1_900,
                    }
                )
                .expect("active sweep")
                .is_empty()
        );
        let expired = store
            .claim_expired_leases(
                "dep_test",
                &LeaseClock {
                    wall: issued.wall + chrono::Duration::milliseconds(901),
                    boot_id: "boot-b".to_owned(),
                    boottime_ms: 100,
                },
            )
            .expect("changed boot sweep");
        assert_eq!(expired.len(), 1);
        assert!(matches!(
            expired[0].resource,
            LeaseResource::Workspace { .. }
        ));
        store
            .complete_workspace_lease_expiry(
                &expired[0],
                issued.wall + chrono::Duration::milliseconds(901),
            )
            .expect("complete expiry");
        assert!(
            store
                .workspace(&scope, "ws_lease")
                .expect("workspace lookup")
                .is_none()
        );
    }

    #[test]
    fn lease_wall_skew_has_an_exact_thirty_second_ceiling() {
        let issued_wall = chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap();
        let renew_by = issued_wall + chrono::Duration::seconds(60);
        let clock = |skew_ms| LeaseClock {
            wall: issued_wall
                + chrono::Duration::milliseconds(500)
                + chrono::Duration::milliseconds(skew_ms),
            boot_id: "boot-a".to_owned(),
            boottime_ms: 1_500,
        };
        assert!(!lease_due(
            &clock(30_000),
            "boot-a",
            &issued_wall,
            1_000,
            &renew_by,
            61_000,
        ));
        assert!(lease_due(
            &clock(30_001),
            "boot-a",
            &issued_wall,
            1_000,
            &renew_by,
            61_000,
        ));
        assert!(lease_due(
            &LeaseClock {
                wall: issued_wall + chrono::Duration::seconds(60),
                boot_id: "boot-a".to_owned(),
                boottime_ms: 61_000,
            },
            "boot-a",
            &issued_wall,
            1_000,
            &renew_by,
            61_000,
        ));
    }

    #[test]
    fn unknown_exec_blocks_cleanup_until_physical_absence_is_proven() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        let resource = workspace("ws_recovery");
        store
            .put_workspace(&scope, "ws_recovery", &resource)
            .expect("seed workspace");
        seed_exec(
            &store,
            &scope,
            &exec("ex_unknown", "ws_recovery", ExecState::Unknown),
        );
        assert!(
            store
                .workspace_has_nonterminal_execs(&scope, "ws_recovery")
                .expect("unknown check")
        );
        store
            .remove_workspace(&scope, "ws_recovery")
            .expect("remove workspace");
        assert!(matches!(
            store
                .merge_workspace_observation(&scope, "ws_recovery", &resource)
                .expect("conditional observation"),
            WorkspaceObservationWrite::Missing
        ));
        assert!(
            store
                .workspace(&scope, "ws_recovery")
                .expect("workspace lookup")
                .is_none()
        );
    }

    #[test]
    fn concurrent_maintenance_cannot_regress_durable_terminal_exec_states() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        for terminal in [ExecState::Exited, ExecState::Cancelled, ExecState::Expired] {
            let id = format!("ex_terminal_{terminal:?}").to_ascii_lowercase();
            seed_exec(&store, &scope, &exec(&id, "ws_terminal", terminal));
            std::thread::scope(|threads| {
                for proposed in [
                    ExecState::Accepted,
                    ExecState::Running,
                    ExecState::Unknown,
                    ExecState::Exited,
                    ExecState::Cancelled,
                    ExecState::Expired,
                ] {
                    let store = &store;
                    let scope = scope.clone();
                    let id = id.clone();
                    threads.spawn(move || {
                        store
                            .put_exec(&scope, &exec(&id, "ws_terminal", proposed))
                            .expect("maintenance update");
                    });
                }
            });
            assert_eq!(
                store
                    .exec(&scope, &id)
                    .expect("terminal lookup")
                    .expect("terminal retained")
                    .resource
                    .state,
                terminal
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Full-row durability is clearest in one scenario.
    fn terminal_exec_authority_preserves_full_winner_across_signal_and_expiry() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        let mut natural = exec("ex_terminal_full", "ws_terminal", ExecState::Exited);
        natural.stdout = b"complete stdout".to_vec();
        natural.stderr = b"complete stderr".to_vec();
        natural.output_complete = true;
        natural.resource.exit = Some(substrate_wire::ExecExit {
            code: Some(7),
            signal: None,
        });
        seed_exec(&store, &scope, &natural);
        authorize_exec_lease(&store, "local:1000", &natural.resource.id);
        let expiry = ExpiredLease {
            scope: scope.clone(),
            id: natural.resource.id.clone(),
            resource: LeaseResource::Exec {
                workspace_id: "ws_terminal".to_owned(),
            },
        };
        assert!(matches!(
            store
                .complete_exec_lease_expiry(
                    &expiry,
                    "2026-08-13T12:01:00Z".parse().expect("time"),
                    None,
                )
                .expect("expiry after natural terminal"),
            ExecWrite::Superseded(_)
        ));
        assert_eq!(
            store
                .exec(&scope, &natural.resource.id)
                .expect("lookup")
                .expect("terminal"),
            natural
        );

        let mut partial = exec("ex_expiry_partial", "ws_terminal", ExecState::Running);
        partial.stdout = b"durable partial stdout".to_vec();
        partial.stderr = b"durable partial stderr".to_vec();
        partial.stdout_truncated = true;
        partial.cgroup = Some("cg-partial".to_owned());
        partial.leader_pid = Some(4242);
        partial.resource.lease = Some(LeaseObservation {
            ttl_ms: 1_000,
            renew_by: "2026-08-13T12:00:02Z".parse().expect("renew by"),
            state: LeaseState::Active,
            clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
            authorizing_operation: format!("exec-lease-authority-{}", partial.resource.id),
            actor: "test".to_owned(),
            principal: None,
        });
        seed_exec(&store, &scope, &partial);
        authorize_exec_lease(&store, "local:1000", &partial.resource.id);
        let partial_expiry = ExpiredLease {
            scope: scope.clone(),
            id: partial.resource.id.clone(),
            resource: LeaseResource::Exec {
                workspace_id: "ws_terminal".to_owned(),
            },
        };
        let ExecWrite::PersistedTransformed(expired_partial) = store
            .complete_exec_lease_expiry(
                &partial_expiry,
                "2026-08-13T12:00:03Z".parse().expect("time"),
                None,
            )
            .expect("expiry without driver observation")
        else {
            panic!("expiry must transform the durable partial observation");
        };
        assert_eq!(expired_partial.stdout, partial.stdout);
        assert_eq!(expired_partial.stderr, partial.stderr);
        assert_eq!(expired_partial.stdout_truncated, partial.stdout_truncated);
        assert_eq!(expired_partial.cgroup, partial.cgroup);
        assert_eq!(expired_partial.leader_pid, partial.leader_pid);
        assert_eq!(expired_partial.resource.state, ExecState::Expired);
        assert!(expired_partial.output_complete);

        let mut running = exec("ex_expiry_first", "ws_terminal", ExecState::Running);
        running.resource.lease = Some(LeaseObservation {
            ttl_ms: 1_000,
            renew_by: "2026-08-13T12:00:02Z".parse().expect("renew by"),
            state: LeaseState::Active,
            clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
            authorizing_operation: format!("exec-lease-authority-{}", running.resource.id),
            actor: "test".to_owned(),
            principal: None,
        });
        seed_exec(&store, &scope, &running);
        authorize_exec_lease(&store, "local:1000", &running.resource.id);
        let mut cancelled = running.clone();
        cancelled.resource.state = ExecState::Cancelled;
        cancelled.resource.exit = Some(substrate_wire::ExecExit {
            code: None,
            signal: Some(substrate_wire::Signal::Kill),
        });
        cancelled.stdout = b"captured before expiry".to_vec();
        cancelled.output_complete = true;
        let expiry = ExpiredLease {
            scope: scope.clone(),
            id: running.resource.id.clone(),
            resource: LeaseResource::Exec {
                workspace_id: "ws_terminal".to_owned(),
            },
        };
        let ExecWrite::PersistedTransformed(expired) = store
            .complete_exec_lease_expiry(
                &expiry,
                "2026-08-13T12:01:00Z".parse().expect("time"),
                Some(&cancelled),
            )
            .expect("expiry wins")
        else {
            panic!("expiry must transform the first terminal observation");
        };
        assert_eq!(expired.resource.state, ExecState::Expired);
        assert_eq!(expired.stdout, cancelled.stdout);
        assert!(expired.output_complete);
        assert_eq!(
            expired.resource.lease.as_ref().expect("lease").state,
            LeaseState::Expired
        );

        let signal = operation_named(
            "local:1000",
            "01JTERMINALRACESIGNAL001",
            "exec.signal",
            &running.resource.id,
            &"d".repeat(64),
        );
        store.reserve(&signal).expect("reserve signal");
        let write = store
            .complete_exec(
                &scope,
                &signal.operation,
                "2026-08-13T12:01:01Z",
                200,
                &cancelled.resource,
                &cancelled.stdout,
                &cancelled.stderr,
                cancelled.stdout_truncated,
                cancelled.stderr_truncated,
                cancelled.output_complete,
                cancelled.cgroup.as_deref(),
                cancelled.leader_pid,
            )
            .expect("stale signal completion");
        assert_eq!(write, ExecWrite::Superseded(expired.clone()));
        let signal_record = store
            .operation(&scope, &signal.operation)
            .expect("signal lookup")
            .expect("signal operation");
        let Some(OperationOutcome::Success { result }) = signal_record.outcome else {
            panic!("signal outcome");
        };
        assert_eq!(
            serde_json::from_value::<Exec>(result).expect("signal result"),
            expired.resource
        );
    }

    #[test]
    fn independent_store_connections_commit_only_one_full_terminal_winner() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("shared.db");
        let store_a = Store::open(&path).expect("store A");
        let store_b = Store::open(&path).expect("store B");
        let scope = scope("local:1000");
        let mut exited = exec("ex_cross_connection", "ws_test", ExecState::Exited);
        exited.stdout = b"winner A".to_vec();
        exited.output_complete = true;
        exited.resource.exit = Some(substrate_wire::ExecExit {
            code: Some(0),
            signal: None,
        });
        let mut cancelled = exec("ex_cross_connection", "ws_test", ExecState::Cancelled);
        cancelled.stderr = b"winner B".to_vec();
        cancelled.output_complete = true;
        cancelled.resource.exit = Some(substrate_wire::ExecExit {
            code: None,
            signal: Some(substrate_wire::Signal::Kill),
        });
        seed_exec(
            &store_a,
            &scope,
            &exec("ex_cross_connection", "ws_test", ExecState::Running),
        );
        let barrier = std::sync::Barrier::new(2);
        let (a, b) = std::thread::scope(|threads| {
            let first = threads.spawn(|| {
                barrier.wait();
                store_a.put_exec(&scope, &exited).expect("write A")
            });
            let second = threads.spawn(|| {
                barrier.wait();
                store_b.put_exec(&scope, &cancelled).expect("write B")
            });
            (
                first.join().expect("A thread"),
                second.join().expect("B thread"),
            )
        });
        assert!(matches!(
            (&a, &b),
            (ExecWrite::PersistedExact(_), ExecWrite::Superseded(_))
                | (ExecWrite::Superseded(_), ExecWrite::PersistedExact(_))
        ));
        let durable = store_a
            .exec(&scope, "ex_cross_connection")
            .expect("lookup")
            .expect("terminal");
        for result in [a, b] {
            match result {
                ExecWrite::PersistedExact(winner) | ExecWrite::Superseded(winner) => {
                    assert_eq!(winner, durable);
                }
                ExecWrite::PersistedTransformed(_) => panic!("put cannot transform"),
                ExecWrite::Retired => panic!("fixture must establish exec membership"),
            }
        }
    }

    #[test]
    fn put_exec_reports_lease_inheritance_as_a_transformation() {
        let store = Store::open(":memory:").expect("open store");
        let scope = scope("local:1000");
        let mut accepted = exec("ex_normalized", "ws_test", ExecState::Accepted);
        accepted.resource.lease = Some(LeaseObservation {
            ttl_ms: 1_000,
            renew_by: "2026-08-13T12:00:02Z".parse().expect("renew by"),
            state: LeaseState::Active,
            clock_tolerance_ms: substrate_wire::LEASE_CLOCK_TOLERANCE_MS,
            authorizing_operation: "exec-lease-authority-ex_normalized".to_owned(),
            actor: "test".to_owned(),
            principal: None,
        });
        seed_exec(&store, &scope, &accepted);
        let mut running_without_lease = accepted.clone();
        running_without_lease.resource.state = ExecState::Running;
        running_without_lease.resource.lease = None;
        let ExecWrite::PersistedTransformed(authoritative) = store
            .put_exec(&scope, &running_without_lease)
            .expect("normalized write")
        else {
            panic!("lease inheritance must be visible to the caller");
        };
        assert_eq!(authoritative.resource.state, ExecState::Running);
        assert_eq!(authoritative.resource.lease, accepted.resource.lease);
    }
}
