use crate::events::{append_event, commit_effect};
use crate::execs::{is_terminal_exec_state, load_exec, upsert_exec};
use crate::leases::{freeze_workspace_lease_if_due, upsert_lease};
use crate::operations::{
    complete_operation_error_transaction, existing_reservation, finalize_operation_accounting,
    insert_accepted_operation, insert_refused_operation, operation_identity,
    operation_identity_full, refresh_nonterminal_operation_accounting,
    resource_partition_at_capacity,
};
use crate::{
    LeaseClock, NewLease, NewOperation, OperationCapacity, Reservation, ResourceCapacity, Scope,
    Store, StoreError, StoredAnswer, StoredExec,
};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use substrate_wire::{
    ErrorClass, ErrorDetail, Exec, ExecState, LeaseState, OperationOutcome, PipeSession,
    SessionAbsence, SessionAttachmentState, SessionKind, SessionState, Workspace, WorkspaceState,
};

#[derive(Debug, Clone, PartialEq)]
pub enum SessionRetireReservation {
    Existing(Reservation),
    Capacity(OperationCapacity),
    Refused(StoredAnswer),
    Retired(SessionAbsence),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAttachmentClaim {
    Claimed,
    AlreadyClaimed,
    NotAttachable,
    Missing,
}

impl Store {
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
}

pub(crate) fn load_session(
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

pub(crate) fn load_session_for_exec(
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

pub(crate) fn upsert_session(
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

pub(crate) fn project_session_from_exec(
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

pub(crate) const fn session_transition(state: SessionState) -> &'static str {
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
