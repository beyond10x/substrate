use std::collections::HashSet;
use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use base64::Engine as _;
use futures_util::StreamExt as _;
use parking_lot::Mutex as ParkingMutex;
use substrate_host::{DispatchOutcome, ExecObservation, PipeStream};
use substrate_store::{
    ExecWrite, Scope, SessionAttachmentClaim, SessionRetireReservation, StoredExec,
    WorkspaceAdmission,
};
use substrate_wire::{
    Base64Content, Base64Encoding, EmptyInput, ErrorClass, Exec, ExecKind, ExecSignalInput,
    ExecState, LeaseRenewInput, MAX_LEASE_TTL_MS, MAX_PTY_WINDOW_COLUMNS, MAX_PTY_WINDOW_ROWS,
    MIN_LEASE_TTL_MS, OutputStream, PipeClientFrame, PipeServerFrame, PipeSession,
    PipeSessionCapabilities, PipeSessionLimits, PipeSessionStartInput, SessionAttachmentState,
    SessionKind, SessionMode, SessionState, Success,
};
use tokio::sync::Semaphore;

use super::events::{ControlRate, enforce_stream_send_deadline, send_protocol_close};
use super::operations::{
    begin, decode_mutation, finish_driver_error, finish_lease_store_error,
    finish_pipe_session_dispatch_absence, finish_pipe_session_dispatch_unknown,
    finish_pipe_session_observation, finish_pipe_session_start, new_lease, new_operation,
    observation_from_stored, pipe_confinement_available, refuse_before_dispatch_response, replay,
    reservation_response, stored_exec, validate_pipe_session_input,
};
use super::responses::{
    failure, not_found, not_found_with_operation, operation_ledger_capacity, outcome_unknown,
    query_is_empty, request_id, schema_invalid, store_failure, success, workspace_frozen_refusal,
};
use super::{
    App, Identity, MAINTENANCE_DRIVER_TIMEOUT, PIPE_MAX_FRAME_BYTES, PIPE_MAX_INPUT_BYTES,
    PIPE_MAX_QUEUED_FRAMES,
};

#[derive(Clone, Copy)]
pub(super) struct PipeSessionPolicy {
    pub(super) global_attachments: usize,
    max_message_bytes: usize,
    write_buffer_bytes: usize,
    send_timeout: std::time::Duration,
    read_poll: std::time::Duration,
    lifetime: std::time::Duration,
    max_controls_per_window: u32,
    control_window: std::time::Duration,
}

impl PipeSessionPolicy {
    pub(super) const fn production() -> Self {
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

pub(super) struct PipeAttachmentLimits {
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
    pub(super) fn new(global_attachments: usize) -> Arc<Self> {
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

/// The modes this daemon serves, derived from the capability facts and nothing else.
///
/// `pty` appears only where a probe proved a controlling terminal end to end (invariant 4). Sorted
/// so the document is stable, and `pipes` is always there because the confinement gate above has
/// already refused a daemon that cannot serve it.
fn served_session_modes(facts: &substrate_wire::CapabilityFacts) -> Vec<SessionMode> {
    let mut modes = vec![SessionMode::Pipes];
    if facts.sessions_pty == Some(true) {
        modes.push(SessionMode::Pty);
    }
    modes
}

pub(super) async fn pipe_session_capabilities(
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
                contract: "substrate-wire/0.4.0".to_owned(),
                transport: "unix-websocket-json".to_owned(),
                capability_snapshot: machine.snapshot,
                lease_required: true,
                single_attachment: true,
                network: substrate_wire::AppliedNetwork::None,
                max_input_bytes: PIPE_MAX_INPUT_BYTES,
                max_frame_bytes: PIPE_MAX_FRAME_BYTES,
                max_queued_frames: PIPE_MAX_QUEUED_FRAMES,
                // The per-mode gate lives here rather than in the operation registry: a
                // `capability_predicate` on `POST /v1/pipe-sessions` would take the whole route
                // away from a daemon that serves pipes perfectly well (design 13). Derived from the
                // fact, so a host that loses the ability stops advertising the mode.
                modes: served_session_modes(facts),
                max_window_columns: MAX_PTY_WINDOW_COLUMNS,
                max_window_rows: MAX_PTY_WINDOW_ROWS,
            },
        ),
    )
}

#[allow(clippy::too_many_lines)] // Durable reservation and fail-closed pipe dispatch stay adjacent.
pub(super) async fn pipe_session_start(
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
            refusal: None,
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
        mode: mutation.input.mode,
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

pub(super) async fn pipe_session_get(
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

pub(super) async fn pipe_session_retire(
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
pub(super) async fn pipe_session_signal(
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
pub(super) async fn pipe_session_lease_renew(
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
pub(super) async fn pipe_session_attach(
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
    let mode = session.mode;
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
                    mode,
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
    mode: SessionMode,
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
                    // Every arm below that answers with a `protocol-error` frame then returns,
                    // ending the attachment. That is not a local choice: ADR 0008 states
                    // "Upgrade failure, disconnect, **protocol failure**, send timeout, or lifetime
                    // expiry triggers whole-tree cancellation and terminal persistence"
                    // (`adr/0008-pipe-sessions-have-distinct-durable-identity.md:36-37`), and
                    // design 05 § 2 says attachment loss "follows typed cancellation or
                    // reconciliation behavior rather than unbounded buffering". A protocol error is
                    // terminal, so a client does not receive an `exit` frame after one; the durable
                    // observation is where it reads the outcome.
                    PipeClientFrame::Resize { window, .. } => {
                        // Rated on the control window that already exists, so a resize storm
                        // cannot become a free ioctl loop (design 13).
                        if control_rate.exceeded(
                            policy.max_controls_per_window,
                            policy.control_window,
                        ) {
                            let _sent = send_protocol_close(
                                &mut socket,
                                1008,
                                "session control-frame rate exceeded",
                                policy.send_timeout,
                            ).await;
                            return false;
                        }
                        if mode != SessionMode::Pty || !window.within_bounds() {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                substrate_wire::SESSION_RESIZE_INVALID,
                                "A resize names 1 to 1000 cells on each axis of a pty session.",
                                policy,
                            ).await;
                            return false;
                        }
                        if let Err(error) = app.driver.resize_pty_session(&exec_id, window).await {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                error.code,
                                "Substrate refused or failed the terminal resize.",
                                policy,
                            ).await;
                            return false;
                        }
                    }
                    PipeClientFrame::CloseInput { .. } => {
                        // A pty has no half-close: a client ends input by sending the terminal's
                        // own end-of-file character as ordinary input bytes (design 13).
                        if mode == SessionMode::Pty {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                "session.frame-invalid",
                                "A pty session has no half-close; send the terminal's own end-of-file character as input.",
                                policy,
                            ).await;
                            return false;
                        }
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
                            mode,
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
                            mode,
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
        | PipeClientFrame::Signal { sequence, .. }
        | PipeClientFrame::Resize { sequence, .. } => *sequence,
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

/// The terminal frames one attachment is owed, in the vocabulary its own mode publishes.
///
/// A `pty` attachment gets no `truncated` frame: the published vocabulary has no branch for one
/// (`contracts/substrate-wire/0.9.0/schemas/pty-channel-frame.json`, `x-b10x-no-truncated`),
/// reaching the output bound *ends* a terminal session and names itself on the exec observation's
/// refusal field instead, and a terminal stream has no per-stream offset for a client to rejoin at
/// — which is why design 13 removed the statement rather than relocating it. The observation still
/// carries `stdout_truncated`, because the bound really was crossed; what changes is that this
/// attachment is not told in a word it cannot parse.
async fn send_pipe_terminal(
    socket: &mut WebSocket,
    sequence: &mut u64,
    mode: SessionMode,
    observation: &ExecObservation,
    policy: PipeSessionPolicy,
) -> Result<(), ()> {
    if !is_pipe_terminal(observation.resource.state) {
        return Err(());
    }
    if let Some(refusal) = &observation.resource.refusal
        && refusal.code == "session.output-backpressure"
    {
        send_pipe_protocol_error(socket, sequence, &refusal.code, &refusal.message, policy).await?;
    }
    let truncation_is_deliverable = mode == SessionMode::Pipes;
    if truncation_is_deliverable && observation.stdout_truncated {
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
    if truncation_is_deliverable && observation.stderr_truncated {
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
