use crate::events::{append_event, commit_effect};
use crate::execs::{is_terminal_exec_state, load_exec, upsert_exec};
use crate::operations::{finalize_operation_accounting, operation_identity};
use crate::sessions::{
    load_session, project_session_from_exec, session_transition, upsert_session,
};
use crate::workspaces::{
    WORKSPACE_CLEANUP_INITIAL_BACKOFF_MS, WORKSPACE_CLEANUP_MAX_BACKOFF_MS, insert_tombstone,
    upsert_workspace,
};
use crate::{ExecWrite, Scope, Store, StoreConfig, StoreError, StoredExec, to_i64, to_u64};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::Serialize;
use substrate_wire::{
    ErrorClass, ErrorDetail, Event, Exec, ExecState, LeaseObservation, LeaseState,
    OperationOutcome, PipeSession, Workspace, WorkspaceState,
};

pub(crate) const LEASE_SWEEPER_ACTOR: &str = "lease-sweeper";

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

impl Store {
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
}

pub(crate) fn upsert_lease(
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

pub(crate) fn lease_due(
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
pub(crate) fn freeze_workspace_lease_if_due(
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
