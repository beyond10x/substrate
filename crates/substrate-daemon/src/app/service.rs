use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::future::Future;
use std::hash::{Hash as _, Hasher as _};
use std::sync::{Arc, Weak};

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use parking_lot::Mutex as ParkingMutex;
use substrate_host::{Driver, DriverError, DriverErrorClass, WorkspaceDestroyProgress};
#[cfg(test)]
use substrate_store::NewOperation;
use substrate_store::{
    CommitEffectSink, ExecWrite, ExpiredLease, LeaseClock, LeaseResource, Scope, Store, StoreError,
    WorkspaceAdmission,
};
use substrate_wire::{
    ExecSignalInput, ExecState, OperationState, WorkspaceAbsence, WorkspaceKind, WorkspaceState,
};
use tokio::sync::{Mutex, Notify, OwnedMutexGuard, Semaphore};
use ulid::Ulid;

use crate::delegation::DelegatedContextPolicy;

use super::events::{EventStreamLimits, EventStreamPolicy, EventWakeups};
use super::operations::{driver_detail, linux_lease_clock, stored_exec};
use super::sessions::{PipeAttachmentLimits, PipeSessionPolicy};
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

pub trait Authority: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    fn request_id(&self) -> String;
    fn workspace_id(&self) -> String;
    fn exec_id(&self) -> String;
    fn session_id(&self) -> String;
    fn snapshot_id(&self) -> String;
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
        format!("req_{}", Ulid::new())
    }

    fn workspace_id(&self) -> String {
        format!("ws_{}", Ulid::new())
    }

    fn exec_id(&self) -> String {
        format!("ex_{}", Ulid::new())
    }

    fn session_id(&self) -> String {
        format!("ses_{}", Ulid::new())
    }

    fn snapshot_id(&self) -> String {
        format!("snap_{}", Ulid::new())
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
    restart_cutoff: DateTime<Utc>,
    pub(super) event_wakeups: Arc<EventWakeups>,
    pub(super) event_stream_limits: Arc<EventStreamLimits>,
    pub(super) event_stream_policy: EventStreamPolicy,
    pub(super) pipe_attachment_limits: Arc<PipeAttachmentLimits>,
    pub(super) pipe_session_policy: PipeSessionPolicy,
    lease_sweep: Mutex<()>,
    workspace_recovery: Mutex<()>,
    provisional_recovery: Mutex<()>,
    workspace_locks: WorkspaceLockDomains,
    maintenance_nudge: Notify,
    blocking_store_slots: Arc<Semaphore>,
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
        let event_wakeups = Arc::new(EventWakeups::default());
        let event_stream_policy = EventStreamPolicy::production();
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
            pipe_attachment_limits: PipeAttachmentLimits::new(
                pipe_session_policy.global_attachments,
            ),
            pipe_session_policy,
            lease_sweep: Mutex::new(()),
            workspace_recovery: Mutex::new(()),
            provisional_recovery: Mutex::new(()),
            workspace_locks: WorkspaceLockDomains::default(),
            maintenance_nudge: Notify::new(),
            blocking_store_slots: Arc::new(Semaphore::new(16)),
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
        let mut hasher = DefaultHasher::new();
        workspace.hash(&mut hasher);
        let stripe = usize::try_from(hasher.finish()).unwrap_or(usize::MAX) % domain.stripes.len();
        let lock = Arc::clone(&domain.stripes[stripe]);
        let guard = lock.lock_owned().await;
        WorkspaceGuard {
            _domain: domain,
            _guard: guard,
        }
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
            let Ok(Some(claimed)) = self
                .store_io(|| self.store.claim_expired_lease(&candidate, &clock))
                .await
            else {
                continue;
            };
            self.cleanup_expired(claimed, clock.wall).await;
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
            let absence = match run_maintenance_driver(
                self.driver
                    .destroy_workspace(&pending.id, &pending.root_name),
            )
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
                if let Ok(Some(stored)) = self
                    .store_io(|| self.store.exec(&expired.scope, &expired.id))
                    .await
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
            LeaseResource::Workspace { root_name } => {
                let mut child_failure = None;
                if let Ok(execs) = self
                    .store_io(|| self.store.execs_for_workspace(&expired.scope, &expired.id))
                    .await
                {
                    for exec in execs {
                        if matches!(
                            exec.resource.state,
                            ExecState::Accepted | ExecState::Running | ExecState::Unknown
                        ) {
                            match run_maintenance_driver(self.driver.signal(
                                &exec.resource.id,
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
                                            self.store.put_exec(
                                                &expired.scope,
                                                &stored_exec(&observation),
                                            )
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
                }
                if let Some(error) = child_failure {
                    Err(error)
                } else {
                    match run_maintenance_driver(
                        self.driver.destroy_workspace(&expired.id, root_name),
                    )
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
