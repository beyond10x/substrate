use crate::events::{append_event, commit_effect};
use crate::execs::{load_exec, mark_exec_unknown};
use crate::operations::{
    operation_resource_kind, parse_operation_state, refresh_nonterminal_operation_accounting,
};
use crate::sessions::project_session_from_exec;
use crate::{Scope, Store, StoreError, StoredExec, to_i64};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde_json::json;
use substrate_wire::{Exec, OperationState, Workspace};

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

impl Store {
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
            mark_exec_unknown(&mut resource, observed_at.parse()?);
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
