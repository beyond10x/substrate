use crate::events::{append_event, commit_effect};
use crate::leases::{freeze_workspace_lease_if_due, upsert_lease};
use crate::operations::{
    existing_reservation, finalize_operation_accounting, insert_accepted_operation,
    insert_refused_operation, operation_identity, operation_identity_full,
    resource_operation_identity, resource_partition_at_capacity, terminal_transition,
};
use crate::sessions::{load_session_for_exec, project_session_from_exec, session_transition};
use crate::{
    LeaseClock, NewLease, NewOperation, OperationCapacity, RecoveryExec, Reservation,
    ResourceCapacity, Scope, Store, StoreError, StoredAnswer,
};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use substrate_wire::{
    ErrorClass, ErrorDetail, Exec, ExecAbsence, ExecKind, ExecState, ExecUsage, LeaseState,
    OperationOutcome, Workspace, WorkspaceState,
};

pub(crate) fn mark_exec_unknown(resource: &mut Exec, observed_at: DateTime<Utc>) {
    resource.state = ExecState::Unknown;
    resource.observed_at = observed_at;
    if matches!(resource.usage, Some(ExecUsage::Pending { .. })) {
        resource.usage = Some(ExecUsage::Unavailable {
            observed_at,
            code: "exec.metrics-unavailable".to_owned(),
            message: "The daemon restarted before a complete resource observation was recovered."
                .to_owned(),
        });
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecRetireReservation {
    Existing(Reservation),
    Capacity(OperationCapacity),
    Refused(StoredAnswer),
    Retired(substrate_wire::ExecAbsence),
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

/// Exactly what workspace lease cleanup reads: an exec's identity and its observed state.
///
/// A projection rather than [`StoredExec`], because the cleanup sweep is the one caller that walks
/// every exec of a workspace at once. `substrate_wire::MAX_CURRENT_EXECS` is 2048 and
/// `substrate_wire::MAX_IO_BYTES` is 1 MiB per stream, so handing whole execs to that sweep put
/// 4 GiB of `stdout` and `stderr` within reach of one expiry. A row with no output field cannot
/// carry them, so no later caller can drag them back in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceExecState {
    pub id: String,
    pub state: ExecState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecWrite {
    PersistedExact(StoredExec),
    PersistedTransformed(StoredExec),
    Superseded(StoredExec),
    Retired,
}

impl Store {
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

    /// Reads the identity and state of every exec still physically present in a workspace.
    ///
    /// Two values per row, and no output column is named: see [`WorkspaceExecState`].
    pub fn execs_for_workspace(
        &self,
        scope: &Scope,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceExecState>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT id, json_extract(resource_json, '$.state') FROM execs
             WHERE deployment = ?1 AND subject = ?2 AND workspace_id = ?3
               AND physically_absent = 0 ORDER BY id",
        )?;
        statement
            .query_map(
                params![scope.deployment, scope.subject, workspace_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .map(|row| {
                let (id, state) = row?;
                Ok(WorkspaceExecState {
                    id,
                    state: serde_json::from_value(serde_json::Value::String(state))?,
                })
            })
            .collect()
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
        mark_exec_unknown(&mut stored.resource, observed_at);
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
}

pub(crate) fn is_terminal_exec_state(state: ExecState) -> bool {
    matches!(
        state,
        ExecState::Exited | ExecState::Cancelled | ExecState::Expired
    )
}

pub(crate) fn load_exec(
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

pub(crate) fn upsert_exec(
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

#[cfg(test)]
mod tests {
    use rusqlite::params;
    use substrate_wire::{
        ConfinementRequest, Exec, ExecKind, ExecState, NetworkMode, SandboxProfile,
    };
    use tempfile::tempdir;

    use super::{StoredExec, WorkspaceExecState, upsert_exec};
    use crate::{Scope, Store};

    /// One stream at the wire bound (`substrate_wire::MAX_IO_BYTES`).
    const OUTPUT_BYTES: usize = 1_048_576;
    const WORKSPACE_EXECS: usize = 64;
    /// Exec metadata for `WORKSPACE_EXECS` rows is kilobytes; the output set is 128 MiB.
    const RESIDENT_GROWTH_CEILING: u64 = 16 * 1_048_576;

    fn scope() -> Scope {
        Scope {
            deployment: "dep_cleanup".to_owned(),
            subject: "local:1000".to_owned(),
        }
    }

    fn exec(id: &str, workspace: &str, state: ExecState, output_bytes: usize) -> StoredExec {
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
                    aperture: None,
                    profile: SandboxProfile::Workspace,
                    required: true,
                },
                applied: None,
                exit: None,
                usage: None,
                lease: None,
                refusal: None,
            },
            stdout: vec![b'o'; output_bytes],
            stderr: vec![b'e'; output_bytes],
            stdout_truncated: false,
            stderr_truncated: false,
            output_complete: true,
            cgroup: None,
            leader_pid: None,
        }
    }

    /// Resident set size of this process, from the kernel's own accounting.
    fn resident_bytes() -> u64 {
        let statm = std::fs::read_to_string("/proc/self/statm").expect("resident page counts");
        let pages: u64 = statm
            .split_whitespace()
            .nth(1)
            .expect("resident set size field")
            .parse()
            .expect("resident page count");
        pages * 4096
    }

    #[test]
    fn workspace_lease_cleanup_never_reads_the_output_columns() {
        let store = Store::open(":memory:").expect("state store");
        let scope = scope();
        for (id, workspace, state) in [
            ("exec_running", "ws_expiring", ExecState::Running),
            ("exec_exited", "ws_expiring", ExecState::Exited),
            ("exec_elsewhere", "ws_other", ExecState::Running),
        ] {
            upsert_exec(
                &store.connection.lock(),
                &scope,
                &exec(id, workspace, state, 16),
            )
            .expect("seed exec membership");
        }
        // Every output column now holds a value the blob decoder rejects, so reading one is an
        // observable failure rather than a silent success.
        store
            .connection
            .lock()
            .execute(
                "UPDATE execs SET stdout = ?1, stderr = ?1",
                params!["not-a-blob"],
            )
            .expect("poison the output columns");
        assert!(
            store.exec(&scope, "exec_running").is_err(),
            "the whole-exec read is the path that reads the output columns"
        );

        let states = store
            .execs_for_workspace(&scope, "ws_expiring")
            .expect("workspace lease cleanup reads no output column");

        assert_eq!(
            states,
            vec![
                WorkspaceExecState {
                    id: "exec_exited".to_owned(),
                    state: ExecState::Exited,
                },
                WorkspaceExecState {
                    id: "exec_running".to_owned(),
                    state: ExecState::Running,
                },
            ]
        );
    }

    #[test]
    fn workspace_lease_cleanup_load_is_bounded_by_exec_metadata() {
        let directory = tempdir().expect("temporary directory");
        let store = Store::open(directory.path().join("state.sqlite3")).expect("state store");
        let scope = scope();
        for index in 0..WORKSPACE_EXECS {
            upsert_exec(
                &store.connection.lock(),
                &scope,
                &exec(
                    &format!("exec_{index:04}"),
                    "ws_expiring",
                    ExecState::Exited,
                    OUTPUT_BYTES,
                ),
            )
            .expect("seed exec membership");
        }
        let stored_output_bytes: i64 = store
            .connection
            .lock()
            .query_row(
                "SELECT sum(length(stdout) + length(stderr)) FROM execs WHERE workspace_id = ?1",
                params!["ws_expiring"],
                |row| row.get(0),
            )
            .expect("stored output bytes");
        assert_eq!(
            stored_output_bytes,
            i64::try_from(2 * OUTPUT_BYTES * WORKSPACE_EXECS).expect("output byte total")
        );

        let mut smallest_growth = u64::MAX;
        for _ in 0..3 {
            let before = resident_bytes();
            let states = store
                .execs_for_workspace(&scope, "ws_expiring")
                .expect("workspace lease cleanup load");
            let after = resident_bytes();
            assert_eq!(states.len(), WORKSPACE_EXECS);
            smallest_growth = smallest_growth.min(after.saturating_sub(before));
            drop(states);
        }

        assert!(
            smallest_growth < RESIDENT_GROWTH_CEILING,
            "workspace lease cleanup grew resident memory by {smallest_growth} bytes over a \
             {stored_output_bytes}-byte output set; exec metadata alone bounds it below \
             {RESIDENT_GROWTH_CEILING} bytes"
        );
    }
}
