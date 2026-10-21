//! Host-process records (ADR 0036, design 26 § S1, S4, S5, S6 and § S7 retention).
//!
//! A host process is a supervised, unconfined process the daemon starts for its subject. This
//! module keeps only its durable record: who asked for it, why, under which lease or keep, with
//! which deadline, in which lifecycle state, and how it ended. Spawning, cgroups, logs and
//! observation belong to the host crate; routes and wire types to the daemon and the successor
//! bundle.
//!
//! The subject is the only authority. Every read and write is keyed by `(deployment, subject)`,
//! and another subject's record answers exactly as an absent one does. The owner session is a
//! label beneath the subject, never an identity.
//!
//! Environment values are secrets in practice, so the record keeps the variable names and the
//! caller's digest of the whole map, never a value (design 26 § S1).

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, TimeZone as _, Utc};
use rusqlite::{Connection, OptionalExtension as _, Row, TransactionBehavior, params};
use substrate_wire::{
    ErrorClass, ErrorDetail, MAX_LEASE_TTL_MS, MIN_LEASE_TTL_MS, OperationOutcome,
};

use crate::events::{append_event, commit_effect};
use crate::operations::{
    existing_reservation, finalize_operation_accounting, insert_accepted_operation,
    insert_refused_operation, operation_identity,
};
use crate::{
    NewOperation, OperationCapacity, Reservation, Scope, Store, StoreError, StoredAnswer, to_i64,
    to_u64,
};

/// The one profile a host process runs under (design 26 § S2).
pub const HOST_PROCESS_PROFILE: &str = "host";

/// What a host process record always reports about its confinement (design 26 § S2).
pub const HOST_PROCESS_CONFINEMENT: &str = "none";

/// The operation kind that starts a host process.
pub const HOST_PROCESS_START_KIND: &str = "process.start";

/// The operation kind that stops one host process, by caller request or by the supervisor.
pub const HOST_PROCESS_STOP_KIND: &str = "process.stop";

/// The operation kind that renews a host process lease.
pub const HOST_PROCESS_RENEW_KIND: &str = "process.lease.renew";

/// The actor of operations the daemon originates for lease expiry and deadlines (design 26 § S4).
pub const HOST_PROCESS_SUPERVISOR_ACTOR: &str = "process-supervisor";

/// Default bound on non-terminal records per subject (`process.limit-exceeded`).
pub const DEFAULT_HOST_PROCESS_SUBJECT_MAX_LIVE: u64 = substrate_wire::MAX_PROCESSES_PER_SUBJECT;

/// Default upper bound of `timeout_ms`, and of how far ahead `expires_at` may lie.
pub const DEFAULT_MAX_PROCESS_TIMEOUT_MS: u64 = substrate_wire::MAX_PROCESS_TIMEOUT_MS;

/// Default time an ended record is retained.
pub const DEFAULT_HOST_PROCESS_RETAIN_ENDED_MS: u64 = substrate_wire::PROCESS_RETAIN_ENDED_MS;

/// Default number of ended records retained per subject.
pub const DEFAULT_HOST_PROCESS_RETAIN_ENDED_COUNT: u64 = substrate_wire::PROCESS_RETAIN_ENDED_COUNT;

/// Default and largest page a listing returns.
#[allow(clippy::cast_lossless)] // `u64::from` is not const; widening u32 to u64 loses nothing.
pub const DEFAULT_HOST_PROCESS_MAX_PAGE: u64 = substrate_wire::MAX_PROCESS_LIST_ITEMS as u64;

/// The bounds this module enforces. Each becomes a wire constant of the successor bundle that
/// carries the host-process family (design 26 § named refusals); until then they are store
/// configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostProcessLimits {
    pub subject_max_live: u64,
    pub max_timeout_ms: u64,
    pub retain_ended_ms: u64,
    pub retain_ended_count: u64,
    pub max_page: u64,
}

impl Default for HostProcessLimits {
    fn default() -> Self {
        Self {
            subject_max_live: DEFAULT_HOST_PROCESS_SUBJECT_MAX_LIVE,
            max_timeout_ms: DEFAULT_MAX_PROCESS_TIMEOUT_MS,
            retain_ended_ms: DEFAULT_HOST_PROCESS_RETAIN_ENDED_MS,
            retain_ended_count: DEFAULT_HOST_PROCESS_RETAIN_ENDED_COUNT,
            max_page: DEFAULT_HOST_PROCESS_MAX_PAGE,
        }
    }
}

/// The named refusals this module decides, each with the code and status design 26 gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProcessRefusal {
    OwnerInvalid,
    ReasonRequired,
    ArgvInvalid,
    CwdInvalid,
    EnvInvalid,
    LeaseInvalid,
    KeepReasonRequired,
    TimeoutInvalid,
    ExpiresAtInvalid,
    NameConflict,
    LimitExceeded,
    NotFound,
}

impl HostProcessRefusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::OwnerInvalid => substrate_wire::PROCESS_OWNER_INVALID,
            Self::ReasonRequired => substrate_wire::PROCESS_REASON_REQUIRED,
            Self::ArgvInvalid => substrate_wire::PROCESS_ARGV_INVALID,
            Self::CwdInvalid => substrate_wire::PROCESS_CWD_INVALID,
            Self::EnvInvalid => substrate_wire::PROCESS_ENV_INVALID,
            Self::LeaseInvalid => substrate_wire::PROCESS_LEASE_INVALID,
            Self::KeepReasonRequired => substrate_wire::PROCESS_KEEP_REASON_REQUIRED,
            Self::TimeoutInvalid => substrate_wire::PROCESS_TIMEOUT_INVALID,
            Self::ExpiresAtInvalid => substrate_wire::PROCESS_EXPIRES_AT_INVALID,
            Self::NameConflict => substrate_wire::PROCESS_NAME_CONFLICT,
            Self::LimitExceeded => substrate_wire::PROCESS_LIMIT_EXCEEDED,
            Self::NotFound => substrate_wire::PROCESS_NOT_FOUND,
        }
    }

    pub fn status(self) -> u16 {
        match self {
            Self::NameConflict => 409,
            Self::LimitExceeded => 429,
            Self::NotFound => 404,
            _ => 422,
        }
    }
}

/// A start request's lease: a TTL, or a keep with its own reason (design 26 § S4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostProcessLeaseRequest {
    Ttl { ttl_ms: u64 },
    Keep { reason: String },
}

/// What a start records. `id` is minted by the caller, as workspace and exec ids are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewHostProcess {
    pub id: String,
    pub owner_session: String,
    pub owner_name: String,
    pub reason: String,
    pub argv: Vec<String>,
    pub cwd: String,
    /// The names of the environment the process receives; never a value.
    pub env_names: Vec<String>,
    /// `sha256:<64 lowercase hex>` over the caller's canonical form of the whole map.
    pub env_digest: String,
    pub lease: HostProcessLeaseRequest,
    pub timeout_ms: Option<u64>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProcessState {
    Starting,
    Running,
    Stopping,
    Ended,
}

impl HostProcessState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Ended => "ended",
        }
    }
}

/// The one cause an ended record names (design 26 § S3, S11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProcessEndedBy {
    Exit,
    Stop,
    LeaseExpiry,
    Timeout,
    ExpiresAt,
    Vanished,
}

impl HostProcessEndedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::Stop => "stop",
            Self::LeaseExpiry => "lease-expiry",
            Self::Timeout => "timeout",
            Self::ExpiresAt => "expires-at",
            Self::Vanished => "vanished",
        }
    }
}

/// Which start field set the effective deadline (design 26 § S11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProcessDeadlineSource {
    Timeout,
    ExpiresAt,
}

/// The main process's exit, as a code or as the terminating signal, never both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostProcessExit {
    Code(i32),
    Signal(String),
}

/// The record's current lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostProcessLease {
    Ttl {
        ttl_ms: u64,
        /// The start or renewal instant the expiry is computed from.
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    },
    Keep {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProcessDeadline {
    pub at: DateTime<Utc>,
    pub source: HostProcessDeadlineSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProcessRecord {
    pub id: String,
    pub owner_session: String,
    pub owner_name: String,
    pub reason: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub env_names: Vec<String>,
    pub env_digest: String,
    pub profile: String,
    pub confinement: String,
    pub lease: HostProcessLease,
    pub timeout_ms: Option<u64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub deadline: Option<HostProcessDeadline>,
    pub state: HostProcessState,
    pub ended_by: Option<HostProcessEndedBy>,
    pub exit: Option<HostProcessExit>,
    pub start_operation: String,
    pub started_at: DateTime<Utc>,
    pub running_at: Option<DateTime<Utc>>,
    pub stopping_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    /// The daemon run that started this record ([`Store::set_host_process_run`]). Absent on a
    /// record written before runs were recorded, which only an earlier run can have written.
    pub run: Option<String>,
}

/// The answer to a start reservation.
#[derive(Debug, Clone, PartialEq)]
pub enum HostProcessStartReservation {
    /// The operation id was seen before: replay, conflict or pending.
    Existing(Reservation),
    /// The operation ledger is full; nothing was recorded.
    Capacity(OperationCapacity),
    /// A named refusal, recorded durably as a refused operation.
    Refused {
        refusal: HostProcessRefusal,
        answer: StoredAnswer,
    },
    /// The operation is accepted and the record is `starting`, both durable before dispatch.
    Accepted(Box<HostProcessRecord>),
}

/// The answer to a stop reservation.
#[derive(Debug, Clone, PartialEq)]
pub enum HostProcessStopReservation {
    Existing(Reservation),
    Capacity(OperationCapacity),
    /// `process.not-found`, recorded durably as a refused operation.
    Refused {
        refusal: HostProcessRefusal,
        answer: StoredAnswer,
    },
    /// The operation is accepted and the record moved to `stopping` with its `ended_by`.
    Accepted(Box<HostProcessRecord>),
    /// The record is already stopping or ended; its `ended_by` stands and nothing was recorded.
    AlreadyEnding(Box<HostProcessRecord>),
    /// A supervisor cause whose instant has not been reached (a renewal raced it); nothing was
    /// recorded.
    NotDue(Box<HostProcessRecord>),
}

/// The answer to a lease renewal. The renewal operation stays accepted unless `Renewed`.
#[derive(Debug, Clone, PartialEq)]
pub enum HostProcessRenewal {
    Renewed(Box<HostProcessRecord>),
    Refused(HostProcessRefusal),
    /// The record is stopping or ended.
    NotLive(Box<HostProcessRecord>),
    /// The lease instant has passed; the supervisor's expiry, not a renewal, decides it.
    Expired(Box<HostProcessRecord>),
}

/// The answer to a supervisor transition.
#[derive(Debug, Clone, PartialEq)]
pub enum HostProcessTransition {
    Applied(Box<HostProcessRecord>),
    /// The record is not in a state the transition moves from; nothing changed.
    Unchanged(Box<HostProcessRecord>),
    Missing,
}

/// How the supervisor saw a record's cgroup empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostProcessEnd {
    Exit(HostProcessExit),
    Vanished,
}

/// One record whose lease or deadline the supervisor clock has reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueHostProcess {
    pub scope: Scope,
    pub id: String,
    pub cause: HostProcessEndedBy,
    pub due_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostProcessFilter {
    pub owner_session: Option<String>,
    pub name: Option<String>,
    pub state: Option<HostProcessState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProcessPage {
    pub items: Vec<HostProcessRecord>,
    /// The id to pass as `after` for the next page, when there is one.
    pub next_after: Option<String>,
}

impl HostProcessRefusal {
    fn class(self) -> ErrorClass {
        match self {
            Self::NameConflict => ErrorClass::Conflict,
            Self::LimitExceeded => ErrorClass::Exhausted,
            _ => ErrorClass::Refused,
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::OwnerInvalid => "The owner session and owner name are both required and bounded.",
            Self::ReasonRequired => "A host process requires a reason.",
            Self::ArgvInvalid => {
                "argv must be a non-empty program and arguments without NUL bytes."
            }
            Self::CwdInvalid => "cwd must be an absolute path.",
            Self::EnvInvalid => {
                "The environment names must be unique, non-empty and contain no '=' or NUL, and its digest must be sha256."
            }
            Self::LeaseInvalid => "The lease TTL is outside the lease bounds.",
            Self::KeepReasonRequired => "A keep lease requires its own keep reason.",
            Self::TimeoutInvalid => {
                "timeout_ms must be positive and within the process timeout bound."
            }
            Self::ExpiresAtInvalid => {
                "expires_at must be at least the minimum lease TTL ahead and within the process timeout bound."
            }
            Self::NameConflict => {
                "Another live host process of this owner session already carries this name."
            }
            Self::LimitExceeded => {
                "The subject holds the maximum number of live host processes; stop one before retrying with a new operation id."
            }
            Self::NotFound => "Host process was not found.",
        }
    }

    fn address(self) -> &'static str {
        match self {
            Self::OwnerInvalid | Self::NameConflict => "owner",
            Self::ReasonRequired => "reason",
            Self::ArgvInvalid => "argv",
            Self::CwdInvalid => "cwd",
            Self::EnvInvalid => "env",
            Self::LeaseInvalid | Self::KeepReasonRequired => "lease",
            Self::TimeoutInvalid => "timeout_ms",
            Self::ExpiresAtInvalid => "expires_at",
            Self::LimitExceeded | Self::NotFound => "process",
        }
    }

    /// The error detail a caller records for this refusal on the named operation.
    pub fn error_detail(self, operation: &str) -> ErrorDetail {
        ErrorDetail {
            class: self.class(),
            code: self.code().to_owned(),
            // The wire's message for the code where it publishes one, so a stored refusal and a
            // daemon-side one read the same (PROCESS_REFUSALS).
            message: substrate_wire::process_refusal(self.code())
                .map_or(self.message(), |refusal| refusal.message)
                .to_owned(),
            retriable: false,
            address: Some(self.address().to_owned()),
            operation: Some(operation.to_owned()),
        }
    }
}

const COLUMNS: &str = "subject, id, owner_session, owner_name, reason, argv_json, cwd,
    env_names_json, env_digest, profile, confinement, lease_ttl_ms, lease_issued_ms,
    lease_expires_ms, keep_reason, timeout_ms, expires_at_ms, deadline_ms, deadline_source, state,
    ended_by, exit_code, exit_signal, start_operation, started_ms, running_ms, stopping_ms, ended_ms,
    run_id";

/// Selected rows of a single subject are bounded by the live bound; this caps the owner query.
const MAX_OWNER_SELECTION: i64 = 4_096;

impl Store {
    /// Names the daemon run every host-process record started from now on belongs to. The daemon
    /// sets it once, when it is constructed, to an identity no earlier run used.
    ///
    /// Which run a record belongs to is decided by this identity and never by comparing wall
    /// instants: a wall clock stepped back across a restart puts an earlier run's `started_at` at
    /// or after the next run's start, and one stepped back inside a run puts this run's before it
    /// (ADR 0036 S3).
    pub fn set_host_process_run(&self, run: impl Into<String>) {
        *self.host_process_run.write() = Some(run.into());
    }

    /// Records a start durably before dispatch: either a refused operation naming the refusal,
    /// or an accepted operation together with a `starting` record, in one transaction.
    pub fn reserve_host_process_start(
        &self,
        new: &NewOperation,
        process: &NewHostProcess,
        limits: &HostProcessLimits,
    ) -> Result<HostProcessStartReservation, StoreError> {
        if new.operation_kind != HOST_PROCESS_START_KIND {
            return Err(StoreError::HostProcessInvariant("start operation kind"));
        }
        if process.id.trim().is_empty() || new.resource.as_deref() != Some(process.id.as_str()) {
            return Err(StoreError::HostProcessInvariant("start operation resource"));
        }
        let now: DateTime<Utc> = new.accepted_at.parse()?;
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(reservation) = existing_reservation(&transaction, new)? {
            transaction.commit()?;
            return Ok(HostProcessStartReservation::Existing(reservation));
        }
        let decided = match validate_start(process, now, limits) {
            Err(refusal) => Err(refusal),
            Ok(deadline) => {
                if live_name_taken(&transaction, &new.scope, process)? {
                    Err(HostProcessRefusal::NameConflict)
                } else if live_bound_reached(&transaction, &new.scope, limits.subject_max_live)? {
                    Err(HostProcessRefusal::LimitExceeded)
                } else {
                    Ok(deadline)
                }
            }
        };
        let deadline = match decided {
            Ok(deadline) => deadline,
            Err(refusal) => {
                let detail = refusal.error_detail(&new.operation);
                return match insert_refused_operation(
                    &transaction,
                    self.event_retention,
                    self.config,
                    new,
                    &new.accepted_at,
                    refusal.status(),
                    &detail,
                ) {
                    Ok((answer, event)) => {
                        transaction.commit()?;
                        drop(connection);
                        self.report_committed(&[commit_effect(&new.scope, &event)]);
                        Ok(HostProcessStartReservation::Refused { refusal, answer })
                    }
                    Err(StoreError::OperationCapacity(capacity)) => {
                        transaction.rollback()?;
                        Ok(HostProcessStartReservation::Capacity(capacity))
                    }
                    Err(error) => Err(error),
                };
            }
        };
        let event =
            match insert_accepted_operation(&transaction, self.event_retention, self.config, new) {
                Ok(event) => event,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(HostProcessStartReservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
        let lease = issued_lease(&process.lease, now)?;
        let record = HostProcessRecord {
            id: process.id.clone(),
            owner_session: process.owner_session.clone(),
            owner_name: process.owner_name.clone(),
            reason: process.reason.clone(),
            argv: process.argv.clone(),
            cwd: process.cwd.clone(),
            env_names: process.env_names.clone(),
            env_digest: process.env_digest.clone(),
            profile: HOST_PROCESS_PROFILE.to_owned(),
            confinement: HOST_PROCESS_CONFINEMENT.to_owned(),
            lease,
            timeout_ms: process.timeout_ms,
            expires_at: process.expires_at,
            deadline,
            state: HostProcessState::Starting,
            ended_by: None,
            exit: None,
            start_operation: new.operation.clone(),
            started_at: now,
            running_at: None,
            stopping_at: None,
            ended_at: None,
            run: self.host_process_run.read().clone(),
        };
        insert_record(&transaction, &new.scope, &record)?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(&new.scope, &event)]);
        Ok(HostProcessStartReservation::Accepted(Box::new(record)))
    }

    /// The supervisor saw the record's cgroup populated: `starting` becomes `running`.
    pub fn mark_host_process_running(
        &self,
        scope: &Scope,
        id: &str,
        at: DateTime<Utc>,
    ) -> Result<HostProcessTransition, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(record) = load_record(&transaction, scope, id)? else {
            return Ok(HostProcessTransition::Missing);
        };
        if record.state != HostProcessState::Starting {
            return Ok(HostProcessTransition::Unchanged(Box::new(record)));
        }
        transaction.execute(
            "UPDATE host_processes SET state = 'running', running_ms = ?4
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3 AND state = 'starting'",
            params![scope.deployment, scope.subject, id, at.timestamp_millis()],
        )?;
        let record = reload(&transaction, scope, id)?;
        transaction.commit()?;
        Ok(HostProcessTransition::Applied(Box::new(record)))
    }

    /// Renews a live record's lease from `terminal_at`, completing the already reserved renewal
    /// operation in the same transaction. A keep may become a TTL; a TTL never becomes a keep.
    /// Every other answer leaves the operation accepted for the caller to conclude.
    pub fn renew_host_process_lease(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: DateTime<Utc>,
        status: u16,
        id: &str,
        ttl_ms: u64,
    ) -> Result<HostProcessRenewal, StoreError> {
        self.renew_host_process_lease_with(
            scope,
            operation,
            terminal_at,
            status,
            id,
            ttl_ms,
            |record| {
                let HostProcessLease::Ttl { expires_at, .. } = &record.lease else {
                    return Err(StoreError::HostProcessInvariant("renewed lease is a TTL"));
                };
                Ok(serde_json::json!({
                    "id": record.id,
                    "lease": { "ttl_ms": ttl_ms, "renew_by": expires_at.to_rfc3339() },
                }))
            },
        )
    }

    /// [`Store::renew_host_process_lease`], with the operation's recorded result built from the
    /// renewed record inside the same transaction, so the answer a replay returns is exactly the
    /// answer the renewal returned (the daemon records the wire record).
    #[allow(clippy::too_many_arguments)] // One atomic renewal and ledger terminal boundary.
    pub fn renew_host_process_lease_with(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: DateTime<Utc>,
        status: u16,
        id: &str,
        ttl_ms: u64,
        result: impl FnOnce(&HostProcessRecord) -> Result<serde_json::Value, StoreError>,
    ) -> Result<HostProcessRenewal, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(record) = load_record(&transaction, scope, id)? else {
            return Ok(HostProcessRenewal::Refused(HostProcessRefusal::NotFound));
        };
        if !(MIN_LEASE_TTL_MS..=MAX_LEASE_TTL_MS).contains(&ttl_ms) {
            return Ok(HostProcessRenewal::Refused(
                HostProcessRefusal::LeaseInvalid,
            ));
        }
        if matches!(
            record.state,
            HostProcessState::Stopping | HostProcessState::Ended
        ) {
            return Ok(HostProcessRenewal::NotLive(Box::new(record)));
        }
        if let HostProcessLease::Ttl { expires_at, .. } = &record.lease
            && terminal_at >= *expires_at
        {
            return Ok(HostProcessRenewal::Expired(Box::new(record)));
        }
        let expires_at = add_ms(terminal_at, ttl_ms)?;
        transaction.execute(
            "UPDATE host_processes
             SET lease_ttl_ms = ?4, lease_issued_ms = ?5, lease_expires_ms = ?6, keep_reason = NULL
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
            params![
                scope.deployment,
                scope.subject,
                id,
                to_i64(ttl_ms)?,
                terminal_at.timestamp_millis(),
                expires_at.timestamp_millis(),
            ],
        )?;
        let record = reload(&transaction, scope, id)?;
        let result = result(&record)?;
        let event = complete_renewal(
            &transaction,
            self,
            scope,
            operation,
            terminal_at,
            status,
            id,
            result,
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(HostProcessRenewal::Renewed(Box::new(record)))
    }

    /// Records a stop durably before the driver acts: an accepted operation and the record in
    /// `stopping` naming `cause`, in one transaction. The first recorded cause stands.
    pub fn reserve_host_process_stop(
        &self,
        new: &NewOperation,
        id: &str,
        cause: HostProcessEndedBy,
    ) -> Result<HostProcessStopReservation, StoreError> {
        if new.operation_kind != HOST_PROCESS_STOP_KIND {
            return Err(StoreError::HostProcessInvariant("stop operation kind"));
        }
        if matches!(
            cause,
            HostProcessEndedBy::Exit | HostProcessEndedBy::Vanished
        ) {
            return Err(StoreError::HostProcessInvariant("stop cause"));
        }
        let now: DateTime<Utc> = new.accepted_at.parse()?;
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(reservation) = existing_reservation(&transaction, new)? {
            transaction.commit()?;
            return Ok(HostProcessStopReservation::Existing(reservation));
        }
        let Some(record) = load_record(&transaction, &new.scope, id)? else {
            let refusal = HostProcessRefusal::NotFound;
            let detail = refusal.error_detail(&new.operation);
            return match insert_refused_operation(
                &transaction,
                self.event_retention,
                self.config,
                new,
                &new.accepted_at,
                refusal.status(),
                &detail,
            ) {
                Ok((answer, event)) => {
                    transaction.commit()?;
                    drop(connection);
                    self.report_committed(&[commit_effect(&new.scope, &event)]);
                    Ok(HostProcessStopReservation::Refused { refusal, answer })
                }
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    Ok(HostProcessStopReservation::Capacity(capacity))
                }
                Err(error) => Err(error),
            };
        };
        if matches!(
            record.state,
            HostProcessState::Stopping | HostProcessState::Ended
        ) {
            return Ok(HostProcessStopReservation::AlreadyEnding(Box::new(record)));
        }
        if !cause_due(&record, cause, now) {
            return Ok(HostProcessStopReservation::NotDue(Box::new(record)));
        }
        let event =
            match insert_accepted_operation(&transaction, self.event_retention, self.config, new) {
                Ok(event) => event,
                Err(StoreError::OperationCapacity(capacity)) => {
                    transaction.rollback()?;
                    return Ok(HostProcessStopReservation::Capacity(capacity));
                }
                Err(error) => return Err(error),
            };
        transaction.execute(
            "UPDATE host_processes SET state = 'stopping', ended_by = ?4, stopping_ms = ?5
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3
               AND state IN ('starting','running')",
            params![
                new.scope.deployment,
                new.scope.subject,
                id,
                cause.as_str(),
                now.timestamp_millis()
            ],
        )?;
        let record = reload(&transaction, &new.scope, id)?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(&new.scope, &event)]);
        Ok(HostProcessStopReservation::Accepted(Box::new(record)))
    }

    /// The supervisor saw the record's cgroup empty. A live record ends by `exit` or `vanished`;
    /// a stopping one keeps the cause its stop recorded. The exit status is kept when observed.
    pub fn record_host_process_end(
        &self,
        scope: &Scope,
        id: &str,
        at: DateTime<Utc>,
        end: &HostProcessEnd,
    ) -> Result<HostProcessTransition, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(record) = load_record(&transaction, scope, id)? else {
            return Ok(HostProcessTransition::Missing);
        };
        let ended_by = match (record.state, end, record.ended_by) {
            (HostProcessState::Ended, _, _) => {
                return Ok(HostProcessTransition::Unchanged(Box::new(record)));
            }
            (HostProcessState::Stopping, _, Some(cause)) => cause,
            (HostProcessState::Stopping, _, None) => {
                return Err(StoreError::HostProcessInvariant("stopping without a cause"));
            }
            (_, HostProcessEnd::Exit(_), _) => HostProcessEndedBy::Exit,
            (_, HostProcessEnd::Vanished, _) => HostProcessEndedBy::Vanished,
        };
        let (exit_code, exit_signal) = match end {
            HostProcessEnd::Exit(HostProcessExit::Code(code)) => (Some(*code), None),
            HostProcessEnd::Exit(HostProcessExit::Signal(signal)) => (None, Some(signal.clone())),
            HostProcessEnd::Vanished => (None, None),
        };
        transaction.execute(
            "UPDATE host_processes
             SET state = 'ended', ended_by = ?4, exit_code = ?5, exit_signal = ?6, ended_ms = ?7
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3 AND state != 'ended'",
            params![
                scope.deployment,
                scope.subject,
                id,
                ended_by.as_str(),
                exit_code,
                exit_signal,
                at.timestamp_millis(),
            ],
        )?;
        let record = reload(&transaction, scope, id)?;
        transaction.commit()?;
        Ok(HostProcessTransition::Applied(Box::new(record)))
    }

    /// The live records of one deployment whose lease or deadline `now` has reached, earliest
    /// first, at most `limit`. The supervisor clock stops each through
    /// [`Store::reserve_host_process_stop`] with the named cause.
    pub fn due_host_processes(
        &self,
        deployment: &str,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<DueHostProcess>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT subject, id, lease_expires_ms, deadline_ms, deadline_source
             FROM host_processes
             WHERE deployment = ?1 AND state IN ('starting','running')
               AND ((lease_expires_ms IS NOT NULL AND lease_expires_ms <= ?2)
                 OR (deadline_ms IS NOT NULL AND deadline_ms <= ?2))
             ORDER BY min(coalesce(lease_expires_ms, deadline_ms),
                          coalesce(deadline_ms, lease_expires_ms)), subject, id
             LIMIT ?3",
        )?;
        let now_ms = now.timestamp_millis();
        let rows = statement
            .query_map(params![deployment, now_ms, to_i64(limit)?], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(subject, id, lease_ms, deadline_ms, source)| {
                let deadline_due = deadline_ms
                    .filter(|due| *due <= now_ms && lease_ms.is_none_or(|lease| *due <= lease));
                let (cause, due_ms) = if let Some(due) = deadline_due {
                    (parse_deadline_cause(source.as_deref())?, due)
                } else {
                    (
                        HostProcessEndedBy::LeaseExpiry,
                        lease_ms.ok_or(StoreError::HostProcessInvariant("due without instant"))?,
                    )
                };
                Ok(DueHostProcess {
                    scope: Scope {
                        deployment: deployment.to_owned(),
                        subject,
                    },
                    id,
                    cause,
                    due_at: from_ms(due_ms)?,
                })
            })
            .collect()
    }

    /// Every non-terminal record of `(subject, owner session)`, for stop by owner (design 26
    /// § S5). `except_keep` leaves records held under `keep` out.
    pub fn host_processes_for_owner(
        &self,
        scope: &Scope,
        owner_session: &str,
        except_keep: bool,
    ) -> Result<Vec<String>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT id FROM host_processes
             WHERE deployment = ?1 AND subject = ?2 AND owner_session = ?3 AND state != 'ended'
               AND (?4 = 0 OR keep_reason IS NULL)
             ORDER BY id LIMIT ?5",
        )?;
        statement
            .query_map(
                params![
                    scope.deployment,
                    scope.subject,
                    owner_session,
                    except_keep,
                    MAX_OWNER_SELECTION
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// One record of the subject; another subject's record answers as absent.
    pub fn host_process(
        &self,
        scope: &Scope,
        id: &str,
    ) -> Result<Option<HostProcessRecord>, StoreError> {
        let connection = self.connection.lock();
        load_record(&connection, scope, id)
    }

    /// The subject's records in id order, filtered, in pages of at most `limits.max_page`.
    pub fn list_host_processes(
        &self,
        scope: &Scope,
        filter: &HostProcessFilter,
        after: Option<&str>,
        limit: u64,
        limits: &HostProcessLimits,
    ) -> Result<HostProcessPage, StoreError> {
        let page = limit.clamp(1, limits.max_page.max(1));
        let connection = self.connection.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {COLUMNS} FROM host_processes
             WHERE deployment = ?1 AND subject = ?2
               AND (?3 IS NULL OR owner_session = ?3)
               AND (?4 IS NULL OR owner_name = ?4)
               AND (?5 IS NULL OR state = ?5)
               AND (?6 IS NULL OR id > ?6)
             ORDER BY id LIMIT ?7"
        ))?;
        let raws = statement
            .query_map(
                params![
                    scope.deployment,
                    scope.subject,
                    filter.owner_session,
                    filter.name,
                    filter.state.map(HostProcessState::as_str),
                    after,
                    to_i64(page.saturating_add(1))?,
                ],
                RawRecord::read,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let mut items = raws
            .into_iter()
            .map(RawRecord::into_record)
            .collect::<Result<Vec<_>, _>>()?;
        let more = items.len() > usize::try_from(page).map_err(|_| StoreError::IntegerRange)?;
        if more {
            items.pop();
        }
        let next_after = if more {
            items.last().map(|record| record.id.clone())
        } else {
            None
        };
        Ok(HostProcessPage { items, next_after })
    }

    /// Removes ended records beyond the retention bounds, at most `batch`, and returns each
    /// removed `(scope, id)` so the caller removes its log with it (design 26 § S7). A record is
    /// beyond the bounds when it ended at or before `now - retain_ended_ms`, or when it is not
    /// among its subject's `retain_ended_count` most recently ended records. Live records are
    /// never removed.
    pub fn prune_ended_host_processes(
        &self,
        deployment: &str,
        now: DateTime<Utc>,
        limits: &HostProcessLimits,
        batch: u64,
    ) -> Result<Vec<(Scope, String)>, StoreError> {
        let cutoff = now
            .timestamp_millis()
            .saturating_sub(to_i64(limits.retain_ended_ms)?);
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidates = {
            let mut statement = transaction.prepare(
                "SELECT subject, id FROM (
                    SELECT subject, id, ended_ms,
                           ROW_NUMBER() OVER (
                               PARTITION BY subject ORDER BY ended_ms DESC, id DESC
                           ) AS recency
                    FROM host_processes WHERE deployment = ?1 AND state = 'ended'
                 )
                 WHERE ended_ms <= ?2 OR recency > ?3
                 ORDER BY subject, ended_ms, id
                 LIMIT ?4",
            )?;
            statement
                .query_map(
                    params![
                        deployment,
                        cutoff,
                        to_i64(limits.retain_ended_count)?,
                        to_i64(batch)?
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut removed = Vec::with_capacity(candidates.len());
        for (subject, id) in candidates {
            transaction.execute(
                "DELETE FROM host_processes
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3 AND state = 'ended'",
                params![deployment, subject, id],
            )?;
            removed.push((
                Scope {
                    deployment: deployment.to_owned(),
                    subject,
                },
                id,
            ));
        }
        transaction.commit()?;
        Ok(removed)
    }
}

impl Store {
    /// Concludes a start whose program never ran (ADR 0036 S1): the start operation is completed
    /// with `error` and the `starting` record is removed, in one transaction. Used when dispatch
    /// failed before anything ran, and on restart for a start interrupted between its durable
    /// record and its dispatch, which is never re-dispatched because its environment values were
    /// never stored. A record that is no longer `starting` is left as it is.
    #[allow(clippy::too_many_arguments)] // One atomic operation/record terminal boundary.
    pub fn abandon_host_process_start(
        &self,
        scope: &Scope,
        operation: &str,
        terminal_at: DateTime<Utc>,
        status: u16,
        id: &str,
        error: &ErrorDetail,
    ) -> Result<(), StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let event = crate::operations::complete_operation_error_transaction(
            &transaction,
            self.event_retention,
            self.config,
            scope,
            operation,
            &terminal_at.to_rfc3339(),
            status,
            Some(id),
            error,
        )?;
        transaction.execute(
            "DELETE FROM host_processes
             WHERE deployment = ?1 AND subject = ?2 AND id = ?3 AND state = 'starting'
               AND start_operation = ?4",
            params![scope.deployment, scope.subject, id, operation],
        )?;
        transaction.commit()?;
        drop(connection);
        self.report_committed(&[commit_effect(scope, &event)]);
        Ok(())
    }

    /// The non-terminal records of one deployment that any run other than `run` started, a
    /// record that names no run included, oldest first, at most `limit`: what a restarted daemon
    /// reconciles against the cgroup tree (ADR 0036 S3). The exact complement of the live records
    /// `run` started ([`Store::set_host_process_run`]).
    pub fn host_processes_to_reconcile(
        &self,
        deployment: &str,
        run: &str,
        limit: u64,
    ) -> Result<Vec<(Scope, HostProcessRecord)>, StoreError> {
        self.live_host_processes_except(deployment, Some(run), limit)
    }

    /// Every non-terminal record of one deployment, whichever run started it, oldest first, at
    /// most `limit`.
    pub fn live_host_processes(
        &self,
        deployment: &str,
        limit: u64,
    ) -> Result<Vec<(Scope, HostProcessRecord)>, StoreError> {
        self.live_host_processes_except(deployment, None, limit)
    }

    fn live_host_processes_except(
        &self,
        deployment: &str,
        run: Option<&str>,
        limit: u64,
    ) -> Result<Vec<(Scope, HostProcessRecord)>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {COLUMNS} FROM host_processes
             WHERE deployment = ?1 AND state != 'ended' AND (?2 IS NULL OR run_id IS NOT ?2)
             ORDER BY started_ms, subject, id LIMIT ?3"
        ))?;
        let rows = statement
            .query_map(params![deployment, run, to_i64(limit)?], |row| {
                Ok((row.get::<_, String>(0)?, RawRecord::read(row)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(subject, raw)| {
                Ok((
                    Scope {
                        deployment: deployment.to_owned(),
                        subject,
                    },
                    raw.into_record()?,
                ))
            })
            .collect()
    }

    /// The operation of `kind` on record `id` that is still open (accepted, or marked unknown by
    /// a restart), most recent first: the stop a crash interrupted, which a restarted daemon
    /// completes rather than leaves open (ADR 0036 § Consequences, invariant 5).
    pub fn open_host_process_operation(
        &self,
        scope: &Scope,
        id: &str,
        kind: &str,
    ) -> Result<Option<String>, StoreError> {
        let connection = self.connection.lock();
        connection
            .query_row(
                "SELECT operation FROM operations
                 WHERE deployment = ?1 AND subject = ?2 AND resource = ?3
                   AND operation_kind = ?4 AND state IN ('accepted','unknown')
                 ORDER BY accepted_at DESC, operation DESC LIMIT 1",
                params![scope.deployment, scope.subject, id, kind],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }
}

pub(crate) fn migrate_host_processes(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS host_processes (
            deployment TEXT NOT NULL,
            subject TEXT NOT NULL,
            id TEXT NOT NULL CHECK (length(id) > 0),
            owner_session TEXT NOT NULL CHECK (length(owner_session) > 0),
            owner_name TEXT NOT NULL CHECK (length(owner_name) > 0),
            reason TEXT NOT NULL CHECK (length(reason) > 0),
            argv_json TEXT NOT NULL,
            cwd TEXT NOT NULL,
            env_names_json TEXT NOT NULL,
            env_digest TEXT NOT NULL,
            profile TEXT NOT NULL CHECK (profile = 'host'),
            confinement TEXT NOT NULL CHECK (confinement = 'none'),
            lease_ttl_ms INTEGER,
            lease_issued_ms INTEGER,
            lease_expires_ms INTEGER,
            keep_reason TEXT,
            timeout_ms INTEGER,
            expires_at_ms INTEGER,
            deadline_ms INTEGER,
            deadline_source TEXT CHECK (deadline_source IN ('timeout','expires-at')),
            state TEXT NOT NULL CHECK (state IN ('starting','running','stopping','ended')),
            ended_by TEXT CHECK (
                ended_by IN ('exit','stop','lease-expiry','timeout','expires-at','vanished')
            ),
            exit_code INTEGER,
            exit_signal TEXT,
            start_operation TEXT NOT NULL,
            started_ms INTEGER NOT NULL,
            running_ms INTEGER,
            stopping_ms INTEGER,
            ended_ms INTEGER,
            run_id TEXT,
            PRIMARY KEY (deployment, subject, id),
            CHECK (
                (keep_reason IS NULL AND lease_ttl_ms IS NOT NULL
                    AND lease_issued_ms IS NOT NULL AND lease_expires_ms IS NOT NULL)
                OR (keep_reason IS NOT NULL AND length(keep_reason) > 0 AND lease_ttl_ms IS NULL
                    AND lease_issued_ms IS NULL AND lease_expires_ms IS NULL)
            ),
            CHECK ((deadline_ms IS NULL) = (deadline_source IS NULL)),
            CHECK ((state IN ('starting','running')) = (ended_by IS NULL)),
            CHECK ((state = 'ended') = (ended_ms IS NOT NULL)),
            CHECK (exit_code IS NULL OR exit_signal IS NULL)
        ) WITHOUT ROWID;
        CREATE UNIQUE INDEX IF NOT EXISTS host_processes_live_owner_name
            ON host_processes (deployment, subject, owner_session, owner_name)
            WHERE state != 'ended';
        CREATE INDEX IF NOT EXISTS host_processes_due_lease
            ON host_processes (deployment, lease_expires_ms)
            WHERE state IN ('starting','running');
        CREATE INDEX IF NOT EXISTS host_processes_due_deadline
            ON host_processes (deployment, deadline_ms)
            WHERE state IN ('starting','running');
        CREATE INDEX IF NOT EXISTS host_processes_ended
            ON host_processes (deployment, subject, ended_ms)
            WHERE state = 'ended';
        ",
    )?;
    migrate_host_process_run(connection)
}

/// A table written before records named their run gains the column, empty on every existing
/// row: each of them was started by an earlier daemon run, and a restart reconciles it as one.
fn migrate_host_process_run(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection.prepare("PRAGMA table_info(host_processes)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if !columns.iter().any(|column| column == "run_id") {
        connection.execute("ALTER TABLE host_processes ADD COLUMN run_id TEXT", [])?;
    }
    Ok(())
}

fn validate_start(
    process: &NewHostProcess,
    now: DateTime<Utc>,
    limits: &HostProcessLimits,
) -> Result<Option<HostProcessDeadline>, HostProcessRefusal> {
    let blank = |value: &str| value.trim().is_empty() || value.contains('\0');
    if blank(&process.owner_session) || blank(&process.owner_name) {
        return Err(HostProcessRefusal::OwnerInvalid);
    }
    if blank(&process.reason) {
        return Err(HostProcessRefusal::ReasonRequired);
    }
    if process.argv.first().is_none_or(String::is_empty)
        || process.argv.iter().any(|arg| arg.contains('\0'))
    {
        return Err(HostProcessRefusal::ArgvInvalid);
    }
    if !process.cwd.starts_with('/') || process.cwd.contains('\0') {
        return Err(HostProcessRefusal::CwdInvalid);
    }
    let mut names = BTreeSet::new();
    let names_valid = process
        .env_names
        .iter()
        .all(|name| !name.is_empty() && !name.contains(['=', '\0']) && names.insert(name.as_str()));
    if !names_valid || !valid_digest(&process.env_digest) {
        return Err(HostProcessRefusal::EnvInvalid);
    }
    match &process.lease {
        HostProcessLeaseRequest::Ttl { ttl_ms } => {
            if !(MIN_LEASE_TTL_MS..=MAX_LEASE_TTL_MS).contains(ttl_ms) {
                return Err(HostProcessRefusal::LeaseInvalid);
            }
        }
        HostProcessLeaseRequest::Keep { reason } => {
            if blank(reason) {
                return Err(HostProcessRefusal::KeepReasonRequired);
            }
        }
    }
    let by_timeout = match process.timeout_ms {
        None => None,
        Some(timeout_ms) if timeout_ms == 0 || timeout_ms > limits.max_timeout_ms => {
            return Err(HostProcessRefusal::TimeoutInvalid);
        }
        Some(timeout_ms) => Some(HostProcessDeadline {
            at: add_ms(now, timeout_ms).map_err(|_| HostProcessRefusal::TimeoutInvalid)?,
            source: HostProcessDeadlineSource::Timeout,
        }),
    };
    let by_instant = match process.expires_at {
        None => None,
        Some(instant) => {
            let ahead = instant.signed_duration_since(now).num_milliseconds();
            let min = i64::try_from(MIN_LEASE_TTL_MS).unwrap_or(i64::MAX);
            let max = i64::try_from(limits.max_timeout_ms).unwrap_or(i64::MAX);
            if ahead < min || ahead > max {
                return Err(HostProcessRefusal::ExpiresAtInvalid);
            }
            Some(HostProcessDeadline {
                at: instant,
                source: HostProcessDeadlineSource::ExpiresAt,
            })
        }
    };
    Ok(match (by_timeout, by_instant) {
        (Some(timeout), Some(instant)) => Some(if instant.at < timeout.at {
            instant
        } else {
            timeout
        }),
        (timeout, instant) => timeout.or(instant),
    })
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn cause_due(record: &HostProcessRecord, cause: HostProcessEndedBy, now: DateTime<Utc>) -> bool {
    match cause {
        HostProcessEndedBy::Stop => true,
        HostProcessEndedBy::LeaseExpiry => {
            matches!(&record.lease, HostProcessLease::Ttl { expires_at, .. } if now >= *expires_at)
        }
        HostProcessEndedBy::Timeout => record.deadline.as_ref().is_some_and(|deadline| {
            deadline.source == HostProcessDeadlineSource::Timeout && now >= deadline.at
        }),
        HostProcessEndedBy::ExpiresAt => record.deadline.as_ref().is_some_and(|deadline| {
            deadline.source == HostProcessDeadlineSource::ExpiresAt && now >= deadline.at
        }),
        HostProcessEndedBy::Exit | HostProcessEndedBy::Vanished => false,
    }
}

fn live_name_taken(
    connection: &Connection,
    scope: &Scope,
    process: &NewHostProcess,
) -> Result<bool, StoreError> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM host_processes
                 WHERE deployment = ?1 AND subject = ?2 AND owner_session = ?3
                   AND owner_name = ?4 AND state != 'ended'
             )",
            params![
                scope.deployment,
                scope.subject,
                process.owner_session,
                process.owner_name
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StoreError::from)
}

fn live_bound_reached(
    connection: &Connection,
    scope: &Scope,
    bound: u64,
) -> Result<bool, StoreError> {
    if bound == 0 {
        return Ok(true);
    }
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM host_processes
                 WHERE deployment = ?1 AND subject = ?2 AND state != 'ended'
                 ORDER BY id LIMIT 1 OFFSET ?3
             )",
            params![scope.deployment, scope.subject, to_i64(bound - 1)?],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StoreError::from)
}

fn insert_record(
    connection: &Connection,
    scope: &Scope,
    record: &HostProcessRecord,
) -> Result<(), StoreError> {
    let (ttl, issued, expires, keep) = match &record.lease {
        HostProcessLease::Ttl {
            ttl_ms,
            issued_at,
            expires_at,
        } => (
            Some(to_i64(*ttl_ms)?),
            Some(issued_at.timestamp_millis()),
            Some(expires_at.timestamp_millis()),
            None,
        ),
        HostProcessLease::Keep { reason } => (None, None, None, Some(reason.as_str())),
    };
    connection.execute(
        "INSERT INTO host_processes (
            deployment, subject, id, owner_session, owner_name, reason, argv_json, cwd,
            env_names_json, env_digest, profile, confinement, lease_ttl_ms, lease_issued_ms,
            lease_expires_ms, keep_reason, timeout_ms, expires_at_ms, deadline_ms,
            deadline_source, state, start_operation, started_ms, run_id
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
            ?20, ?21, ?22, ?23, ?24
         )",
        params![
            scope.deployment,
            scope.subject,
            record.id,
            record.owner_session,
            record.owner_name,
            record.reason,
            serde_json::to_string(&record.argv)?,
            record.cwd,
            serde_json::to_string(&record.env_names)?,
            record.env_digest,
            record.profile,
            record.confinement,
            ttl,
            issued,
            expires,
            keep,
            record.timeout_ms.map(to_i64).transpose()?,
            record.expires_at.map(|at| at.timestamp_millis()),
            record
                .deadline
                .as_ref()
                .map(|deadline| deadline.at.timestamp_millis()),
            record
                .deadline
                .as_ref()
                .map(|deadline| match deadline.source {
                    HostProcessDeadlineSource::Timeout => "timeout",
                    HostProcessDeadlineSource::ExpiresAt => "expires-at",
                }),
            record.state.as_str(),
            record.start_operation,
            record.started_at.timestamp_millis(),
            record.run,
        ],
    )?;
    Ok(())
}

fn load_record(
    connection: &Connection,
    scope: &Scope,
    id: &str,
) -> Result<Option<HostProcessRecord>, StoreError> {
    connection
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM host_processes
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3"
            ),
            params![scope.deployment, scope.subject, id],
            RawRecord::read,
        )
        .optional()?
        .map(RawRecord::into_record)
        .transpose()
}

fn reload(
    connection: &Connection,
    scope: &Scope,
    id: &str,
) -> Result<HostProcessRecord, StoreError> {
    load_record(connection, scope, id)?.ok_or(StoreError::HostProcessInvariant(
        "record vanished inside its transaction",
    ))
}

#[allow(clippy::too_many_arguments)] // One atomic renewal and ledger terminal boundary.
fn complete_renewal(
    connection: &Connection,
    store: &Store,
    scope: &Scope,
    operation: &str,
    terminal_at: DateTime<Utc>,
    status: u16,
    id: &str,
    result: serde_json::Value,
) -> Result<substrate_wire::Event, StoreError> {
    let outcome = OperationOutcome::Success { result };
    let terminal_at = terminal_at.to_rfc3339();
    let changed = connection.execute(
        "UPDATE operations
         SET state = 'terminal', terminal_at = ?4, resource = ?5, outcome_json = ?6,
             response_status = ?7
         WHERE deployment = ?1 AND subject = ?2 AND operation = ?3 AND state = 'accepted'
           AND operation_kind = ?8",
        params![
            scope.deployment,
            scope.subject,
            operation,
            terminal_at,
            id,
            serde_json::to_string(&outcome)?,
            i64::from(status),
            HOST_PROCESS_RENEW_KIND,
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::NotAccepted(operation.to_owned()));
    }
    finalize_operation_accounting(connection, store.config, scope, operation)?;
    let (actor, principal) = operation_identity(connection, scope, operation)?;
    append_event(
        connection,
        store.event_retention,
        scope,
        id,
        "operation",
        "operation.terminal",
        &terminal_at,
        &actor,
        principal.as_deref(),
        operation,
        None,
    )
}

fn add_ms(at: DateTime<Utc>, ms: u64) -> Result<DateTime<Utc>, StoreError> {
    at.checked_add_signed(Duration::milliseconds(to_i64(ms)?))
        .ok_or(StoreError::IntegerRange)
}

/// The lease a start accepted at `now` holds: a TTL lease issued then, or a keep.
fn issued_lease(
    request: &HostProcessLeaseRequest,
    now: DateTime<Utc>,
) -> Result<HostProcessLease, StoreError> {
    Ok(match request {
        HostProcessLeaseRequest::Ttl { ttl_ms } => HostProcessLease::Ttl {
            ttl_ms: *ttl_ms,
            issued_at: now,
            expires_at: add_ms(now, *ttl_ms)?,
        },
        HostProcessLeaseRequest::Keep { reason } => HostProcessLease::Keep {
            reason: reason.clone(),
        },
    })
}

fn from_ms(ms: i64) -> Result<DateTime<Utc>, StoreError> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .ok_or(StoreError::IntegerRange)
}

fn from_ms_opt(ms: Option<i64>) -> Result<Option<DateTime<Utc>>, StoreError> {
    ms.map(from_ms).transpose()
}

fn parse_deadline_cause(source: Option<&str>) -> Result<HostProcessEndedBy, StoreError> {
    match source {
        Some("timeout") => Ok(HostProcessEndedBy::Timeout),
        Some("expires-at") => Ok(HostProcessEndedBy::ExpiresAt),
        _ => Err(StoreError::HostProcessInvariant("deadline source")),
    }
}

struct RawRecord {
    id: String,
    owner_session: String,
    owner_name: String,
    reason: String,
    argv_json: String,
    cwd: String,
    env_names_json: String,
    env_digest: String,
    profile: String,
    confinement: String,
    lease_ttl_ms: Option<i64>,
    lease_issued_ms: Option<i64>,
    lease_expires_ms: Option<i64>,
    keep_reason: Option<String>,
    timeout_ms: Option<i64>,
    expires_at_ms: Option<i64>,
    deadline_ms: Option<i64>,
    deadline_source: Option<String>,
    state: String,
    ended_by: Option<String>,
    exit_code: Option<i32>,
    exit_signal: Option<String>,
    start_operation: String,
    started_ms: i64,
    running_ms: Option<i64>,
    stopping_ms: Option<i64>,
    ended_ms: Option<i64>,
    run_id: Option<String>,
}

impl RawRecord {
    /// Reads the row selected by [`COLUMNS`]; column 0 is the subject, already known.
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(1)?,
            owner_session: row.get(2)?,
            owner_name: row.get(3)?,
            reason: row.get(4)?,
            argv_json: row.get(5)?,
            cwd: row.get(6)?,
            env_names_json: row.get(7)?,
            env_digest: row.get(8)?,
            profile: row.get(9)?,
            confinement: row.get(10)?,
            lease_ttl_ms: row.get(11)?,
            lease_issued_ms: row.get(12)?,
            lease_expires_ms: row.get(13)?,
            keep_reason: row.get(14)?,
            timeout_ms: row.get(15)?,
            expires_at_ms: row.get(16)?,
            deadline_ms: row.get(17)?,
            deadline_source: row.get(18)?,
            state: row.get(19)?,
            ended_by: row.get(20)?,
            exit_code: row.get(21)?,
            exit_signal: row.get(22)?,
            start_operation: row.get(23)?,
            started_ms: row.get(24)?,
            running_ms: row.get(25)?,
            stopping_ms: row.get(26)?,
            ended_ms: row.get(27)?,
            run_id: row.get(28)?,
        })
    }

    fn into_record(self) -> Result<HostProcessRecord, StoreError> {
        let lease = match (
            self.keep_reason,
            self.lease_ttl_ms,
            self.lease_issued_ms,
            self.lease_expires_ms,
        ) {
            (None, Some(ttl), Some(issued), Some(expires)) => HostProcessLease::Ttl {
                ttl_ms: to_u64(ttl)?,
                issued_at: from_ms(issued)?,
                expires_at: from_ms(expires)?,
            },
            (Some(reason), None, None, None) => HostProcessLease::Keep { reason },
            _ => return Err(StoreError::HostProcessInvariant("lease xor keep")),
        };
        let deadline = match (self.deadline_ms, self.deadline_source.as_deref()) {
            (None, None) => None,
            (Some(at), source) => Some(HostProcessDeadline {
                at: from_ms(at)?,
                source: match parse_deadline_cause(source)? {
                    HostProcessEndedBy::Timeout => HostProcessDeadlineSource::Timeout,
                    _ => HostProcessDeadlineSource::ExpiresAt,
                },
            }),
            (None, Some(_)) => return Err(StoreError::HostProcessInvariant("deadline source")),
        };
        let state = match self.state.as_str() {
            "starting" => HostProcessState::Starting,
            "running" => HostProcessState::Running,
            "stopping" => HostProcessState::Stopping,
            "ended" => HostProcessState::Ended,
            _ => return Err(StoreError::HostProcessInvariant("state")),
        };
        let ended_by = match self.ended_by.as_deref() {
            None => None,
            Some("exit") => Some(HostProcessEndedBy::Exit),
            Some("stop") => Some(HostProcessEndedBy::Stop),
            Some("lease-expiry") => Some(HostProcessEndedBy::LeaseExpiry),
            Some("timeout") => Some(HostProcessEndedBy::Timeout),
            Some("expires-at") => Some(HostProcessEndedBy::ExpiresAt),
            Some("vanished") => Some(HostProcessEndedBy::Vanished),
            Some(_) => return Err(StoreError::HostProcessInvariant("ended_by")),
        };
        let exit = match (self.exit_code, self.exit_signal) {
            (None, None) => None,
            (Some(code), None) => Some(HostProcessExit::Code(code)),
            (None, Some(signal)) => Some(HostProcessExit::Signal(signal)),
            (Some(_), Some(_)) => {
                return Err(StoreError::HostProcessInvariant("exit code xor signal"));
            }
        };
        Ok(HostProcessRecord {
            id: self.id,
            owner_session: self.owner_session,
            owner_name: self.owner_name,
            reason: self.reason,
            argv: serde_json::from_str(&self.argv_json)?,
            cwd: self.cwd,
            env_names: serde_json::from_str(&self.env_names_json)?,
            env_digest: self.env_digest,
            profile: self.profile,
            confinement: self.confinement,
            lease,
            timeout_ms: self.timeout_ms.map(to_u64).transpose()?,
            expires_at: from_ms_opt(self.expires_at_ms)?,
            deadline,
            state,
            ended_by,
            exit,
            start_operation: self.start_operation,
            started_at: from_ms(self.started_ms)?,
            running_at: from_ms_opt(self.running_ms)?,
            stopping_at: from_ms_opt(self.stopping_ms)?,
            ended_at: from_ms_opt(self.ended_ms)?,
            run: self.run_id,
        })
    }
}

#[cfg(test)]
mod tests;
