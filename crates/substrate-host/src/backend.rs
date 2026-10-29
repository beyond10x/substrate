//! The backend ledger port (design 34 § 4).
//!
//! A driver that creates objects in a backend it does not own, such as a Kubernetes namespace,
//! names them through [`crate::Driver::backend_names`]. The daemon reserves those names in the
//! transaction that records the operation's `accepted` state, before any dispatch (ADR 0005), and
//! hands the driver a [`BackendLedger`] at open to bind, read and settle them. The driver never
//! sees the subject: each row carries only the operation's digest.
//!
//! The host driver names nothing, so it writes no row and nothing it answers changes.

use chrono::{DateTime, Utc};

use crate::DriverError;

/// Which resource a backend object serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    Workspace,
    Exec,
    Probe,
}

/// What a backend object is to its resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendRole {
    Pvc,
    Keeper,
    Pod,
}

/// One object a driver will create, named before the call that creates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendName {
    pub role: BackendRole,
    pub name: String,
}

/// One ledger row, as the driver reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendRecord {
    pub kind: BackendKind,
    pub resource_id: String,
    pub role: BackendRole,
    pub name: String,
    /// Lowercase hex of `sha256(subject ‖ operation id)`, length-prefixed; for a probe, of its
    /// nonce. What the object's operation annotation carries.
    pub operation_digest: String,
    /// The uid the create answered, once committed. `None` means no create answer was committed:
    /// the object may never have been stored.
    pub uid: Option<String>,
    pub created_at: DateTime<Utc>,
    /// When the object was read absent and the row closed.
    pub settled_at: Option<DateTime<Utc>>,
}

/// What [`BackendLedger::bind_uid`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UidBinding {
    /// The row's bound uid is this one: newly bound, or bound to it before.
    Bound,
    /// The row is bound to a different uid and keeps it: a collision (ADR 0047 decision 4).
    Collision { bound: String },
    /// No row names the object.
    Unrecorded,
    /// The row was settled; it binds nothing again.
    Settled,
}

/// The daemon's durable record of a driver's backend objects, scoped to one deployment.
///
/// Each call is one store transaction, committed before it returns.
pub trait BackendLedger: Send + Sync {
    /// Every row of one resource, settled or not.
    ///
    /// # Errors
    ///
    /// Returns a failure when the store cannot be read.
    fn recorded(&self, kind: BackendKind, id: &str) -> Result<Vec<BackendRecord>, DriverError>;

    /// Commits `uid` as the backend identity of `name`, unless another uid is already bound.
    ///
    /// # Errors
    ///
    /// Returns a failure when the store cannot be written.
    fn bind_uid(&self, name: &str, uid: &str) -> Result<UidBinding, DriverError>;

    /// Closes the row of `name`: its object read absent. `false` when no row names it.
    ///
    /// # Errors
    ///
    /// Returns a failure when the store cannot be written.
    fn settle(&self, name: &str) -> Result<bool, DriverError>;

    /// Reserves the objects of one probe, keyed by its nonce, before any of them is created.
    ///
    /// # Errors
    ///
    /// Returns a failure when the store cannot be written or a name is already reserved.
    fn reserve_probe(&self, nonce: &str, names: &[BackendName]) -> Result<(), DriverError>;

    /// The oldest rows that are not settled, at most one bounded page.
    ///
    /// # Errors
    ///
    /// Returns a failure when the store cannot be read.
    fn unsettled(&self) -> Result<Vec<BackendRecord>, DriverError>;
}
