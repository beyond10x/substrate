//! Host processes (ADR 0036 S1-S7 and S11): the routes, the admission of the `host` profile, and
//! the supervisor clock that ends records at lease expiry and at their hard deadlines.
//!
//! **Admission (S2).** Every `/v1/processes` route is served only on the personal Unix socket and
//! only to a peer whose uid is the daemon's own effective uid. Any other listener is
//! `process.host-posture-refused` and any other peer `process.host-subject-mismatch`, decided
//! from the listener the router was built for and from the kernel-derived subject, never from
//! request data. A start is then validated in the order of ADR 0036's table; a daemon that opened
//! no host-process driver answers a valid start `process.host-unserved` and records nothing. No
//! exec or session route reaches this module, and nothing here selects a confined profile.
//!
//! **Invariant 5.** A start, a stop, each record a stop by owner stops, a renewal, a lease expiry
//! and a deadline stop are each an operation recorded durably before the driver acts. Lease expiry and deadline
//! stops are operations the daemon originates under its own actor
//! ([`HOST_PROCESS_SUPERVISOR_ACTOR`]).
//!
//! **One clock.** Every host-process instant is the authority's wall clock, `Authority::now`:
//! each operation's `accepted_at`, a lease's issue and expiry, the effective deadline, and the
//! instant the supervisor clock compares them with. The boot-relative `LeaseClock` exec and
//! session leases use is not consulted here, because `expires_at` is an absolute wall instant by
//! definition and the earlier of it and `timeout_ms` has to be decided on one clock with the
//! lease. The driver runs no clock of its own: the daemon passes it no deadline and stops each
//! record with the cause the store recorded first.
//!
//! **Environment values are never persisted.** The ledger keeps the request hash, the record
//! keeps the variable names and a digest of the whole map; the values reach the driver and
//! nothing else.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde_json::{Map, Number, Value, json};
use sha2::{Digest as _, Sha256};
use substrate_host::{
    Deadline, DriverError, DriverErrorClass, HostProcessEnd as DriverEnd, HostProcessObservation,
    HostProcessSpec, LogStream as DriverLogStream, LogStreamSelection, ProcessExit as DriverExit,
    ProcessLogs, ProcessSupervisor, Reattached, SocketProtocol, StopCause,
};
use substrate_store::{
    HOST_PROCESS_RENEW_KIND, HOST_PROCESS_START_KIND, HOST_PROCESS_STOP_KIND,
    HOST_PROCESS_SUPERVISOR_ACTOR, HostProcessDeadlineSource, HostProcessEnd, HostProcessEndedBy,
    HostProcessExit, HostProcessFilter, HostProcessLease, HostProcessLeaseRequest,
    HostProcessLimits, HostProcessRecord, HostProcessRenewal, HostProcessStartReservation,
    HostProcessState, HostProcessStopReservation, HostProcessTransition, NewHostProcess,
    NewOperation, Scope, Store, StoreError,
};
use substrate_wire::{
    EmptyInput, ErrorClass, ErrorDetail, LeaseRenewInput, MAX_PROCESS_ARGV_BYTES,
    MAX_PROCESS_ARGV_ITEMS, MAX_PROCESS_ENV_BYTES, MAX_PROCESS_ENV_ITEMS, MAX_PROCESS_LIST_ITEMS,
    MAX_PROCESS_LOG_TAIL_BYTES, MAX_PROCESS_OWNER_BYTES, MAX_PROCESS_REASON_BYTES,
    PROCESS_ARGV_INVALID, PROCESS_CWD_INVALID, PROCESS_ENV_INVALID, PROCESS_EXPIRES_AT_INVALID,
    PROCESS_HOST_POSTURE_REFUSED, PROCESS_HOST_SUBJECT_MISMATCH, PROCESS_HOST_UNSERVED,
    PROCESS_KEEP_REASON_REQUIRED, PROCESS_LEASE_INVALID, PROCESS_NOT_FOUND, PROCESS_OWNER_INVALID,
    PROCESS_PROFILE_REQUIRED, PROCESS_PROFILE_UNKNOWN, PROCESS_REASON_REQUIRED,
    PROCESS_TIMEOUT_INVALID, ProcessConfinement, ProcessCpu, ProcessDeadline,
    ProcessDeadlineSource, ProcessEndedBy, ProcessEnv, ProcessEnvDigest, ProcessExit, ProcessKind,
    ProcessLease, ProcessLeaseRequest, ProcessLeaseState, ProcessListQuery, ProcessListening,
    ProcessListeningSocket, ProcessLogLine, ProcessLogStream, ProcessLogStreamSelection,
    ProcessLogTail, ProcessLogsQuery, ProcessMemory, ProcessOwner, ProcessPage, ProcessPresence,
    ProcessProfile, ProcessRecord, ProcessSocketProtocol, ProcessStartInput, ProcessState,
    ProcessStopByOwnerInput, ProcessStopByOwnerResult, ProcessStopped, ProcessTimestamp, Success,
    canonical_json, canonical_request_hash_v2, process_refusal,
};
use tokio::sync::Semaphore;
use ulid::Ulid;

use super::operations::{
    BoundMutation, begin, decode_mutation, driver_detail, new_operation, read_bounded_body,
    refuse_before_dispatch_response, replay, reservation_response,
};
use super::responses::{
    failure, failure_detail, operation_ledger_capacity, outcome_unknown, query_is_empty,
    request_id, schema_invalid, store_failure, success,
};
use super::service::bounded_blocking;
use super::{App, Authority, Identity, SessionTransport};

/// A start a restart found recorded but never dispatched (S1): concluded, never re-dispatched,
/// because its environment values were never stored.
pub(super) const PROCESS_START_ABANDONED: &str = "process.start-abandoned";
/// Records the supervisor clock stops per sweep.
const DUE_BATCH: u64 = 32;
/// Ended records, and their logs, the retention sweep removes per sweep (S7).
const PRUNE_BATCH: u64 = 32;
/// Records an earlier run left that one restart reconciles (S3); at most the live bound of 64
/// subjects' worth, and each is visited once.
const RECONCILE_LIMIT: u64 = 4_096;

/// The members a start input may carry; anything else is the generic schema refusal.
const START_MEMBERS: [&str; 9] = [
    "profile",
    "argv",
    "cwd",
    "env",
    "owner",
    "reason",
    "lease",
    "timeout_ms",
    "expires_at",
];

/// Everything a host-process action needs: the store, the driver, the clock and the store-I/O
/// bound, for a request and for the maintenance sweep alike.
#[derive(Clone)]
pub(super) struct ProcessCtx {
    store: Arc<Store>,
    supervisor: Arc<dyn ProcessSupervisor>,
    authority: Arc<dyn Authority>,
    slots: Arc<Semaphore>,
    deployment: String,
}

impl App {
    fn process_ctx(&self) -> Option<ProcessCtx> {
        self.host_processes.as_ref().map(|supervisor| ProcessCtx {
            store: Arc::clone(&self.store),
            supervisor: Arc::clone(supervisor),
            authority: Arc::clone(&self.authority),
            slots: Arc::clone(&self.blocking_store_slots),
            deployment: self.deployment.clone(),
        })
    }

    /// The supervisor clock, run from every maintenance sweep (spec points 8 and 9). Once per
    /// daemon run the records an earlier run left are reconciled against the cgroup tree; then
    /// every live record whose lease or deadline the clock has reached is stopped through an
    /// operation the daemon originates; then every end the driver published is recorded, and the
    /// stop operation that caused it completed; then ended records beyond retention are removed
    /// with their logs.
    ///
    /// No task is detached here: the driver owns each process's supervision and each stop it was
    /// asked to begin, and this sweep collects what they concluded.
    pub(super) async fn supervise_host_processes(&self) {
        let Some(ctx) = self.process_ctx() else {
            return;
        };
        {
            let mut reconciled = self.host_process_reconciled.lock().await;
            if !*reconciled {
                *reconciled = ctx.reconcile_after_restart(self.restart_cutoff).await;
            }
        }
        ctx.stop_due().await;
        ctx.collect_ends().await;
        ctx.prune_ended().await;
    }
}

impl ProcessCtx {
    async fn io<T, F>(&self, operation: F) -> T
    where
        T: Send,
        F: FnOnce() -> T + Send,
    {
        bounded_blocking(&self.slots, operation).await
    }

    fn now(&self) -> DateTime<Utc> {
        self.authority.now()
    }

    /// Records the driver's end of one record. A live record ends `exit` with the observed code
    /// or signal, or `vanished` when no exit status was observed; a stopping record keeps the
    /// cause its stop recorded first (the store decides). A driver failure leaves the record as
    /// it is, for the next restart's reconciliation.
    async fn record_end(
        &self,
        scope: &Scope,
        id: &str,
        end: Result<DriverEnd, DriverError>,
    ) -> Option<HostProcessRecord> {
        let end = match end {
            Ok(end) => match end.exit {
                Some(DriverExit::Code(code)) => HostProcessEnd::Exit(HostProcessExit::Code(code)),
                Some(DriverExit::Signal(signal)) => {
                    HostProcessEnd::Exit(HostProcessExit::Signal(signal_name(signal)))
                }
                None => HostProcessEnd::Vanished,
            },
            Err(error) if error.class == DriverErrorClass::NotFound => HostProcessEnd::Vanished,
            Err(error) => {
                tracing::warn!(process = %id, code = error.code, "host process did not end cleanly");
                return None;
            }
        };
        self.record_end_as(scope, id, &end).await
    }

    async fn record_end_as(
        &self,
        scope: &Scope,
        id: &str,
        end: &HostProcessEnd,
    ) -> Option<HostProcessRecord> {
        let at = self.now();
        match self
            .io(|| self.store.record_host_process_end(scope, id, at, end))
            .await
        {
            Ok(
                HostProcessTransition::Applied(record) | HostProcessTransition::Unchanged(record),
            ) => Some(*record),
            Ok(HostProcessTransition::Missing) => None,
            Err(error) => {
                tracing::warn!(process = %id, %error, "host process end was not recorded");
                None
            }
        }
    }

    /// Completes `operation` with the ended record. Already terminal is not an error: the request
    /// that asked for the stop and the sweep that collected its end may both conclude it, and the
    /// first one stands.
    async fn complete_stop(&self, scope: &Scope, operation: &str, record: &HostProcessRecord) {
        let result = wire_record(record, None, self.now());
        let at = self.now().to_rfc3339();
        if let Err(error) = self
            .io(|| {
                self.store.complete_success(
                    scope,
                    operation,
                    &at,
                    StatusCode::OK.as_u16(),
                    Some(&record.id),
                    &result,
                )
            })
            .await
            && !matches!(error, StoreError::NotAccepted(_))
        {
            tracing::warn!(process = %record.id, %error, "host process stop was not completed");
        }
    }

    /// Completes the stop operation still open on an ended record, if any.
    async fn complete_open_stop(&self, scope: &Scope, record: &HostProcessRecord) {
        if record.state != HostProcessState::Ended {
            return;
        }
        if let Ok(Some(operation)) = self
            .io(|| {
                self.store
                    .open_host_process_operation(scope, &record.id, HOST_PROCESS_STOP_KIND)
            })
            .await
        {
            self.complete_stop(scope, &operation, record).await;
        }
    }

    /// Answers one accepted stop: its operation is completed with the record as recorded
    /// `stopping`, built once so the stored result and the response are the same record, and then
    /// the driver's stop sequence is begun. Nothing waits for the end: the supervisor clock
    /// records it ([`ProcessCtx::collect_ends`]).
    async fn accept_stop(
        &self,
        scope: &Scope,
        operation: &str,
        record: &HostProcessRecord,
    ) -> Result<ProcessRecord, StoreError> {
        let result = wire_record(record, None, self.now());
        let at = self.now().to_rfc3339();
        self.io(|| {
            self.store.complete_success(
                scope,
                operation,
                &at,
                StatusCode::OK.as_u16(),
                Some(&record.id),
                &result,
            )
        })
        .await?;
        let cause = record.ended_by.unwrap_or(HostProcessEndedBy::Stop);
        self.begin_recorded_stop(scope, &record.id, cause).await;
        Ok(result)
    }

    /// Begins the stop of a record already recorded `stopping`. A record the driver does not hold
    /// has nothing left running: it ends `vanished` under the cause its stop recorded, and the
    /// open stop operation is completed.
    async fn begin_recorded_stop(&self, scope: &Scope, id: &str, cause: HostProcessEndedBy) {
        match self.supervisor.begin_stop(id, stop_cause(cause)) {
            Ok(()) => {}
            Err(error) if error.class == DriverErrorClass::NotFound => {
                if let Some(record) = self
                    .record_end_as(scope, id, &HostProcessEnd::Vanished)
                    .await
                {
                    self.complete_open_stop(scope, &record).await;
                }
            }
            Err(error) => {
                tracing::warn!(process = %id, code = error.code, "host process stop did not begin");
            }
        }
    }

    /// The supervisor clock: every live record whose lease or deadline the clock reached is
    /// stopped through an operation the daemon originates with its own actor, recorded before
    /// the driver acts. The driver runs the stop sequence; [`ProcessCtx::collect_ends`] records
    /// its end, so a process that ignores `SIGTERM` holds no sweep for its grace.
    async fn stop_due(&self) {
        let now = self.now();
        let Ok(due) = self
            .io(|| {
                self.store
                    .due_host_processes(&self.deployment, now, DUE_BATCH)
            })
            .await
        else {
            return;
        };
        for due in due {
            let Some(new) = supervisor_operation(&due.scope, &due.id, now) else {
                continue;
            };
            let reserved = self
                .io(|| {
                    self.store
                        .reserve_host_process_stop(&new, &due.id, due.cause)
                })
                .await;
            if let Ok(HostProcessStopReservation::Accepted(_)) = reserved {
                self.begin_recorded_stop(&due.scope, &due.id, due.cause)
                    .await;
            }
        }
    }

    /// Records every end the driver has published for a record not yet ended, however it came:
    /// the main process's exit, a stop the driver ran, or a re-attached cgroup that emptied. The
    /// stop operation that caused an end is completed with it.
    async fn collect_ends(&self) {
        let Ok(live) = self
            .io(|| {
                self.store.host_processes_to_reconcile(
                    &self.deployment,
                    DateTime::<Utc>::MAX_UTC,
                    RECONCILE_LIMIT,
                )
            })
            .await
        else {
            return;
        };
        for (scope, record) in live {
            let end = match self.supervisor.ended(&record.id) {
                Ok(Some(end)) => Ok(end),
                // A stopping record the driver does not hold has nothing left running.
                Err(error)
                    if error.class == DriverErrorClass::NotFound
                        && record.state == HostProcessState::Stopping =>
                {
                    Err(error)
                }
                Ok(None) | Err(_) => continue,
            };
            if let Some(ended) = self.record_end(&scope, &record.id, end).await {
                self.complete_open_stop(&scope, &ended).await;
            }
        }
    }

    /// Removes ended records beyond retention, and each one's log with it (S7).
    async fn prune_ended(&self) {
        let now = self.now();
        let limits = HostProcessLimits::default();
        let Ok(removed) = self
            .io(|| {
                self.store
                    .prune_ended_host_processes(&self.deployment, now, &limits, PRUNE_BATCH)
            })
            .await
        else {
            return;
        };
        for (_, id) in removed {
            let _ = self.supervisor.forget(&id);
        }
    }

    /// Reconciles the records an earlier daemon run left (S3, S1, S4, S11). Returns whether the
    /// pass completed; an incomplete pass is retried on the next sweep.
    ///
    /// - `starting`: a cgroup with members means the start was dispatched; it runs, and its
    ///   start operation is completed. Anything else was never dispatched, or nothing of it is
    ///   left: the start is concluded `process.start-abandoned` and the record and its log are
    ///   removed. It is never re-dispatched, because its environment values were never stored.
    /// - `running`: a cgroup with members is observed again; a cgroup gone ends `vanished`.
    /// - `stopping`: the interrupted stop is begun again with the cause it recorded first, and
    ///   its operation completed when it ends.
    ///
    /// A deadline or lease that passed during the downtime is then enforced by the same sweep's
    /// clock, at once.
    async fn reconcile_after_restart(&self, cutoff: DateTime<Utc>) -> bool {
        let Ok(records) = self
            .io(|| {
                self.store
                    .host_processes_to_reconcile(&self.deployment, cutoff, RECONCILE_LIMIT)
            })
            .await
        else {
            return false;
        };
        for (scope, record) in records {
            let Ok(found) = Arc::clone(&self.supervisor).reattach(&record.id) else {
                continue;
            };
            match (record.state, found) {
                (HostProcessState::Starting, Reattached::Gone) => {
                    self.abandon_start(&scope, &record).await;
                }
                (HostProcessState::Starting, Reattached::Live) => {
                    let at = self.now();
                    if let Ok(HostProcessTransition::Applied(running)) = self
                        .io(|| self.store.mark_host_process_running(&scope, &record.id, at))
                        .await
                    {
                        let result = wire_record(&running, None, at);
                        let terminal_at = at.to_rfc3339();
                        let _ = self
                            .io(|| {
                                self.store.complete_success(
                                    &scope,
                                    &record.start_operation,
                                    &terminal_at,
                                    StatusCode::OK.as_u16(),
                                    Some(&record.id),
                                    &result,
                                )
                            })
                            .await;
                    }
                }
                (HostProcessState::Running, Reattached::Live) | (HostProcessState::Ended, _) => {}
                (HostProcessState::Running, Reattached::Gone) => {
                    self.record_end_as(&scope, &record.id, &HostProcessEnd::Vanished)
                        .await;
                }
                (HostProcessState::Stopping, _) => {
                    let cause = record.ended_by.unwrap_or(HostProcessEndedBy::Stop);
                    self.begin_recorded_stop(&scope, &record.id, cause).await;
                }
            }
        }
        true
    }

    async fn abandon_start(&self, scope: &Scope, record: &HostProcessRecord) {
        let detail = ErrorDetail {
            class: ErrorClass::Failed,
            code: PROCESS_START_ABANDONED.to_owned(),
            message: "The daemon restarted before this start was dispatched; it is never \
                      re-dispatched, because its environment values were never stored."
                .to_owned(),
            retriable: false,
            address: Some("process".to_owned()),
            operation: Some(record.start_operation.clone()),
        };
        let at = self.now();
        if self
            .io(|| {
                self.store.abandon_host_process_start(
                    scope,
                    &record.start_operation,
                    at,
                    StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                    &record.id,
                    &detail,
                )
            })
            .await
            .is_ok()
        {
            let _ = self.supervisor.forget(&record.id);
        }
    }
}

fn stop_cause(cause: HostProcessEndedBy) -> StopCause {
    match cause {
        HostProcessEndedBy::LeaseExpiry => StopCause::LeaseExpiry,
        HostProcessEndedBy::Timeout => StopCause::Timeout,
        HostProcessEndedBy::ExpiresAt => StopCause::ExpiresAt,
        HostProcessEndedBy::Stop | HostProcessEndedBy::Exit | HostProcessEndedBy::Vanished => {
            StopCause::Stop
        }
    }
}

fn signal_name(signal: i32) -> String {
    nix::sys::signal::Signal::try_from(signal)
        .map_or_else(|_| signal.to_string(), |name| name.as_str().to_owned())
}

/// The stop operation the daemon originates for one due record, under its own actor.
fn supervisor_operation(scope: &Scope, id: &str, now: DateTime<Utc>) -> Option<NewOperation> {
    let address = format!("/v1/processes/{id}/stop");
    let request_hash = canonical_request_hash_v2("POST", &address, &json!({}), None).ok()?;
    Some(NewOperation {
        scope: scope.clone(),
        operation: format!("op_supervisor_{}", Ulid::generate()),
        operation_kind: HOST_PROCESS_STOP_KIND.to_owned(),
        request_hash,
        accepted_at: now.to_rfc3339(),
        capability_snapshot: None,
        actor: HOST_PROCESS_SUPERVISOR_ACTOR.to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(id.to_owned()),
    })
}

// ---------------------------------------------------------------------------------------------
// The wire record.
// ---------------------------------------------------------------------------------------------

fn number(value: u64) -> Number {
    Number::from(value)
}

fn wire_state(state: HostProcessState) -> ProcessState {
    match state {
        HostProcessState::Starting => ProcessState::Starting,
        HostProcessState::Running => ProcessState::Running,
        HostProcessState::Stopping => ProcessState::Stopping,
        HostProcessState::Ended => ProcessState::Ended,
    }
}

fn store_state(state: ProcessState) -> HostProcessState {
    match state.as_str() {
        "starting" => HostProcessState::Starting,
        "running" => HostProcessState::Running,
        "stopping" => HostProcessState::Stopping,
        _ => HostProcessState::Ended,
    }
}

fn wire_ended_by(cause: HostProcessEndedBy) -> ProcessEndedBy {
    match cause {
        HostProcessEndedBy::Exit => ProcessEndedBy::Exit,
        HostProcessEndedBy::Stop => ProcessEndedBy::Stop,
        HostProcessEndedBy::LeaseExpiry => ProcessEndedBy::LeaseExpiry,
        HostProcessEndedBy::Timeout => ProcessEndedBy::Timeout,
        HostProcessEndedBy::ExpiresAt => ProcessEndedBy::ExpiresAt,
        HostProcessEndedBy::Vanished => ProcessEndedBy::Vanished,
    }
}

/// One record as the wire carries it (S6). `profile: host` and `confinement: none` are on every
/// record; an observation the daemon did not make is absent, never zero; environment values are
/// never on it.
#[allow(clippy::too_many_lines)] // One field per wire member, in the generated struct's order.
fn wire_record(
    record: &HostProcessRecord,
    observation: Option<HostProcessObservation>,
    now: DateTime<Utc>,
) -> ProcessRecord {
    let live = matches!(
        record.state,
        HostProcessState::Starting | HostProcessState::Running
    );
    let lease = match &record.lease {
        HostProcessLease::Ttl {
            ttl_ms, expires_at, ..
        } => {
            let state = if record.ended_by == Some(HostProcessEndedBy::LeaseExpiry)
                || (live && now >= *expires_at)
            {
                ProcessLeaseState::Expired
            } else {
                ProcessLeaseState::Active
            };
            ProcessLease {
                keep: false,
                keep_reason: ProcessPresence::Absent,
                renew_by: ProcessPresence::Present(ProcessTimestamp::from(*expires_at)),
                state: ProcessPresence::Present(Box::new(state)),
                ttl_ms: ProcessPresence::Present(number(*ttl_ms)),
            }
        }
        HostProcessLease::Keep { reason } => ProcessLease {
            keep: true,
            keep_reason: ProcessPresence::Present(reason.clone()),
            renew_by: ProcessPresence::Absent,
            state: ProcessPresence::Absent,
            ttl_ms: ProcessPresence::Absent,
        },
    };
    let exit = record.exit.as_ref().map(|exit| {
        Box::new(match exit {
            HostProcessExit::Code(code) => ProcessExit {
                code: ProcessPresence::Present(Number::from(*code)),
                signal: ProcessPresence::Absent,
            },
            HostProcessExit::Signal(signal) => ProcessExit {
                code: ProcessPresence::Absent,
                signal: ProcessPresence::Present(signal.clone()),
            },
        })
    });
    let observation = observation.filter(|_| live);
    let observed_at = observation
        .as_ref()
        .map(|observed| ProcessTimestamp::from(observed.observed_at));
    let cpu = observation.as_ref().and_then(|observed| {
        observed.cpu_usec.map(|usage| {
            Box::new(ProcessCpu {
                observed_at: observed_at.unwrap_or_else(|| ProcessTimestamp::from(now)),
                usage_usec: number(usage),
            })
        })
    });
    let memory = observation.as_ref().and_then(|observed| {
        observed.memory_bytes.map(|bytes| {
            Box::new(ProcessMemory {
                memory_bytes: number(bytes),
                observed_at: observed_at.unwrap_or_else(|| ProcessTimestamp::from(now)),
            })
        })
    });
    let listening = observation.as_ref().and_then(|observed| {
        observed.listening.as_ref().map(|sockets| {
            Box::new(ProcessListening {
                observed_at: observed_at.unwrap_or_else(|| ProcessTimestamp::from(now)),
                sockets: sockets
                    .iter()
                    .map(|socket| {
                        Box::new(ProcessListeningSocket {
                            address: socket.address.to_string(),
                            port: number(u64::from(socket.port)),
                            protocol: Box::new(match socket.protocol {
                                SocketProtocol::Tcp => ProcessSocketProtocol::Tcp,
                                SocketProtocol::Udp => ProcessSocketProtocol::Udp,
                            }),
                        })
                    })
                    .collect(),
            })
        })
    });
    ProcessRecord {
        argv: record.argv.clone(),
        confinement: Box::new(ProcessConfinement::None),
        cpu: cpu.into(),
        cwd: record.cwd.clone(),
        deadline: record
            .deadline
            .as_ref()
            .map(|deadline| {
                Box::new(ProcessDeadline {
                    at: ProcessTimestamp::from(deadline.at),
                    set_by: Box::new(match deadline.source {
                        HostProcessDeadlineSource::Timeout => ProcessDeadlineSource::Timeout,
                        HostProcessDeadlineSource::ExpiresAt => ProcessDeadlineSource::ExpiresAt,
                    }),
                })
            })
            .into(),
        ended_by: record
            .ended_by
            .filter(|_| record.state == HostProcessState::Ended)
            .map(|cause| Box::new(wire_ended_by(cause)))
            .into(),
        env_digest: Box::new(ProcessEnvDigest(record.env_digest.clone())),
        env_names: record.env_names.clone(),
        exit: exit.into(),
        expires_at: record.expires_at.map(ProcessTimestamp::from).into(),
        id: record.id.clone(),
        kind: Box::new(ProcessKind::Process),
        lease: Box::new(lease),
        listening: listening.into(),
        memory: memory.into(),
        observed_at: ProcessTimestamp::from(now),
        owner: ProcessPresence::Present(Box::new(ProcessOwner {
            name: record.owner_name.clone(),
            session: record.owner_session.clone(),
        })),
        profile: Box::new(ProcessProfile::Host),
        reason: record.reason.clone(),
        started_at: ProcessTimestamp::from(record.started_at),
        state: Box::new(wire_state(record.state)),
        timeout_ms: record.timeout_ms.map(number).into(),
    }
}

/// The record with what the driver observes of it now, when it is live and held.
fn observed(
    supervisor: Option<&Arc<dyn ProcessSupervisor>>,
    record: &HostProcessRecord,
    now: DateTime<Utc>,
) -> ProcessRecord {
    let observation = supervisor
        .filter(|_| {
            matches!(
                record.state,
                HostProcessState::Starting | HostProcessState::Running
            )
        })
        .and_then(|supervisor| supervisor.observe(&record.id).ok());
    wire_record(record, observation, now)
}

// ---------------------------------------------------------------------------------------------
// Admission and refusals.
// ---------------------------------------------------------------------------------------------

/// S2: the personal Unix socket, and a peer whose uid is the daemon's own. Decided from the
/// listener the router was built for and the kernel-derived subject, never from request data.
fn admission(
    app: &App,
    identity: &Identity,
    transport: SessionTransport,
) -> Result<(), &'static str> {
    if !matches!(transport, SessionTransport::Unix) {
        return Err(PROCESS_HOST_POSTURE_REFUSED);
    }
    if identity.subject != format!("local:{}", app.host_process_uid) {
        return Err(PROCESS_HOST_SUBJECT_MISMATCH);
    }
    Ok(())
}

/// A named refusal of this family as a failure response, with the status and message the wire
/// publishes for it.
fn refusal(request_id: &str, operation: Option<&str>, code: &str, address: &str) -> Response {
    let named = process_refusal(code).expect("every refusal this module raises is named");
    failure(
        StatusCode::from_u16(named.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        request_id,
        operation,
        named.class,
        code,
        named.message,
        Some(address),
        false,
    )
}

/// Records a refusal decided before anything was reserved, under the request's operation id.
#[allow(clippy::too_many_arguments)] // The exact durable refusal tuple.
async fn refuse<T>(
    app: &App,
    identity: &Identity,
    request_id: &str,
    kind: &str,
    address: &str,
    mutation: &BoundMutation<T>,
    code: &str,
    member: &str,
) -> Response {
    let response = refusal(request_id, Some(&mutation.op), code, member);
    refuse_before_dispatch_response(
        app, identity, request_id, kind, "POST", address, mutation, response,
    )
    .await
}

// ---------------------------------------------------------------------------------------------
// S1, S2, S4, S11: start.
// ---------------------------------------------------------------------------------------------

/// What a validated start carries to the store and the driver.
struct StartPlan {
    argv: Vec<String>,
    cwd: String,
    env: BTreeMap<String, String>,
    owner_session: String,
    owner_name: String,
    reason: String,
    lease: HostProcessLeaseRequest,
    timeout_ms: Option<u64>,
    expires_at: Option<String>,
}

fn fails_as<T: DeserializeOwned>(input: &Map<String, Value>, member: &str) -> bool {
    input
        .get(member)
        .is_some_and(|value| serde_json::from_value::<T>(value.clone()).is_err())
}

/// The named refusal of the first member, in the ADR's order, that does not even have its type.
fn malformed_member(input: &Map<String, Value>) -> Option<(&'static str, &'static str)> {
    let checks: [(bool, &'static str, &'static str); 8] = [
        (
            fails_as::<Vec<String>>(input, "argv"),
            PROCESS_ARGV_INVALID,
            "argv",
        ),
        (fails_as::<String>(input, "cwd"), PROCESS_CWD_INVALID, "cwd"),
        (
            fails_as::<ProcessEnv>(input, "env"),
            PROCESS_ENV_INVALID,
            "env",
        ),
        (
            fails_as::<ProcessOwner>(input, "owner"),
            PROCESS_OWNER_INVALID,
            "owner",
        ),
        (
            fails_as::<String>(input, "reason"),
            PROCESS_REASON_REQUIRED,
            "reason",
        ),
        (
            fails_as::<ProcessLeaseRequest>(input, "lease"),
            PROCESS_LEASE_INVALID,
            "lease",
        ),
        (
            fails_as::<Number>(input, "timeout_ms"),
            PROCESS_TIMEOUT_INVALID,
            "timeout_ms",
        ),
        (
            fails_as::<String>(input, "expires_at"),
            PROCESS_EXPIRES_AT_INVALID,
            "expires_at",
        ),
    ];
    checks
        .into_iter()
        .find_map(|(failed, code, member)| failed.then_some((code, member)))
}

fn bounded_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

/// S1, S4, S11, in the order of ADR 0036's table. `profile` was decided before this.
fn plan_start(input: &ProcessStartInput) -> Result<StartPlan, (&'static str, &'static str)> {
    let argv = input
        .argv
        .as_option()
        .cloned()
        .ok_or((PROCESS_ARGV_INVALID, "argv"))?;
    if argv.first().is_none_or(String::is_empty)
        || argv.len() > MAX_PROCESS_ARGV_ITEMS
        || argv.iter().map(String::len).sum::<usize>() > MAX_PROCESS_ARGV_BYTES
        || argv.iter().any(|item| item.contains('\0'))
    {
        return Err((PROCESS_ARGV_INVALID, "argv"));
    }
    let cwd = input
        .cwd
        .as_option()
        .cloned()
        .ok_or((PROCESS_CWD_INVALID, "cwd"))?;
    HostProcessSpec::check_cwd(FsPath::new(&cwd)).map_err(|_| (PROCESS_CWD_INVALID, "cwd"))?;
    let env = input
        .env
        .as_option()
        .map(|env| env.ess_extra.clone())
        .ok_or((PROCESS_ENV_INVALID, "env"))?;
    if env.len() > MAX_PROCESS_ENV_ITEMS
        || env
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum::<usize>()
            > MAX_PROCESS_ENV_BYTES
        || env
            .iter()
            .any(|(name, value)| !HostProcessSpec::env_name_is_valid(name) || value.contains('\0'))
    {
        return Err((PROCESS_ENV_INVALID, "env"));
    }
    let owner = input
        .owner
        .as_option()
        .ok_or((PROCESS_OWNER_INVALID, "owner"))?;
    if !bounded_text(&owner.session, MAX_PROCESS_OWNER_BYTES)
        || !bounded_text(&owner.name, MAX_PROCESS_OWNER_BYTES)
    {
        return Err((PROCESS_OWNER_INVALID, "owner"));
    }
    let reason = input
        .reason
        .as_option()
        .filter(|reason| bounded_text(reason, MAX_PROCESS_REASON_BYTES))
        .cloned()
        .ok_or((PROCESS_REASON_REQUIRED, "reason"))?;
    let lease = plan_lease(input.lease.as_option().map(AsRef::as_ref))?;
    let timeout_ms = match input.timeout_ms.as_option() {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or((PROCESS_TIMEOUT_INVALID, "timeout_ms"))?,
        ),
    };
    Ok(StartPlan {
        argv,
        cwd,
        env,
        owner_session: owner.session.clone(),
        owner_name: owner.name.clone(),
        reason,
        lease,
        timeout_ms,
        expires_at: input.expires_at.as_option().cloned(),
    })
}

/// S4: exactly one of a TTL within the lease bounds and `keep: true` with its own reason.
fn plan_lease(
    lease: Option<&ProcessLeaseRequest>,
) -> Result<HostProcessLeaseRequest, (&'static str, &'static str)> {
    let invalid = (PROCESS_LEASE_INVALID, "lease");
    let lease = lease.ok_or(invalid)?;
    let keep = lease.keep.as_option().copied().unwrap_or(false);
    if keep {
        if !lease.ttl_ms.is_absent() {
            return Err(invalid);
        }
        return lease
            .keep_reason
            .as_option()
            .filter(|reason| bounded_text(reason, MAX_PROCESS_REASON_BYTES))
            .map(|reason| HostProcessLeaseRequest::Keep {
                reason: reason.clone(),
            })
            .ok_or((PROCESS_KEEP_REASON_REQUIRED, "lease.keep_reason"));
    }
    if !lease.keep_reason.is_absent() {
        return Err(invalid);
    }
    let ttl_ms = lease
        .ttl_ms
        .as_option()
        .and_then(Number::as_u64)
        .filter(|ttl| {
            (substrate_wire::MIN_LEASE_TTL_MS..=substrate_wire::MAX_LEASE_TTL_MS).contains(ttl)
        })
        .ok_or(invalid)?;
    Ok(HostProcessLeaseRequest::Ttl { ttl_ms })
}

/// `sha256:<hex>` over the canonical JSON of the whole environment map: one digest, never a value.
fn env_digest(env: &BTreeMap<String, String>) -> String {
    let canonical = canonical_json(&json!(env)).unwrap_or_default();
    let digest = Sha256::digest(canonical.as_bytes());
    let mut text = String::with_capacity(7 + 64);
    text.push_str("sha256:");
    for byte in digest {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[allow(clippy::too_many_lines)] // Admission, ordered validation, durable reservation, dispatch.
pub(super) async fn process_start(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    Extension(transport): Extension<SessionTransport>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    const ADDRESS: &str = "/v1/processes";
    let request_id = request_id(&app, &headers);
    let mutation = match decode_mutation::<Value>(
        &app,
        &identity,
        HOST_PROCESS_START_KIND,
        "POST",
        ADDRESS,
        raw_query.as_deref(),
        body,
        &request_id,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let refused = |code: &'static str, member: &'static str| {
        refuse(
            &app,
            &identity,
            &request_id,
            HOST_PROCESS_START_KIND,
            ADDRESS,
            &mutation,
            code,
            member,
        )
    };
    if let Err(code) = admission(&app, &identity, transport) {
        return refused(code, "process").await;
    }
    let Some(input) = mutation.input.as_object() else {
        let response = schema_invalid(&request_id, Some(&mutation.op), "input");
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            HOST_PROCESS_START_KIND,
            "POST",
            ADDRESS,
            &mutation,
            response,
        )
        .await;
    };
    if input
        .keys()
        .any(|member| !START_MEMBERS.contains(&member.as_str()))
    {
        let response = schema_invalid(&request_id, Some(&mutation.op), "input");
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            HOST_PROCESS_START_KIND,
            "POST",
            ADDRESS,
            &mutation,
            response,
        )
        .await;
    }
    // S2: requested, never defaulted; `host` is the only profile.
    match input.get("profile") {
        None => return refused(PROCESS_PROFILE_REQUIRED, "profile").await,
        Some(Value::String(profile)) if profile == ProcessProfile::Host.as_str() => {}
        Some(_) => return refused(PROCESS_PROFILE_UNKNOWN, "profile").await,
    }
    let Ok(typed) = serde_json::from_value::<ProcessStartInput>(Value::Object(input.clone()))
    else {
        let Some((code, member)) = malformed_member(input) else {
            let response = schema_invalid(&request_id, Some(&mutation.op), "input");
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                HOST_PROCESS_START_KIND,
                "POST",
                ADDRESS,
                &mutation,
                response,
            )
            .await;
        };
        return refused(code, member).await;
    };
    let plan = match plan_start(&typed) {
        Ok(plan) => plan,
        Err((code, member)) => return refused(code, member).await,
    };
    let now = app.authority.now();
    // The driver's S11 rule decides both deadline refusals; the deadline itself is recomputed by
    // the store from the operation's own instant and enforced by the daemon's clock.
    if let Err(error) = Deadline::effective(plan.timeout_ms, plan.expires_at.as_deref(), now) {
        return if error.code == PROCESS_TIMEOUT_INVALID {
            refused(PROCESS_TIMEOUT_INVALID, "timeout_ms").await
        } else {
            refused(PROCESS_EXPIRES_AT_INVALID, "expires_at").await
        };
    }
    let expires_at = match plan.expires_at.as_deref().map(DateTime::parse_from_rfc3339) {
        None => None,
        Some(Ok(instant)) => Some(instant.with_timezone(&Utc)),
        Some(Err(_)) => return refused(PROCESS_EXPIRES_AT_INVALID, "expires_at").await,
    };
    // A valid start on a daemon that serves no host process: named, and nothing runs (S2).
    let Some(ctx) = app.process_ctx() else {
        return refused(PROCESS_HOST_UNSERVED, "process.host").await;
    };
    let scope = app.scope(&identity);
    let id = app.authority.process_id();
    let process = NewHostProcess {
        id: id.clone(),
        owner_session: plan.owner_session.clone(),
        owner_name: plan.owner_name.clone(),
        reason: plan.reason.clone(),
        argv: plan.argv.clone(),
        cwd: plan.cwd.clone(),
        env_names: plan.env.keys().cloned().collect(),
        env_digest: env_digest(&plan.env),
        lease: plan.lease.clone(),
        timeout_ms: plan.timeout_ms,
        expires_at,
    };
    let new = new_operation(
        &app,
        &identity,
        HOST_PROCESS_START_KIND,
        "POST",
        ADDRESS,
        &mutation,
        None,
        Some(id.clone()),
    );
    let limits = HostProcessLimits::default();
    let operation = mutation.op.clone();
    match ctx
        .io(|| {
            ctx.store
                .reserve_host_process_start(&new, &process, &limits)
        })
        .await
    {
        Ok(HostProcessStartReservation::Accepted(_)) => {}
        Ok(HostProcessStartReservation::Existing(reservation)) => {
            return reservation_response(Ok(reservation), &request_id, &operation)
                .unwrap_or_else(|| outcome_unknown(&request_id, &operation));
        }
        Ok(HostProcessStartReservation::Capacity(_)) => {
            return operation_ledger_capacity(&request_id);
        }
        Ok(HostProcessStartReservation::Refused { answer, .. }) => {
            return replay(&request_id, &operation, answer);
        }
        Err(error) => return store_failure(&request_id, Some(&operation), &error),
    }
    // Durable: the accepted operation and the `starting` record. The values reach the driver
    // and nothing else; the deadline is the daemon's clock's, not the driver's.
    let spec = HostProcessSpec {
        argv: plan.argv,
        cwd: PathBuf::from(&plan.cwd),
        env: plan.env,
        deadline: None,
    };
    if let Err(error) = Arc::clone(&ctx.supervisor).start(&id, &spec) {
        let (status, detail) = driver_detail(Some(&operation), &error);
        let at = ctx.now();
        if let Err(store_error) = ctx
            .io(|| {
                ctx.store.abandon_host_process_start(
                    &scope,
                    &operation,
                    at,
                    status.as_u16(),
                    &id,
                    &detail,
                )
            })
            .await
        {
            return store_failure(&request_id, Some(&operation), &store_error);
        }
        let _ = ctx.supervisor.forget(&id);
        return failure_detail(status, &request_id, detail);
    }
    let at = ctx.now();
    let record = match ctx
        .io(|| ctx.store.mark_host_process_running(&scope, &id, at))
        .await
    {
        Ok(HostProcessTransition::Applied(record) | HostProcessTransition::Unchanged(record)) => {
            *record
        }
        Ok(HostProcessTransition::Missing) => {
            return outcome_unknown(&request_id, &operation);
        }
        Err(error) => return store_failure(&request_id, Some(&operation), &error),
    };
    let result = wire_record(&record, None, at);
    let terminal_at = at.to_rfc3339();
    if let Err(error) = ctx
        .io(|| {
            ctx.store.complete_success(
                &scope,
                &operation,
                &terminal_at,
                StatusCode::OK.as_u16(),
                Some(&id),
                &result,
            )
        })
        .await
    {
        return store_failure(&request_id, Some(&operation), &error);
    }
    success(
        StatusCode::OK,
        Success::mutation(request_id, operation, result),
    )
}

// ---------------------------------------------------------------------------------------------
// S6: read and list.
// ---------------------------------------------------------------------------------------------

pub(super) async fn process_get(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    Extension(transport): Extension<SessionTransport>,
    headers: HeaderMap,
    Path(process_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    if let Err(code) = admission(&app, &identity, transport) {
        return refusal(&request_id, None, code, "process");
    }
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let scope = app.scope(&identity);
    match app
        .store_io(|| app.store.host_process(&scope, &process_id))
        .await
    {
        Ok(Some(record)) => {
            let now = app.authority.now();
            success(
                StatusCode::OK,
                Success::observed(
                    request_id,
                    observed(app.host_processes.as_ref(), &record, now),
                ),
            )
        }
        Ok(None) => refusal(&request_id, None, PROCESS_NOT_FOUND, "process"),
        Err(error) => store_failure(&request_id, None, &error),
    }
}

fn valid_cursor(cursor: &str) -> bool {
    !cursor.is_empty()
        && cursor.len() <= 64
        && cursor
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub(super) async fn process_list(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    Extension(transport): Extension<SessionTransport>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    if let Err(code) = admission(&app, &identity, transport) {
        return refusal(&request_id, None, code, "process");
    }
    let Ok(query) =
        serde_urlencoded::from_str::<ProcessListQuery>(raw_query.as_deref().unwrap_or(""))
    else {
        return schema_invalid(&request_id, None, "query");
    };
    let limit = query.limit.unwrap_or(MAX_PROCESS_LIST_ITEMS);
    if limit == 0
        || limit > MAX_PROCESS_LIST_ITEMS
        || query
            .cursor
            .as_deref()
            .is_some_and(|cursor| !valid_cursor(cursor))
    {
        return schema_invalid(&request_id, None, "query");
    }
    let filter = HostProcessFilter {
        owner_session: query.owner_session.clone(),
        name: query.name.clone(),
        state: query.state.map(store_state),
    };
    let scope = app.scope(&identity);
    let limits = HostProcessLimits::default();
    let page = match app
        .store_io(|| {
            app.store.list_host_processes(
                &scope,
                &filter,
                query.cursor.as_deref(),
                u64::from(limit),
                &limits,
            )
        })
        .await
    {
        Ok(page) => page,
        Err(error) => return store_failure(&request_id, None, &error),
    };
    let now = app.authority.now();
    let result = ProcessPage {
        items: page
            .items
            .iter()
            .map(|record| Box::new(observed(app.host_processes.as_ref(), record, now)))
            .collect(),
        next_cursor: page.next_after.into(),
        observed_at: ProcessTimestamp::from(now),
    };
    success(StatusCode::OK, Success::observed(request_id, result))
}

// ---------------------------------------------------------------------------------------------
// S3, S5: stop one, stop by owner.
// ---------------------------------------------------------------------------------------------

#[allow(clippy::too_many_lines)] // Admission, durable reservation, stop and completion stay adjacent.
pub(super) async fn process_stop(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    Extension(transport): Extension<SessionTransport>,
    headers: HeaderMap,
    Path(process_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v1/processes/{process_id}/stop");
    let mutation = match decode_mutation::<EmptyInput>(
        &app,
        &identity,
        HOST_PROCESS_STOP_KIND,
        "POST",
        &address,
        raw_query.as_deref(),
        body,
        &request_id,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(code) = admission(&app, &identity, transport) {
        return refuse(
            &app,
            &identity,
            &request_id,
            HOST_PROCESS_STOP_KIND,
            &address,
            &mutation,
            code,
            "process",
        )
        .await;
    }
    let scope = app.scope(&identity);
    // Bundle 0.20.0: `process.stop` requires `process.host`. A run that opened no driver answers
    // every stop `process.host-unserved`, a known record and an unknown id alike, and the record
    // stays as it is.
    let Some(ctx) = app.process_ctx() else {
        return refuse(
            &app,
            &identity,
            &request_id,
            HOST_PROCESS_STOP_KIND,
            &address,
            &mutation,
            PROCESS_HOST_UNSERVED,
            "process.host",
        )
        .await;
    };
    let new = new_operation(
        &app,
        &identity,
        HOST_PROCESS_STOP_KIND,
        "POST",
        &address,
        &mutation,
        None,
        Some(process_id.clone()),
    );
    let operation = mutation.op.clone();
    match ctx
        .io(|| {
            ctx.store
                .reserve_host_process_stop(&new, &process_id, HostProcessEndedBy::Stop)
        })
        .await
    {
        Ok(HostProcessStopReservation::Accepted(record)) => {
            // Bundle 0.20.0 vectors/http/process-stop.json: the answer is the record as recorded
            // `stopping`; the supervisor clock records the end.
            match ctx.accept_stop(&scope, &operation, &record).await {
                Ok(result) => success(
                    StatusCode::OK,
                    Success::mutation(request_id, operation, result),
                ),
                Err(error) => store_failure(&request_id, Some(&operation), &error),
            }
        }
        Ok(
            HostProcessStopReservation::AlreadyEnding(record)
            | HostProcessStopReservation::NotDue(record),
        ) => {
            // Already stopping or ended: the first recorded cause stands. The request is still
            // an operation in the ledger, answered with the record as it is.
            if let Some(response) = begin(
                &app,
                &identity,
                &request_id,
                HOST_PROCESS_STOP_KIND,
                "POST",
                &address,
                &mutation,
                None,
                Some(process_id.clone()),
            )
            .await
            {
                return response;
            }
            let result = wire_record(&record, None, ctx.now());
            let terminal_at = ctx.now().to_rfc3339();
            if let Err(error) = ctx
                .io(|| {
                    ctx.store.complete_success(
                        &scope,
                        &operation,
                        &terminal_at,
                        StatusCode::OK.as_u16(),
                        Some(&process_id),
                        &result,
                    )
                })
                .await
            {
                return store_failure(&request_id, Some(&operation), &error);
            }
            success(
                StatusCode::OK,
                Success::mutation(request_id, operation, result),
            )
        }
        Ok(HostProcessStopReservation::Existing(reservation)) => {
            reservation_response(Ok(reservation), &request_id, &operation)
                .unwrap_or_else(|| outcome_unknown(&request_id, &operation))
        }
        Ok(HostProcessStopReservation::Capacity(_)) => operation_ledger_capacity(&request_id),
        Ok(HostProcessStopReservation::Refused { answer, .. }) => {
            replay(&request_id, &operation, answer)
        }
        Err(error) => store_failure(&request_id, Some(&operation), &error),
    }
}

/// S5. The body is the input itself (bundle 0.20.0 binds the whole body to
/// `schemas/inputs/process-stop-by-owner.json`), so this request has no operation of its own:
/// each record it stops gets its own [`HOST_PROCESS_STOP_KIND`] operation, attributed to the
/// caller and recorded before the driver acts, and the answer is an observation.
pub(super) async fn process_stop_by_owner(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    Extension(transport): Extension<SessionTransport>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    if let Err(code) = admission(&app, &identity, transport) {
        return refusal(&request_id, None, code, "process");
    }
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let bytes = match read_bounded_body(body, &request_id).await {
        Ok(bytes) => bytes,
        Err(response) => return response,
    };
    let Ok(input) = serde_json::from_slice::<ProcessStopByOwnerInput>(&bytes) else {
        return schema_invalid(&request_id, None, "input");
    };
    // Bundle 0.20.0: `process.stop-by-owner` requires `process.host`; a run that opened no driver
    // answers it `process.host-unserved`, never an empty list.
    let Some(ctx) = app.process_ctx() else {
        return refusal(&request_id, None, PROCESS_HOST_UNSERVED, "process.host");
    };
    let except_keep = input.except_keep.as_option().copied().unwrap_or(false);
    let owner_session = input.owner_session;
    let scope = app.scope(&identity);
    // The subject is the only authority: the selection is keyed by it, and the owner session is
    // a label beneath it. A session that labels nothing selects nothing.
    let selected = match ctx
        .io(|| {
            ctx.store
                .host_processes_for_owner(&scope, &owner_session, except_keep)
        })
        .await
    {
        Ok(selected) => selected,
        Err(error) => return store_failure(&request_id, None, &error),
    };
    let mut stopped = Vec::new();
    for id in selected {
        let now = ctx.now();
        let Some(mut new) = supervisor_operation(&scope, &id, now) else {
            continue;
        };
        // One operation per record, attributed to the caller, not to the supervisor.
        new.operation = format!("op_owner_stop_{}", Ulid::generate());
        new.actor.clone_from(&identity.actor);
        new.principal.clone_from(&identity.principal);
        let reserved = ctx
            .io(|| {
                ctx.store
                    .reserve_host_process_stop(&new, &id, HostProcessEndedBy::Stop)
            })
            .await;
        if let Ok(HostProcessStopReservation::Accepted(record)) = reserved {
            if let Err(error) = ctx.accept_stop(&scope, &new.operation, &record).await {
                tracing::warn!(process = %id, %error, "host process stop was not completed");
            }
            stopped.push(Box::new(ProcessStopped {
                process: id,
                operation: new.operation,
            }));
        }
    }
    let result = ProcessStopByOwnerResult {
        owner_session,
        stopped,
    };
    success(StatusCode::OK, Success::observed(request_id, result))
}

// ---------------------------------------------------------------------------------------------
// S4: lease renewal.
// ---------------------------------------------------------------------------------------------

#[allow(clippy::too_many_lines)] // Durable refusal and renewal stay in one mutation path.
pub(super) async fn process_lease_renew(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    Extension(transport): Extension<SessionTransport>,
    headers: HeaderMap,
    Path(process_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v1/processes/{process_id}/lease/renew");
    // Bundle 0.20.0 schemas/inputs/process-lease-renew.json is closed: exactly `{ "ttl_ms": n }`.
    // Anything else — `keep` included, because a lease never becomes a keep after start — is the
    // strict schema's refusal, recorded under the request's operation.
    let mutation = match decode_mutation::<LeaseRenewInput>(
        &app,
        &identity,
        HOST_PROCESS_RENEW_KIND,
        "POST",
        &address,
        raw_query.as_deref(),
        body,
        &request_id,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(code) = admission(&app, &identity, transport) {
        return refuse(
            &app,
            &identity,
            &request_id,
            HOST_PROCESS_RENEW_KIND,
            &address,
            &mutation,
            code,
            "process",
        )
        .await;
    }
    let ttl_ms = mutation.input.ttl_ms;
    if !(substrate_wire::MIN_LEASE_TTL_MS..=substrate_wire::MAX_LEASE_TTL_MS).contains(&ttl_ms) {
        let response = schema_invalid(&request_id, Some(&mutation.op), "input");
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            HOST_PROCESS_RENEW_KIND,
            "POST",
            &address,
            &mutation,
            response,
        )
        .await;
    }
    // Bundle 0.20.0: `process.lease.renew` requires `process.host`. A run that opened no driver
    // answers every renewal `process.host-unserved`, and the lease stays as it is.
    if app.host_processes.is_none() {
        return refuse(
            &app,
            &identity,
            &request_id,
            HOST_PROCESS_RENEW_KIND,
            &address,
            &mutation,
            PROCESS_HOST_UNSERVED,
            "process.host",
        )
        .await;
    }
    let scope = app.scope(&identity);
    match app
        .store_io(|| app.store.host_process(&scope, &process_id))
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return refuse(
                &app,
                &identity,
                &request_id,
                HOST_PROCESS_RENEW_KIND,
                &address,
                &mutation,
                PROCESS_NOT_FOUND,
                "process",
            )
            .await;
        }
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    }
    let operation = mutation.op.clone();
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        HOST_PROCESS_RENEW_KIND,
        "POST",
        &address,
        &mutation,
        None,
        Some(process_id.clone()),
    )
    .await
    {
        return response;
    }
    let now = app.authority.now();
    let mut answered = None;
    let renewed = app
        .store_io(|| {
            app.store.renew_host_process_lease_with(
                &scope,
                &operation,
                now,
                StatusCode::OK.as_u16(),
                &process_id,
                ttl_ms,
                |record| {
                    let result = wire_record(record, None, now);
                    let value = serde_json::to_value(&result)?;
                    answered = Some(result);
                    Ok(value)
                },
            )
        })
        .await;
    let (status, detail) = match renewed {
        Ok(HostProcessRenewal::Renewed(_)) => {
            if let Some(result) = answered {
                return success(
                    StatusCode::OK,
                    Success::mutation(request_id, operation, result),
                );
            }
            return outcome_unknown(&request_id, &operation);
        }
        Ok(HostProcessRenewal::Refused(refused)) => (
            StatusCode::from_u16(refused.status()).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY),
            refused.error_detail(&operation),
        ),
        // A lease that already expired, or a record that is stopping or ended, is not renewed:
        // the supervisor's expiry, or the end, decided it first.
        Ok(HostProcessRenewal::Expired(_) | HostProcessRenewal::NotLive(_)) => (
            StatusCode::CONFLICT,
            ErrorDetail {
                class: ErrorClass::Conflict,
                code: "lease.expired".to_owned(),
                message: "An expired lease cannot be renewed.".to_owned(),
                retriable: false,
                address: Some("lease".to_owned()),
                operation: Some(operation.clone()),
            },
        ),
        Err(error) => return store_failure(&request_id, Some(&operation), &error),
    };
    let terminal_at = now.to_rfc3339();
    if let Err(error) = app
        .store_io(|| {
            app.store.complete_error(
                &scope,
                &operation,
                &terminal_at,
                status.as_u16(),
                Some(&process_id),
                &detail,
            )
        })
        .await
    {
        return store_failure(&request_id, Some(&operation), &error);
    }
    failure_detail(status, &request_id, detail)
}

// ---------------------------------------------------------------------------------------------
// S7: logs.
// ---------------------------------------------------------------------------------------------

pub(super) async fn process_logs(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    Extension(transport): Extension<SessionTransport>,
    headers: HeaderMap,
    Path(process_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    if let Err(code) = admission(&app, &identity, transport) {
        return refusal(&request_id, None, code, "process");
    }
    let Ok(query) =
        serde_urlencoded::from_str::<ProcessLogsQuery>(raw_query.as_deref().unwrap_or(""))
    else {
        return schema_invalid(&request_id, None, "query");
    };
    let budget = query.budget();
    if budget == 0 || budget > MAX_PROCESS_LOG_TAIL_BYTES {
        return schema_invalid(&request_id, None, "query");
    }
    let scope = app.scope(&identity);
    // The subject's record first: another subject's log answers exactly as an absent one.
    match app
        .store_io(|| app.store.host_process(&scope, &process_id))
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return refusal(&request_id, None, PROCESS_NOT_FOUND, "process"),
        Err(error) => return store_failure(&request_id, None, &error),
    }
    // `process.logs.get` carries no capability predicate: the driver's reader when this run
    // opened one, else the stored logs an earlier run left. A record with no log has an empty
    // tail.
    let reader: Option<&dyn ProcessLogs> = match app.host_processes.as_deref() {
        Some(supervisor) => Some(supervisor as &dyn ProcessLogs),
        None => app.host_process_logs.as_deref(),
    };
    let selection = query.selection();
    let now = app.authority.now();
    let read =
        reader.map(|reader| reader.log_tail(&process_id, driver_selection(selection), budget));
    let tail = match read {
        None => None,
        Some(Ok(tail)) => Some(tail),
        Some(Err(error)) if error.class == DriverErrorClass::NotFound => None,
        Some(Err(error)) => {
            let (status, detail) = driver_detail(None, &error);
            return failure_detail(status, &request_id, detail);
        }
    };
    let (lines, discarded, retained) = tail.map_or((Vec::new(), 0, 0), |tail| {
        (
            tail.lines
                .into_iter()
                .map(|line| {
                    let text = String::from_utf8(line.bytes.clone());
                    Box::new(ProcessLogLine {
                        full_bytes: number(line.full_len),
                        lossy: text.is_err(),
                        stream: Box::new(match line.stream {
                            DriverLogStream::Stdout => ProcessLogStream::Stdout,
                            DriverLogStream::Stderr => ProcessLogStream::Stderr,
                        }),
                        text: text.unwrap_or_else(|_| String::from_utf8_lossy(&line.bytes).into()),
                        truncated: line.truncated,
                    })
                })
                .collect(),
            tail.discarded_bytes,
            tail.retained_bytes,
        )
    });
    let result = ProcessLogTail {
        confinement: Box::new(ProcessConfinement::None),
        discarded_bytes: number(discarded),
        lines,
        observed_at: ProcessTimestamp::from(now),
        process: process_id,
        profile: Box::new(ProcessProfile::Host),
        retained_bytes: number(retained),
        stream: Box::new(selection),
    };
    success(StatusCode::OK, Success::observed(request_id, result))
}

fn driver_selection(selection: ProcessLogStreamSelection) -> LogStreamSelection {
    match selection.as_str() {
        "stdout" => LogStreamSelection::Stdout,
        "stderr" => LogStreamSelection::Stderr,
        _ => LogStreamSelection::Both,
    }
}

#[cfg(test)]
mod tests;
