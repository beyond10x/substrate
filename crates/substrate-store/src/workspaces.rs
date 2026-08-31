use crate::events::{append_event, commit_effect};
use crate::leases::{freeze_workspace_lease_if_due, upsert_lease};
use crate::operations::{
    existing_reservation, finalize_operation_accounting, insert_accepted_operation,
    insert_refused_operation, operation_identity, resource_partition_at_capacity,
};
use crate::{
    LeaseClock, NewLease, NewOperation, OperationCapacity, Reservation, ResourceCapacity, Scope,
    Store, StoreError, StoredAnswer, to_i64,
};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::Serialize;
use serde_json::Value;
use substrate_wire::{
    ErrorClass, ErrorDetail, Exec, ExecState, LeaseState, OperationOutcome, Workspace,
    WorkspaceState,
};

pub(crate) const WORKSPACE_CLEANUP_INITIAL_BACKOFF_MS: i64 = 250;

pub(crate) const WORKSPACE_CLEANUP_MAX_BACKOFF_MS: i64 = 30_000;

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
    Authoritative(Box<Workspace>),
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
pub struct PendingWorkspaceDestroy {
    pub scope: Scope,
    pub id: String,
    pub root_name: String,
    pub operation: String,
    pub attempt_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tombstone {
    pub kind: String,
    pub id: String,
    pub deleted_at: DateTime<Utc>,
    pub reason: String,
    pub last_observation: Value,
}

impl Store {
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

    #[cfg(test)]
    pub(crate) fn put_workspace(
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
        Ok(WorkspaceObservationWrite::Authoritative(Box::new(durable)))
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
    pub(crate) fn mark_workspace_destroying(
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
    pub(crate) fn remove_workspace(&self, scope: &Scope, id: &str) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        connection.execute(
            "DELETE FROM workspaces WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, id],
        )?;
        Ok(())
    }

    pub fn workspace_has_nonterminal_execs(
        &self,
        scope: &Scope,
        workspace_id: &str,
    ) -> Result<bool, StoreError> {
        let connection = self.connection.lock();
        workspace_has_nonterminal_execs(&connection, scope, workspace_id)
    }
}

pub(crate) fn insert_tombstone(
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

pub(crate) fn upsert_workspace(
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
