//! The backend ledger (design 34 § 4): one row per object a driver creates in a backend it does
//! not own, such as a Kubernetes namespace.
//!
//! A row holds the object's name before any API call and, once the create answered, the uid that
//! answer named. Names are reserved by the daemon inside the transaction that commits the
//! operation's `accepted` state (ADR 0005), so a crash after acceptance never leaves an object the
//! ledger cannot name. A driver that names no backend object (the host driver) reserves nothing,
//! and this table stays empty.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

use crate::{Store, StoreError, to_i64};

/// Which resource a backend object serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendObjectKind {
    Workspace,
    Exec,
    Probe,
}

impl BackendObjectKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Exec => "exec",
            Self::Probe => "probe",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "workspace" => Ok(Self::Workspace),
            "exec" => Ok(Self::Exec),
            "probe" => Ok(Self::Probe),
            _ => Err(StoreError::BackendObjectShape),
        }
    }
}

/// What a backend object is to its resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendObjectRole {
    Pvc,
    Keeper,
    Pod,
}

impl BackendObjectRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pvc => "pvc",
            Self::Keeper => "keeper",
            Self::Pod => "pod",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "pvc" => Ok(Self::Pvc),
            "keeper" => Ok(Self::Keeper),
            "pod" => Ok(Self::Pod),
            _ => Err(StoreError::BackendObjectShape),
        }
    }
}

/// One name a driver will create, with its role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendObjectName {
    pub role: BackendObjectRole,
    pub name: String,
}

/// The names one accepted operation reserves, with the digest that ties each object to it.
///
/// Empty by default: a reservation without objects writes no row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackendReservation {
    /// `sha256(subject ‖ operation id)`, length-prefixed, as lowercase hex (design 34 § 4).
    pub operation_digest: String,
    pub objects: Vec<BackendObjectName>,
}

/// One ledger row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendObject {
    pub kind: BackendObjectKind,
    pub resource_id: String,
    pub role: BackendObjectRole,
    pub name: String,
    pub operation_digest: String,
    /// The uid the create answered, once bound; `None` while no create answer was committed.
    pub uid: Option<String>,
    pub created_at: DateTime<Utc>,
    /// When the object was read absent and the row closed.
    pub settled_at: Option<DateTime<Utc>>,
}

/// What binding a uid to a name did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendUidBinding {
    /// The uid is the row's bound uid: newly bound, or bound to this same uid before.
    Bound,
    /// The row is bound to a different uid, which it keeps: a collision (ADR 0047 decision 4).
    Collision { bound: String },
    /// No row names this object: the ledger never reserved it.
    Unrecorded,
    /// The row was closed when its object read absent; it binds nothing again.
    Settled,
}

pub(crate) fn migrate_backend_objects(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS backend_objects (
            deployment TEXT NOT NULL,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('workspace','exec','probe')),
            resource_id TEXT NOT NULL CHECK (length(resource_id) > 0),
            role TEXT NOT NULL CHECK (role IN ('pvc','keeper','pod')),
            name TEXT NOT NULL CHECK (length(name) > 0),
            operation_digest TEXT NOT NULL CHECK (length(operation_digest) > 0),
            uid TEXT CHECK (uid IS NULL OR length(uid) > 0),
            created_at TEXT NOT NULL,
            settled_at TEXT,
            PRIMARY KEY (deployment, name),
            UNIQUE (deployment, resource_kind, resource_id, role)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS backend_objects_unsettled
            ON backend_objects (deployment, settled_at, created_at, name);
        ",
    )?;
    Ok(())
}

/// Writes the rows of `reservation` for one resource inside the caller's transaction. A name
/// another row of the deployment already holds is refused by name, and the caller's transaction
/// then commits nothing.
pub(crate) fn insert_backend_objects(
    connection: &Connection,
    deployment: &str,
    kind: BackendObjectKind,
    resource_id: &str,
    created_at: &str,
    reservation: &BackendReservation,
) -> Result<(), StoreError> {
    for object in &reservation.objects {
        let held = connection
            .query_row(
                "SELECT 1 FROM backend_objects WHERE deployment = ?1 AND name = ?2",
                params![deployment, object.name],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if held {
            return Err(StoreError::BackendNameHeld(object.name.clone()));
        }
        connection.execute(
            "INSERT INTO backend_objects (
                deployment, resource_kind, resource_id, role, name, operation_digest, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                deployment,
                kind.as_str(),
                resource_id,
                object.role.as_str(),
                object.name,
                reservation.operation_digest,
                created_at,
            ],
        )?;
    }
    Ok(())
}

const BACKEND_OBJECT_COLUMNS: &str = "resource_kind, resource_id, role, name, operation_digest, \
     uid, created_at, settled_at";

fn backend_object(row: &rusqlite::Row<'_>) -> rusqlite::Result<[Option<String>; 8]> {
    Ok([
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ])
}

fn decode(columns: [Option<String>; 8]) -> Result<BackendObject, StoreError> {
    let [
        kind,
        resource_id,
        role,
        name,
        operation_digest,
        uid,
        created_at,
        settled_at,
    ] = columns;
    let required = |value: Option<String>| value.ok_or(StoreError::BackendObjectShape);
    Ok(BackendObject {
        kind: BackendObjectKind::parse(&required(kind)?)?,
        resource_id: required(resource_id)?,
        role: BackendObjectRole::parse(&required(role)?)?,
        name: required(name)?,
        operation_digest: required(operation_digest)?,
        uid,
        created_at: parse_time(&required(created_at)?)?,
        settled_at: settled_at.as_deref().map(parse_time).transpose()?,
    })
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, StoreError> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

impl Store {
    /// Every row of one resource, settled or not, in role then name order.
    pub fn backend_objects(
        &self,
        deployment: &str,
        kind: BackendObjectKind,
        resource_id: &str,
    ) -> Result<Vec<BackendObject>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {BACKEND_OBJECT_COLUMNS} FROM backend_objects
             WHERE deployment = ?1 AND resource_kind = ?2 AND resource_id = ?3
             ORDER BY role, name"
        ))?;
        let rows = statement
            .query_map(
                params![deployment, kind.as_str(), resource_id],
                backend_object,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(decode).collect()
    }

    /// Commits `uid` as the backend identity of `name`, unless the row is bound to another uid,
    /// settled, or absent. One transaction, committed before it returns.
    pub fn bind_backend_uid(
        &self,
        deployment: &str,
        name: &str,
        uid: &str,
    ) -> Result<BackendUidBinding, StoreError> {
        if uid.is_empty() {
            return Err(StoreError::BackendObjectShape);
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<(Option<String>, Option<String>)> = transaction
            .query_row(
                "SELECT uid, settled_at FROM backend_objects
                 WHERE deployment = ?1 AND name = ?2",
                params![deployment, name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let binding = match row {
            None => BackendUidBinding::Unrecorded,
            Some((_, Some(_))) => BackendUidBinding::Settled,
            Some((Some(bound), None)) if bound == uid => BackendUidBinding::Bound,
            Some((Some(bound), None)) => BackendUidBinding::Collision { bound },
            Some((None, None)) => {
                transaction.execute(
                    "UPDATE backend_objects SET uid = ?3
                     WHERE deployment = ?1 AND name = ?2 AND uid IS NULL AND settled_at IS NULL",
                    params![deployment, name, uid],
                )?;
                BackendUidBinding::Bound
            }
        };
        transaction.commit()?;
        Ok(binding)
    }

    /// Closes the row of `name`: its object read absent. Settling a settled row keeps its first
    /// time. `false` when no row names the object.
    pub fn settle_backend_object(
        &self,
        deployment: &str,
        name: &str,
        settled_at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists = transaction
            .query_row(
                "SELECT 1 FROM backend_objects WHERE deployment = ?1 AND name = ?2",
                params![deployment, name],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            transaction.execute(
                "UPDATE backend_objects SET settled_at = ?3
                 WHERE deployment = ?1 AND name = ?2 AND settled_at IS NULL",
                params![deployment, name, settled_at.to_rfc3339()],
            )?;
        }
        transaction.commit()?;
        Ok(exists)
    }

    /// Reserves the objects of one probe, keyed by its nonce, in one transaction.
    pub fn reserve_backend_probe(
        &self,
        deployment: &str,
        nonce: &str,
        created_at: DateTime<Utc>,
        reservation: &BackendReservation,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_backend_objects(
            &transaction,
            deployment,
            BackendObjectKind::Probe,
            nonce,
            &created_at.to_rfc3339(),
            reservation,
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// At most `limit` rows of the deployment that are not settled, oldest first, in
    /// `(created_at, name)` order. `after` names the last row of the previous page; the page
    /// starts past that row's position, read from its stored `created_at` (rows are never
    /// deleted, only settled, so the position stays). A name no row holds yields no rows.
    pub fn unsettled_backend_objects(
        &self,
        deployment: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<BackendObject>, StoreError> {
        let limit = to_i64(u64::try_from(limit).map_err(|_| StoreError::IntegerRange)?)?;
        let connection = self.connection.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {BACKEND_OBJECT_COLUMNS} FROM backend_objects
             WHERE deployment = ?1 AND settled_at IS NULL
               AND (?3 IS NULL OR (created_at, name) > (
                   (SELECT created_at FROM backend_objects WHERE deployment = ?1 AND name = ?3),
                   ?3
               ))
             ORDER BY created_at, name LIMIT ?2"
        ))?;
        let rows = statement
            .query_map(params![deployment, limit, after], backend_object)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(decode).collect()
    }
}

#[cfg(test)]
mod tests;
