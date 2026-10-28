use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::future::Future;
use std::hash::{Hash as _, Hasher as _};
use std::sync::{Arc, Weak};

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use parking_lot::Mutex as ParkingMutex;
use substrate_host::{
    Driver, DriverError, DriverErrorClass, ProcessLogs, ProcessSupervisor, WorkspaceDestroyProgress,
};
#[cfg(test)]
use substrate_store::NewOperation;
use substrate_store::{
    CommitEffectSink, ExecWrite, ExpiredLease, LeaseClock, LeaseResource, Scope, Store, StoreError,
    WorkspaceAdmission,
};
use substrate_wire::{
    ExecSignalInput, ExecState, OperationState, WorkspaceAbsence, WorkspaceKind, WorkspaceOrigin,
    WorkspaceState,
};
use tokio::sync::{Mutex, Notify, OwnedMutexGuard, Semaphore};
use ulid::Ulid;

use crate::delegation::DelegatedContextPolicy;

use super::events::{EventStreamLimits, EventStreamPolicy, EventWakeups};
use super::metrics::MetricsStreamPolicy;
use super::operations::{driver_detail, linux_lease_clock, stored_exec};
use super::sessions::{PendingContainments, PipeAttachmentLimits, PipeSessionPolicy};
use super::{
    LEASE_CLEANUP_BATCH, MAINTENANCE_DRIVER_TIMEOUT, PROVISIONAL_RECOVERY_BATCH,
    RESTART_RECONCILE_BATCH, WORKSPACE_CLEANUP_BATCH, WORKSPACE_LOCK_STRIPES,
};

#[derive(Default)]
pub(super) struct WorkspaceLockDomains {
    subjects: ParkingMutex<HashMap<Scope, Weak<WorkspaceLockDomain>>>,
}

pub(super) struct WorkspaceLockDomain {
    pub(super) stripes: Vec<Arc<Mutex<()>>>,
}

pub(super) struct WorkspaceGuard {
    _domain: Arc<WorkspaceLockDomain>,
    _guard: OwnedMutexGuard<()>,
}

impl WorkspaceLockDomains {
    pub(super) fn domain(&self, scope: &Scope) -> Arc<WorkspaceLockDomain> {
        let mut subjects = self.subjects.lock();
        subjects.retain(|_, domain| domain.strong_count() != 0);
        if let Some(domain) = subjects.get(scope).and_then(Weak::upgrade) {
            return domain;
        }
        let domain = Arc::new(WorkspaceLockDomain {
            stripes: (0..WORKSPACE_LOCK_STRIPES)
                .map(|_| Arc::new(Mutex::new(())))
                .collect(),
        });
        subjects.insert(scope.clone(), Arc::downgrade(&domain));
        domain
    }
}

fn workspace_stripe(domain: &WorkspaceLockDomain, workspace: &str) -> usize {
    let mut hasher = DefaultHasher::new();
    workspace.hash(&mut hasher);
    usize::try_from(hasher.finish()).unwrap_or(usize::MAX) % domain.stripes.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletedDriverAction {
    Acknowledge,
    DiscardSuperseded,
    Retain,
}

pub(super) fn completed_driver_action(
    scope_count: usize,
    exact_count: usize,
    any_superseded: bool,
    failed: bool,
) -> CompletedDriverAction {
    if scope_count != 0 && !failed && exact_count == scope_count {
        CompletedDriverAction::Acknowledge
    } else if !failed && any_superseded {
        CompletedDriverAction::DiscardSuperseded
    } else {
        CompletedDriverAction::Retain
    }
}

#[derive(Debug, Clone)]
pub struct Identity {
    pub subject: String,
    pub actor: String,
    pub principal: Option<String>,
}

/// Transport fact selected by the listener, never by caller-written HTTP data (ADR 0027).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionTransport {
    Unix,
    DevelopmentTcp,
    HostedTls { exporter: [u8; 32] },
}

pub trait Authority: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    fn request_id(&self) -> String;
    fn workspace_id(&self) -> String;
    fn exec_id(&self) -> String;
    fn session_id(&self) -> String;
    fn snapshot_id(&self) -> String;
    /// A fresh host-process id (ADR 0036): `proc_` and ASCII alphanumerics, bundle `0.20.0`
    /// `process-id`. Defaulted, so an authority written before the family existed still mints one.
    fn process_id(&self) -> String {
        format!("{}{}", substrate_wire::PROCESS_ID_PREFIX, Ulid::generate())
    }
    /// Returns durable wall and boot-relative evidence for lease accounting.
    ///
    /// # Errors
    ///
    /// Returns an error when Linux boot identity or boot-relative time cannot be observed.
    fn lease_clock(&self) -> Result<LeaseClock, String>;
}

pub struct SystemAuthority;

impl Authority for SystemAuthority {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn request_id(&self) -> String {
        format!("req_{}", Ulid::generate())
    }

    fn workspace_id(&self) -> String {
        format!("ws_{}", Ulid::generate())
    }

    fn exec_id(&self) -> String {
        format!("ex_{}", Ulid::generate())
    }

    fn session_id(&self) -> String {
        format!("ses_{}", Ulid::generate())
    }

    fn snapshot_id(&self) -> String {
        format!("snap_{}", Ulid::generate())
    }

    fn lease_clock(&self) -> Result<LeaseClock, String> {
        linux_lease_clock()
    }
}

#[cfg(test)]
pub(super) type RefusalBeforeRecordHook = Arc<dyn Fn(&NewOperation) + Send + Sync>;

pub struct App {
    pub store: Arc<Store>,
    pub driver: Arc<dyn Driver>,
    pub deployment: String,
    /// What this deployment will accept as a delegated context, and whether it insists on one.
    ///
    /// Immutable for the process's lifetime and resolved at startup, like an egress aperture: a
    /// trust anchor a request could move is not a trust anchor (ADR 0011).
    pub(super) delegated_context: DelegatedContextPolicy,
    pub(super) authority: Arc<dyn Authority>,
    pub(super) restart_cutoff: DateTime<Utc>,
    pub(super) event_wakeups: Arc<EventWakeups>,
    pub(super) event_stream_limits: Arc<EventStreamLimits>,
    pub(super) event_stream_policy: EventStreamPolicy,
    /// Metrics streams are capped by the same permit shape the event stream uses: one global
    /// semaphore and one per subject, both released by RAII when the stream ends.
    pub(super) metrics_stream_limits: Arc<EventStreamLimits>,
    pub(super) metrics_stream_policy: MetricsStreamPolicy,
    pub(super) pipe_attachment_limits: Arc<PipeAttachmentLimits>,
    pub(super) pipe_session_policy: PipeSessionPolicy,
    pub allow_unrecorded_sessions: bool,
    /// The host-process driver (ADR 0036), present only when the composition root opened it over
    /// a usable delegated cgroup root for a daemon whose one listener is the personal Unix socket.
    /// Absent, every start is `process.host-unserved` and nothing runs; nothing selects a
    /// confined profile instead.
    pub host_processes: Option<Arc<dyn ProcessSupervisor>>,
    /// The stored host-process logs, read when this run opened no host-process driver: the logs an
    /// earlier run left beside their records stay readable, because `process.logs.get` carries no
    /// capability predicate (S7). With a driver present its own reader answers instead.
    pub host_process_logs: Option<Arc<dyn ProcessLogs>>,
    /// The uid a host-process peer must have: the daemon's own effective uid (S2). A peer of any
    /// other uid is `process.host-subject-mismatch`, because a host process runs as this uid.
    pub host_process_uid: u32,
    /// Whether this run reconciled the host-process records an earlier run left (S3). A pass with a
    /// failed re-attachment leaves it false, so the next sweep retries.
    pub(super) host_process_reconciled: Mutex<bool>,
    /// What this run's host-process driver is known to hold, and the stops it has begun.
    pub(super) host_process_run: Arc<super::processes::DriverRun>,
    /// The open event streams of each host-process record, at most
    /// `MAX_PROCESS_EVENT_READERS` per record (S12).
    pub(super) process_event_readers: Arc<super::process_events::ProcessEventReaders>,
    /// This daemon run's identity, minted at construction and stamped by the store on every
    /// host-process record it starts: a record of any other run is reconciled as an earlier
    /// run's (S3), whatever its wall-clock `started_at` says.
    pub(super) host_process_run_id: String,
    lease_sweep: Mutex<()>,
    workspace_recovery: Mutex<()>,
    provisional_recovery: Mutex<()>,
    workspace_locks: WorkspaceLockDomains,
    maintenance_nudge: Notify,
    /// Claimed sessions whose containment kill is unproven, retried by `contain_pending`.
    pub(super) containments: PendingContainments,
    pub(super) blocking_store_slots: Arc<Semaphore>,
    /// The operational telemetry every control listener records into and the operational
    /// listener reads (ADR 0045). It holds counts only, never a request's content.
    pub(crate) telemetry: Arc<super::health::Telemetry>,
    #[cfg(test)]
    pub(super) refusal_before_record: ParkingMutex<Option<RefusalBeforeRecordHook>>,
}

impl App {
    pub fn new(
        store: Arc<Store>,
        driver: Arc<dyn Driver>,
        deployment: impl Into<String>,
    ) -> Arc<Self> {
        Self::with_authority(store, driver, deployment, Arc::new(SystemAuthority))
    }

    pub fn with_authority(
        store: Arc<Store>,
        driver: Arc<dyn Driver>,
        deployment: impl Into<String>,
        authority: Arc<dyn Authority>,
    ) -> Arc<Self> {
        Self::with_delegated_context(
            store,
            driver,
            deployment,
            authority,
            DelegatedContextPolicy::none(),
        )
    }

    /// The composition root's constructor: everything above, plus the configured trust anchor.
    ///
    /// Separate rather than a fifth parameter on `with_authority`, because a test that says nothing
    /// about delegated context should keep meaning "no trust anchor configured" — which is a real
    /// posture (ADR 0011: until an issuer ships, the field is optional everywhere), not a default
    /// standing in for one.
    pub fn with_delegated_context(
        store: Arc<Store>,
        driver: Arc<dyn Driver>,
        deployment: impl Into<String>,
        authority: Arc<dyn Authority>,
        delegated_context: DelegatedContextPolicy,
    ) -> Arc<Self> {
        let restart_cutoff = authority.now();
        let host_process_run_id = format!("run_{}", Ulid::generate());
        store.set_host_process_run(host_process_run_id.clone());
        let event_wakeups = Arc::new(EventWakeups::default());
        let event_stream_policy = EventStreamPolicy::production();
        let metrics_stream_policy = MetricsStreamPolicy::production();
        let pipe_session_policy = PipeSessionPolicy::production();
        let effect_sink: Arc<dyn CommitEffectSink> = event_wakeups.clone();
        store.set_commit_effect_sink(effect_sink);
        Arc::new(Self {
            store,
            driver,
            deployment: deployment.into(),
            delegated_context,
            authority,
            restart_cutoff,
            event_wakeups,
            event_stream_limits: EventStreamLimits::new(
                event_stream_policy.global_streams,
                event_stream_policy.streams_per_subject,
            ),
            event_stream_policy,
            metrics_stream_limits: EventStreamLimits::new(
                metrics_stream_policy.global_streams,
                metrics_stream_policy.streams_per_subject,
            ),
            metrics_stream_policy,
            pipe_attachment_limits: PipeAttachmentLimits::new(
                pipe_session_policy.global_attachments,
                pipe_session_policy.attachments_per_subject,
            ),
            pipe_session_policy,
            allow_unrecorded_sessions: false,
            host_processes: None,
            host_process_logs: None,
            host_process_uid: nix::unistd::geteuid().as_raw(),
            host_process_reconciled: Mutex::new(false),
            host_process_run: Arc::default(),
            process_event_readers: Arc::default(),
            host_process_run_id,
            lease_sweep: Mutex::new(()),
            workspace_recovery: Mutex::new(()),
            provisional_recovery: Mutex::new(()),
            workspace_locks: WorkspaceLockDomains::default(),
            maintenance_nudge: Notify::new(),
            containments: PendingContainments::default(),
            blocking_store_slots: Arc::new(Semaphore::new(16)),
            telemetry: Arc::default(),
            #[cfg(test)]
            refusal_before_record: ParkingMutex::new(None),
        })
    }

    pub(super) fn scope(&self, identity: &Identity) -> Scope {
        Scope {
            deployment: self.deployment.clone(),
            subject: identity.subject.clone(),
        }
    }

    pub(super) fn lease_clock(&self) -> Result<LeaseClock, String> {
        self.authority.lease_clock()
    }

    pub(super) async fn store_io<T, F>(&self, operation: F) -> T
    where
        T: Send,
        F: FnOnce() -> T + Send,
    {
        bounded_blocking(&self.blocking_store_slots, operation).await
    }

    pub(super) async fn lock_workspace(&self, scope: &Scope, workspace: &str) -> WorkspaceGuard {
        let domain = self.workspace_locks.domain(scope);
        let stripe = workspace_stripe(&domain, workspace);
        let lock = Arc::clone(&domain.stripes[stripe]);
        let guard = lock.lock_owned().await;
        WorkspaceGuard {
            _domain: domain,
            _guard: guard,
        }
    }

    /// Locks every workspace a start involves: its own and each writable Git root (ADR 0038
    /// kind 2). Each stripe is taken once, in ascending order, so two starts naming the same
    /// workspaces in opposite roles cannot deadlock, and two names sharing a stripe do not
    /// deadlock on themselves.
    pub(super) async fn lock_workspaces(
        &self,
        scope: &Scope,
        workspaces: &[&str],
    ) -> Vec<WorkspaceGuard> {
        let domain = self.workspace_locks.domain(scope);
        let mut stripes: Vec<usize> = workspaces
            .iter()
            .map(|workspace| workspace_stripe(&domain, workspace))
            .collect();
        stripes.sort_unstable();
        stripes.dedup();
        let mut guards = Vec::with_capacity(stripes.len());
        for stripe in stripes {
            let lock = Arc::clone(&domain.stripes[stripe]);
            guards.push(WorkspaceGuard {
                _domain: Arc::clone(&domain),
                _guard: lock.lock_owned().await,
            });
        }
        guards
    }

    pub(super) fn nudge_maintenance(&self) {
        self.maintenance_nudge.notify_one();
    }

    pub async fn maintenance_notified(&self) {
        self.maintenance_nudge.notified().await;
    }

    pub(super) async fn admit_workspace(
        &self,
        scope: &Scope,
        workspace: &str,
    ) -> Result<WorkspaceAdmission, StoreError> {
        let clock = self.lease_clock().ok();
        let admission = self
            .store_io(|| self.store.admit_workspace(scope, workspace, clock.as_ref()))
            .await?;
        if matches!(
            admission,
            WorkspaceAdmission::Frozen {
                newly_frozen: true,
                ..
            }
        ) {
            self.nudge_maintenance();
        }
        Ok(admission)
    }

    pub async fn sweep_expired(&self) {
        let _guard = self.lease_sweep.lock().await;
        // First: a restarted daemon's host-process stops and starts are concluded by their own
        // family's rules (ADR 0036 S1, S3) before the generic restart pass marks every open
        // operation unknown. Then the supervisor clock fires due leases and deadlines.
        self.supervise_host_processes().await;
        let observed_at = self.authority.now();
        let _ = self
            .store_io(|| {
                self.store.reconcile_after_restart(
                    &self.deployment,
                    self.restart_cutoff,
                    observed_at,
                    RESTART_RECONCILE_BATCH,
                )
            })
            .await;
        self.reconcile_provisional_resources().await;
        self.persist_completed_execs().await;
        // An exec recorded `unknown` because the driver could not prove its cgroup empty may
        // still hold processes, and acknowledging it above did not end them. Every round kills
        // those trees again, before any early return below, so none waits for a restart. Its
        // recorded observation stays `unknown`; a failure here is retried next round.
        let _ = run_maintenance_driver(self.driver.retry_unproven_exec_trees()).await;
        let Ok(clock) = self.lease_clock() else {
            return;
        };
        self.reconcile_destroying_workspaces_at(clock.wall).await;
        let _ = self
            .store_io(|| {
                self.store
                    .prune_expired_snapshots(&self.deployment, clock.wall)
            })
            .await;
        let Ok(candidates) = self
            .store_io(|| {
                self.store
                    .lease_cleanup_candidates(&self.deployment, &clock, LEASE_CLEANUP_BATCH)
            })
            .await
        else {
            return;
        };
        for candidate in candidates {
            let workspace_id = match &candidate.resource {
                LeaseResource::Workspace { .. } => &candidate.id,
                LeaseResource::Exec { workspace_id } => workspace_id,
            };
            let _workspace_guard = self.lock_workspace(&candidate.scope, workspace_id).await;
            // The claim decides by a clock it reads inside its own transaction, never by `clock`:
            // a start or renewal can issue and commit a lease from a later reading in between,
            // and `clock` would then read as a regression that expires it
            // (`story:pty-session-refuses-first-attach-under-load`).
            let claim = self
                .store_io(|| {
                    self.store
                        .claim_expired_lease_reading(&candidate, || self.lease_clock().ok())
                })
                .await;
            match claim {
                Ok(Some((claimed, claimed_at))) => {
                    self.cleanup_expired(claimed, claimed_at.wall).await;
                }
                Ok(None) => {}
                // Invariant 3: a due lease the store cannot claim is a named cleanup failure,
                // retried with the same backoff as any other, never a silent skip.
                Err(error) => {
                    let code = error.to_string();
                    let _ = self
                        .store_io(|| {
                            self.store
                                .record_lease_cleanup_failure(&candidate, clock.wall, &code)
                        })
                        .await;
                }
            }
        }
    }

    pub async fn reconcile_destroying_workspaces(&self) {
        self.reconcile_destroying_workspaces_at(self.authority.now())
            .await;
    }

    async fn reconcile_provisional_resources(&self) {
        let Ok(_guard) = self.provisional_recovery.try_lock() else {
            return;
        };
        self.reconcile_provisional_workspaces().await;
        self.reconcile_provisional_execs().await;
    }

    async fn reconcile_provisional_workspaces(&self) {
        let workspaces = self
            .store_io(|| {
                self.store.recovery_workspaces(
                    &self.deployment,
                    self.restart_cutoff,
                    PROVISIONAL_RECOVERY_BATCH,
                )
            })
            .await
            .unwrap_or_default();
        for candidate in workspaces {
            let _workspace_guard = self
                .lock_workspace(&candidate.scope, &candidate.resource.id)
                .await;
            match run_maintenance_driver(self.driver.observe_workspace(
                &candidate.resource.id,
                &candidate.root_name,
                &candidate.resource,
            ))
            .await
            {
                Ok(observed) => {
                    let _ = self
                        .store_io(|| {
                            self.store.complete_workspace(
                                &candidate.scope,
                                &candidate.operation,
                                &self.authority.now().to_rfc3339(),
                                StatusCode::CREATED.as_u16(),
                                &candidate.root_name,
                                &observed,
                            )
                        })
                        .await;
                }
                Err(error) if error.class == DriverErrorClass::NotFound => {
                    let (status, mut detail) = driver_detail(Some(&candidate.operation), &error);
                    detail.retriable = false;
                    let _ = self
                        .store_io(|| {
                            self.store.complete_dispatch_absence(
                                &candidate.scope,
                                &candidate.operation,
                                &self.authority.now().to_rfc3339(),
                                status.as_u16(),
                                "workspace",
                                &candidate.resource.id,
                                &detail,
                            )
                        })
                        .await;
                }
                Err(_) => {}
            }
        }
    }

    async fn reconcile_provisional_execs(&self) {
        let execs = self
            .store_io(|| {
                self.store.recovery_execs(
                    &self.deployment,
                    self.restart_cutoff,
                    PROVISIONAL_RECOVERY_BATCH,
                )
            })
            .await
            .unwrap_or_default();
        for candidate in execs {
            let _workspace_guard = self
                .lock_workspace(&candidate.scope, &candidate.stored.resource.workspace)
                .await;
            match run_maintenance_driver(self.driver.observe_exec(&candidate.stored.resource.id))
                .await
            {
                Ok(observation) => {
                    if candidate.operation_state == OperationState::Terminal {
                        let _ = self
                            .store_io(|| {
                                self.store
                                    .put_exec(&candidate.scope, &stored_exec(&observation))
                            })
                            .await;
                    } else {
                        let _ = self
                            .store_io(|| {
                                self.store.complete_exec(
                                    &candidate.scope,
                                    &candidate.operation,
                                    &self.authority.now().to_rfc3339(),
                                    StatusCode::ACCEPTED.as_u16(),
                                    &observation.resource,
                                    &observation.stdout,
                                    &observation.stderr,
                                    observation.stdout_truncated,
                                    observation.stderr_truncated,
                                    observation.output_complete,
                                    observation.cgroup.as_deref(),
                                    observation.leader_pid,
                                )
                            })
                            .await;
                    }
                }
                Err(error) if error.class == DriverErrorClass::NotFound => {
                    // Absence after restart proves only that no process remains now. It cannot
                    // prove whether dispatch crossed the launch barrier before the crash. Preserve
                    // the durable `unknown` operation/exec result while recording physical absence
                    // so cleanup can proceed without inventing a definitive not-found outcome.
                    let _ = self
                        .store_io(|| {
                            self.store
                                .mark_exec_physically_absent(&candidate, self.authority.now())
                        })
                        .await;
                }
                Err(_) => {}
            }
        }
    }

    async fn reconcile_destroying_workspaces_at(&self, now: DateTime<Utc>) {
        let Ok(_recovery_guard) = self.workspace_recovery.try_lock() else {
            return;
        };
        let Ok(pending) = self
            .store_io(|| {
                self.store
                    .due_destroying_workspaces(&self.deployment, now, WORKSPACE_CLEANUP_BATCH)
            })
            .await
        else {
            return;
        };
        for pending in pending {
            let _workspace_guard = self.lock_workspace(&pending.scope, &pending.id).await;
            let still_destroying = self
                .store_io(|| self.store.workspace(&pending.scope, &pending.id))
                .await
                .ok()
                .flatten()
                .is_some_and(|(root_name, workspace)| {
                    root_name == pending.root_name && workspace.state == WorkspaceState::Destroying
                });
            if !still_destroying {
                continue;
            }
            let absence = match run_maintenance_driver(end_workspace(
                self.driver.as_ref(),
                &pending.id,
                &pending.root_name,
                pending.origin,
            ))
            .await
            {
                Ok(WorkspaceDestroyProgress::Absent(value)) => value,
                Ok(WorkspaceDestroyProgress::Pending { removed_items }) => {
                    let _ = self
                        .store_io(|| {
                            self.store.record_workspace_cleanup_progress(
                                &pending,
                                self.authority.now(),
                                removed_items,
                            )
                        })
                        .await;
                    continue;
                }
                Err(error) if error.class == DriverErrorClass::NotFound => WorkspaceAbsence {
                    kind: WorkspaceKind::Workspace,
                    id: pending.id.clone(),
                    absent: true,
                    observed_at: self.authority.now(),
                },
                Err(error) => {
                    let _ = self
                        .store_io(|| {
                            self.store.record_workspace_cleanup_failure(
                                &pending,
                                self.authority.now(),
                                error.code,
                            )
                        })
                        .await;
                    tracing::warn!(
                        workspace = %pending.id,
                        code = error.code,
                        "workspace destroy reconciliation will retry"
                    );
                    continue;
                }
            };
            let _ = self
                .store_io(|| {
                    self.store.complete_reconciled_workspace_absence(
                        &pending.scope,
                        &pending.operation,
                        &self.authority.now().to_rfc3339(),
                        StatusCode::OK.as_u16(),
                        &pending.id,
                        &absence,
                    )
                })
                .await;
        }
    }

    async fn persist_completed_execs(&self) {
        for observation in self.driver.completed_execs() {
            let Ok(scopes) = self
                .store_io(|| {
                    self.store
                        .scopes_for_exec(&self.deployment, &observation.resource.id)
                })
                .await
            else {
                continue;
            };
            if scopes.is_empty() {
                continue;
            }
            let scope_count = scopes.len();
            let mut exact_count = 0;
            let mut any_superseded = false;
            let mut failed = false;
            for scope in scopes {
                match self
                    .store_io(|| self.store.put_exec(&scope, &stored_exec(&observation)))
                    .await
                {
                    Ok(ExecWrite::PersistedExact(_)) => {
                        exact_count += 1;
                    }
                    Ok(ExecWrite::Superseded(_) | ExecWrite::Retired) => {
                        any_superseded = true;
                    }
                    Ok(ExecWrite::PersistedTransformed(authoritative)) => {
                        self.driver
                            .set_exec_lease(&observation.resource.id, authoritative.resource.lease);
                    }
                    Err(_) => {
                        failed = true;
                    }
                }
            }
            match completed_driver_action(scope_count, exact_count, any_superseded, failed) {
                CompletedDriverAction::Acknowledge => {
                    self.driver.acknowledge_exec(&observation);
                }
                CompletedDriverAction::DiscardSuperseded => {
                    self.driver
                        .discard_superseded_exec(&observation.resource.id);
                }
                CompletedDriverAction::Retain => {}
            }
        }
    }

    #[allow(clippy::too_many_lines)] // Lease cleanup keeps one explicit crash-consistency path.
    async fn cleanup_expired(&self, expired: ExpiredLease, observed_at: DateTime<Utc>) {
        let result = match &expired.resource {
            LeaseResource::Exec { .. } => {
                let read = self
                    .store_io(|| self.store.exec(&expired.scope, &expired.id))
                    .await;
                // Invariant 3: an exec that could not be read is not recorded expired unsignalled;
                // the round fails by name and the lease is retried.
                if let Err(error) = &read {
                    Err(error.to_string())
                } else if let Ok(Some(stored)) = read
                    && matches!(
                        stored.resource.state,
                        ExecState::Accepted | ExecState::Running | ExecState::Unknown
                    )
                {
                    let signal = ExecSignalInput {
                        signal: substrate_wire::Signal::Kill,
                        grace_ms: 0,
                    };
                    match run_maintenance_driver(self.driver.signal(&expired.id, &signal)).await {
                        Ok(observation) => {
                            let persisted = self
                                .store_io(|| {
                                    self.store
                                        .complete_exec_lease_expiry(
                                            &expired,
                                            observed_at,
                                            Some(&stored_exec(&observation)),
                                        )
                                        .map_err(|error| error.to_string())
                                })
                                .await;
                            if persisted.is_ok() {
                                self.driver
                                    .discard_superseded_exec(&observation.resource.id);
                            }
                            persisted.map(|_| ())
                        }
                        Err(error) if error.class == DriverErrorClass::NotFound => {
                            self.store_io(|| {
                                self.store
                                    .complete_exec_lease_expiry(&expired, observed_at, None)
                                    .map(|_| ())
                                    .map_err(|error| error.to_string())
                            })
                            .await
                        }
                        Err(error) => Err(error.code.to_owned()),
                    }
                } else {
                    self.store_io(|| {
                        self.store
                            .complete_exec_lease_expiry(&expired, observed_at, None)
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    })
                    .await
                }
            }
            LeaseResource::Workspace { root_name, origin } => {
                let mut child_failure = None;
                // Invariant 3: execs that could not be read cannot be ended, so the workspace is
                // not ended either; the round fails by name and the lease is retried.
                let execs = match self
                    .store_io(|| {
                        self.store
                            .execs_holding_workspace(&expired.scope, &expired.id)
                    })
                    .await
                {
                    Ok(execs) => execs,
                    Err(error) => {
                        child_failure = Some(error.to_string());
                        Vec::new()
                    }
                };
                for exec in execs {
                    if matches!(
                        exec.state,
                        ExecState::Accepted | ExecState::Running | ExecState::Unknown
                    ) {
                        match run_maintenance_driver(self.driver.signal(
                            &exec.id,
                            &ExecSignalInput {
                                signal: substrate_wire::Signal::Kill,
                                grace_ms: 0,
                            },
                        ))
                        .await
                        {
                            Ok(observation) => {
                                let write = self
                                    .store_io(|| {
                                        self.store
                                            .put_exec(&expired.scope, &stored_exec(&observation))
                                    })
                                    .await;
                                match write {
                                    Ok(ExecWrite::PersistedExact(_)) => {
                                        self.driver.acknowledge_exec(&observation);
                                    }
                                    Ok(ExecWrite::Superseded(_) | ExecWrite::Retired) => {
                                        self.driver
                                            .discard_superseded_exec(&observation.resource.id);
                                    }
                                    Ok(ExecWrite::PersistedTransformed(authoritative)) => {
                                        self.driver.set_exec_lease(
                                            &observation.resource.id,
                                            authoritative.resource.lease,
                                        );
                                    }
                                    Err(error) => {
                                        child_failure = Some(error.to_string());
                                        break;
                                    }
                                }
                            }
                            Err(error) if error.class == DriverErrorClass::NotFound => {}
                            Err(error) => {
                                child_failure = Some(error.code.to_owned());
                                break;
                            }
                        }
                    }
                }
                if let Some(error) = child_failure {
                    Err(error)
                } else {
                    match run_maintenance_driver(end_workspace(
                        self.driver.as_ref(),
                        &expired.id,
                        root_name,
                        *origin,
                    ))
                    .await
                    {
                        Ok(WorkspaceDestroyProgress::Absent(_)) => {
                            self.store_io(|| {
                                self.store
                                    .complete_workspace_lease_expiry(&expired, observed_at)
                                    .map_err(|error| error.to_string())
                            })
                            .await
                        }
                        Ok(WorkspaceDestroyProgress::Pending { .. }) => {
                            self.store_io(|| {
                                self.store
                                    .record_lease_cleanup_progress(&expired, observed_at)
                                    .map_err(|error| error.to_string())
                            })
                            .await
                        }
                        Err(error) if error.class == DriverErrorClass::NotFound => {
                            self.store_io(|| {
                                self.store
                                    .complete_workspace_lease_expiry(&expired, observed_at)
                                    .map_err(|error| error.to_string())
                            })
                            .await
                        }
                        Err(error) => Err(error.code.to_owned()),
                    }
                }
            }
        };
        if let Err(code) = result {
            let _ = self
                .store_io(|| {
                    self.store
                        .record_lease_cleanup_failure(&expired, observed_at, &code)
                })
                .await;
        }
    }
}

pub(super) async fn run_maintenance_driver<T>(
    operation: impl Future<Output = Result<T, DriverError>>,
) -> Result<T, DriverError> {
    tokio::time::timeout(MAINTENANCE_DRIVER_TIMEOUT, operation)
        .await
        .unwrap_or_else(|_| {
            Err(DriverError::failed(
                "maintenance.driver-timeout",
                "Maintenance driver operation exceeded its deadline.",
            ))
        })
}

pub(super) async fn bounded_blocking<T, F>(slots: &Arc<Semaphore>, operation: F) -> T
where
    T: Send,
    F: FnOnce() -> T + Send,
{
    let permit = Arc::clone(slots)
        .acquire_owned()
        .await
        .expect("blocking I/O semaphore remains open");
    let result = tokio::task::block_in_place(operation);
    drop(permit);
    result
}

/// End a workspace on disk by its origin (ADR 0035 decision 7), on every removal path: a created
/// workspace's tree is removed in batches; an adopted one is forgotten, and the driver removes only
/// what it made outside the operator's directory.
pub(super) async fn end_workspace(
    driver: &dyn Driver,
    workspace_id: &str,
    root_name: &str,
    origin: WorkspaceOrigin,
) -> Result<WorkspaceDestroyProgress, DriverError> {
    if origin.is_adopted() {
        driver
            .forget_workspace(workspace_id, root_name)
            .await
            .map(WorkspaceDestroyProgress::Absent)
    } else {
        driver.destroy_workspace(workspace_id, root_name).await
    }
}
