#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc)] // The crate is an internal persistence boundary.

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::Serialize;
use substrate_wire::{
    ErrorDetail, Exec, OperationOutcome, OperationRecord, OperationState, Workspace,
};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub deployment: String,
    pub subject: String,
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

pub struct Store {
    connection: Mutex<Connection>,
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
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
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
                PRIMARY KEY (deployment, subject, operation)
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
                cgroup TEXT,
                leader_pid INTEGER,
                PRIMARY KEY (deployment, subject, id)
            ) WITHOUT ROWID;
            ",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn reserve(&self, new: &NewOperation) -> Result<Reservation, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = load_operation(&transaction, &new.scope, &new.operation)? {
            let reservation = if existing.record.request_hash != new.request_hash {
                Reservation::Conflict
            } else if let Some(answer) = existing.answer {
                Reservation::Replay(answer)
            } else {
                Reservation::Pending(existing.record)
            };
            transaction.commit()?;
            return Ok(reservation);
        }
        transaction.execute(
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
        transaction.commit()?;
        Ok(Reservation::Accepted)
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
        transaction.commit()?;
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
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = OperationOutcome::Success {
            result: serde_json::to_value(result)?,
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
                workspace_id,
                serde_json::to_string(&outcome)?,
                i64::from(status),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(operation.to_owned()));
        }
        transaction.execute(
            "DELETE FROM workspaces WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, workspace_id],
        )?;
        transaction.commit()?;
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
    ) -> Result<(), StoreError> {
        let outcome = OperationOutcome::Success {
            result: serde_json::to_value(resource)?,
        };
        let stored = StoredExec {
            resource: resource.clone(),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
            stdout_truncated,
            stderr_truncated,
            output_complete,
            cgroup: cgroup.map(ToOwned::to_owned),
            leader_pid,
        };
        self.complete(
            scope,
            operation,
            terminal_at,
            status,
            Some(&resource.id),
            &outcome,
            None,
            Some(&stored),
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
        )
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
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
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
                resource_id,
                serde_json::to_string(outcome)?,
                i64::from(status),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::NotAccepted(operation.to_owned()));
        }
        if let Some((root_name, resource)) = workspace {
            upsert_workspace(&transaction, scope, root_name, resource)?;
        }
        if let Some(resource) = exec {
            upsert_exec(&transaction, scope, resource)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn put_workspace(
        &self,
        scope: &Scope,
        root_name: &str,
        workspace: &Workspace,
    ) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        upsert_workspace(&connection, scope, root_name, workspace)
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

    pub fn remove_workspace(&self, scope: &Scope, id: &str) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        connection.execute(
            "DELETE FROM workspaces WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![scope.deployment, scope.subject, id],
        )?;
        Ok(())
    }

    pub fn put_exec(&self, scope: &Scope, resource: &StoredExec) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        upsert_exec(&connection, scope, resource)
    }

    pub fn exec(&self, scope: &Scope, id: &str) -> Result<Option<StoredExec>, StoreError> {
        let connection = self.connection.lock();
        connection
            .query_row(
                "SELECT resource_json, stdout, stderr, stdout_truncated, stderr_truncated,
                        output_complete, cgroup, leader_pid
                 FROM execs WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![scope.deployment, scope.subject, id],
                |row| {
                    let resource_json: String = row.get(0)?;
                    let leader_pid: Option<i64> = row.get(7)?;
                    Ok((
                        resource_json,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, bool>(4)?,
                        row.get::<_, bool>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        leader_pid,
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

    pub fn workspace_has_nonterminal_execs(
        &self,
        scope: &Scope,
        workspace_id: &str,
    ) -> Result<bool, StoreError> {
        let connection = self.connection.lock();
        let resources = {
            let mut statement = connection.prepare(
                "SELECT resource_json FROM execs
                 WHERE deployment = ?1 AND subject = ?2 AND workspace_id = ?3",
            )?;
            statement
                .query_map(
                    params![scope.deployment, scope.subject, workspace_id],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        for json in resources {
            let resource: Exec = serde_json::from_str(&json)?;
            if matches!(
                resource.state,
                substrate_wire::ExecState::Accepted
                    | substrate_wire::ExecState::Running
                    | substrate_wire::ExecState::Unknown
            ) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn operation(
        &self,
        scope: &Scope,
        operation: &str,
    ) -> Result<Option<OperationRecord>, StoreError> {
        let connection = self.connection.lock();
        Ok(load_operation(&connection, scope, operation)?.map(|value| value.record))
    }

    pub fn reconcile_after_restart(&self) -> Result<usize, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let operations = transaction.execute(
            "UPDATE operations SET state = 'unknown' WHERE state = 'accepted'",
            [],
        )?;
        let mut statement =
            transaction.prepare("SELECT deployment, subject, id, resource_json FROM execs")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for (deployment, subject, id, json) in rows {
            let mut resource: Exec = serde_json::from_str(&json)?;
            if matches!(
                resource.state,
                substrate_wire::ExecState::Accepted | substrate_wire::ExecState::Running
            ) {
                resource.state = substrate_wire::ExecState::Unknown;
                resource.observed_at = chrono::Utc::now();
                transaction.execute(
                    "UPDATE execs SET resource_json = ?4, output_complete = 1
                     WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                    params![deployment, subject, id, serde_json::to_string(&resource)?],
                )?;
            }
        }
        transaction.commit()?;
        Ok(operations)
    }
}

struct LoadedOperation {
    record: OperationRecord,
    answer: Option<StoredAnswer>,
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
            stdout_truncated, stderr_truncated, output_complete, cgroup, leader_pid
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (deployment, subject, id) DO UPDATE SET
            resource_json = excluded.resource_json,
            stdout = excluded.stdout,
            stderr = excluded.stderr,
            stdout_truncated = excluded.stdout_truncated,
            stderr_truncated = excluded.stderr_truncated,
            output_complete = excluded.output_complete,
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
    use std::collections::BTreeMap;

    use substrate_wire::{OperationOutcome, Workspace, WorkspaceKind, WorkspaceState};
    use tempfile::tempdir;

    use super::{NewOperation, Reservation, Scope, Store};

    fn scope(subject: &str) -> Scope {
        Scope {
            deployment: "dep_test".to_owned(),
            subject: subject.to_owned(),
        }
    }

    fn operation(subject: &str, hash: &str) -> NewOperation {
        NewOperation {
            scope: scope(subject),
            operation: "01JSTORETEST0000000001".to_owned(),
            operation_kind: "workspace.create".to_owned(),
            request_hash: hash.to_owned(),
            accepted_at: "2026-08-13T12:00:00Z".to_owned(),
            capability_snapshot: Some(format!("sha256:{}", "7".repeat(64))),
            actor: "test".to_owned(),
            principal: None,
            resource: Some("ws_reserved".to_owned()),
        }
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
        let workspace = Workspace {
            id: "ws_test".to_owned(),
            kind: WorkspaceKind::Workspace,
            labels: BTreeMap::new(),
            observed_at: "2026-08-13T12:00:01Z".parse().expect("time"),
            state: WorkspaceState::Ready,
        };
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
        drop(store);

        let reopened = Store::open(&path).expect("reopen");
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
        assert_eq!(store.reconcile_after_restart().expect("reconcile"), 1);
        let reconciled = store
            .operation(&accepted.scope, &accepted.operation)
            .expect("lookup")
            .expect("record");
        assert_eq!(reconciled.state, substrate_wire::OperationState::Unknown);
        assert_eq!(reconciled.resource.as_deref(), Some("ws_reserved"));
    }
}
