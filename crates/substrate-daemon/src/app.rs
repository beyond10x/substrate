#![allow(clippy::result_large_err)] // Axum responses are the natural typed rejection at this seam.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::future::Future;
use std::hash::{Hash as _, Hasher as _};
use std::sync::{Arc, Weak};

use axum::body::{Body, to_bytes};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use futures_util::StreamExt as _;
use parking_lot::Mutex as ParkingMutex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use substrate_host::{
    DispatchOutcome, Driver, DriverError, DriverErrorClass, ExecObservation, PipeStream,
    WorkspaceDestroyProgress,
};
use substrate_store::{
    CommitEffect, CommitEffectSink, EventCursorError, ExecRetireReservation, ExecWrite,
    ExpiredLease, LeaseClock, LeaseResource, NewLease, NewOperation, Reservation, Scope,
    SessionAttachmentClaim, SessionRetireReservation, SnapshotReadError, Store, StoreError,
    StoredAnswer, StoredExec, WorkspaceAdmission, WorkspaceDestroyReservation,
    WorkspaceObservationWrite,
};
use substrate_wire::{
    Base64Content, Base64Encoding, EmptyInput, ErrorClass, ErrorDetail, EventPage, EventQuery,
    Exec, ExecKind, ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState, Failure,
    FileReadQuery, FileWriteInput, LeaseRenewInput, MAX_EVENT_PAGE_ITEMS, MAX_LEASE_TTL_MS,
    MAX_SNAPSHOT_PAGE_ITEMS, MIN_LEASE_TTL_MS, NetworkMode, OperationOutcome, OperationState,
    OutputSlice, OutputStream, PipeClientFrame, PipeServerFrame, PipeSession,
    PipeSessionCapabilities, PipeSessionLimits, PipeSessionStartInput, SessionAttachmentState,
    SessionKind, SessionMode, SessionState, Success, WireValidationError, WorkspaceAbsence,
    WorkspaceCreateInput, WorkspaceKind, WorkspaceSource, WorkspaceState,
    canonical_request_hash_v2, validate_operation_id, validate_relative_path,
};
use tokio::sync::{Mutex, Notify, OwnedMutexGuard, Semaphore, watch};
use ulid::Ulid;

const BODY_LIMIT: usize = 2_097_152;
const WORKSPACE_LOCK_STRIPES: usize = 256;
const LEASE_CLEANUP_BATCH: usize = 32;
const WORKSPACE_CLEANUP_BATCH: usize = 32;
const RESTART_RECONCILE_BATCH: usize = 64;
const PROVISIONAL_RECOVERY_BATCH: usize = 16;
const REQUEST_BODY_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const PIPE_MAX_INPUT_BYTES: u64 = 16 * 1_024 * 1_024;
const PIPE_MAX_FRAME_BYTES: u64 = 64 * 1_024;
const PIPE_MAX_QUEUED_FRAMES: u32 = 16;

#[derive(Debug)]
struct BoundMutation<T> {
    op: String,
    input: T,
    request_hash: String,
}
const MAINTENANCE_DRIVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Default)]
struct WorkspaceLockDomains {
    subjects: ParkingMutex<HashMap<Scope, Weak<WorkspaceLockDomain>>>,
}

struct WorkspaceLockDomain {
    stripes: Vec<Arc<Mutex<()>>>,
}

struct WorkspaceGuard {
    _domain: Arc<WorkspaceLockDomain>,
    _guard: OwnedMutexGuard<()>,
}

impl WorkspaceLockDomains {
    fn domain(&self, scope: &Scope) -> Arc<WorkspaceLockDomain> {
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
#[derive(Clone, Copy)]
struct EventStreamPolicy {
    global_streams: usize,
    streams_per_subject: usize,
    max_input_bytes: usize,
    max_output_bytes: usize,
    write_buffer_bytes: usize,
    max_controls_per_window: u32,
    control_window: std::time::Duration,
    send_timeout: std::time::Duration,
    lifetime: std::time::Duration,
    max_catch_up_pages: usize,
    max_page_items: u32,
}

impl EventStreamPolicy {
    const fn production() -> Self {
        Self {
            global_streams: 64,
            streams_per_subject: 4,
            max_input_bytes: 1_024,
            max_output_bytes: BODY_LIMIT,
            write_buffer_bytes: 16 * 1_024,
            max_controls_per_window: 120,
            control_window: std::time::Duration::from_mins(1),
            send_timeout: std::time::Duration::from_secs(5),
            lifetime: std::time::Duration::from_hours(1),
            max_catch_up_pages: 16,
            max_page_items: 64,
        }
    }
}

struct EventStreamLimits {
    global: Arc<Semaphore>,
    streams_per_subject: usize,
    scopes: ParkingMutex<HashMap<Scope, ScopeStreamLimit>>,
}

struct ScopeStreamLimit {
    permits: Arc<Semaphore>,
    holders: usize,
}

struct EventStreamPermit {
    limits: Arc<EventStreamLimits>,
    scope: Scope,
    _global: tokio::sync::OwnedSemaphorePermit,
    _subject: tokio::sync::OwnedSemaphorePermit,
}

#[derive(Clone, Copy)]
struct PipeSessionPolicy {
    global_attachments: usize,
    max_message_bytes: usize,
    write_buffer_bytes: usize,
    send_timeout: std::time::Duration,
    read_poll: std::time::Duration,
    lifetime: std::time::Duration,
    max_controls_per_window: u32,
    control_window: std::time::Duration,
}

impl PipeSessionPolicy {
    const fn production() -> Self {
        Self {
            global_attachments: 32,
            // One 64-KiB binary frame expands below this bound in the closed base64 JSON shape.
            max_message_bytes: 96 * 1_024,
            write_buffer_bytes: 16 * 1_024,
            send_timeout: std::time::Duration::from_secs(5),
            read_poll: std::time::Duration::from_millis(250),
            lifetime: std::time::Duration::from_hours(1),
            max_controls_per_window: 120,
            control_window: std::time::Duration::from_mins(1),
        }
    }
}

struct PipeAttachmentLimits {
    global: Arc<Semaphore>,
    attached: ParkingMutex<HashSet<(Scope, String)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipeAttachmentRefusal {
    Capacity,
    AlreadyAttached,
}

struct PipeAttachmentPermit {
    limits: Arc<PipeAttachmentLimits>,
    scope: Scope,
    exec_id: String,
    remove_key_on_drop: bool,
    global: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl PipeAttachmentLimits {
    fn new(global_attachments: usize) -> Arc<Self> {
        assert!(
            global_attachments > 0,
            "global pipe attachment limit must be nonzero"
        );
        Arc::new(Self {
            global: Arc::new(Semaphore::new(global_attachments)),
            attached: ParkingMutex::new(HashSet::new()),
        })
    }

    fn acquire(
        self: &Arc<Self>,
        scope: &Scope,
        exec_id: &str,
    ) -> Result<PipeAttachmentPermit, PipeAttachmentRefusal> {
        let global = Arc::clone(&self.global)
            .try_acquire_owned()
            .map_err(|_| PipeAttachmentRefusal::Capacity)?;
        let key = (scope.clone(), exec_id.to_owned());
        if !self.attached.lock().insert(key) {
            return Err(PipeAttachmentRefusal::AlreadyAttached);
        }
        Ok(PipeAttachmentPermit {
            limits: Arc::clone(self),
            scope: scope.clone(),
            exec_id: exec_id.to_owned(),
            remove_key_on_drop: true,
            global: Some(global),
        })
    }
}

impl PipeAttachmentPermit {
    /// Keeps a process-local tombstone when cancellation could not be proven. Capacity is still
    /// recovered, but the uncertain exec cannot be attached again before restart reconciliation.
    fn retain_attachment_tombstone(&mut self) {
        self.remove_key_on_drop = false;
    }
}

impl Drop for PipeAttachmentPermit {
    fn drop(&mut self) {
        if self.remove_key_on_drop {
            self.limits
                .attached
                .lock()
                .remove(&(self.scope.clone(), self.exec_id.clone()));
        } else if let Some(global) = self.global.take() {
            // One uncertain cancellation consumes one of the fixed global attachment slots until
            // daemon restart. This keeps both reattachment and process-local tombstones bounded.
            global.forget();
        }
    }
}

impl EventStreamLimits {
    fn new(global_streams: usize, streams_per_subject: usize) -> Arc<Self> {
        assert!(
            global_streams > 0,
            "global event stream limit must be nonzero"
        );
        assert!(
            streams_per_subject > 0,
            "subject event stream limit must be nonzero"
        );
        Arc::new(Self {
            global: Arc::new(Semaphore::new(global_streams)),
            streams_per_subject,
            scopes: ParkingMutex::new(HashMap::new()),
        })
    }

    fn acquire(self: &Arc<Self>, scope: &Scope) -> Option<EventStreamPermit> {
        let global = Arc::clone(&self.global).try_acquire_owned().ok()?;
        let mut scopes = self.scopes.lock();
        let entry = scopes
            .entry(scope.clone())
            .or_insert_with(|| ScopeStreamLimit {
                permits: Arc::new(Semaphore::new(self.streams_per_subject)),
                holders: 0,
            });
        let subject = Arc::clone(&entry.permits).try_acquire_owned().ok()?;
        entry.holders += 1;
        Some(EventStreamPermit {
            limits: Arc::clone(self),
            scope: scope.clone(),
            _global: global,
            _subject: subject,
        })
    }
}

impl Drop for EventStreamPermit {
    fn drop(&mut self) {
        let mut scopes = self.limits.scopes.lock();
        let Some(entry) = scopes.get_mut(&self.scope) else {
            return;
        };
        entry.holders = entry.holders.saturating_sub(1);
        if entry.holders == 0 {
            scopes.remove(&self.scope);
        }
    }
}

#[derive(Default)]
struct EventWakeups {
    scopes: ParkingMutex<HashMap<Scope, ScopeWakeup>>,
}

struct ScopeWakeup {
    sender: watch::Sender<Option<WakePosition>>,
    subscribers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct WakePosition {
    generation: u64,
    through_seq: u64,
    source_scope: String,
}

struct EventSubscription {
    wakeups: Arc<EventWakeups>,
    scope: Scope,
    receiver: watch::Receiver<Option<WakePosition>>,
}

impl EventWakeups {
    fn subscribe(self: &Arc<Self>, scope: &Scope) -> EventSubscription {
        let mut scopes = self.scopes.lock();
        let wakeup = scopes.entry(scope.clone()).or_insert_with(|| {
            let (sender, receiver) = watch::channel(None);
            drop(receiver);
            ScopeWakeup {
                sender,
                subscribers: 0,
            }
        });
        wakeup.subscribers += 1;
        let receiver = wakeup.sender.subscribe();
        EventSubscription {
            wakeups: Arc::clone(self),
            scope: scope.clone(),
            receiver,
        }
    }
}

impl CommitEffectSink for EventWakeups {
    fn committed(&self, effects: &[CommitEffect]) {
        let mut scopes = self.scopes.lock();
        let mut latest = HashMap::<Scope, WakePosition>::new();
        for effect in effects {
            let candidate = WakePosition {
                generation: effect.generation,
                through_seq: effect.through_seq,
                source_scope: effect.source_scope.clone(),
            };
            let entry = latest
                .entry(effect.scope.clone())
                .or_insert(candidate.clone());
            if candidate > *entry {
                *entry = candidate;
            }
        }
        for (scope, candidate) in latest {
            if let Some(wakeup) = scopes.get_mut(&scope) {
                let should_replace = wakeup
                    .sender
                    .borrow()
                    .as_ref()
                    .is_none_or(|current| candidate > *current);
                if should_replace {
                    wakeup.sender.send_replace(Some(candidate));
                }
            }
        }
    }
}

impl EventSubscription {
    async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.receiver.changed().await
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        let mut scopes = self.wakeups.scopes.lock();
        let Some(wakeup) = scopes.get_mut(&self.scope) else {
            return;
        };
        wakeup.subscribers = wakeup.subscribers.saturating_sub(1);
        if wakeup.subscribers == 0 {
            scopes.remove(&self.scope);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletedDriverAction {
    Acknowledge,
    DiscardSuperseded,
    Retain,
}

fn completed_driver_action(
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
type RefusalBeforeRecordHook = Arc<dyn Fn(&NewOperation) + Send + Sync>;

pub struct App {
    pub store: Arc<Store>,
    pub driver: Arc<dyn Driver>,
    pub deployment: String,
    authority: Arc<dyn Authority>,
    restart_cutoff: DateTime<Utc>,
    event_wakeups: Arc<EventWakeups>,
    event_stream_limits: Arc<EventStreamLimits>,
    event_stream_policy: EventStreamPolicy,
    pipe_attachment_limits: Arc<PipeAttachmentLimits>,
    pipe_session_policy: PipeSessionPolicy,
    lease_sweep: Mutex<()>,
    workspace_recovery: Mutex<()>,
    provisional_recovery: Mutex<()>,
    workspace_locks: WorkspaceLockDomains,
    maintenance_nudge: Notify,
    blocking_store_slots: Arc<Semaphore>,
    #[cfg(test)]
    refusal_before_record: ParkingMutex<Option<RefusalBeforeRecordHook>>,
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

    fn scope(&self, identity: &Identity) -> Scope {
        Scope {
            deployment: self.deployment.clone(),
            subject: identity.subject.clone(),
        }
    }

    fn lease_clock(&self) -> Result<LeaseClock, String> {
        self.authority.lease_clock()
    }

    async fn store_io<T, F>(&self, operation: F) -> T
    where
        T: Send,
        F: FnOnce() -> T + Send,
    {
        bounded_blocking(&self.blocking_store_slots, operation).await
    }

    async fn lock_workspace(&self, scope: &Scope, workspace: &str) -> WorkspaceGuard {
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

    fn nudge_maintenance(&self) {
        self.maintenance_nudge.notify_one();
    }

    pub async fn maintenance_notified(&self) {
        self.maintenance_nudge.notified().await;
    }

    async fn admit_workspace(
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
                    if candidate.operation_state == OperationState::Terminal {
                        let _ = self
                            .store_io(|| {
                                self.store
                                    .mark_exec_physically_absent(&candidate, self.authority.now())
                            })
                            .await;
                    } else {
                        let (status, mut detail) =
                            driver_detail(Some(&candidate.operation), &error);
                        detail.retriable = false;
                        let _ = self
                            .store_io(|| {
                                self.store.complete_dispatch_absence(
                                    &candidate.scope,
                                    &candidate.operation,
                                    &self.authority.now().to_rfc3339(),
                                    status.as_u16(),
                                    "exec",
                                    &candidate.stored.resource.id,
                                    &detail,
                                )
                            })
                            .await;
                    }
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

async fn run_maintenance_driver<T>(
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

async fn bounded_blocking<T, F>(slots: &Arc<Semaphore>, operation: F) -> T
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

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/v1/machine", get(machine_get))
        .route("/v1/workspaces", post(workspace_create))
        .route(
            "/v1/workspaces/{workspace_id}",
            get(workspace_get).delete(workspace_destroy),
        )
        .route(
            "/v1/workspaces/{workspace_id}/files/{*path}",
            get(workspace_file_read)
                .put(workspace_file_write)
                .delete(workspace_file_delete),
        )
        .route("/v1/execs", post(exec_start))
        .route("/v1/execs/{exec_id}", get(exec_get).delete(exec_retire))
        .route("/v1/execs/{exec_id}/output", get(exec_output_get))
        .route("/v1/execs/{exec_id}/signal", post(exec_signal))
        .route(
            "/v1/pipe-sessions",
            get(pipe_session_capabilities).post(pipe_session_start),
        )
        .route(
            "/v1/pipe-sessions/{session_id}",
            get(pipe_session_get).delete(pipe_session_retire),
        )
        .route(
            "/v1/pipe-sessions/{session_id}/attach",
            get(pipe_session_attach),
        )
        .route(
            "/v1/pipe-sessions/{session_id}/signal",
            post(pipe_session_signal),
        )
        .route(
            "/v1/pipe-sessions/{session_id}/lease/renew",
            post(pipe_session_lease_renew),
        )
        .route(
            "/v1/workspaces/{workspace_id}/lease/renew",
            post(workspace_lease_renew),
        )
        .route("/v1/execs/{exec_id}/lease/renew", post(exec_lease_renew))
        .route("/v1/events", get(event_list))
        .route("/v1/events/stream", get(event_stream))
        .route(
            "/v1/reconciliation-snapshots",
            post(reconciliation_snapshot_create),
        )
        .route(
            "/v1/reconciliation-snapshots/{snapshot_id}",
            get(reconciliation_snapshot_get),
        )
        .route("/v1/ops/{operation_id}", get(operation_get))
        .fallback(route_not_found)
        .with_state(app)
}

async fn machine_get(
    State(app): State<Arc<App>>,
    Extension(_identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    success(
        StatusCode::OK,
        Success::observed(request_id, app.driver.machine()),
    )
}

#[allow(clippy::too_many_lines)] // Durable admission and driver dispatch stay auditable together.
async fn workspace_create(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let mutation = match decode_mutation::<WorkspaceCreateInput>(
        &app,
        &identity,
        "workspace.create",
        "POST",
        "/v1/workspaces",
        raw_query.as_deref(),
        body,
        &request_id,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = validate_workspace_input(&mutation, &request_id) {
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            "workspace.create",
            "POST",
            "/v1/workspaces",
            &mutation,
            response,
        )
        .await;
    }
    if !mutation.input.source.is_empty() {
        return refuse_before_dispatch(
            &app,
            &identity,
            &request_id,
            "workspace.create",
            "POST",
            "/v1/workspaces",
            &mutation,
            &DriverError::unserved(
                "workspace.source-unserved",
                "Workspace Git sources are not served by the minimum host slice.",
                "workspace.git",
            ),
        )
        .await;
    }
    let operation = mutation.op.clone();
    let lease = match mutation.input.lease_ttl_ms {
        Some(ttl_ms) => match new_lease(&app, &identity, ttl_ms, &request_id, &operation) {
            Ok(value) => Some(value),
            Err(response) => {
                return refuse_before_dispatch_response(
                    &app,
                    &identity,
                    &request_id,
                    "workspace.create",
                    "POST",
                    "/v1/workspaces",
                    &mutation,
                    response,
                )
                .await;
            }
        },
        None => None,
    };
    let id = app.authority.workspace_id();
    let scope = app.scope(&identity);
    let _workspace_guard = app.lock_workspace(&scope, &id).await;
    let root_name = match app.driver.workspace_root_identity(&id) {
        Ok(value) => value,
        Err(error) => {
            return refuse_before_dispatch(
                &app,
                &identity,
                &request_id,
                "workspace.create",
                "POST",
                "/v1/workspaces",
                &mutation,
                &error,
            )
            .await;
        }
    };
    let new = new_operation(
        &app,
        &identity,
        "workspace.create",
        "POST",
        "/v1/workspaces",
        &mutation,
        None,
        Some(id.clone()),
    );
    let provisional = substrate_wire::Workspace {
        id: id.clone(),
        kind: WorkspaceKind::Workspace,
        labels: mutation.input.labels.clone(),
        observed_at: app.authority.now(),
        state: WorkspaceState::Unknown,
        lease: lease.as_ref().map(NewLease::observation),
    };
    if let Some(response) = reservation_response(
        app.store_io(|| {
            app.store
                .reserve_workspace_create(&new, &root_name, &provisional, lease.as_ref())
        })
        .await,
        &request_id,
        &mutation.op,
    ) {
        return response;
    }
    match app
        .driver
        .create_workspace(&id, &root_name, &mutation.input)
        .await
    {
        DispatchOutcome::Observed(mut workspace) => {
            workspace.lease = lease.as_ref().map(NewLease::observation);
            if let Err(error) = app
                .store_io(|| {
                    app.store.complete_workspace_leased(
                        &scope,
                        &operation,
                        &app.authority.now().to_rfc3339(),
                        201,
                        &root_name,
                        &workspace,
                        lease.as_ref(),
                    )
                })
                .await
            {
                return store_failure(&request_id, Some(&operation), &error);
            }
            success(
                StatusCode::CREATED,
                Success::mutation(request_id, operation, workspace),
            )
        }
        DispatchOutcome::NotDispatched(error) | DispatchOutcome::ContainedAbsent(error) => {
            finish_dispatch_absence(
                &app,
                &scope,
                &request_id,
                &operation,
                "workspace",
                &id,
                &error,
            )
            .await
        }
        DispatchOutcome::OutcomeUnknown(error) => {
            finish_dispatch_unknown(
                &app,
                &scope,
                &request_id,
                &operation,
                "workspace",
                &id,
                &error,
            )
            .await
        }
    }
}

async fn workspace_get(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(workspace_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let scope = app.scope(&identity);
    let _workspace_guard = app.lock_workspace(&scope, &workspace_id).await;
    let (root_name, previous) = match app.admit_workspace(&scope, &workspace_id).await {
        Ok(WorkspaceAdmission::Missing) => return not_found(&request_id),
        Ok(WorkspaceAdmission::Frozen { resource, .. }) => {
            return success(StatusCode::OK, Success::observed(request_id, resource));
        }
        Ok(WorkspaceAdmission::Admitted {
            root_name,
            resource,
        }) => (root_name, resource),
        Err(error) => return store_failure(&request_id, None, &error),
    };
    match app
        .driver
        .observe_workspace(&workspace_id, &root_name, &previous)
        .await
    {
        Ok(workspace) => {
            match app
                .store_io(|| {
                    app.store
                        .merge_workspace_observation(&scope, &root_name, &workspace)
                })
                .await
            {
                Ok(WorkspaceObservationWrite::Authoritative(authoritative)) => {
                    success(StatusCode::OK, Success::observed(request_id, authoritative))
                }
                Ok(WorkspaceObservationWrite::Missing) => not_found(&request_id),
                Err(error) => store_failure(&request_id, None, &error),
            }
        }
        Err(error) => driver_failure(&request_id, None, &error),
    }
}

async fn workspace_file_read(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path((workspace_id, path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    if let Err(error) = validate_relative_path(&path) {
        return path_refusal(&request_id, None, &error);
    }
    let query: FileReadQuery =
        match serde_urlencoded::from_str::<FileReadQuery>(raw_query.as_deref().unwrap_or("")) {
            Ok(value) if value.validate_shape().is_ok() => value,
            _ => return schema_invalid(&request_id, None, "query"),
        };
    let scope = app.scope(&identity);
    let _workspace_guard = app.lock_workspace(&scope, &workspace_id).await;
    let root_name = match app.admit_workspace(&scope, &workspace_id).await {
        Ok(WorkspaceAdmission::Missing) => return not_found(&request_id),
        Ok(WorkspaceAdmission::Frozen { .. }) => {
            return failure(
                StatusCode::CONFLICT,
                &request_id,
                None,
                ErrorClass::Conflict,
                "workspace.not-ready",
                "Workspace is not ready for filesystem access.",
                Some("workspace"),
                false,
            );
        }
        Ok(WorkspaceAdmission::Admitted { root_name, .. }) => root_name,
        Err(error) => return store_failure(&request_id, None, &error),
    };
    match app
        .driver
        .read_workspace_path(&workspace_id, &root_name, &path, &query)
        .await
    {
        Ok(result) => success(StatusCode::OK, Success::observed(request_id, result)),
        Err(error) => driver_failure(&request_id, None, &error),
    }
}

#[allow(clippy::too_many_lines)] // Durable refusal and atomic host write stay adjacent.
async fn workspace_file_write(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path((workspace_id, path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = normalized_file_address(&workspace_id, &path);
    let mutation = match decode_mutation::<FileWriteInput>(
        &app,
        &identity,
        "workspace.file.write",
        "PUT",
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
    if let Err(error) = validate_relative_path(&path) {
        let response = path_refusal(&request_id, Some(&mutation.op), &error);
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            "workspace.file.write",
            "PUT",
            &address,
            &mutation,
            response,
        )
        .await;
    }
    let content = match mutation.input.content.decode() {
        Ok(value)
            if base64::engine::general_purpose::STANDARD.encode(&value)
                == mutation.input.content.data =>
        {
            value
        }
        _ => {
            let response = schema_invalid(&request_id, Some(&mutation.op), "input");
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "workspace.file.write",
                "PUT",
                &address,
                &mutation,
                response,
            )
            .await;
        }
    };
    let scope = app.scope(&identity);
    let _workspace_guard = app.lock_workspace(&scope, &workspace_id).await;
    let root_name = match app.admit_workspace(&scope, &workspace_id).await {
        Ok(WorkspaceAdmission::Missing) => {
            return refuse_workspace_mutation(
                &app,
                &identity,
                &request_id,
                "workspace.file.write",
                "PUT",
                &address,
                &mutation,
                workspace_missing_refusal(&request_id, &mutation.op),
            )
            .await;
        }
        Ok(WorkspaceAdmission::Frozen { .. }) => {
            return refuse_workspace_mutation(
                &app,
                &identity,
                &request_id,
                "workspace.file.write",
                "PUT",
                &address,
                &mutation,
                workspace_frozen_refusal(&request_id, &mutation.op),
            )
            .await;
        }
        Ok(WorkspaceAdmission::Admitted { root_name, .. }) => root_name,
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    };
    let operation = mutation.op.clone();
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        "workspace.file.write",
        "PUT",
        &address,
        &mutation,
        None,
        Some(workspace_id.clone()),
    )
    .await
    {
        return response;
    }
    match app
        .driver
        .write_workspace_file(&workspace_id, &root_name, &path, &content)
        .await
    {
        Ok(observation) => {
            finish_success(
                &app,
                &scope,
                &request_id,
                &operation,
                StatusCode::OK,
                Some(&workspace_id),
                observation,
            )
            .await
        }
        Err(error) => {
            finish_driver_error(
                &app,
                &scope,
                &request_id,
                &operation,
                Some(&workspace_id),
                &error,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn workspace_file_delete(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path((workspace_id, path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = normalized_file_address(&workspace_id, &path);
    let mutation = match decode_mutation::<EmptyInput>(
        &app,
        &identity,
        "workspace.file.delete",
        "DELETE",
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
    if let Err(error) = validate_relative_path(&path) {
        let response = path_refusal(&request_id, Some(&mutation.op), &error);
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            "workspace.file.delete",
            "DELETE",
            &address,
            &mutation,
            response,
        )
        .await;
    }
    let scope = app.scope(&identity);
    let _workspace_guard = app.lock_workspace(&scope, &workspace_id).await;
    let root_name = match app.admit_workspace(&scope, &workspace_id).await {
        Ok(WorkspaceAdmission::Missing) => {
            return refuse_workspace_mutation(
                &app,
                &identity,
                &request_id,
                "workspace.file.delete",
                "DELETE",
                &address,
                &mutation,
                workspace_missing_refusal(&request_id, &mutation.op),
            )
            .await;
        }
        Ok(WorkspaceAdmission::Frozen { .. }) => {
            return refuse_workspace_mutation(
                &app,
                &identity,
                &request_id,
                "workspace.file.delete",
                "DELETE",
                &address,
                &mutation,
                workspace_frozen_refusal(&request_id, &mutation.op),
            )
            .await;
        }
        Ok(WorkspaceAdmission::Admitted { root_name, .. }) => root_name,
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    };
    let operation = mutation.op.clone();
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        "workspace.file.delete",
        "DELETE",
        &address,
        &mutation,
        None,
        Some(workspace_id.clone()),
    )
    .await
    {
        return response;
    }
    match app
        .driver
        .delete_workspace_file(&workspace_id, &root_name, &path)
        .await
    {
        Ok(absence) => {
            finish_success(
                &app,
                &scope,
                &request_id,
                &operation,
                StatusCode::OK,
                Some(&workspace_id),
                absence,
            )
            .await
        }
        Err(error) => {
            finish_driver_error(
                &app,
                &scope,
                &request_id,
                &operation,
                Some(&workspace_id),
                &error,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_lines)] // Destroying-state transitions guard the adjacent driver call.
async fn workspace_destroy(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(workspace_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v1/workspaces/{workspace_id}");
    let mutation = match decode_mutation::<EmptyInput>(
        &app,
        &identity,
        "workspace.destroy",
        "DELETE",
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
    let scope = app.scope(&identity);
    let _workspace_guard = app.lock_workspace(&scope, &workspace_id).await;
    let operation = mutation.op.clone();
    let new = new_operation(
        &app,
        &identity,
        "workspace.destroy",
        "DELETE",
        &address,
        &mutation,
        None,
        Some(workspace_id.clone()),
    );
    let clock = app.lease_clock().ok();
    let root_name = match app
        .store_io(|| app.store.reserve_workspace_destroy(&new, clock.as_ref()))
        .await
    {
        Ok(WorkspaceDestroyReservation::Existing(reservation)) => {
            return reservation_response(Ok(reservation), &request_id, &operation)
                .unwrap_or_else(|| outcome_unknown(&request_id, &operation));
        }
        Ok(WorkspaceDestroyReservation::Capacity(_)) => {
            return operation_ledger_capacity(&request_id);
        }
        Ok(WorkspaceDestroyReservation::Missing) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "workspace.destroy",
                "DELETE",
                &address,
                &mutation,
                not_found_with_operation(&request_id, &operation),
            )
            .await;
        }
        Ok(WorkspaceDestroyReservation::Refused {
            answer,
            newly_frozen,
        }) => {
            if newly_frozen {
                app.nudge_maintenance();
            }
            return replay(&request_id, &operation, answer);
        }
        Ok(WorkspaceDestroyReservation::Frozen { .. }) => {
            unreachable!("atomic destroy reservation returns durable refusal")
        }
        Ok(WorkspaceDestroyReservation::Admitted { root_name, .. }) => root_name,
        Err(error) => return store_failure(&request_id, Some(&operation), &error),
    };
    match run_maintenance_driver(app.driver.destroy_workspace(&workspace_id, &root_name)).await {
        Ok(WorkspaceDestroyProgress::Absent(absence)) => {
            if let Err(error) = app
                .store_io(|| {
                    app.store.complete_workspace_absence(
                        &scope,
                        &operation,
                        &app.authority.now().to_rfc3339(),
                        200,
                        &workspace_id,
                        &absence,
                    )
                })
                .await
            {
                return store_failure(&request_id, Some(&operation), &error);
            }
            success(
                StatusCode::OK,
                Success::mutation(request_id, operation, absence),
            )
        }
        Ok(WorkspaceDestroyProgress::Pending { removed_items }) => {
            let pending = substrate_store::PendingWorkspaceDestroy {
                scope: scope.clone(),
                id: workspace_id.clone(),
                root_name,
                operation: operation.clone(),
                attempt_count: 0,
            };
            let _ = app
                .store_io(|| {
                    app.store.record_workspace_cleanup_progress(
                        &pending,
                        app.authority.now(),
                        removed_items,
                    )
                })
                .await;
            app.nudge_maintenance();
            outcome_unknown(&request_id, &operation)
        }
        Err(error) => {
            let pending = substrate_store::PendingWorkspaceDestroy {
                scope: scope.clone(),
                id: workspace_id.clone(),
                root_name,
                operation: operation.clone(),
                attempt_count: 0,
            };
            let _ = app
                .store_io(|| {
                    app.store.record_workspace_cleanup_failure(
                        &pending,
                        app.authority.now(),
                        error.code,
                    )
                })
                .await;
            tracing::warn!(
                workspace = %workspace_id,
                code = error.code,
                "workspace destroy remains pending for reconciliation"
            );
            failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                &request_id,
                Some(&operation),
                ErrorClass::Failed,
                "workspace.destroy-incomplete",
                "Workspace destruction was accepted but absence is not yet proved; reconciliation will continue cleanup.",
                Some("workspace"),
                true,
            )
        }
    }
}

#[allow(clippy::too_many_lines)] // Admission, durable reservation, and dispatch stay adjacent.
async fn exec_start(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let mutation = match decode_mutation::<ExecStartInput>(
        &app,
        &identity,
        "exec.start",
        "POST",
        "/v1/execs",
        raw_query.as_deref(),
        body,
        &request_id,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = validate_exec_input(&app, &mutation, &request_id) {
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            "exec.start",
            "POST",
            "/v1/execs",
            &mutation,
            response,
        )
        .await;
    }
    let scope = app.scope(&identity);
    let _workspace_guard = app.lock_workspace(&scope, &mutation.input.workspace).await;
    let root_name = match app.admit_workspace(&scope, &mutation.input.workspace).await {
        Ok(WorkspaceAdmission::Missing) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "exec.start",
                "POST",
                "/v1/execs",
                &mutation,
                not_found_with_operation(&request_id, &mutation.op),
            )
            .await;
        }
        Ok(WorkspaceAdmission::Frozen { .. }) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "exec.start",
                "POST",
                "/v1/execs",
                &mutation,
                workspace_frozen_refusal(&request_id, &mutation.op),
            )
            .await;
        }
        Ok(WorkspaceAdmission::Admitted { root_name, .. }) => root_name,
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    };
    let operation = mutation.op.clone();
    let lease = match mutation.input.lease_ttl_ms {
        Some(ttl_ms) => match new_lease(&app, &identity, ttl_ms, &request_id, &operation) {
            Ok(value) => Some(value),
            Err(response) => {
                return refuse_before_dispatch_response(
                    &app,
                    &identity,
                    &request_id,
                    "exec.start",
                    "POST",
                    "/v1/execs",
                    &mutation,
                    response,
                )
                .await;
            }
        },
        None => None,
    };
    let id = app.authority.exec_id();
    let capability = Some(mutation.input.sandbox.capability_snapshot.clone());
    let new = new_operation(
        &app,
        &identity,
        "exec.start",
        "POST",
        "/v1/execs",
        &mutation,
        capability,
        Some(id.clone()),
    );
    let provisional = StoredExec {
        resource: Exec {
            id: id.clone(),
            kind: ExecKind::Exec,
            workspace: mutation.input.workspace.clone(),
            state: ExecState::Accepted,
            observed_at: app.authority.now(),
            requested: mutation.input.sandbox.clone(),
            applied: None,
            exit: None,
            lease: lease.as_ref().map(NewLease::observation),
        },
        stdout: Vec::new(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        output_complete: false,
        cgroup: None,
        leader_pid: None,
    };
    let workspace_clock = app.lease_clock().ok();
    if let Some(response) = reservation_response(
        app.store_io(|| {
            app.store.reserve_exec_start(
                &new,
                &provisional,
                lease.as_ref(),
                workspace_clock.as_ref(),
            )
        })
        .await,
        &request_id,
        &mutation.op,
    ) {
        return response;
    }
    match app
        .driver
        .start_exec(&id, &root_name, &mutation.input)
        .await
    {
        DispatchOutcome::Observed(mut observation) => {
            observation.resource.lease = lease.as_ref().map(NewLease::observation);
            app.driver
                .set_exec_lease(&observation.resource.id, observation.resource.lease.clone());
            finish_exec_leased(
                &app,
                &scope,
                &request_id,
                &operation,
                if mutation.input.wait {
                    StatusCode::OK
                } else {
                    StatusCode::ACCEPTED
                },
                observation,
                lease.as_ref(),
            )
            .await
        }
        DispatchOutcome::NotDispatched(error) | DispatchOutcome::ContainedAbsent(error) => {
            finish_dispatch_absence(&app, &scope, &request_id, &operation, "exec", &id, &error)
                .await
        }
        DispatchOutcome::OutcomeUnknown(error) => {
            finish_dispatch_unknown(&app, &scope, &request_id, &operation, "exec", &id, &error)
                .await
        }
    }
}

async fn pipe_session_capabilities(
    State(app): State<Arc<App>>,
    Extension(_identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let machine = app.driver.machine();
    let facts = &machine.facts;
    if !pipe_confinement_available(facts) {
        return failure(
            StatusCode::NOT_IMPLEMENTED,
            &request_id,
            None,
            ErrorClass::Unserved,
            "session.confinement-unavailable",
            "Raw-pipe sessions require namespaces, delegated cgroups, whole-tree kill, explicit leases, and no egress.",
            Some("session"),
            false,
        );
    }
    success(
        StatusCode::OK,
        Success::observed(
            request_id,
            PipeSessionCapabilities {
                contract: "substrate-wire/0.3.0".to_owned(),
                transport: "unix-websocket-json".to_owned(),
                capability_snapshot: machine.snapshot,
                lease_required: true,
                single_attachment: true,
                network: substrate_wire::AppliedNetwork::None,
                max_input_bytes: PIPE_MAX_INPUT_BYTES,
                max_frame_bytes: PIPE_MAX_FRAME_BYTES,
                max_queued_frames: PIPE_MAX_QUEUED_FRAMES,
            },
        ),
    )
}

#[allow(clippy::too_many_lines)] // Durable reservation and fail-closed pipe dispatch stay adjacent.
async fn pipe_session_start(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let mutation = match decode_mutation::<PipeSessionStartInput>(
        &app,
        &identity,
        "session.start",
        "POST",
        "/v1/pipe-sessions",
        raw_query.as_deref(),
        body,
        &request_id,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = validate_pipe_session_input(&app, &mutation, &request_id) {
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            "session.start",
            "POST",
            "/v1/pipe-sessions",
            &mutation,
            response,
        )
        .await;
    }
    let scope = app.scope(&identity);
    let _workspace_guard = app
        .lock_workspace(&scope, &mutation.input.exec.workspace)
        .await;
    let root_name = match app
        .admit_workspace(&scope, &mutation.input.exec.workspace)
        .await
    {
        Ok(WorkspaceAdmission::Missing) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "session.start",
                "POST",
                "/v1/pipe-sessions",
                &mutation,
                not_found_with_operation(&request_id, &mutation.op),
            )
            .await;
        }
        Ok(WorkspaceAdmission::Frozen { .. }) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "session.start",
                "POST",
                "/v1/pipe-sessions",
                &mutation,
                workspace_frozen_refusal(&request_id, &mutation.op),
            )
            .await;
        }
        Ok(WorkspaceAdmission::Admitted { root_name, .. }) => root_name,
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    };
    let operation = mutation.op.clone();
    let ttl_ms = mutation
        .input
        .exec
        .lease_ttl_ms
        .expect("validated pipe sessions always have leases");
    let lease = match new_lease(&app, &identity, ttl_ms, &request_id, &operation) {
        Ok(value) => value,
        Err(response) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "session.start",
                "POST",
                "/v1/pipe-sessions",
                &mutation,
                response,
            )
            .await;
        }
    };
    let exec_id = app.authority.exec_id();
    let session_id = app.authority.session_id();
    let capability = Some(mutation.input.exec.sandbox.capability_snapshot.clone());
    let new = new_operation(
        &app,
        &identity,
        "session.start",
        "POST",
        "/v1/pipe-sessions",
        &mutation,
        capability,
        Some(session_id.clone()),
    );
    let provisional = StoredExec {
        resource: Exec {
            id: exec_id.clone(),
            kind: ExecKind::Exec,
            workspace: mutation.input.exec.workspace.clone(),
            state: ExecState::Accepted,
            observed_at: app.authority.now(),
            requested: mutation.input.exec.sandbox.clone(),
            applied: None,
            exit: None,
            lease: Some(lease.observation()),
        },
        stdout: Vec::new(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        output_complete: false,
        cgroup: None,
        leader_pid: None,
    };
    let provisional_session = PipeSession {
        id: session_id.clone(),
        kind: SessionKind::Session,
        mode: SessionMode::Pipes,
        exec: exec_id.clone(),
        workspace: mutation.input.exec.workspace.clone(),
        state: SessionState::Accepted,
        attachment: SessionAttachmentState::Pending,
        observed_at: app.authority.now(),
        capability_snapshot: mutation.input.exec.sandbox.capability_snapshot.clone(),
        limits: PipeSessionLimits {
            input_bytes: mutation.input.input_limit_bytes,
            frame_bytes: mutation.input.frame_limit_bytes,
            queued_frames: mutation.input.queued_frames,
        },
        exit: None,
        lease: lease.observation(),
    };
    let workspace_clock = app.lease_clock().ok();
    if let Some(response) = reservation_response(
        app.store_io(|| {
            app.store.reserve_pipe_session_start(
                &new,
                &provisional_session,
                &provisional,
                &lease,
                workspace_clock.as_ref(),
            )
        })
        .await,
        &request_id,
        &mutation.op,
    ) {
        return response;
    }
    match app
        .driver
        .start_pipe_session(&exec_id, &root_name, &mutation.input)
        .await
    {
        DispatchOutcome::Observed(mut observation) => {
            observation.resource.lease = Some(lease.observation());
            app.driver
                .set_exec_lease(&observation.resource.id, observation.resource.lease.clone());
            finish_pipe_session_start(
                &app,
                &scope,
                &request_id,
                &operation,
                &provisional_session,
                observation,
                &lease,
            )
            .await
        }
        DispatchOutcome::NotDispatched(error) | DispatchOutcome::ContainedAbsent(error) => {
            finish_pipe_session_dispatch_absence(
                &app,
                &scope,
                &request_id,
                &operation,
                &session_id,
                &exec_id,
                &error,
            )
            .await
        }
        DispatchOutcome::OutcomeUnknown(error) => {
            finish_pipe_session_dispatch_unknown(
                &app,
                &scope,
                &request_id,
                &operation,
                &session_id,
                &exec_id,
                &error,
            )
            .await
        }
    }
}

async fn pipe_session_get(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    app.sweep_expired().await;
    let scope = app.scope(&identity);
    let session = match app
        .store_io(|| app.store.session(&scope, &session_id))
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => return not_found(&request_id),
        Err(error) => return store_failure(&request_id, None, &error),
    };
    if matches!(
        session.state,
        SessionState::Exited
            | SessionState::Cancelled
            | SessionState::Expired
            | SessionState::Unknown
    ) {
        return success(StatusCode::OK, Success::observed(request_id, session));
    }
    if let Ok(observation) = app.driver.observe_exec(&session.exec).await {
        let _write = app
            .store_io(|| app.store.put_exec(&scope, &stored_exec(&observation)))
            .await;
    }
    match app
        .store_io(|| app.store.session(&scope, &session_id))
        .await
    {
        Ok(Some(value)) => success(StatusCode::OK, Success::observed(request_id, value)),
        Ok(None) => not_found(&request_id),
        Err(error) => store_failure(&request_id, None, &error),
    }
}

async fn pipe_session_retire(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v1/pipe-sessions/{session_id}");
    let mutation = match decode_mutation::<EmptyInput>(
        &app,
        &identity,
        "session.retire",
        "DELETE",
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
    let operation = mutation.op.clone();
    let new = new_operation(
        &app,
        &identity,
        "session.retire",
        "DELETE",
        &address,
        &mutation,
        None,
        Some(session_id.clone()),
    );
    match app
        .store_io(|| {
            app.store
                .retire_pipe_session(&new, &session_id, app.authority.now())
        })
        .await
    {
        Ok(SessionRetireReservation::Existing(reservation)) => {
            reservation_response(Ok(reservation), &request_id, &operation)
                .unwrap_or_else(|| outcome_unknown(&request_id, &operation))
        }
        Ok(SessionRetireReservation::Capacity(_)) => operation_ledger_capacity(&request_id),
        Ok(SessionRetireReservation::Refused(answer)) => replay(&request_id, &operation, answer),
        Ok(SessionRetireReservation::Retired(absence)) => success(
            StatusCode::OK,
            Success::mutation(request_id, operation, absence),
        ),
        Err(error) => store_failure(&request_id, Some(&operation), &error),
    }
}

#[allow(clippy::too_many_lines)]
async fn pipe_session_signal(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v1/pipe-sessions/{session_id}/signal");
    let mutation = match decode_mutation::<ExecSignalInput>(
        &app,
        &identity,
        "session.signal",
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
    if mutation.input.grace_ms > 30_000 {
        let response = schema_invalid(&request_id, Some(&mutation.op), "input");
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            "session.signal",
            "POST",
            &address,
            &mutation,
            response,
        )
        .await;
    }
    let scope = app.scope(&identity);
    let session = match app
        .store_io(|| app.store.session(&scope, &session_id))
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "session.signal",
                "POST",
                &address,
                &mutation,
                not_found_with_operation(&request_id, &mutation.op),
            )
            .await;
        }
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    };
    let operation = mutation.op.clone();
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        "session.signal",
        "POST",
        &address,
        &mutation,
        None,
        Some(session_id.clone()),
    )
    .await
    {
        return response;
    }
    let exec = match app.store_io(|| app.store.exec(&scope, &session.exec)).await {
        Ok(Some(value)) => value,
        Ok(None) => return outcome_unknown(&request_id, &operation),
        Err(error) => return store_failure(&request_id, Some(&operation), &error),
    };
    if is_pipe_terminal(exec.resource.state) {
        return finish_pipe_session_observation(
            &app,
            &scope,
            &request_id,
            &operation,
            &session_id,
            observation_from_stored(exec),
        )
        .await;
    }
    match app.driver.signal(&session.exec, &mutation.input).await {
        Ok(observation) => {
            finish_pipe_session_observation(
                &app,
                &scope,
                &request_id,
                &operation,
                &session_id,
                observation,
            )
            .await
        }
        Err(error) => {
            finish_driver_error(
                &app,
                &scope,
                &request_id,
                &operation,
                Some(&session_id),
                &error,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn pipe_session_lease_renew(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v1/pipe-sessions/{session_id}/lease/renew");
    let mutation = match decode_mutation::<LeaseRenewInput>(
        &app,
        &identity,
        "session.lease.renew",
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
    if !(MIN_LEASE_TTL_MS..=MAX_LEASE_TTL_MS).contains(&mutation.input.ttl_ms) {
        let response = schema_invalid(&request_id, Some(&mutation.op), "input");
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            "session.lease.renew",
            "POST",
            &address,
            &mutation,
            response,
        )
        .await;
    }
    let scope = app.scope(&identity);
    let session = match app
        .store_io(|| app.store.session(&scope, &session_id))
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "session.lease.renew",
                "POST",
                &address,
                &mutation,
                not_found_with_operation(&request_id, &mutation.op),
            )
            .await;
        }
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    };
    let operation = mutation.op.clone();
    let lease = match new_lease(
        &app,
        &identity,
        mutation.input.ttl_ms,
        &request_id,
        &operation,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        "session.lease.renew",
        "POST",
        &address,
        &mutation,
        None,
        Some(session_id.clone()),
    )
    .await
    {
        return response;
    }
    match app
        .store_io(|| {
            app.store.renew_pipe_session_lease(
                &scope,
                &operation,
                &app.authority.now().to_rfc3339(),
                200,
                &session_id,
                &lease,
            )
        })
        .await
    {
        Ok(resource) => {
            app.driver
                .set_exec_lease(&session.exec, Some(resource.lease.clone()));
            success(
                StatusCode::OK,
                Success::mutation(request_id, operation, resource),
            )
        }
        Err(error) => finish_lease_store_error(&app, &scope, &request_id, &operation, &error).await,
    }
}

#[allow(clippy::too_many_lines)] // Attachment preflight keeps scope, lease, and capacity adjacent.
async fn pipe_session_attach(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    ws: WebSocketUpgrade,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    app.sweep_expired().await;
    let scope = app.scope(&identity);
    let session = match app
        .store_io(|| app.store.session(&scope, &session_id))
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => return not_found(&request_id),
        Err(error) => return store_failure(&request_id, None, &error),
    };
    if session.state != SessionState::Ready
        || session.attachment != SessionAttachmentState::Available
        || session.lease.state != substrate_wire::LeaseState::Active
    {
        return failure(
            StatusCode::CONFLICT,
            &request_id,
            None,
            ErrorClass::Conflict,
            "session.not-attachable",
            "The raw-pipe session is not running under an active lease.",
            Some("session"),
            false,
        );
    }
    let permit = match app.pipe_attachment_limits.acquire(&scope, &session_id) {
        Ok(value) => value,
        Err(PipeAttachmentRefusal::AlreadyAttached) => {
            return failure(
                StatusCode::CONFLICT,
                &request_id,
                None,
                ErrorClass::Conflict,
                "session.already-attached",
                "The raw-pipe session already has its single permitted attachment.",
                Some("session"),
                false,
            );
        }
        Err(PipeAttachmentRefusal::Capacity) => {
            return failure(
                StatusCode::TOO_MANY_REQUESTS,
                &request_id,
                None,
                ErrorClass::Exhausted,
                "session.attachment-capacity",
                "The bounded raw-pipe attachment capacity is exhausted.",
                Some("session"),
                true,
            );
        }
    };
    match app
        .store_io(|| {
            app.store
                .claim_pipe_session_attachment(&scope, &session_id, app.authority.now())
        })
        .await
    {
        Ok(SessionAttachmentClaim::Claimed) => {}
        Ok(SessionAttachmentClaim::AlreadyClaimed) => {
            return failure(
                StatusCode::CONFLICT,
                &request_id,
                None,
                ErrorClass::Conflict,
                "session.already-attached",
                "The raw-pipe session attachment right has already been consumed.",
                Some("session"),
                false,
            );
        }
        Ok(SessionAttachmentClaim::NotAttachable) => {
            return failure(
                StatusCode::CONFLICT,
                &request_id,
                None,
                ErrorClass::Conflict,
                "session.not-attachable",
                "The raw-pipe session is not attachable.",
                Some("session"),
                false,
            );
        }
        Ok(SessionAttachmentClaim::Missing) => return not_found(&request_id),
        Err(error) => return store_failure(&request_id, None, &error),
    }
    let exec_id = session.exec;
    let policy = app.pipe_session_policy;
    ws.read_buffer_size(policy.max_message_bytes)
        .write_buffer_size(policy.write_buffer_bytes)
        .max_frame_size(policy.max_message_bytes)
        .max_message_size(policy.max_message_bytes)
        .max_write_buffer_size(
            policy
                .max_message_bytes
                .saturating_add(policy.write_buffer_bytes),
        )
        .on_upgrade(move |socket| async move {
            let mut permit = permit;
            let completed = tokio::time::timeout(
                policy.lifetime,
                run_pipe_attachment(
                    Arc::clone(&app),
                    scope.clone(),
                    exec_id.clone(),
                    &permit,
                    policy,
                    socket,
                ),
            )
            .await
            .is_ok_and(|terminal| terminal);
            if !completed && !terminate_pipe_session(&app, &scope, &exec_id).await {
                permit.retain_attachment_tombstone();
            }
        })
        .into_response()
}

#[allow(clippy::too_many_lines)] // The closed bidirectional state machine stays in one audit unit.
async fn run_pipe_attachment(
    app: Arc<App>,
    scope: Scope,
    exec_id: String,
    _permit: &PipeAttachmentPermit,
    policy: PipeSessionPolicy,
    mut socket: WebSocket,
) -> bool {
    let mut expected_client_sequence = 1_u64;
    let mut server_sequence = 1_u64;
    let mut input_closed = false;
    let mut control_rate = ControlRate::new();
    loop {
        tokio::select! {
            incoming = socket.next() => {
                let frame = match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(value) = serde_json::from_slice::<PipeClientFrame>(text.as_bytes()) else {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                "session.frame-invalid",
                                "The client frame is outside the closed raw-pipe vocabulary.",
                                policy,
                            ).await;
                            return false;
                        };
                        value
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                        if control_rate.exceeded(
                            policy.max_controls_per_window,
                            policy.control_window,
                        ) {
                            let _sent = send_protocol_close(
                                &mut socket,
                                1008,
                                "raw-pipe control-frame rate exceeded",
                                policy.send_timeout,
                            ).await;
                            return false;
                        }
                        continue;
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return false,
                    Some(Ok(Message::Binary(_))) => {
                        let _sent = send_pipe_protocol_error(
                            &mut socket,
                            &mut server_sequence,
                            "session.frame-invalid",
                            "Raw-pipe client frames use the closed JSON text encoding.",
                            policy,
                        ).await;
                        return false;
                    }
                };
                let sequence = pipe_client_sequence(&frame);
                if sequence != expected_client_sequence {
                    let _sent = send_pipe_protocol_error(
                        &mut socket,
                        &mut server_sequence,
                        "session.sequence-invalid",
                        "Raw-pipe client sequences must be contiguous and start at one.",
                        policy,
                    ).await;
                    return false;
                }
                expected_client_sequence = expected_client_sequence.saturating_add(1);
                match frame {
                    PipeClientFrame::Stdin { content, .. } => {
                        if input_closed {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                "session.input-closed",
                                "Raw-pipe stdin is already closed.",
                                policy,
                            ).await;
                            return false;
                        }
                        let Ok(bytes) = content.decode() else {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                "session.base64-invalid",
                                "Raw-pipe stdin content is not valid standard base64.",
                                policy,
                            ).await;
                            return false;
                        };
                        if let Err(error) = app.driver.write_pipe_session(&exec_id, &bytes).await {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                error.code,
                                "Substrate refused or failed the raw-pipe input frame.",
                                policy,
                            ).await;
                            return false;
                        }
                    }
                    PipeClientFrame::CloseInput { .. } => {
                        if input_closed || app.driver.close_pipe_session_input(&exec_id).await.is_err() {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                "session.input-closed",
                                "Raw-pipe stdin cannot be closed again.",
                                policy,
                            ).await;
                            return false;
                        }
                        input_closed = true;
                    }
                    PipeClientFrame::Signal { signal, grace_ms, .. } => {
                        if grace_ms > 60_000 {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                "session.signal-invalid",
                                "Raw-pipe signal grace exceeds the closed bound.",
                                policy,
                            ).await;
                            return false;
                        }
                        let observation = match app.driver.signal(
                            &exec_id,
                            &ExecSignalInput { signal, grace_ms },
                        ).await {
                            Ok(value) => value,
                            Err(error) => {
                                let _sent = send_pipe_protocol_error(
                                    &mut socket,
                                    &mut server_sequence,
                                    error.code,
                                    "Substrate could not terminally observe the signalled raw-pipe process.",
                                    policy,
                                ).await;
                                return false;
                            }
                        };
                        if persist_pipe_observation(&app, &scope, &observation).await.is_err() {
                            return false;
                        }
                        return send_pipe_terminal(
                            &mut socket,
                            &mut server_sequence,
                            &observation,
                            policy,
                        ).await.is_ok();
                    }
                }
            }
            output = app.driver.read_pipe_session(&exec_id, policy.read_poll) => {
                match output {
                    Ok(Some(frame)) => {
                        let stream = match frame.stream {
                            PipeStream::Stdout => OutputStream::Stdout,
                            PipeStream::Stderr => OutputStream::Stderr,
                        };
                        let server_frame = PipeServerFrame::Output {
                            sequence: server_sequence,
                            stream,
                            content: Base64Content {
                                encoding: Base64Encoding::Base64,
                                data: base64::engine::general_purpose::STANDARD.encode(frame.bytes),
                            },
                        };
                        server_sequence = server_sequence.saturating_add(1);
                        if send_pipe_server_frame(&mut socket, &server_frame, policy).await.is_err() {
                            return false;
                        }
                    }
                    Ok(None) => {
                        let Ok(observation) = app.driver.observe_exec(&exec_id).await else {
                            return false;
                        };
                        if persist_pipe_observation(&app, &scope, &observation).await.is_err() {
                            return false;
                        }
                        return send_pipe_terminal(
                            &mut socket,
                            &mut server_sequence,
                            &observation,
                            policy,
                        ).await.is_ok();
                    }
                    Err(error) if error.code == "session.read-timeout" => {}
                    Err(error) => {
                        let _sent = send_pipe_protocol_error(
                            &mut socket,
                            &mut server_sequence,
                            error.code,
                            "Substrate could not continue raw-pipe output observation.",
                            policy,
                        ).await;
                        return false;
                    }
                }
            }
        }
    }
}

const fn pipe_client_sequence(frame: &PipeClientFrame) -> u64 {
    match frame {
        PipeClientFrame::Stdin { sequence, .. }
        | PipeClientFrame::CloseInput { sequence }
        | PipeClientFrame::Signal { sequence, .. } => *sequence,
    }
}

async fn send_pipe_server_frame(
    socket: &mut WebSocket,
    frame: &PipeServerFrame,
    policy: PipeSessionPolicy,
) -> Result<(), ()> {
    let bytes = serde_json::to_vec(frame).map_err(|_| ())?;
    if bytes.len() > policy.max_message_bytes {
        return Err(());
    }
    enforce_stream_send_deadline(
        policy.send_timeout,
        socket.send(Message::Text(
            String::from_utf8(bytes).map_err(|_| ())?.into(),
        )),
    )
    .await
}

async fn send_pipe_protocol_error(
    socket: &mut WebSocket,
    sequence: &mut u64,
    code: &str,
    message: &str,
    policy: PipeSessionPolicy,
) -> Result<(), ()> {
    let frame = PipeServerFrame::ProtocolError {
        sequence: *sequence,
        code: code.to_owned(),
        message: message.to_owned(),
    };
    *sequence = sequence.saturating_add(1);
    send_pipe_server_frame(socket, &frame, policy).await
}

async fn send_pipe_terminal(
    socket: &mut WebSocket,
    sequence: &mut u64,
    observation: &ExecObservation,
    policy: PipeSessionPolicy,
) -> Result<(), ()> {
    if !is_pipe_terminal(observation.resource.state) {
        return Err(());
    }
    if observation.stdout_truncated {
        send_pipe_server_frame(
            socket,
            &PipeServerFrame::Truncated {
                sequence: *sequence,
                stream: OutputStream::Stdout,
            },
            policy,
        )
        .await?;
        *sequence = sequence.saturating_add(1);
    }
    if observation.stderr_truncated {
        send_pipe_server_frame(
            socket,
            &PipeServerFrame::Truncated {
                sequence: *sequence,
                stream: OutputStream::Stderr,
            },
            policy,
        )
        .await?;
        *sequence = sequence.saturating_add(1);
    }
    let frame = PipeServerFrame::Exit {
        sequence: *sequence,
        state: observation.resource.state,
        exit: observation.resource.exit.clone(),
    };
    *sequence = sequence.saturating_add(1);
    send_pipe_server_frame(socket, &frame, policy).await
}

async fn persist_pipe_observation(
    app: &App,
    scope: &Scope,
    observation: &ExecObservation,
) -> Result<(), ()> {
    if !is_pipe_terminal(observation.resource.state) {
        return Err(());
    }
    match app
        .store_io(|| app.store.put_exec(scope, &stored_exec(observation)))
        .await
    {
        Ok(ExecWrite::PersistedExact(stored)) if is_pipe_terminal(stored.resource.state) => {
            app.driver.acknowledge_exec(observation);
            Ok(())
        }
        Ok(ExecWrite::Superseded(stored)) if is_pipe_terminal(stored.resource.state) => {
            app.driver.discard_superseded_exec(&observation.resource.id);
            Ok(())
        }
        Ok(ExecWrite::Retired) => {
            app.driver.discard_superseded_exec(&observation.resource.id);
            Ok(())
        }
        Ok(ExecWrite::PersistedTransformed(stored)) if is_pipe_terminal(stored.resource.state) => {
            let authoritative = observation_from_stored(stored);
            app.driver.set_exec_lease(
                &observation.resource.id,
                authoritative.resource.lease.clone(),
            );
            app.driver.acknowledge_exec(&authoritative);
            Ok(())
        }
        Ok(
            ExecWrite::PersistedExact(_)
            | ExecWrite::Superseded(_)
            | ExecWrite::PersistedTransformed(_),
        )
        | Err(_) => Err(()),
    }
}

const fn is_pipe_terminal(state: ExecState) -> bool {
    matches!(
        state,
        ExecState::Exited | ExecState::Cancelled | ExecState::Expired | ExecState::Unknown
    )
}

async fn terminate_pipe_session(app: &App, scope: &Scope, exec_id: &str) -> bool {
    let signal = ExecSignalInput {
        signal: substrate_wire::Signal::Kill,
        grace_ms: 0,
    };
    let Ok(Ok(observation)) = tokio::time::timeout(
        MAINTENANCE_DRIVER_TIMEOUT,
        app.driver.signal(exec_id, &signal),
    )
    .await
    else {
        return false;
    };
    persist_pipe_observation(app, scope, &observation)
        .await
        .is_ok()
}

async fn exec_get(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(exec_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let scope = app.scope(&identity);
    let stored = match app.store_io(|| app.store.exec(&scope, &exec_id)).await {
        Ok(Some(value)) => value,
        Ok(None) => return not_found(&request_id),
        Err(error) => return store_failure(&request_id, None, &error),
    };
    if stored.resource.state == ExecState::Expired {
        return success(
            StatusCode::OK,
            Success::observed(request_id, stored.resource),
        );
    }
    match app.driver.observe_exec(&exec_id).await {
        Ok(observation) => {
            let write = app
                .store_io(|| app.store.put_exec(&scope, &stored_exec(&observation)))
                .await;
            match write {
                Ok(ExecWrite::PersistedExact(stored)) => {
                    app.driver.acknowledge_exec(&observation);
                    success(
                        StatusCode::OK,
                        Success::observed(request_id, stored.resource),
                    )
                }
                Ok(ExecWrite::Superseded(stored)) => {
                    app.driver.discard_superseded_exec(&observation.resource.id);
                    success(
                        StatusCode::OK,
                        Success::observed(request_id, stored.resource),
                    )
                }
                Ok(ExecWrite::PersistedTransformed(stored)) => {
                    app.driver
                        .set_exec_lease(&observation.resource.id, stored.resource.lease.clone());
                    success(
                        StatusCode::OK,
                        Success::observed(request_id, stored.resource),
                    )
                }
                Ok(ExecWrite::Retired) => {
                    app.driver.discard_superseded_exec(&observation.resource.id);
                    not_found(&request_id)
                }
                Err(error) => store_failure(&request_id, None, &error),
            }
        }
        Err(_) => match app.store_io(|| app.store.exec(&scope, &exec_id)).await {
            Ok(Some(stored)) => success(
                StatusCode::OK,
                Success::observed(request_id, stored.resource),
            ),
            Ok(None) => not_found(&request_id),
            Err(error) => store_failure(&request_id, None, &error),
        },
    }
}

async fn exec_retire(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(exec_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v1/execs/{exec_id}");
    let mutation = match decode_mutation::<EmptyInput>(
        &app,
        &identity,
        "exec.retire",
        "DELETE",
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
    let operation = mutation.op.clone();
    let new = new_operation(
        &app,
        &identity,
        "exec.retire",
        "DELETE",
        &address,
        &mutation,
        None,
        Some(exec_id.clone()),
    );
    match app
        .store_io(|| app.store.retire_exec(&new, &exec_id, app.authority.now()))
        .await
    {
        Ok(ExecRetireReservation::Existing(reservation)) => {
            reservation_response(Ok(reservation), &request_id, &operation)
                .unwrap_or_else(|| outcome_unknown(&request_id, &operation))
        }
        Ok(ExecRetireReservation::Capacity(_)) => operation_ledger_capacity(&request_id),
        Ok(ExecRetireReservation::Refused(answer)) => replay(&request_id, &operation, answer),
        Ok(ExecRetireReservation::Retired(absence)) => success(
            StatusCode::OK,
            Success::mutation(request_id, operation, absence),
        ),
        Err(error) => store_failure(&request_id, Some(&operation), &error),
    }
}

async fn exec_output_get(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(exec_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    let query: ExecOutputQuery =
        match serde_urlencoded::from_str(raw_query.as_deref().unwrap_or("")) {
            Ok(value) => value,
            Err(_) => return schema_invalid(&request_id, None, "query"),
        };
    let scope = app.scope(&identity);
    let mut stored = match app.store_io(|| app.store.exec(&scope, &exec_id)).await {
        Ok(Some(value)) => value,
        Ok(None) => return not_found(&request_id),
        Err(error) => return store_failure(&request_id, None, &error),
    };
    let mut serve_durable_output = false;
    if let Ok(observation) = app.driver.observe_exec(&exec_id).await {
        let proposed = stored_exec(&observation);
        match app.store_io(|| app.store.put_exec(&scope, &proposed)).await {
            Ok(ExecWrite::PersistedExact(authoritative)) => {
                stored = authoritative;
                app.driver.acknowledge_exec(&observation);
            }
            Ok(ExecWrite::Superseded(authoritative)) => {
                stored = authoritative;
                serve_durable_output = true;
                app.driver.discard_superseded_exec(&observation.resource.id);
            }
            Ok(ExecWrite::PersistedTransformed(authoritative)) => {
                app.driver.set_exec_lease(
                    &observation.resource.id,
                    authoritative.resource.lease.clone(),
                );
                stored = authoritative;
            }
            Ok(ExecWrite::Retired) => {
                app.driver.discard_superseded_exec(&observation.resource.id);
                return not_found(&request_id);
            }
            Err(error) => return store_failure(&request_id, None, &error),
        }
    }
    if serve_durable_output {
        return match stored_output(&app, &stored, &exec_id, &query) {
            Ok(output) => success(StatusCode::OK, Success::observed(request_id, output)),
            Err(error) => driver_failure(&request_id, None, &error),
        };
    }
    match app.driver.output(&exec_id, &query).await {
        Ok(output) => success(StatusCode::OK, Success::observed(request_id, output)),
        Err(error) if error.class == DriverErrorClass::NotFound => {
            match stored_output(&app, &stored, &exec_id, &query) {
                Ok(output) => success(StatusCode::OK, Success::observed(request_id, output)),
                Err(error) => driver_failure(&request_id, None, &error),
            }
        }
        Err(error) => driver_failure(&request_id, None, &error),
    }
}

#[allow(clippy::too_many_lines)] // The terminal-state race and one durable completion stay adjacent.
async fn exec_signal(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(exec_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v1/execs/{exec_id}/signal");
    let mutation = match decode_mutation::<ExecSignalInput>(
        &app,
        &identity,
        "exec.signal",
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
    if mutation.input.grace_ms > 30_000 {
        let response = schema_invalid(&request_id, Some(&mutation.op), "input");
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            "exec.signal",
            "POST",
            &address,
            &mutation,
            response,
        )
        .await;
    }
    let scope = app.scope(&identity);
    let stored = match app.store_io(|| app.store.exec(&scope, &exec_id)).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "exec.signal",
                "POST",
                &address,
                &mutation,
                not_found_with_operation(&request_id, &mutation.op),
            )
            .await;
        }
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    };
    let operation = mutation.op.clone();
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        "exec.signal",
        "POST",
        &address,
        &mutation,
        None,
        Some(exec_id.clone()),
    )
    .await
    {
        return response;
    }
    if matches!(
        stored.resource.state,
        substrate_wire::ExecState::Exited
            | substrate_wire::ExecState::Cancelled
            | substrate_wire::ExecState::Unknown
            | substrate_wire::ExecState::Expired
    ) {
        return finish_exec(
            &app,
            &scope,
            &request_id,
            &operation,
            StatusCode::OK,
            observation_from_stored(stored),
        )
        .await;
    }
    match app.driver.signal(&exec_id, &mutation.input).await {
        Ok(observation) => {
            finish_exec(
                &app,
                &scope,
                &request_id,
                &operation,
                StatusCode::OK,
                observation,
            )
            .await
        }
        Err(error) if error.class == DriverErrorClass::NotFound => {
            match app.store_io(|| app.store.exec(&scope, &exec_id)).await {
                Ok(Some(stored))
                    if matches!(
                        stored.resource.state,
                        ExecState::Exited
                            | ExecState::Cancelled
                            | ExecState::Unknown
                            | ExecState::Expired
                    ) =>
                {
                    finish_exec(
                        &app,
                        &scope,
                        &request_id,
                        &operation,
                        StatusCode::OK,
                        observation_from_stored(stored),
                    )
                    .await
                }
                Ok(_) => {
                    finish_driver_error(
                        &app,
                        &scope,
                        &request_id,
                        &operation,
                        Some(&exec_id),
                        &error,
                    )
                    .await
                }
                Err(store_error) => store_failure(&request_id, Some(&operation), &store_error),
            }
        }
        Err(error) => {
            finish_driver_error(
                &app,
                &scope,
                &request_id,
                &operation,
                Some(&exec_id),
                &error,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_lines)] // Durable refusal and renewal stay in one mutation path.
async fn workspace_lease_renew(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(workspace_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v1/workspaces/{workspace_id}/lease/renew");
    let mutation = match decode_mutation::<LeaseRenewInput>(
        &app,
        &identity,
        "workspace.lease.renew",
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
    if !(MIN_LEASE_TTL_MS..=MAX_LEASE_TTL_MS).contains(&mutation.input.ttl_ms) {
        let response = schema_invalid(&request_id, Some(&mutation.op), "input");
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            "workspace.lease.renew",
            "POST",
            &address,
            &mutation,
            response,
        )
        .await;
    }
    let scope = app.scope(&identity);
    let _workspace_guard = app.lock_workspace(&scope, &workspace_id).await;
    match app
        .store_io(|| app.store.workspace(&scope, &workspace_id))
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "workspace.lease.renew",
                "POST",
                &address,
                &mutation,
                not_found_with_operation(&request_id, &mutation.op),
            )
            .await;
        }
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    }
    let operation = mutation.op.clone();
    let lease = match new_lease(
        &app,
        &identity,
        mutation.input.ttl_ms,
        &request_id,
        &operation,
    ) {
        Ok(value) => value,
        Err(response) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "workspace.lease.renew",
                "POST",
                &address,
                &mutation,
                response,
            )
            .await;
        }
    };
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        "workspace.lease.renew",
        "POST",
        &address,
        &mutation,
        None,
        Some(workspace_id.clone()),
    )
    .await
    {
        return response;
    }
    match app
        .store_io(|| {
            app.store.renew_workspace_lease(
                &scope,
                &operation,
                &app.authority.now().to_rfc3339(),
                200,
                &workspace_id,
                &lease,
            )
        })
        .await
    {
        Ok(resource) => success(
            StatusCode::OK,
            Success::mutation(request_id, operation, resource),
        ),
        Err(error) => finish_lease_store_error(&app, &scope, &request_id, &operation, &error).await,
    }
}

#[allow(clippy::too_many_lines)] // Durable refusal and renewal stay in one mutation path.
async fn exec_lease_renew(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(exec_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v1/execs/{exec_id}/lease/renew");
    let mutation = match decode_mutation::<LeaseRenewInput>(
        &app,
        &identity,
        "exec.lease.renew",
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
    if !(MIN_LEASE_TTL_MS..=MAX_LEASE_TTL_MS).contains(&mutation.input.ttl_ms) {
        let response = schema_invalid(&request_id, Some(&mutation.op), "input");
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            "exec.lease.renew",
            "POST",
            &address,
            &mutation,
            response,
        )
        .await;
    }
    let scope = app.scope(&identity);
    match app.store_io(|| app.store.exec(&scope, &exec_id)).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "exec.lease.renew",
                "POST",
                &address,
                &mutation,
                not_found_with_operation(&request_id, &mutation.op),
            )
            .await;
        }
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    }
    let operation = mutation.op.clone();
    let lease = match new_lease(
        &app,
        &identity,
        mutation.input.ttl_ms,
        &request_id,
        &operation,
    ) {
        Ok(value) => value,
        Err(response) => {
            return refuse_before_dispatch_response(
                &app,
                &identity,
                &request_id,
                "exec.lease.renew",
                "POST",
                &address,
                &mutation,
                response,
            )
            .await;
        }
    };
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        "exec.lease.renew",
        "POST",
        &address,
        &mutation,
        None,
        Some(exec_id.clone()),
    )
    .await
    {
        return response;
    }
    match app
        .store_io(|| {
            app.store.renew_exec_lease(
                &scope,
                &operation,
                &app.authority.now().to_rfc3339(),
                200,
                &exec_id,
                &lease,
            )
        })
        .await
    {
        Ok(resource) => success(
            StatusCode::OK,
            Success::mutation(request_id, operation, resource),
        ),
        Err(error) => finish_lease_store_error(&app, &scope, &request_id, &operation, &error).await,
    }
}

async fn event_list(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    let Some(query) = decode_event_query(raw_query.as_deref()) else {
        return schema_invalid(&request_id, None, "query");
    };
    event_page_response(&app, &identity, &request_id, &query).await
}

async fn event_stream(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    ws: WebSocketUpgrade,
) -> Response {
    let request_id = request_id(&app, &headers);
    let Some(query) = decode_event_query(raw_query.as_deref()) else {
        return schema_invalid(&request_id, None, "query");
    };
    let scope = app.scope(&identity);
    let Some(stream_permit) = app.event_stream_limits.acquire(&scope) else {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            &request_id,
            None,
            ErrorClass::Exhausted,
            "event.stream-capacity",
            "The bounded event stream capacity is exhausted.",
            Some("stream"),
            true,
        );
    };
    // Register before the first durable read. A concurrent commit then either appears in this
    // catch-up or advances the coalesced watch version and forces one more catch-up after upgrade.
    let subscription = app.event_wakeups.subscribe(&scope);
    match app
        .store_io(|| {
            app.store
                .events(&scope, query.cursor.as_deref(), query.limit)
        })
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => return event_cursor_failure(&request_id, &error),
        Err(error) => return store_failure(&request_id, None, &error),
    }
    let policy = app.event_stream_policy;
    ws.read_buffer_size(policy.max_input_bytes)
        .write_buffer_size(policy.write_buffer_bytes)
        .max_frame_size(policy.max_input_bytes)
        .max_message_size(policy.max_input_bytes)
        .max_write_buffer_size(
            policy
                .max_output_bytes
                .saturating_add(policy.write_buffer_bytes),
        )
        .on_upgrade(move |socket| async move {
            let session = run_event_stream(
                app,
                scope,
                query,
                subscription,
                stream_permit,
                policy,
                socket,
            );
            let _completed = enforce_event_stream_lifetime(policy.lifetime, session).await;
        })
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPageQuery {
    #[serde(default)]
    cursor: Option<String>,
    limit: u32,
}

async fn reconciliation_snapshot_create(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let bytes = match read_bounded_body(body, &request_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if serde_json::from_slice::<EmptyInput>(&bytes).is_err() {
        return schema_invalid(&request_id, None, "input");
    }
    let snapshot_id = app.authority.snapshot_id();
    let scope = app.scope(&identity);
    let observed_at = app.authority.now();
    match app
        .store_io(|| {
            app.store.complete_snapshot(
                &scope,
                &identity.actor,
                identity.principal.as_deref(),
                observed_at,
                &snapshot_id,
                observed_at + chrono::Duration::minutes(5),
            )
        })
        .await
    {
        Ok(metadata) => success(StatusCode::CREATED, Success::observed(request_id, metadata)),
        Err(StoreError::SnapshotLimit) => failure(
            StatusCode::INSUFFICIENT_STORAGE,
            &request_id,
            None,
            ErrorClass::Exhausted,
            "snapshot.materialization-limit",
            "Snapshot materialization exceeds the bounded item limit.",
            Some("snapshot"),
            false,
        ),
        Err(error) => store_failure(&request_id, None, &error),
    }
}

async fn reconciliation_snapshot_get(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(snapshot_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    let query =
        match serde_urlencoded::from_str::<SnapshotPageQuery>(raw_query.as_deref().unwrap_or("")) {
            Ok(value) if (1..=MAX_SNAPSHOT_PAGE_ITEMS).contains(&value.limit) => value,
            _ => return schema_invalid(&request_id, None, "query"),
        };
    match app
        .store_io(|| {
            app.store.snapshot_page(
                &app.scope(&identity),
                &snapshot_id,
                query.cursor.as_deref(),
                query.limit,
                app.authority.now(),
            )
        })
        .await
    {
        Ok(Ok(page)) => success(StatusCode::OK, Success::observed(request_id, page)),
        Ok(Err(SnapshotReadError::NotFound)) => not_found(&request_id),
        Ok(Err(SnapshotReadError::Expired)) => failure(
            StatusCode::CONFLICT,
            &request_id,
            None,
            ErrorClass::Conflict,
            "snapshot.expired",
            "The reconciliation snapshot has expired.",
            Some("snapshot"),
            false,
        ),
        Ok(Err(SnapshotReadError::Incomplete)) => failure(
            StatusCode::CONFLICT,
            &request_id,
            None,
            ErrorClass::Conflict,
            "snapshot.incomplete",
            "The materialized reconciliation snapshot is incomplete.",
            Some("snapshot"),
            true,
        ),
        Ok(Err(SnapshotReadError::InvalidCursor)) => failure(
            StatusCode::CONFLICT,
            &request_id,
            None,
            ErrorClass::Conflict,
            "snapshot.cursor-invalid",
            "The cursor does not belong to this reconciliation snapshot.",
            Some("cursor"),
            false,
        ),
        Err(error) => store_failure(&request_id, None, &error),
    }
}

fn decode_event_query(raw_query: Option<&str>) -> Option<EventQuery> {
    serde_urlencoded::from_str::<EventQuery>(raw_query.unwrap_or(""))
        .ok()
        .filter(|query| (1..=MAX_EVENT_PAGE_ITEMS).contains(&query.limit))
}

async fn event_page_response(
    app: &App,
    identity: &Identity,
    request_id: &str,
    query: &EventQuery,
) -> Response {
    match app
        .store_io(|| {
            app.store
                .events(&app.scope(identity), query.cursor.as_deref(), query.limit)
        })
        .await
    {
        Ok(Ok(page)) => success(StatusCode::OK, Success::observed(request_id, page)),
        Ok(Err(error)) => event_cursor_failure(request_id, &error),
        Err(error) => store_failure(request_id, None, &error),
    }
}

fn event_cursor_failure(request_id: &str, error: &EventCursorError) -> Response {
    let (code, message, retriable) = match error {
        EventCursorError::Source => (
            "event.source-mismatch",
            "The cursor does not belong to the current event source; reconcile before resuming.",
            true,
        ),
        EventCursorError::Retention { .. } => (
            "event.retention-gap",
            "The cursor is older than retained native history; reconcile before resuming.",
            true,
        ),
        EventCursorError::Invalid => (
            "event.cursor-invalid",
            "The event cursor is malformed or beyond the stream barrier.",
            false,
        ),
    };
    failure(
        StatusCode::CONFLICT,
        request_id,
        None,
        ErrorClass::Conflict,
        code,
        message,
        Some("cursor"),
        retriable,
    )
}

async fn run_event_stream(
    app: Arc<App>,
    scope: Scope,
    query: EventQuery,
    mut subscription: EventSubscription,
    _stream_permit: EventStreamPermit,
    policy: EventStreamPolicy,
    mut socket: WebSocket,
) {
    let mut cursor = query.cursor;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    let mut control_rate = ControlRate::new();
    loop {
        match stream_catch_up(&app, &scope, &mut cursor, query.limit, policy, &mut socket).await {
            Ok(()) => {}
            Err(()) => return,
        }
        loop {
            tokio::select! {
                wakeup = subscription.changed() => {
                    if wakeup.is_err() {
                        return;
                    }
                    break;
                }
                incoming = socket.next() => {
                    match incoming {
                        Some(Err(_)) | None => return,
                        Some(Ok(message)) => match classify_client_frame(&message) {
                            ClientFrame::Close => return,
                            ClientFrame::Data => {
                                let _ = send_protocol_close(
                                    &mut socket,
                                    1003,
                                    "event streams accept control frames only",
                                    policy.send_timeout,
                                )
                                .await;
                                return;
                            }
                            ClientFrame::Control => {
                                if control_rate.exceeded(
                                    policy.max_controls_per_window,
                                    policy.control_window,
                                ) {
                                    let _ = send_protocol_close(
                                        &mut socket,
                                        1008,
                                        "event stream control-frame rate exceeded",
                                        policy.send_timeout,
                                    )
                                    .await;
                                    return;
                                }
                            }
                        },
                    }
                }
                _ = interval.tick() => break,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientFrame {
    Close,
    Data,
    Control,
}

#[derive(Serialize)]
struct EventStreamPageFrame<'a> {
    kind: &'static str,
    page: &'a EventPage,
}

struct ControlRate {
    window_started: tokio::time::Instant,
    count: u32,
}

impl ControlRate {
    fn new() -> Self {
        Self {
            window_started: tokio::time::Instant::now(),
            count: 0,
        }
    }

    fn exceeded(&mut self, maximum: u32, window: std::time::Duration) -> bool {
        if self.window_started.elapsed() >= window {
            self.window_started = tokio::time::Instant::now();
            self.count = 0;
        }
        self.count = self.count.saturating_add(1);
        self.count > maximum
    }
}

struct BoundedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl std::io::Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other(
                "serialized event frame limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bounded_event_frame(page: &EventPage, limit: usize) -> Result<String, ()> {
    let mut writer = BoundedJsonWriter {
        bytes: Vec::with_capacity(limit.min(16 * 1_024)),
        limit,
    };
    serde_json::to_writer(
        &mut writer,
        &EventStreamPageFrame {
            kind: "events",
            page,
        },
    )
    .map_err(|_| ())?;
    String::from_utf8(writer.bytes).map_err(|_| ())
}

fn event_frame_or_backpressure(
    page: &EventPage,
    limit: usize,
    last_cursor: Option<&str>,
) -> Result<String, Value> {
    bounded_event_frame(page, limit).map_err(|()| {
        stream_boundary_payload("backpressure", "event.stream-backpressure", last_cursor)
    })
}

fn classify_client_frame(message: &Message) -> ClientFrame {
    match message {
        Message::Close(_) => ClientFrame::Close,
        Message::Text(_) | Message::Binary(_) => ClientFrame::Data,
        Message::Ping(_) | Message::Pong(_) => ClientFrame::Control,
    }
}

async fn enforce_event_stream_lifetime<F>(lifetime: std::time::Duration, session: F) -> bool
where
    F: std::future::Future<Output = ()>,
{
    tokio::time::timeout(lifetime, session).await.is_ok()
}

async fn stream_catch_up(
    app: &App,
    scope: &Scope,
    cursor: &mut Option<String>,
    limit: u32,
    policy: EventStreamPolicy,
    socket: &mut WebSocket,
) -> Result<(), ()> {
    let page_limit = limit.min(policy.max_page_items);
    for _ in 0..policy.max_catch_up_pages {
        let page = match app
            .store_io(|| app.store.events(scope, cursor.as_deref(), page_limit))
            .await
        {
            Ok(Ok(page)) => page,
            Ok(Err(_)) => {
                send_stream_boundary(
                    socket,
                    "gap",
                    "event.cursor-gap",
                    cursor.as_deref(),
                    policy.send_timeout,
                )
                .await?;
                return Err(());
            }
            Err(_) => {
                send_stream_boundary(
                    socket,
                    "failure",
                    "event.store-failed",
                    cursor.as_deref(),
                    policy.send_timeout,
                )
                .await?;
                return Err(());
            }
        };
        let next = page.next_cursor.clone();
        let caught_up = page
            .items
            .last()
            .is_none_or(|event| event.seq == page.through_seq)
            || page.items.len() < usize::try_from(page_limit).unwrap_or(usize::MAX);
        if !page.items.is_empty() {
            let encoded = match event_frame_or_backpressure(
                &page,
                policy.max_output_bytes,
                cursor.as_deref(),
            ) {
                Ok(encoded) => encoded,
                Err(boundary) => {
                    send_stream_boundary_payload(socket, boundary, policy.send_timeout).await?;
                    return Err(());
                }
            };
            send_stream_message(socket, Message::Text(encoded.into()), policy.send_timeout).await?;
        }
        *cursor = Some(next);
        if caught_up {
            return Ok(());
        }
    }
    send_stream_boundary(
        socket,
        "backpressure",
        "event.catch-up-limit",
        cursor.as_deref(),
        policy.send_timeout,
    )
    .await?;
    Err(())
}

async fn send_stream_boundary(
    socket: &mut WebSocket,
    kind: &str,
    code: &str,
    cursor: Option<&str>,
    send_timeout: std::time::Duration,
) -> Result<(), ()> {
    send_stream_boundary_payload(
        socket,
        stream_boundary_payload(kind, code, cursor),
        send_timeout,
    )
    .await
}

async fn send_stream_boundary_payload(
    socket: &mut WebSocket,
    payload: Value,
    send_timeout: std::time::Duration,
) -> Result<(), ()> {
    send_stream_message(
        socket,
        Message::Text(payload.to_string().into()),
        send_timeout,
    )
    .await?;
    send_stream_message(
        socket,
        Message::Close(Some(CloseFrame {
            code: 1013,
            reason: "resume with pull from last_cursor".into(),
        })),
        send_timeout,
    )
    .await
}

fn stream_boundary_payload(kind: &str, code: &str, cursor: Option<&str>) -> Value {
    json!({
        "kind": kind,
        "code": code,
        "last_cursor": cursor,
        "recovery": "pull"
    })
}

async fn send_protocol_close(
    socket: &mut WebSocket,
    code: u16,
    reason: &'static str,
    send_timeout: std::time::Duration,
) -> Result<(), ()> {
    send_stream_message(
        socket,
        Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })),
        send_timeout,
    )
    .await
}

async fn send_stream_message(
    socket: &mut WebSocket,
    message: Message,
    send_timeout: std::time::Duration,
) -> Result<(), ()> {
    enforce_stream_send_deadline(send_timeout, socket.send(message)).await
}

async fn enforce_stream_send_deadline<F, E>(
    send_timeout: std::time::Duration,
    send: F,
) -> Result<(), ()>
where
    F: std::future::Future<Output = Result<(), E>>,
{
    tokio::time::timeout(send_timeout, send)
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

async fn operation_get(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    if validate_operation_id(&operation_id).is_err() {
        return not_found(&request_id);
    }
    match app
        .store_io(|| app.store.operation(&app.scope(&identity), &operation_id))
        .await
    {
        Ok(Some(operation)) => success(StatusCode::OK, Success::observed(request_id, operation)),
        Ok(None) => not_found_at(&request_id, "operation"),
        Err(error) => store_failure(&request_id, None, &error),
    }
}

async fn route_not_found(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let request_id = request_id(&app, &headers);
    not_found(&request_id)
}

async fn read_bounded_body(body: Body, request_id: &str) -> Result<axum::body::Bytes, Response> {
    match tokio::time::timeout(REQUEST_BODY_READ_TIMEOUT, to_bytes(body, BODY_LIMIT)).await {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(_)) => Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            request_id,
            None,
            ErrorClass::Exhausted,
            "request.body-limit",
            "Request body exceeds the configured byte limit.",
            Some("body"),
            false,
        )),
        Err(_) => Err(failure(
            StatusCode::REQUEST_TIMEOUT,
            request_id,
            None,
            ErrorClass::Refused,
            "request.body-timeout",
            "Request body did not complete within the configured deadline.",
            Some("body"),
            true,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn decode_mutation<T: DeserializeOwned>(
    app: &App,
    identity: &Identity,
    operation_kind: &str,
    method: &str,
    address: &str,
    raw_query: Option<&str>,
    body: Body,
    request_id: &str,
) -> Result<BoundMutation<T>, Response> {
    let bytes = read_bounded_body(body, request_id).await?;
    let raw: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Err(schema_invalid(request_id, None, "input")),
    };
    let Some(object) = raw.as_object() else {
        return Err(schema_invalid(request_id, None, "input"));
    };
    let Some(operation) = object.get("op").and_then(Value::as_str) else {
        return Err(schema_invalid(request_id, None, "input"));
    };
    if validate_operation_id(operation).is_err() {
        return Err(schema_invalid(request_id, Some(operation), "input"));
    }
    let Some(raw_input) = object.get("input") else {
        return Err(schema_invalid(request_id, Some(operation), "input"));
    };
    let request_hash = canonical_request_hash_v2(method, address, raw_input, raw_query)
        .map_err(|_| schema_invalid(request_id, Some(operation), "input"))?;
    let operation = operation.to_owned();
    let scope = app.scope(identity);
    match app
        .store_io(|| {
            app.store
                .inspect_reservation(&scope, &operation, &request_hash)
        })
        .await
    {
        Ok(None) => {}
        Ok(Some(reservation)) => {
            return Err(
                reservation_response(Ok(reservation), request_id, &operation)
                    .unwrap_or_else(|| outcome_unknown(request_id, &operation)),
            );
        }
        Err(error) => return Err(store_failure(request_id, Some(&operation), &error)),
    }
    let new = NewOperation {
        scope,
        operation: operation.clone(),
        operation_kind: operation_kind.to_owned(),
        request_hash: request_hash.clone(),
        accepted_at: app.authority.now().to_rfc3339(),
        capability_snapshot: None,
        actor: identity.actor.clone(),
        principal: identity.principal.clone(),
        resource: None,
    };
    if !query_is_empty(raw_query) {
        let response = schema_invalid(request_id, Some(&operation), "query");
        return Err(record_bound_refusal(app, request_id, &new, response).await);
    }
    if object.len() != 2 {
        let response = schema_invalid(request_id, Some(&operation), "input");
        return Err(record_bound_refusal(app, request_id, &new, response).await);
    }
    let Ok(input) = serde_json::from_value(raw_input.clone()) else {
        let response = schema_invalid(request_id, Some(&operation), "input");
        return Err(record_bound_refusal(app, request_id, &new, response).await);
    };
    Ok(BoundMutation {
        op: operation,
        input,
        request_hash,
    })
}

async fn record_bound_refusal(
    app: &App,
    request_id: &str,
    new: &NewOperation,
    response: Response,
) -> Response {
    let Some(detail) = response.extensions().get::<ErrorDetail>().cloned() else {
        return store_failure(
            request_id,
            Some(&new.operation),
            &StoreError::NotAccepted(new.operation.clone()),
        );
    };
    let status = response.status().as_u16();
    #[cfg(test)]
    if let Some(hook) = app.refusal_before_record.lock().take() {
        hook(new);
    }
    match app
        .store_io(|| {
            app.store
                .record_refusal(new, &app.authority.now().to_rfc3339(), status, &detail)
        })
        .await
    {
        Ok(Reservation::Replay(answer)) => replay(request_id, &new.operation, answer),
        Ok(Reservation::Conflict) => conflict(
            request_id,
            &new.operation,
            "operation.request-conflict",
            "Operation id is already bound to different input.",
            "operation",
        ),
        Ok(Reservation::Pending(_) | Reservation::Accepted) => {
            outcome_unknown(request_id, &new.operation)
        }
        Ok(Reservation::Capacity(_)) => operation_ledger_capacity(request_id),
        Err(error) => store_failure(request_id, Some(&new.operation), &error),
    }
}

fn validate_workspace_input(
    mutation: &BoundMutation<WorkspaceCreateInput>,
    request_id: &str,
) -> Result<(), Response> {
    if mutation.input.labels().len() > 64
        || mutation.input.labels().iter().any(|(key, value)| {
            key.is_empty()
                || key.len() > 128
                || value.len() > 256
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        })
    {
        return Err(schema_invalid(request_id, Some(&mutation.op), "input"));
    }
    if mutation
        .input
        .lease_ttl_ms
        .is_some_and(|ttl| !(MIN_LEASE_TTL_MS..=MAX_LEASE_TTL_MS).contains(&ttl))
    {
        return Err(schema_invalid(request_id, Some(&mutation.op), "input"));
    }
    if let WorkspaceSource::Git(source) = &mutation.input.source {
        let git = &source.git;
        if git.source.is_empty()
            || git.source.len() > 128
            || git.reference.is_empty()
            || git.reference.len() > 512
            || !(1..=1000).contains(&git.depth)
        {
            return Err(schema_invalid(request_id, Some(&mutation.op), "input"));
        }
    }
    Ok(())
}

trait WorkspaceInput {
    fn labels(&self) -> &substrate_wire::Labels;
}

impl WorkspaceInput for WorkspaceCreateInput {
    fn labels(&self) -> &substrate_wire::Labels {
        &self.labels
    }
}

fn validate_exec_input(
    app: &App,
    mutation: &BoundMutation<ExecStartInput>,
    request_id: &str,
) -> Result<(), Response> {
    let input = &mutation.input;
    let allow = input.env.allow.iter().copied().collect::<BTreeSet<_>>();
    let shape_valid = input.workspace.starts_with("ws_")
        && input.workspace[3..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
        && input.argv.len() <= 256
        && !input.argv.is_empty()
        && input.argv.iter().all(|argument| {
            !argument.is_empty() && argument.len() <= 4096 && !argument.contains('\0')
        })
        && allow.len() == input.env.allow.len()
        && input.env.set.len() <= 64
        && input.env.set.iter().all(|(name, value)| {
            valid_environment_name(name)
                && value.len() <= 4096
                && !value.contains('\0')
                && !secretish_name(name)
        })
        && input.limits.timeout_ms > 0
        && input.limits.timeout_ms <= 86_400_000
        && input.limits.output_bytes > 0
        && input.limits.output_bytes <= 1_048_576
        && (1..=4096).contains(&input.limits.processes)
        && (1_048_576..=1_099_511_627_776).contains(&input.limits.memory_bytes)
        && (1..=86_400_000).contains(&input.limits.cpu_millis)
        && input
            .lease_ttl_ms
            .is_none_or(|ttl| (MIN_LEASE_TTL_MS..=MAX_LEASE_TTL_MS).contains(&ttl))
        && input.sandbox.required
        && input.sandbox.capability_snapshot.starts_with("sha256:")
        && input.sandbox.capability_snapshot.len() == 71
        && input.sandbox.capability_snapshot[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    if !shape_valid {
        return Err(schema_invalid(request_id, Some(&mutation.op), "input"));
    }
    let facts = app.driver.machine().facts;
    if input.sandbox.capability_snapshot != app.driver.machine().snapshot {
        return Err(failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            request_id,
            Some(&mutation.op),
            ErrorClass::Refused,
            "exec.capability-stale",
            "The admitted capability snapshot is stale.",
            Some("capability_snapshot"),
            false,
        ));
    }
    if input.sandbox.network == NetworkMode::Aperture {
        return Err(failure(
            StatusCode::NOT_IMPLEMENTED,
            request_id,
            Some(&mutation.op),
            ErrorClass::Unserved,
            "exec.network-unserved",
            "Requested network aperture is not served by this host.",
            Some("exec.network-aperture"),
            false,
        ));
    }
    if facts.exec_namespaces.is_none()
        || facts.exec_cgroup_limits.is_none()
        || facts.exec_cgroup_kill != Some(true)
        || facts.exec_no_egress != Some(true)
    {
        return Err(failure(
            StatusCode::NOT_IMPLEMENTED,
            request_id,
            Some(&mutation.op),
            ErrorClass::Unserved,
            "exec.sandbox-unavailable",
            "Required host confinement is not available.",
            Some("exec.namespaces"),
            false,
        ));
    }
    Ok(())
}

fn validate_pipe_session_input(
    app: &App,
    mutation: &BoundMutation<PipeSessionStartInput>,
    request_id: &str,
) -> Result<(), Response> {
    let exec_mutation = BoundMutation {
        op: mutation.op.clone(),
        input: mutation.input.exec.clone(),
        request_hash: mutation.request_hash.clone(),
    };
    validate_exec_input(app, &exec_mutation, request_id)?;
    if !pipe_confinement_available(&app.driver.machine().facts) {
        return Err(failure(
            StatusCode::NOT_IMPLEMENTED,
            request_id,
            Some(&mutation.op),
            ErrorClass::Unserved,
            "session.confinement-unavailable",
            "Raw-pipe sessions require complete namespaces and cgroup limits, whole-tree kill, explicit leases, no egress, and bounded output.",
            Some("session"),
            false,
        ));
    }
    let input = &mutation.input;
    if input.exec.wait
        || input.exec.lease_ttl_ms.is_none()
        || input.input_limit_bytes == 0
        || input.input_limit_bytes > PIPE_MAX_INPUT_BYTES
        || input.frame_limit_bytes == 0
        || input.frame_limit_bytes > PIPE_MAX_FRAME_BYTES
        || input.queued_frames == 0
        || input.queued_frames > PIPE_MAX_QUEUED_FRAMES
    {
        return Err(schema_invalid(request_id, Some(&mutation.op), "input"));
    }
    Ok(())
}

fn pipe_confinement_available(facts: &substrate_wire::CapabilityFacts) -> bool {
    let namespaces = facts.exec_namespaces.as_ref();
    let cgroups = facts.exec_cgroup_limits.as_ref();
    namespaces.is_some_and(|value| {
        value.user && value.mount && value.pid && value.ipc && value.uts && value.network
    }) && cgroups.is_some_and(|value| value.processes && value.memory && value.cpu)
        && facts.exec_cgroup_kill == Some(true)
        && facts.exec_no_egress == Some(true)
        && facts.leases_explicit == Some(true)
        && facts.exec_output_limit_bytes.is_some_and(|value| value > 0)
}

fn valid_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_uppercase()
            } else {
                byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'
            }
        })
}

fn secretish_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "authorization",
        "bearer",
        "credential",
        "password",
        "proxy",
        "secret",
        "token",
    ]
    .iter()
    .any(|fragment| lower.contains(fragment))
}

#[allow(clippy::too_many_arguments)] // These fields are the exact durable admission tuple.
async fn begin<T>(
    app: &App,
    identity: &Identity,
    request_id: &str,
    operation_kind: &str,
    method: &str,
    address: &str,
    mutation: &BoundMutation<T>,
    capability_snapshot: Option<String>,
    resource: Option<String>,
) -> Option<Response> {
    let new = new_operation(
        app,
        identity,
        operation_kind,
        method,
        address,
        mutation,
        capability_snapshot,
        resource,
    );
    reservation_response(
        app.store_io(|| app.store.reserve(&new)).await,
        request_id,
        &mutation.op,
    )
}

#[allow(clippy::too_many_arguments)]
fn new_operation<T>(
    app: &App,
    identity: &Identity,
    operation_kind: &str,
    _method: &str,
    _address: &str,
    mutation: &BoundMutation<T>,
    capability_snapshot: Option<String>,
    resource: Option<String>,
) -> NewOperation {
    NewOperation {
        scope: app.scope(identity),
        operation: mutation.op.clone(),
        operation_kind: operation_kind.to_owned(),
        request_hash: mutation.request_hash.clone(),
        accepted_at: app.authority.now().to_rfc3339(),
        capability_snapshot: capability_snapshot.or_else(|| Some(app.driver.machine().snapshot)),
        actor: identity.actor.clone(),
        principal: identity.principal.clone(),
        resource,
    }
}

fn reservation_response(
    reservation: Result<Reservation, StoreError>,
    request_id: &str,
    operation: &str,
) -> Option<Response> {
    match reservation {
        Ok(Reservation::Accepted) => None,
        Ok(Reservation::Replay(answer)) => Some(replay(request_id, operation, answer)),
        Ok(Reservation::Conflict) => Some(conflict(
            request_id,
            operation,
            "operation.request-conflict",
            "Operation id is already bound to different input.",
            "operation",
        )),
        Ok(Reservation::Pending(_)) => Some(outcome_unknown(request_id, operation)),
        Ok(Reservation::Capacity(_)) => Some(operation_ledger_capacity(request_id)),
        Err(error) => Some(store_failure(request_id, Some(operation), &error)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn refuse_before_dispatch<T>(
    app: &App,
    identity: &Identity,
    request_id: &str,
    operation_kind: &str,
    method: &str,
    address: &str,
    mutation: &BoundMutation<T>,
    error: &DriverError,
) -> Response {
    let response = driver_failure(request_id, Some(&mutation.op), error);
    refuse_before_dispatch_response(
        app,
        identity,
        request_id,
        operation_kind,
        method,
        address,
        mutation,
        response,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn refuse_workspace_mutation<T>(
    app: &App,
    identity: &Identity,
    request_id: &str,
    operation_kind: &str,
    method: &str,
    address: &str,
    mutation: &BoundMutation<T>,
    response: Response,
) -> Response {
    refuse_before_dispatch_response(
        app,
        identity,
        request_id,
        operation_kind,
        method,
        address,
        mutation,
        response,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn refuse_before_dispatch_response<T>(
    app: &App,
    identity: &Identity,
    request_id: &str,
    operation_kind: &str,
    method: &str,
    address: &str,
    mutation: &BoundMutation<T>,
    response: Response,
) -> Response {
    let new = NewOperation {
        scope: app.scope(identity),
        operation: mutation.op.clone(),
        operation_kind: operation_kind.to_owned(),
        request_hash: mutation.request_hash.clone(),
        accepted_at: app.authority.now().to_rfc3339(),
        capability_snapshot: None,
        actor: identity.actor.clone(),
        principal: identity.principal.clone(),
        resource: None,
    };
    let _ = (method, address);
    record_bound_refusal(app, request_id, &new, response).await
}

async fn finish_success<T: Serialize + Sync>(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    status: StatusCode,
    resource_id: Option<&str>,
    result: T,
) -> Response {
    if let Err(error) = app
        .store_io(|| {
            app.store.complete_success(
                scope,
                operation,
                &app.authority.now().to_rfc3339(),
                status.as_u16(),
                resource_id,
                &result,
            )
        })
        .await
    {
        return store_failure(request_id, Some(operation), &error);
    }
    success(status, Success::mutation(request_id, operation, result))
}

async fn finish_exec(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    status: StatusCode,
    observation: ExecObservation,
) -> Response {
    finish_exec_leased(app, scope, request_id, operation, status, observation, None).await
}

#[allow(clippy::too_many_arguments)]
async fn finish_exec_leased(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    status: StatusCode,
    observation: ExecObservation,
    lease: Option<&NewLease>,
) -> Response {
    let write = app
        .store_io(|| {
            app.store.complete_exec_leased(
                scope,
                operation,
                &app.authority.now().to_rfc3339(),
                status.as_u16(),
                &observation.resource,
                &observation.stdout,
                &observation.stderr,
                observation.stdout_truncated,
                observation.stderr_truncated,
                observation.output_complete,
                observation.cgroup.as_deref(),
                observation.leader_pid,
                lease,
            )
        })
        .await;
    let authoritative = match write {
        Ok(ExecWrite::PersistedExact(stored)) => {
            app.driver.acknowledge_exec(&observation);
            stored.resource
        }
        Ok(ExecWrite::Superseded(stored)) => {
            app.driver.discard_superseded_exec(&observation.resource.id);
            stored.resource
        }
        Ok(ExecWrite::PersistedTransformed(stored)) => stored.resource,
        Ok(ExecWrite::Retired) => {
            app.driver.discard_superseded_exec(&observation.resource.id);
            return finish_exec_retired_race(
                app,
                scope,
                request_id,
                operation,
                &observation.resource.id,
            )
            .await;
        }
        Err(error) => return store_failure(request_id, Some(operation), &error),
    };
    success(
        status,
        Success::mutation(request_id, operation, authoritative),
    )
}

async fn finish_pipe_session_start(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    provisional: &PipeSession,
    observation: ExecObservation,
    lease: &NewLease,
) -> Response {
    let mut session = provisional.clone();
    session.state = SessionState::Ready;
    session.attachment = SessionAttachmentState::Available;
    session.observed_at = observation.resource.observed_at;
    session.exit.clone_from(&observation.resource.exit);
    session.lease = lease.observation();
    let stored = stored_exec(&observation);
    match app
        .store_io(|| {
            app.store.complete_pipe_session_start(
                scope,
                operation,
                &app.authority.now().to_rfc3339(),
                StatusCode::ACCEPTED.as_u16(),
                &session,
                &stored,
                lease,
            )
        })
        .await
    {
        Ok((authoritative, _)) => {
            app.driver.acknowledge_exec(&observation);
            success(
                StatusCode::ACCEPTED,
                Success::mutation(request_id, operation, authoritative),
            )
        }
        Err(error) => store_failure(request_id, Some(operation), &error),
    }
}

async fn finish_pipe_session_observation(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    session_id: &str,
    observation: ExecObservation,
) -> Response {
    let stored = stored_exec(&observation);
    match app
        .store_io(|| {
            app.store.complete_pipe_session_observation(
                scope,
                operation,
                &app.authority.now().to_rfc3339(),
                StatusCode::OK.as_u16(),
                session_id,
                &stored,
            )
        })
        .await
    {
        Ok(session) => {
            app.driver.acknowledge_exec(&observation);
            success(
                StatusCode::OK,
                Success::mutation(request_id, operation, session),
            )
        }
        Err(error) => store_failure(request_id, Some(operation), &error),
    }
}

fn stored_exec(observation: &ExecObservation) -> StoredExec {
    StoredExec {
        resource: observation.resource.clone(),
        stdout: observation.stdout.clone(),
        stderr: observation.stderr.clone(),
        stdout_truncated: observation.stdout_truncated,
        stderr_truncated: observation.stderr_truncated,
        output_complete: observation.output_complete,
        cgroup: observation.cgroup.clone(),
        leader_pid: observation.leader_pid,
    }
}

fn observation_from_stored(stored: StoredExec) -> ExecObservation {
    ExecObservation {
        resource: stored.resource,
        stdout: stored.stdout,
        stderr: stored.stderr,
        stdout_truncated: stored.stdout_truncated,
        stderr_truncated: stored.stderr_truncated,
        output_complete: stored.output_complete,
        cgroup: stored.cgroup,
        leader_pid: stored.leader_pid,
    }
}

fn stored_output(
    app: &App,
    stored: &StoredExec,
    exec_id: &str,
    query: &ExecOutputQuery,
) -> Result<OutputSlice, DriverError> {
    let limit = app
        .driver
        .machine()
        .facts
        .exec_output_limit_bytes
        .unwrap_or(0);
    if query.limit_bytes > limit {
        return Err(DriverError::exhausted(
            "exec.output-limit",
            "Requested output range exceeds the probed limit.",
            "limit",
        ));
    }
    let (source, truncated) = match query.stream {
        substrate_wire::OutputStream::Stdout => (&stored.stdout, stored.stdout_truncated),
        substrate_wire::OutputStream::Stderr => (&stored.stderr, stored.stderr_truncated),
    };
    let start = usize::try_from(query.offset)
        .unwrap_or(usize::MAX)
        .min(source.len());
    let end = start
        .saturating_add(usize::try_from(query.limit_bytes).unwrap_or(usize::MAX))
        .min(source.len());
    Ok(OutputSlice {
        exec: exec_id.to_owned(),
        stream: query.stream,
        offset: query.offset,
        returned_bytes: u64::try_from(end - start).expect("usize fits u64"),
        next_offset: u64::try_from(end).expect("usize fits u64"),
        eof: stored.output_complete && end == source.len(),
        truncated,
        content: Base64Content {
            encoding: Base64Encoding::Base64,
            data: base64::engine::general_purpose::STANDARD.encode(&source[start..end]),
        },
        observed_at: app.authority.now(),
    })
}

async fn finish_driver_error(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    resource_id: Option<&str>,
    error: &DriverError,
) -> Response {
    let (status, mut detail) = driver_detail(Some(operation), error);
    detail.retriable = false;
    if let Err(store_error) = app
        .store_io(|| {
            app.store.complete_error(
                scope,
                operation,
                &app.authority.now().to_rfc3339(),
                status.as_u16(),
                resource_id,
                &detail,
            )
        })
        .await
    {
        return store_failure(request_id, Some(operation), &store_error);
    }
    failure_detail(status, request_id, detail)
}

async fn finish_exec_retired_race(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    exec_id: &str,
) -> Response {
    let detail = ErrorDetail {
        class: ErrorClass::Conflict,
        code: "exec.retired".to_owned(),
        message: "Exec was retired while its terminal observation was being committed.".to_owned(),
        retriable: false,
        address: Some("exec".to_owned()),
        operation: Some(operation.to_owned()),
    };
    if let Err(error) = app
        .store_io(|| {
            app.store.complete_error(
                scope,
                operation,
                &app.authority.now().to_rfc3339(),
                StatusCode::CONFLICT.as_u16(),
                Some(exec_id),
                &detail,
            )
        })
        .await
    {
        return store_failure(request_id, Some(operation), &error);
    }
    failure_detail(StatusCode::CONFLICT, request_id, detail)
}

async fn finish_dispatch_absence(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    resource_kind: &str,
    resource_id: &str,
    error: &DriverError,
) -> Response {
    let (status, mut detail) = driver_detail(Some(operation), error);
    detail.retriable = false;
    if let Err(store_error) = app
        .store_io(|| {
            app.store.complete_dispatch_absence(
                scope,
                operation,
                &app.authority.now().to_rfc3339(),
                status.as_u16(),
                resource_kind,
                resource_id,
                &detail,
            )
        })
        .await
    {
        return store_failure(request_id, Some(operation), &store_error);
    }
    failure_detail(status, request_id, detail)
}

#[allow(clippy::too_many_arguments)]
async fn finish_pipe_session_dispatch_absence(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    session_id: &str,
    exec_id: &str,
    error: &DriverError,
) -> Response {
    let (status, mut detail) = driver_detail(Some(operation), error);
    detail.retriable = false;
    if let Err(store_error) = app
        .store_io(|| {
            app.store.complete_pipe_session_dispatch_absence(
                scope,
                operation,
                &app.authority.now().to_rfc3339(),
                status.as_u16(),
                session_id,
                exec_id,
                &detail,
            )
        })
        .await
    {
        return store_failure(request_id, Some(operation), &store_error);
    }
    failure_detail(status, request_id, detail)
}

#[allow(clippy::too_many_arguments)]
async fn finish_pipe_session_dispatch_unknown(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    session_id: &str,
    exec_id: &str,
    error: &DriverError,
) -> Response {
    if let Err(store_error) = app
        .store_io(|| {
            app.store.mark_pipe_session_dispatch_unknown(
                scope,
                operation,
                app.authority.now(),
                session_id,
                exec_id,
            )
        })
        .await
    {
        return store_failure(request_id, Some(operation), &store_error);
    }
    tracing::warn!(
        resource = session_id,
        code = error.code,
        "pipe session outcome remains unknown"
    );
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        request_id,
        Some(operation),
        ErrorClass::Failed,
        "operation.outcome-unknown",
        "The session was accepted, but driver containment or the resulting state is unproven.",
        Some("session"),
        true,
    )
}

async fn finish_dispatch_unknown(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    resource_kind: &str,
    resource_id: &str,
    error: &DriverError,
) -> Response {
    if let Err(store_error) = app
        .store_io(|| {
            app.store.mark_dispatch_unknown(
                scope,
                operation,
                &app.authority.now().to_rfc3339(),
                resource_kind,
                resource_id,
            )
        })
        .await
    {
        return store_failure(request_id, Some(operation), &store_error);
    }
    tracing::warn!(
        resource = resource_id,
        code = error.code,
        "driver dispatch outcome remains unknown"
    );
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        request_id,
        Some(operation),
        ErrorClass::Failed,
        "operation.outcome-unknown",
        "The operation was accepted, but driver containment or the resulting state is unproven.",
        Some(resource_kind),
        true,
    )
}

fn replay(request_id: &str, operation: &str, answer: StoredAnswer) -> Response {
    let status = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    match answer.outcome {
        OperationOutcome::Success { result } => {
            success(status, Success::mutation(request_id, operation, result))
        }
        OperationOutcome::Error { mut error } => {
            error.operation = Some(operation.to_owned());
            failure_detail(status, request_id, error)
        }
    }
}

fn driver_failure(request_id: &str, operation: Option<&str>, error: &DriverError) -> Response {
    let (status, detail) = driver_detail(operation, error);
    failure_detail(status, request_id, detail)
}

fn driver_detail(operation: Option<&str>, error: &DriverError) -> (StatusCode, ErrorDetail) {
    let (status, class) = match error.class {
        DriverErrorClass::Refused => (StatusCode::UNPROCESSABLE_ENTITY, ErrorClass::Refused),
        DriverErrorClass::NotFound => (StatusCode::NOT_FOUND, ErrorClass::Refused),
        DriverErrorClass::Conflict => (StatusCode::CONFLICT, ErrorClass::Conflict),
        DriverErrorClass::Unserved => (StatusCode::NOT_IMPLEMENTED, ErrorClass::Unserved),
        DriverErrorClass::Exhausted => (StatusCode::TOO_MANY_REQUESTS, ErrorClass::Exhausted),
        DriverErrorClass::Failed => (StatusCode::INTERNAL_SERVER_ERROR, ErrorClass::Failed),
    };
    (
        status,
        ErrorDetail {
            class,
            code: error.code.to_owned(),
            message: error.message.clone(),
            retriable: error.retriable,
            address: error.address.clone(),
            operation: operation.map(ToOwned::to_owned),
        },
    )
}

fn new_lease(
    app: &App,
    identity: &Identity,
    ttl_ms: u64,
    request_id: &str,
    operation: &str,
) -> Result<NewLease, Response> {
    app.lease_clock()
        .map(|clock| NewLease {
            ttl_ms,
            clock,
            authorizing_operation: operation.to_owned(),
            actor: identity.actor.clone(),
            principal: identity.principal.clone(),
        })
        .map_err(|_| {
            failure(
                StatusCode::NOT_IMPLEMENTED,
                request_id,
                Some(operation),
                ErrorClass::Unserved,
                "lease.clock-unavailable",
                "A durable Linux boot-relative lease clock is unavailable.",
                Some("lease.clock"),
                false,
            )
        })
}

async fn finish_lease_store_error(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    error: &StoreError,
) -> Response {
    let (status, detail) = match error {
        StoreError::LeaseAbsent => (
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorDetail {
                class: ErrorClass::Refused,
                code: "lease.absent".to_owned(),
                message: "The resource has no explicit lease to renew.".to_owned(),
                retriable: false,
                address: Some("lease".to_owned()),
                operation: Some(operation.to_owned()),
            },
        ),
        StoreError::LeaseExpired => (
            StatusCode::CONFLICT,
            ErrorDetail {
                class: ErrorClass::Conflict,
                code: "lease.expired".to_owned(),
                message: "An expired lease cannot be renewed.".to_owned(),
                retriable: false,
                address: Some("lease".to_owned()),
                operation: Some(operation.to_owned()),
            },
        ),
        StoreError::WorkspaceFrozen => (
            StatusCode::CONFLICT,
            ErrorDetail {
                class: ErrorClass::Conflict,
                code: "workspace.not-ready".to_owned(),
                message: "Workspace is not ready for lease renewal.".to_owned(),
                retriable: false,
                address: Some("workspace".to_owned()),
                operation: Some(operation.to_owned()),
            },
        ),
        _ => return store_failure(request_id, Some(operation), error),
    };
    if let Err(store_error) = app
        .store_io(|| {
            app.store.complete_error(
                scope,
                operation,
                &app.authority.now().to_rfc3339(),
                status.as_u16(),
                None,
                &detail,
            )
        })
        .await
    {
        return store_failure(request_id, Some(operation), &store_error);
    }
    failure_detail(status, request_id, detail)
}

fn linux_lease_clock() -> Result<LeaseClock, String> {
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|error| format!("read boot id: {error}"))?;
    let boot_id = boot_id.trim();
    if boot_id.is_empty()
        || !boot_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err("invalid Linux boot identity".to_owned());
    }
    let uptime = std::fs::read_to_string("/proc/uptime")
        .map_err(|error| format!("read boot-relative clock: {error}"))?;
    let seconds = uptime
        .split_whitespace()
        .next()
        .ok_or_else(|| "missing boot-relative clock".to_owned())?;
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let whole = whole
        .parse::<u64>()
        .map_err(|_| "invalid boot-relative seconds".to_owned())?;
    let mut milliseconds = 0_u64;
    for (index, byte) in fraction.bytes().take(3).enumerate() {
        if !byte.is_ascii_digit() {
            return Err("invalid boot-relative fraction".to_owned());
        }
        let place = match index {
            0 => 100,
            1 => 10,
            _ => 1,
        };
        milliseconds += u64::from(byte - b'0') * place;
    }
    Ok(LeaseClock {
        wall: Utc::now(),
        boot_id: boot_id.to_owned(),
        boottime_ms: whole
            .checked_mul(1_000)
            .and_then(|value| value.checked_add(milliseconds))
            .ok_or_else(|| "boot-relative clock overflow".to_owned())?,
    })
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::Extension;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use serde_json::Value;
    use substrate_host::{HostConfig, HostDriver};
    use tokio::sync::Semaphore;
    use tower::ServiceExt as _;

    use substrate_store::{CommitEffect, CommitEffectSink, Reservation, Scope, Store};

    use super::{
        ClientFrame, CompletedDriverAction, ControlRate, EventStreamLimits, EventStreamPolicy,
        EventWakeups, MAINTENANCE_DRIVER_TIMEOUT, REQUEST_BODY_READ_TIMEOUT, WakePosition,
        WorkspaceLockDomains, bounded_blocking, bounded_event_frame, classify_client_frame,
        completed_driver_action, enforce_event_stream_lifetime, enforce_stream_send_deadline,
        event_frame_or_backpressure, read_bounded_body, run_maintenance_driver,
    };

    async fn bound_refusal_response(app: Arc<super::App>, operation: &str) -> Value {
        let request = Request::builder()
            .method("POST")
            .uri("/v1/workspaces")
            .header("content-type", "application/json")
            .header("x-request-id", "req_refusal_race")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "op": operation,
                    "input": {
                        "source": "empty",
                        "labels": {},
                        "unexpected": true
                    }
                }))
                .expect("request JSON"),
            ))
            .expect("request");
        let identity = super::Identity {
            subject: "local:1000".to_owned(),
            actor: "refusal-test".to_owned(),
            principal: None,
        };
        let response = super::router(app)
            .layer(Extension(identity))
            .oneshot(request)
            .await
            .expect("router response");
        serde_json::from_slice(
            &to_bytes(response.into_body(), 2_097_152)
                .await
                .expect("response bytes"),
        )
        .expect("response JSON")
    }

    fn effect(subject: &str, through_seq: u64) -> CommitEffect {
        CommitEffect {
            scope: Scope {
                deployment: "dep_test".to_owned(),
                subject: subject.to_owned(),
            },
            source_scope: format!("source-{subject}"),
            generation: 1,
            through_seq,
        }
    }

    #[test]
    fn completed_exec_ack_requires_every_scope_to_commit_exactly() {
        assert_eq!(
            completed_driver_action(2, 2, false, false),
            CompletedDriverAction::Acknowledge
        );
        assert_eq!(
            completed_driver_action(2, 1, false, true),
            CompletedDriverAction::Retain
        );
        assert_eq!(
            completed_driver_action(2, 1, false, false),
            CompletedDriverAction::Retain
        );
        assert_eq!(
            completed_driver_action(2, 1, true, false),
            CompletedDriverAction::DiscardSuperseded
        );
        assert_eq!(
            completed_driver_action(2, 1, true, true),
            CompletedDriverAction::Retain
        );
        assert_eq!(
            completed_driver_action(0, 0, false, false),
            CompletedDriverAction::Retain
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn saturated_blocking_lane_does_not_starve_unrelated_async_work() {
        let slots = Arc::new(Semaphore::new(2));
        let tasks = (0..8)
            .map(|_| {
                let slots = Arc::clone(&slots);
                tokio::spawn(async move {
                    bounded_blocking(&slots, || std::thread::sleep(Duration::from_millis(50)))
                        .await;
                })
            })
            .collect::<Vec<_>>();

        tokio::time::timeout(Duration::from_millis(25), async {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(1)).await;
        })
        .await
        .expect("unrelated accept/event-shaped work remains schedulable under saturation");

        for task in tasks {
            task.await.expect("bounded blocking task");
        }
    }

    #[tokio::test]
    async fn commit_wakeups_are_scope_local_coalesced_hints_with_raii_cleanup() {
        let wakeups = Arc::new(EventWakeups::default());
        let scope_a = effect("subject-a", 1).scope;
        let scope_b = effect("subject-b", 1).scope;
        let mut a = wakeups.subscribe(&scope_a);
        let mut b = wakeups.subscribe(&scope_b);

        for sequence in 1..=10_000 {
            wakeups.committed(&[effect("subject-b", sequence)]);
        }
        b.changed().await.expect("B receives its coalesced hint");
        assert!(!a.receiver.has_changed().expect("A watch remains open"));

        // Callback order may differ from commit order. Coalescing retains the greatest durable
        // position observed for this source; the store remains the read authority.
        wakeups.committed(&[effect("subject-a", 9)]);
        wakeups.committed(&[effect("subject-a", 8)]);
        a.changed().await.expect("A receives a change hint");
        assert_eq!(
            a.receiver.borrow_and_update().as_ref(),
            Some(&WakePosition {
                generation: 1,
                through_seq: 9,
                source_scope: "source-subject-a".to_owned(),
            })
        );

        drop(a);
        drop(b);
        assert!(wakeups.scopes.lock().is_empty());
    }

    #[test]
    fn event_stream_limits_are_scope_local_and_recover_by_raii() {
        let limits = EventStreamLimits::new(2, 1);
        let scope_a = effect("subject-a", 1).scope;
        let scope_b = effect("subject-b", 1).scope;
        let scope_c = effect("subject-c", 1).scope;

        let a = limits.acquire(&scope_a).expect("A stream capacity");
        assert!(limits.acquire(&scope_a).is_none());
        let b = limits.acquire(&scope_b).expect("B stream capacity");
        assert!(limits.acquire(&scope_c).is_none());
        assert_eq!(limits.scopes.lock().len(), 2);

        drop(a);
        let a_again = limits.acquire(&scope_a).expect("A capacity recovered");
        drop(a_again);
        drop(b);
        assert!(limits.scopes.lock().is_empty());
        assert_eq!(limits.global.available_permits(), 2);
    }

    #[test]
    fn event_stream_client_data_is_rejected_and_controls_are_classified() {
        assert_eq!(
            classify_client_frame(&axum::extract::ws::Message::Text("data".into())),
            ClientFrame::Data
        );
        assert_eq!(
            classify_client_frame(&axum::extract::ws::Message::Binary(Vec::new().into())),
            ClientFrame::Data
        );
        assert_eq!(
            classify_client_frame(&axum::extract::ws::Message::Ping(Vec::new().into())),
            ClientFrame::Control
        );
        assert_eq!(
            classify_client_frame(&axum::extract::ws::Message::Close(None)),
            ClientFrame::Close
        );
    }

    #[test]
    fn event_stream_output_serialization_stops_at_the_byte_limit() {
        let page = substrate_wire::EventPage {
            source_scope: "source-a".to_owned(),
            generation: 1,
            items: Vec::new(),
            next_cursor: "cursor-a".to_owned(),
            through_seq: 0,
            first_retained_seq: None,
        };
        let encoded = bounded_event_frame(&page, 1_024).expect("bounded frame");
        assert!(encoded.len() <= 1_024);
        assert!(bounded_event_frame(&page, encoded.len() - 1).is_err());
    }

    #[test]
    fn manifest_stream_backpressure_vector_executes_the_production_boundary() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../contracts/substrate-wire/0.2.0/vectors/driver/event-stream-backpressure.json"
        ))
        .expect("backpressure vector");
        let setup = &vector["setup"][0]["state"];
        let event_count = setup["event_count"].as_u64().expect("event count");
        let payload_bytes = setup["observation_payload_bytes"]
            .as_u64()
            .expect("observation payload bytes");
        let max_output_bytes = usize::try_from(
            setup["max_output_bytes"]
                .as_u64()
                .expect("max output bytes"),
        )
        .expect("max output range");
        let items = (0..event_count)
            .map(|index| {
                serde_json::from_value(serde_json::json!({
                    "actor": "vector-client",
                    "cause": {
                        "kind": "operation",
                        "operation": "01JPHASE3EVENTSOURCE0001"
                    },
                    "generation": 41,
                    "observation": {
                        "payload": "x".repeat(usize::try_from(payload_bytes).expect("payload range"))
                    },
                    "observed_at": "2026-08-13T12:00:00Z",
                    "principal": null,
                    "resource": format!("ws_event{index:02}"),
                    "resource_kind": "workspace",
                    "seq": index + 8,
                    "transition": "workspace.created"
                }))
                .expect("event fixture")
            })
            .collect();
        let expected = &vector["expected"]["outcome"];
        let page = substrate_wire::EventPage {
            source_scope: "scope_vector_subject".to_owned(),
            generation: 41,
            items,
            next_cursor: "ev2.scope_vector_subject.41.72".to_owned(),
            through_seq: 72,
            first_retained_seq: None,
        };
        let boundary =
            event_frame_or_backpressure(&page, max_output_bytes, expected["last_cursor"].as_str())
                .expect_err("oversized event frame must produce a recovery boundary");
        assert_eq!(boundary["kind"], "backpressure");
        assert_eq!(boundary["code"], expected["code"]);
        assert_eq!(boundary["last_cursor"], expected["last_cursor"]);
        assert_eq!(boundary["recovery"], expected["recovery"]);
    }

    #[tokio::test(start_paused = true)]
    async fn event_stream_control_rate_is_bounded_and_resets_per_window() {
        let mut policy = EventStreamPolicy::production();
        policy.max_controls_per_window = 2;
        policy.control_window = Duration::from_secs(5);
        let mut rate = ControlRate::new();

        assert!(!rate.exceeded(policy.max_controls_per_window, policy.control_window));
        assert!(!rate.exceeded(policy.max_controls_per_window, policy.control_window));
        assert!(rate.exceeded(policy.max_controls_per_window, policy.control_window));
        tokio::time::advance(policy.control_window).await;
        assert!(!rate.exceeded(policy.max_controls_per_window, policy.control_window));
    }

    #[tokio::test(start_paused = true)]
    async fn event_stream_lifetime_cancels_session_and_recovers_permits() {
        let limits = EventStreamLimits::new(1, 1);
        let scope = effect("subject-a", 1).scope;
        let permit = limits.acquire(&scope).expect("stream capacity");
        let task = tokio::spawn(enforce_event_stream_lifetime(
            Duration::from_secs(5),
            async move {
                let _permit = permit;
                std::future::pending::<()>().await;
            },
        ));
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(!task.await.expect("lifetime task"));
        assert!(limits.acquire(&scope).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn event_stream_send_deadline_is_hard() {
        let task = tokio::spawn(enforce_stream_send_deadline(
            Duration::from_secs(2),
            std::future::pending::<Result<(), std::convert::Infallible>>(),
        ));
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(task.await.expect("deadline task"), Err(()));
    }

    #[tokio::test(start_paused = true)]
    async fn mutation_body_read_has_a_hard_deadline() {
        let body = axum::body::Body::from_stream(futures_util::stream::pending::<
            Result<axum::body::Bytes, std::convert::Infallible>,
        >());
        let task = tokio::spawn(read_bounded_body(body, "req_body_timeout"));
        tokio::task::yield_now().await;

        tokio::time::advance(REQUEST_BODY_READ_TIMEOUT).await;
        let result = task.await.expect("decode task");
        let Err(response) = result else {
            panic!("pending body must time out");
        };
        assert_eq!(response.status(), axum::http::StatusCode::REQUEST_TIMEOUT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bound_refusal_store_failure_never_returns_the_original_refusal() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let state_path = directory.path().join("state.db");
        let store = Arc::new(Store::open(&state_path).expect("state store"));
        let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver");
        let app = super::App::new(Arc::clone(&store), driver, "dep_refusal_test");
        let break_path = state_path.clone();
        app.refusal_before_record.lock().replace(Arc::new(move |_| {
            rusqlite::Connection::open(&break_path)
                .expect("fault connection")
                .execute("DROP TABLE operations", [])
                .expect("inject refusal persistence failure");
        }));

        let response = bound_refusal_response(app, "01JREFUSALSTOREFAILURE1").await;
        assert_eq!(response["error"]["code"], "state.store-failed");
        assert_ne!(response["error"]["code"], "request.schema-invalid");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bound_refusal_losing_to_accepted_reservation_returns_outcome_unknown() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver");
        let app = super::App::new(Arc::clone(&store), driver, "dep_refusal_test");
        let racing_store = Arc::clone(&store);
        app.refusal_before_record
            .lock()
            .replace(Arc::new(move |new| {
                assert_eq!(
                    racing_store.reserve(new).expect("racing acceptance"),
                    Reservation::Accepted
                );
            }));

        let operation = "01JREFUSALACCEPTEDRACE1";
        let response = bound_refusal_response(Arc::clone(&app), operation).await;
        assert_eq!(response["error"]["code"], "operation.outcome-unknown");
        assert_eq!(
            store
                .operation(
                    &Scope {
                        deployment: "dep_refusal_test".to_owned(),
                        subject: "local:1000".to_owned(),
                    },
                    operation,
                )
                .expect("operation lookup")
                .expect("accepted operation")
                .state,
            substrate_wire::OperationState::Accepted
        );
    }

    #[tokio::test(start_paused = true)]
    async fn maintenance_driver_deadline_is_hard_and_retriable() {
        let task = tokio::spawn(run_maintenance_driver(std::future::pending::<
            Result<(), substrate_host::DriverError>,
        >()));
        tokio::task::yield_now().await;

        tokio::time::advance(MAINTENANCE_DRIVER_TIMEOUT).await;
        let error = task
            .await
            .expect("deadline task")
            .expect_err("pending driver call must time out");
        assert_eq!(error.code, "maintenance.driver-timeout");
        assert!(error.retriable);
    }

    #[tokio::test]
    async fn workspace_lock_domains_isolate_subjects_but_serialize_one_scope() {
        let locks = WorkspaceLockDomains::default();
        let scope_a = effect("subject-a", 1).scope;
        let scope_b = effect("subject-b", 1).scope;
        let domain_a = locks.domain(&scope_a);
        let same_a = locks.domain(&scope_a);
        let domain_b = locks.domain(&scope_b);

        assert!(Arc::ptr_eq(&domain_a, &same_a));
        assert!(!Arc::ptr_eq(&domain_a, &domain_b));

        let held = Arc::clone(&domain_a.stripes[0]).lock_owned().await;
        assert!(domain_a.stripes[0].try_lock().is_err());
        assert!(domain_b.stripes[0].try_lock().is_ok());
        drop(held);
        assert!(domain_a.stripes[0].try_lock().is_ok());
    }
}

fn request_id(app: &App, headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            (8..=128).contains(&value.len())
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        .map_or_else(|| app.authority.request_id(), ToOwned::to_owned)
}

fn query_is_empty(raw_query: Option<&str>) -> bool {
    raw_query.is_none_or(str::is_empty)
}

fn normalized_file_address(workspace_id: &str, path: &str) -> String {
    let mut address = format!("/v1/workspaces/{workspace_id}/files/");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            address.push(char::from(byte));
        } else {
            let _ = write!(address, "%{byte:02X}");
        }
    }
    address
}

fn path_refusal(
    request_id: &str,
    operation: Option<&str>,
    error: &WireValidationError,
) -> Response {
    let (code, message) = if matches!(error, WireValidationError::InvalidPathDepth) {
        (
            "workspace.path-depth",
            "Workspace path exceeds the configured component limit.",
        )
    } else {
        (
            "workspace.path-escape",
            "Workspace path is outside the confined root.",
        )
    };
    failure(
        StatusCode::UNPROCESSABLE_ENTITY,
        request_id,
        operation,
        ErrorClass::Refused,
        code,
        message,
        Some("path"),
        false,
    )
}

fn schema_invalid(request_id: &str, operation: Option<&str>, address: &str) -> Response {
    failure(
        StatusCode::UNPROCESSABLE_ENTITY,
        request_id,
        operation,
        ErrorClass::Refused,
        "request.schema-invalid",
        "Request does not match the closed operation schema.",
        Some(address),
        false,
    )
}

fn operation_ledger_capacity(request_id: &str) -> Response {
    failure(
        StatusCode::INSUFFICIENT_STORAGE,
        request_id,
        None,
        ErrorClass::Exhausted,
        "operation.ledger-capacity",
        "Operation ledger capacity is exhausted.",
        Some("operation-ledger"),
        false,
    )
}

fn outcome_unknown(request_id: &str, operation: &str) -> Response {
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        request_id,
        Some(operation),
        ErrorClass::Failed,
        "operation.outcome-unknown",
        "Operation was accepted but its terminal outcome is unknown.",
        Some("operation"),
        true,
    )
}

fn conflict(
    request_id: &str,
    operation: &str,
    code: &str,
    message: &str,
    address: &str,
) -> Response {
    failure(
        StatusCode::CONFLICT,
        request_id,
        Some(operation),
        ErrorClass::Conflict,
        code,
        message,
        Some(address),
        false,
    )
}

fn workspace_frozen_refusal(request_id: &str, operation: &str) -> Response {
    conflict(
        request_id,
        operation,
        "workspace.not-ready",
        "Workspace is not ready for this operation.",
        "workspace",
    )
}

fn workspace_missing_refusal(request_id: &str, operation: &str) -> Response {
    not_found_with_operation(request_id, operation)
}

fn not_found(request_id: &str) -> Response {
    not_found_at(request_id, "resource")
}

fn not_found_at(request_id: &str, address: &str) -> Response {
    failure(
        StatusCode::NOT_FOUND,
        request_id,
        None,
        ErrorClass::Refused,
        "resource.not-found",
        "Resource was not found.",
        Some(address),
        false,
    )
}

fn not_found_with_operation(request_id: &str, operation: &str) -> Response {
    failure(
        StatusCode::NOT_FOUND,
        request_id,
        Some(operation),
        ErrorClass::Refused,
        "resource.not-found",
        "Resource was not found.",
        Some("resource"),
        false,
    )
}

fn store_failure(
    request_id: &str,
    operation: Option<&str>,
    error: &substrate_store::StoreError,
) -> Response {
    tracing::error!(%error, "state store failure");
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        request_id,
        operation,
        ErrorClass::Failed,
        "state.store-failed",
        "Durable state operation failed.",
        Some("state"),
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn failure(
    status: StatusCode,
    request_id: &str,
    operation: Option<&str>,
    class: ErrorClass,
    code: &str,
    message: &str,
    address: Option<&str>,
    retriable: bool,
) -> Response {
    failure_detail(
        status,
        request_id,
        ErrorDetail {
            class,
            code: code.to_owned(),
            message: message.to_owned(),
            retriable,
            address: address.map(ToOwned::to_owned),
            operation: operation.map(ToOwned::to_owned),
        },
    )
}

fn failure_detail(status: StatusCode, request_id: &str, detail: ErrorDetail) -> Response {
    let mut response = (status, Json(Failure::new(request_id, detail.clone()))).into_response();
    response.extensions_mut().insert(detail);
    response
}

fn success<T: Serialize>(status: StatusCode, body: Success<T>) -> Response {
    (status, Json(body)).into_response()
}
