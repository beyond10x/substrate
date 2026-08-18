use std::path::Path;

use crate::operations::operation_row_bytes;
use crate::snapshots::MAX_SNAPSHOT_ITEMS;
use crate::{
    OperationCapacity, ResourceCapacity, Scope, Store, StoreConfig, StoreError, to_i64, to_u64,
};
use parking_lot::{Mutex, RwLock};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

const MAX_AUTOMATIC_MIGRATION_ROWS: u64 = 4_096;

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

pub(crate) fn migrate_stream_scope_grammar(connection: &Connection) -> Result<(), StoreError> {
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
