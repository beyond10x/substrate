use crate::events::{append_event, commit_effect};
use crate::execs::upsert_exec;
use crate::leases::upsert_lease;
use crate::workspaces::upsert_workspace;
use crate::{NewLease, Scope, Store, StoreConfig, StoreError, StoredExec, to_i64, to_u64};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::Serialize;
use serde_json::json;
use substrate_wire::{
    ErrorClass, ErrorDetail, Event, Exec, ExecState, OperationOutcome, OperationRecord,
    OperationState, Workspace,
};

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
    /// The declared grant a verified delegated context named, or `None` (ADR 0011).
    pub grant_ref: Option<String>,
    /// The initiating platform principal a verified delegated context named, or `None`.
    ///
    /// Never `principal`: that column keeps the calling process id, and the two are separate
    /// because collapsing them is what design 06 § 2 forbids.
    pub platform_principal: Option<String>,
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
    /// Boxed because the ledger row is the one large member of an otherwise small enum, and this
    /// value is returned by value on every mutation path (ADR 0011 added two members to the row).
    Pending(Box<OperationRecord>),
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

impl Store {
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

    pub(crate) fn persist_resource_capacity_refusal(
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
            Reservation::Pending(Box::new(existing.record))
        }))
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
                Reservation::Pending(Box::new(existing.record))
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
                terminal_at, capability_snapshot, actor, principal, grant_ref, platform_principal,
                resource, outcome_json, response_status
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, 'refused', NULL, ?6, NULL, ?7, ?8, ?9, ?10, NULL, ?11, ?12
             )",
            params![
                new.scope.deployment,
                new.scope.subject,
                new.operation,
                new.operation_kind,
                new.request_hash,
                terminal_at,
                new.actor,
                new.principal,
                new.grant_ref,
                new.platform_principal,
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn complete(
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

    pub fn operation(
        &self,
        scope: &Scope,
        operation: &str,
    ) -> Result<Option<OperationRecord>, StoreError> {
        let connection = self.connection.lock();
        Ok(load_operation(&connection, scope, operation)?.map(|value| value.record))
    }
}

pub(crate) fn parse_operation_state(value: &str) -> Result<OperationState, StoreError> {
    match value {
        "refused" => Ok(OperationState::Refused),
        "accepted" => Ok(OperationState::Accepted),
        "unknown" => Ok(OperationState::Unknown),
        "terminal" => Ok(OperationState::Terminal),
        _ => Err(StoreError::NotAccepted(value.to_owned())),
    }
}

pub(crate) fn resource_partition_at_capacity(
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

pub(crate) fn operation_row_bytes(
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
          + COALESCE(length(CAST(grant_ref AS BLOB)), 0)
          + COALESCE(length(CAST(platform_principal AS BLOB)), 0)
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

pub(crate) fn finalize_operation_accounting(
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

pub(crate) fn refresh_nonterminal_operation_accounting(
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

pub(crate) fn existing_reservation(
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
        Reservation::Pending(Box::new(existing.record))
    }))
}

pub(crate) fn insert_accepted_operation(
    connection: &Connection,
    retention: u64,
    config: StoreConfig,
    new: &NewOperation,
) -> Result<Event, StoreError> {
    connection.execute(
        "INSERT INTO operations (
            deployment, subject, operation, operation_kind, request_hash, state, accepted_at,
            capability_snapshot, actor, principal, grant_ref, platform_principal, resource
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'accepted', ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
            new.grant_ref,
            new.platform_principal,
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

pub(crate) fn insert_refused_operation(
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
            terminal_at, capability_snapshot, actor, principal, grant_ref, platform_principal,
            resource, outcome_json, response_status
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, 'refused', NULL, ?6, NULL, ?7, ?8, ?9, ?10, ?11, ?12, ?13
         )",
        params![
            new.scope.deployment,
            new.scope.subject,
            new.operation,
            new.operation_kind,
            new.request_hash,
            terminal_at,
            new.actor,
            new.principal,
            new.grant_ref,
            new.platform_principal,
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

pub(crate) fn operation_resource_kind(operation_kind: &str) -> &str {
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

pub(crate) fn terminal_transition(
    operation_kind: &str,
    outcome: &OperationOutcome,
) -> &'static str {
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

pub(crate) fn operation_identity(
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

pub(crate) fn operation_identity_full(
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

pub(crate) fn resource_operation_identity(
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn complete_operation_error_transaction(
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

pub(crate) struct LoadedOperation {
    pub(crate) record: OperationRecord,
    pub(crate) answer: Option<StoredAnswer>,
}

pub(crate) fn load_operation(
    connection: &Connection,
    scope: &Scope,
    operation: &str,
) -> Result<Option<LoadedOperation>, StoreError> {
    let stored = connection
        .query_row(
            "SELECT operation_kind, request_hash, state, accepted_at, terminal_at,
                    capability_snapshot, actor, principal, resource, outcome_json,
                    response_status, grant_ref, platform_principal
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
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
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
        grant_ref,
        platform_principal,
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
            grant_ref,
            platform_principal,
        },
        answer,
    }))
}
