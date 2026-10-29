//! The daemon's side of the backend ledger (design 34 § 4): the names a driver declares are
//! reserved in the transaction that records `accepted`, and [`StoreBackendLedger`] is the
//! `BackendLedger` port over the store, handed to a driver at open.
//!
//! The subject never reaches a driver. Each row carries `operation_digest`, the length-prefixed
//! SHA-256 of subject and operation id, which is all an object's annotation needs to tie it to its
//! operation.

use std::fmt::Write as _;
use std::sync::Arc;

use chrono::Utc;
use sha2::{Digest as _, Sha256};
use substrate_host::{
    BackendKind, BackendLedger, BackendName, BackendRecord, BackendRole, Driver, DriverError,
    UidBinding,
};
use substrate_store::{
    BackendObject, BackendObjectKind, BackendObjectName, BackendObjectRole, BackendReservation,
    BackendUidBinding, Store, StoreError,
};

/// How many unsettled rows one [`BackendLedger::unsettled`] call returns, oldest first.
pub const UNSETTLED_PAGE: usize = 256;

/// `sha256(u32be(len) ‖ subject ‖ u32be(len) ‖ operation id)` as lowercase hex.
#[must_use]
pub fn operation_digest(subject: &str, operation: &str) -> String {
    let mut hasher = Sha256::new();
    for part in [subject, operation] {
        let length = u32::try_from(part.len()).unwrap_or(u32::MAX);
        hasher.update(length.to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// The names `driver` declares for a workspace, ready for the `accepted` transaction. Empty for a
/// driver that declares none.
pub(crate) fn workspace_reservation(
    driver: &dyn Driver,
    id: &str,
    subject: &str,
    operation: &str,
) -> BackendReservation {
    reservation(driver, BackendKind::Workspace, id, subject, operation)
}

/// The names `driver` declares for an exec, ready for the `accepted` transaction.
pub(crate) fn exec_reservation(
    driver: &dyn Driver,
    id: &str,
    subject: &str,
    operation: &str,
) -> BackendReservation {
    reservation(driver, BackendKind::Exec, id, subject, operation)
}

fn reservation(
    driver: &dyn Driver,
    kind: BackendKind,
    id: &str,
    subject: &str,
    operation: &str,
) -> BackendReservation {
    let names = driver.backend_names(kind, id);
    if names.is_empty() {
        return BackendReservation::default();
    }
    BackendReservation {
        operation_digest: operation_digest(subject, operation),
        objects: names.iter().map(store_name).collect(),
    }
}

fn store_name(name: &BackendName) -> BackendObjectName {
    BackendObjectName {
        role: store_role(name.role),
        name: name.name.clone(),
    }
}

const fn store_kind(kind: BackendKind) -> BackendObjectKind {
    match kind {
        BackendKind::Workspace => BackendObjectKind::Workspace,
        BackendKind::Exec => BackendObjectKind::Exec,
        BackendKind::Probe => BackendObjectKind::Probe,
    }
}

const fn port_kind(kind: BackendObjectKind) -> BackendKind {
    match kind {
        BackendObjectKind::Workspace => BackendKind::Workspace,
        BackendObjectKind::Exec => BackendKind::Exec,
        BackendObjectKind::Probe => BackendKind::Probe,
    }
}

const fn store_role(role: BackendRole) -> BackendObjectRole {
    match role {
        BackendRole::Pvc => BackendObjectRole::Pvc,
        BackendRole::Keeper => BackendObjectRole::Keeper,
        BackendRole::Pod => BackendObjectRole::Pod,
    }
}

const fn port_role(role: BackendObjectRole) -> BackendRole {
    match role {
        BackendObjectRole::Pvc => BackendRole::Pvc,
        BackendObjectRole::Keeper => BackendRole::Keeper,
        BackendObjectRole::Pod => BackendRole::Pod,
    }
}

fn port_record(row: BackendObject) -> BackendRecord {
    BackendRecord {
        kind: port_kind(row.kind),
        resource_id: row.resource_id,
        role: port_role(row.role),
        name: row.name,
        operation_digest: row.operation_digest,
        uid: row.uid,
        created_at: row.created_at,
        settled_at: row.settled_at,
    }
}

fn ledger_failure(error: &StoreError) -> DriverError {
    tracing::error!(%error, "backend ledger failure");
    DriverError::failed("state.store-failed", "Durable state operation failed.")
}

/// The `BackendLedger` port over the store, scoped to one deployment.
pub struct StoreBackendLedger {
    store: Arc<Store>,
    deployment: String,
}

impl StoreBackendLedger {
    #[must_use]
    pub fn new(store: Arc<Store>, deployment: impl Into<String>) -> Self {
        Self {
            store,
            deployment: deployment.into(),
        }
    }
}

impl BackendLedger for StoreBackendLedger {
    fn recorded(&self, kind: BackendKind, id: &str) -> Result<Vec<BackendRecord>, DriverError> {
        self.store
            .backend_objects(&self.deployment, store_kind(kind), id)
            .map(|rows| rows.into_iter().map(port_record).collect())
            .map_err(|error| ledger_failure(&error))
    }

    fn bind_uid(&self, name: &str, uid: &str) -> Result<UidBinding, DriverError> {
        self.store
            .bind_backend_uid(&self.deployment, name, uid)
            .map(|binding| match binding {
                BackendUidBinding::Bound => UidBinding::Bound,
                BackendUidBinding::Collision { bound } => UidBinding::Collision { bound },
                BackendUidBinding::Unrecorded => UidBinding::Unrecorded,
                BackendUidBinding::Settled => UidBinding::Settled,
            })
            .map_err(|error| ledger_failure(&error))
    }

    fn settle(&self, name: &str) -> Result<bool, DriverError> {
        self.store
            .settle_backend_object(&self.deployment, name, Utc::now())
            .map_err(|error| ledger_failure(&error))
    }

    fn reserve_probe(&self, nonce: &str, names: &[BackendName]) -> Result<(), DriverError> {
        let reservation = BackendReservation {
            operation_digest: hex(&Sha256::digest(nonce.as_bytes())),
            objects: names.iter().map(store_name).collect(),
        };
        self.store
            .reserve_backend_probe(&self.deployment, nonce, Utc::now(), &reservation)
            .map_err(|error| ledger_failure(&error))
    }

    fn unsettled(&self) -> Result<Vec<BackendRecord>, DriverError> {
        self.store
            .unsettled_backend_objects(&self.deployment, UNSETTLED_PAGE)
            .map(|rows| rows.into_iter().map(port_record).collect())
            .map_err(|error| ledger_failure(&error))
    }
}
