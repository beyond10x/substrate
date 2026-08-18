use crate::events::{
    append_control_event, commit_effect, ensure_stream, event_cursor, parse_canonical_u64,
    stream_position,
};
use crate::{Scope, Store, StoreError, to_i64, to_u64};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use substrate_wire::{
    ErrorClass, ErrorDetail, Event, SnapshotHistory, SnapshotItem, SnapshotItemKind,
    SnapshotMetadata, SnapshotPage, SnapshotPartitions,
};

const MAX_ACTIVE_SNAPSHOTS_PER_SCOPE: i64 = 64;

pub(crate) const MAX_SNAPSHOT_ITEMS: usize = 4_096;

const MAX_EXPIRED_SNAPSHOT_MARKERS_PER_SCOPE: i64 = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotReadError {
    Expired,
    Incomplete,
    InvalidCursor,
    NotFound,
}

impl Store {
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
}

fn snapshot_cursor(snapshot: &str, ordinal: u64) -> String {
    format!("sp2.{snapshot}.{ordinal}")
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
