use std::collections::HashSet;
use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use ed25519_dalek::{Signature, VerifyingKey};
use futures_util::StreamExt as _;
use parking_lot::Mutex as ParkingMutex;
use sha2::{Digest as _, Sha256};
use substrate_host::{DispatchOutcome, ExecObservation, PipeStream};
use substrate_store::{
    ExecWrite, NewSessionAuthority, Scope, SessionAttachmentClaim, SessionAuthorityLookup,
    SessionAuthorityMint, SessionRetireReservation, StoredExec, WorkspaceAdmission,
};
use substrate_wire::{
    Base64Content, Base64Encoding, EmptyInput, ErrorClass, Exec, ExecKind, ExecSignalInput,
    ExecState, LeaseRenewInput, MAX_LEASE_TTL_MS, MAX_PTY_WINDOW_COLUMNS, MAX_PTY_WINDOW_ROWS,
    MIN_LEASE_TTL_MS, OutputStream, PipeClientFrame, PipeServerFrame, PipeSession,
    PipeSessionCapabilities, PipeSessionLimits, PipeSessionStartInput, SessionAttachmentAuthority,
    SessionAttachmentState, SessionAuthorityMintInput, SessionKind, SessionMode,
    SessionProtocolErrorCode, SessionState, Success, session_authority_transcript,
};
use tokio::sync::Semaphore;

use crate::runtime::TransportPermit;

use super::events::{ControlRate, enforce_stream_send_deadline};
use super::operations::{
    begin, decode_mutation, finish_driver_error, finish_lease_store_error,
    finish_pipe_session_dispatch_absence, finish_pipe_session_dispatch_unknown,
    finish_pipe_session_observation, finish_pipe_session_start, new_lease, new_operation,
    observation_from_stored, pipe_confinement_available, read_bounded_body,
    refuse_before_dispatch_response, replay, reservation_response, stored_exec,
    validate_pipe_session_input,
};
use super::responses::{
    failure, not_found, not_found_with_operation, operation_ledger_capacity, outcome_unknown,
    query_is_empty, request_id, schema_invalid, store_failure, success, workspace_frozen_refusal,
};
use super::{
    App, Identity, MAINTENANCE_DRIVER_TIMEOUT, PIPE_MAX_FRAME_BYTES, PIPE_MAX_INPUT_BYTES,
    PIPE_MAX_QUEUED_FRAMES, SessionTransport,
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
    /// Held for the attachment's lifetime and never read. Dropping it is what returns the slot to
    /// the fixed global bound, and every path out of the attach handler must reach that drop:
    /// `session.attachment-capacity` is published as *exhausted* and retriable, so capacity a
    /// retry cannot recover is not a state this permit is allowed to produce (invariant 3).
    _global: tokio::sync::OwnedSemaphorePermit,
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
            _global: global,
        })
    }
}

/// Every permit returns its slot and drops its key, on every path.
///
/// There used to be a second shape here: a permit whose cancellation could not be proven kept a
/// process-local tombstone — the key stayed in `attached` and `Drop` answered that by *forgetting*
/// the global permit, spending one of the fixed slots until restart. It bought nothing. The key is
/// only ever tested by `acquire`, which the attach handler reaches only for a session whose
/// durable attachment is `Available`; a claimed session is `Attached`, `Consumed` or `Uncertain`
/// from then on, `substrate_store` answers `AlreadyClaimed` for all three, and only starting a
/// session sets `Available` again. So the tombstone barred a session the durable claim had already
/// barred, while the forgotten permit was real, permanent loss of capacity that the published
/// `session.attachment-capacity` refusal calls exhausted and retriable.
impl Drop for PipeAttachmentPermit {
    fn drop(&mut self) {
        self.limits
            .attached
            .lock()
            .remove(&(self.scope.clone(), self.exec_id.clone()));
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
            substrate_wire::SESSION_CONFINEMENT_UNAVAILABLE,
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
                // Bound, not written out, and `0.10.0` rather than `0.4.0`: this document carries
                // `modes` and the window and control-rate ceilings, which `0.4.0`'s closed
                // nine-property schema forbids, so naming `0.4.0` made the body validate against no
                // released bundle at all. See `PIPE_SESSION_CAPABILITY_CONTRACT` for why this is not
                // the `x-b10x-contract` header's claim.
                contract: substrate_wire::PIPE_SESSION_CAPABILITY_CONTRACT.to_owned(),
                transport: "unix-websocket-json".to_owned(),
                capability_snapshot: machine.snapshot,
                lease_required: true,
                single_attachment: true,
                network: substrate_wire::AppliedNetwork::None,
                max_input_bytes: PIPE_MAX_INPUT_BYTES,
                max_frame_bytes: PIPE_MAX_FRAME_BYTES,
                max_queued_frames: PIPE_MAX_QUEUED_FRAMES,
                // The per-mode gate lives here rather than in the operation registry: a
                // `capability_predicate` on `POST /v1/sessions` would take the whole route
                // away from a daemon that serves pipes perfectly well (design 13). Derived from the
                // fact, so a host that loses the ability stops advertising the mode.
                modes: served_session_modes(facts),
                max_window_columns: MAX_PTY_WINDOW_COLUMNS,
                max_window_rows: MAX_PTY_WINDOW_ROWS,
                max_controls_per_window: substrate_wire::MAX_SESSION_CONTROLS_PER_WINDOW,
                control_window_ms: substrate_wire::SESSION_CONTROL_WINDOW_MS,
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
        "/v1/sessions",
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
            "/v1/sessions",
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
                "/v1/sessions",
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
                "/v1/sessions",
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
                "/v1/sessions",
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
        "/v1/sessions",
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
            usage: mutation
                .input
                .exec
                .measurements
                .contains(&substrate_wire::ExecMeasurement::ResourceUsage)
                .then(|| substrate_wire::ExecUsage::Pending {
                    observed_at: app.authority.now(),
                }),
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
    let address = format!("/v1/sessions/{session_id}");
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
    let address = format!("/v1/sessions/{session_id}/signal");
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
    let address = format!("/v1/sessions/{session_id}/lease/renew");
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
        Err(response) => {
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

#[allow(clippy::too_many_lines)] // Mint keeps every secret-producing and durable bound adjacent.
pub(super) async fn pipe_session_authority_mint(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    Extension(transport): Extension<SessionTransport>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !matches!(transport, SessionTransport::HostedTls { .. }) {
        return not_found(&request_id);
    }
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let bytes = match read_bounded_body(body, &request_id).await {
        Ok(bytes) => bytes,
        Err(response) => return response,
    };
    let input: SessionAuthorityMintInput = match serde_json::from_slice(&bytes) {
        Ok(input) => input,
        Err(_) => return schema_invalid(&request_id, None, "input"),
    };
    let public_key = match decode_fixed_base64url::<32>(&input.public_key) {
        Some(key) if VerifyingKey::from_bytes(&key).is_ok() => key,
        _ => return schema_invalid(&request_id, None, "public_key"),
    };
    let now = app.authority.now();
    let Some(expires_at) = now.checked_add_signed(chrono::Duration::seconds(
        substrate_wire::SESSION_AUTHORITY_LIFETIME_SECONDS,
    )) else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            &request_id,
            None,
            ErrorClass::Failed,
            "state.clock-unavailable",
            "A bounded authority expiry could not be established.",
            Some("state"),
            true,
        );
    };
    let mut authority_bytes = [0_u8; 32];
    if getrandom::fill(&mut authority_bytes).is_err() {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            &request_id,
            None,
            ErrorClass::Failed,
            "state.entropy-unavailable",
            "Secure authority generation is unavailable.",
            Some("state"),
            true,
        );
    }
    let authority = format!("session_authority_v1_{}", BASE64URL.encode(authority_bytes));
    let authority_id = format!("sa_{}", ulid::Ulid::generate());
    let new_authority = NewSessionAuthority {
        authority_id: authority_id.clone(),
        bearer_sha256: Sha256::digest(authority.as_bytes()).into(),
        public_key,
        expires_at,
    };
    let scope = app.scope(&identity);
    match app
        .store_io(|| {
            app.store
                .mint_session_authority(&scope, &session_id, &new_authority, now)
        })
        .await
    {
        Ok(SessionAuthorityMint::Minted) => success(
            StatusCode::CREATED,
            Success::observed(
                request_id,
                SessionAttachmentAuthority {
                    authority_id,
                    authority,
                    expires_at,
                },
            ),
        ),
        Ok(SessionAuthorityMint::Capacity) => failure(
            StatusCode::TOO_MANY_REQUESTS,
            &request_id,
            None,
            ErrorClass::Exhausted,
            substrate_wire::SESSION_ATTACHMENT_CAPACITY,
            "The bounded session attachment capacity is exhausted.",
            Some("session-authority"),
            substrate_wire::session_refusal_is_retriable(
                substrate_wire::SESSION_ATTACHMENT_CAPACITY,
            ),
        ),
        Ok(SessionAuthorityMint::NotAttachable) => failure(
            StatusCode::CONFLICT,
            &request_id,
            None,
            ErrorClass::Conflict,
            substrate_wire::SESSION_NOT_ATTACHABLE,
            "The session is not ready to mint an attachment authority.",
            Some("session"),
            false,
        ),
        Ok(SessionAuthorityMint::Missing) => not_found(&request_id),
        Err(error) => store_failure(&request_id, None, &error),
    }
}

struct VerifiedSessionAuthority {
    authority_id: String,
    bearer_sha256: [u8; 32],
}

#[allow(clippy::too_many_lines)] // Every authority byte is parsed and verified at one boundary.
async fn verify_network_session_authority(
    app: &App,
    scope: &Scope,
    session_id: &str,
    headers: &HeaderMap,
    exporter: &[u8; 32],
    request_id: &str,
) -> Result<VerifiedSessionAuthority, Response> {
    let Some(authority_id) = exact_header(headers, substrate_wire::SESSION_AUTHORITY_ID_HEADER)
    else {
        return Err(authority_failure(
            request_id,
            StatusCode::UNAUTHORIZED,
            ErrorClass::Refused,
            substrate_wire::SESSION_AUTHORITY_ABSENT,
            false,
        ));
    };
    let Some(authority) = exact_header(headers, substrate_wire::SESSION_AUTHORITY_BEARER_HEADER)
    else {
        return Err(authority_failure(
            request_id,
            StatusCode::UNAUTHORIZED,
            ErrorClass::Refused,
            substrate_wire::SESSION_AUTHORITY_ABSENT,
            false,
        ));
    };
    let Some(timestamp) = exact_header(headers, substrate_wire::SESSION_AUTHORITY_TIMESTAMP_HEADER)
    else {
        return Err(authority_failure(
            request_id,
            StatusCode::UNAUTHORIZED,
            ErrorClass::Refused,
            substrate_wire::SESSION_AUTHORITY_ABSENT,
            false,
        ));
    };
    let Some(proof) = exact_header(headers, substrate_wire::SESSION_AUTHORITY_PROOF_HEADER) else {
        return Err(authority_failure(
            request_id,
            StatusCode::UNAUTHORIZED,
            ErrorClass::Refused,
            substrate_wire::SESSION_AUTHORITY_ABSENT,
            false,
        ));
    };
    if !valid_authority_id(authority_id)
        || !valid_authority_bearer(authority)
        || timestamp.is_empty()
        || timestamp.len() > 16
        || !timestamp.bytes().all(|byte| byte.is_ascii_digit())
        || proof.len() != 86
    {
        return Err(authority_unbound(request_id));
    }
    let Ok(timestamp_ms) = timestamp.parse::<i64>() else {
        return Err(authority_unbound(request_id));
    };
    let now = app.authority.now();
    if now.timestamp_millis().abs_diff(timestamp_ms)
        > u64::try_from(substrate_wire::SESSION_AUTHORITY_PROOF_SKEW_SECONDS)
            .unwrap_or(0)
            .saturating_mul(1_000)
    {
        return Err(authority_unbound(request_id));
    }
    let Some(signature) = decode_fixed_base64url::<64>(proof) else {
        return Err(authority_unbound(request_id));
    };
    let bearer_sha256: [u8; 32] = Sha256::digest(authority.as_bytes()).into();
    let public_key = match app
        .store_io(|| {
            app.store
                .session_authority(scope, session_id, authority_id, &bearer_sha256, now)
        })
        .await
    {
        Ok(SessionAuthorityLookup::Available { public_key }) => public_key,
        Ok(SessionAuthorityLookup::Expired) => {
            return Err(authority_failure(
                request_id,
                StatusCode::UNAUTHORIZED,
                ErrorClass::Refused,
                substrate_wire::SESSION_AUTHORITY_EXPIRED,
                false,
            ));
        }
        Ok(SessionAuthorityLookup::Redeemed) => {
            return Err(authority_failure(
                request_id,
                StatusCode::CONFLICT,
                ErrorClass::Conflict,
                substrate_wire::SESSION_AUTHORITY_REDEEMED,
                false,
            ));
        }
        Ok(SessionAuthorityLookup::Unbound | SessionAuthorityLookup::Missing) => {
            return Err(authority_unbound(request_id));
        }
        Err(error) => return Err(store_failure(request_id, None, &error)),
    };
    if !valid_channel_proof(
        &public_key,
        &signature,
        authority_id,
        exporter,
        timestamp_ms,
    ) {
        return Err(authority_unbound(request_id));
    }
    Ok(VerifiedSessionAuthority {
        authority_id: authority_id.to_owned(),
        bearer_sha256,
    })
}

fn valid_channel_proof(
    public_key: &[u8; 32],
    signature: &[u8; 64],
    authority_id: &str,
    exporter: &[u8; 32],
    timestamp_ms: i64,
) -> bool {
    VerifyingKey::from_bytes(public_key).is_ok_and(|verifying_key| {
        verifying_key
            .verify_strict(
                &session_authority_transcript(authority_id, exporter, timestamp_ms),
                &Signature::from_bytes(signature),
            )
            .is_ok()
    })
}

fn exact_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        None
    } else {
        Some(value)
    }
}

fn decode_fixed_base64url<const N: usize>(value: &str) -> Option<[u8; N]> {
    let bytes = BASE64URL.decode(value).ok()?;
    if BASE64URL.encode(&bytes) != value {
        return None;
    }
    bytes.try_into().ok()
}

fn valid_authority_bearer(value: &str) -> bool {
    value
        .strip_prefix("session_authority_v1_")
        .and_then(decode_fixed_base64url::<32>)
        .is_some()
}

fn valid_authority_id(value: &str) -> bool {
    value.strip_prefix("sa_").is_some_and(|ulid| {
        ulid.len() == 26
            && ulid.bytes().all(|byte| {
                byte.is_ascii_digit()
                    || matches!(byte, b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z')
            })
    })
}

fn authority_unbound(request_id: &str) -> Response {
    authority_failure(
        request_id,
        StatusCode::UNAUTHORIZED,
        ErrorClass::Refused,
        substrate_wire::SESSION_AUTHORITY_UNBOUND,
        false,
    )
}

fn authority_failure(
    request_id: &str,
    status: StatusCode,
    class: ErrorClass,
    code: &str,
    retriable: bool,
) -> Response {
    failure(
        status,
        request_id,
        None,
        class,
        code,
        "The network session attachment authority was not admitted.",
        Some("session-authority"),
        retriable,
    )
}

// An axum handler's parameters are its extractors, and this route reads six things off the request
// before it answers: the app, the caller, the listener's transport admission, the headers, the
// session it names and its query. Bundling any of them would hide what the route depends on.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)] // Attachment preflight keeps scope, lease, and capacity adjacent.
pub(super) async fn pipe_session_attach(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    Extension(transport): Extension<SessionTransport>,
    transport_permit: Option<Extension<TransportPermit>>,
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
    let network_authority = match transport {
        SessionTransport::Unix => None,
        SessionTransport::HostedTls { exporter } => {
            match verify_network_session_authority(
                &app,
                &scope,
                &session_id,
                &headers,
                &exporter,
                &request_id,
            )
            .await
            {
                Ok(authority) => Some(authority),
                Err(response) => return response,
            }
        }
        SessionTransport::DevelopmentTcp => return not_found(&request_id),
    };
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
            substrate_wire::SESSION_NOT_ATTACHABLE,
            channel_message(
                session.mode,
                "The raw-pipe session is not running under an active lease.",
                "The pty session is not running under an active lease.",
            ),
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
                substrate_wire::SESSION_ALREADY_ATTACHED,
                channel_message(
                    session.mode,
                    "The raw-pipe session already has its single permitted attachment.",
                    "The pty session already has its single permitted attachment.",
                ),
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
                substrate_wire::SESSION_ATTACHMENT_CAPACITY,
                channel_message(
                    session.mode,
                    "The bounded raw-pipe attachment capacity is exhausted.",
                    "The bounded session attachment capacity is exhausted.",
                ),
                Some("session"),
                substrate_wire::session_refusal_is_retriable(
                    substrate_wire::SESSION_ATTACHMENT_CAPACITY,
                ),
            );
        }
    };
    let observed_at = app.authority.now();
    match app
        .store_io(|| match network_authority.as_ref() {
            Some(authority) => app.store.claim_pipe_session_attachment_with_authority(
                &scope,
                &session_id,
                &authority.authority_id,
                &authority.bearer_sha256,
                observed_at,
            ),
            None => app
                .store
                .claim_pipe_session_attachment(&scope, &session_id, observed_at),
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
                substrate_wire::SESSION_ALREADY_ATTACHED,
                channel_message(
                    session.mode,
                    "The raw-pipe session attachment right has already been consumed.",
                    "The pty session attachment right has already been consumed.",
                ),
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
                substrate_wire::SESSION_NOT_ATTACHABLE,
                channel_message(
                    session.mode,
                    "The raw-pipe session is not attachable.",
                    "The pty session is not attachable.",
                ),
                Some("session"),
                false,
            );
        }
        Ok(SessionAttachmentClaim::AuthorityExpired) => {
            return authority_failure(
                &request_id,
                StatusCode::UNAUTHORIZED,
                ErrorClass::Refused,
                substrate_wire::SESSION_AUTHORITY_EXPIRED,
                false,
            );
        }
        Ok(SessionAttachmentClaim::AuthorityRedeemed) => {
            return authority_failure(
                &request_id,
                StatusCode::CONFLICT,
                ErrorClass::Conflict,
                substrate_wire::SESSION_AUTHORITY_REDEEMED,
                false,
            );
        }
        Ok(SessionAttachmentClaim::AuthorityUnbound) => return authority_unbound(&request_id),
        Ok(SessionAttachmentClaim::Missing) => return not_found(&request_id),
        Err(error) => return store_failure(&request_id, None, &error),
    }
    let exec_id = session.exec;
    let mode = session.mode;
    let policy = app.pipe_session_policy;
    // The transport admission this connection was accepted under, moved into the upgraded task
    // below. hyper resolves an upgradeable connection future when it hands the socket over, so an
    // admission left with the connection stops counting an attachment that is still serving.
    // Absent when no listener published one — the crate's own tests drive this route without a
    // transport.
    let transport_admission = transport_permit.map(|Extension(permit)| permit);

    // The claim above is already consumed and the session is no longer attachable, so an upgrade
    // that never completes would leave the process running unattached until its lease or timeout
    // ended it. Both hand-offs out of this handler therefore end the session; exactly one runs.
    let stranded_app = Arc::clone(&app);
    let stranded_scope = scope.clone();
    let stranded_exec = exec_id.clone();
    ws.read_buffer_size(policy.max_message_bytes)
        .write_buffer_size(policy.write_buffer_bytes)
        .max_frame_size(policy.max_message_bytes)
        .max_message_size(policy.max_message_bytes)
        .max_write_buffer_size(
            policy
                .max_message_bytes
                .saturating_add(policy.write_buffer_bytes),
        )
        .on_failed_upgrade(move |error: axum::Error| {
            // The permit is not carried in here. It is dropped with the callback that owns it,
            // which returns its slot to the global bound and clears its key — and nothing here
            // needs it, because this session's durable attachment claim is already spent and the
            // state gate above refuses every further attach for it before `acquire` is reached.
            tokio::spawn(async move {
                if terminate_pipe_session(&stranded_app, &stranded_scope, &stranded_exec).await {
                    tracing::info!(
                        exec = %stranded_exec,
                        %error,
                        "terminated a claimed session whose attachment never upgraded"
                    );
                } else {
                    // Invariant 3: an unproven containment is said out loud, never assumed. The
                    // process outlives this attempt and nothing here retries it; only lease
                    // expiry ends it (`app/service.rs`, `cleanup_expired`).
                    tracing::warn!(
                        exec = %stranded_exec,
                        %error,
                        "could not terminate a claimed session whose attachment never upgraded"
                    );
                }
            });
        })
        .on_upgrade(move |socket| async move {
            // Held for as long as this attachment serves, so the transport budget counts it.
            let _transport_admission = transport_admission;
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
            if !completed {
                if terminate_pipe_session(&app, &scope, &exec_id).await {
                    tracing::info!(
                        exec = %exec_id,
                        "terminated a session whose attachment ended without a terminal observation"
                    );
                } else {
                    // Invariant 3: an unproven containment is said out loud, never assumed. The
                    // process outlives this attempt and nothing here retries it; only lease
                    // expiry ends it (`app/service.rs`, `cleanup_expired`).
                    tracing::warn!(
                        exec = %exec_id,
                        "could not terminate a session whose attachment ended without a terminal \
                         observation"
                    );
                }
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
                                SessionProtocolErrorCode::FrameInvalid,
                                channel_message(
                                    mode,
                                    "The client frame is outside the closed raw-pipe vocabulary.",
                                    "The client frame is outside the closed pty vocabulary.",
                                ),
                                policy,
                            ).await;
                            return false;
                        };
                        value
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                        // The same budget, so the same answer. The 1008 close was kept here on the
                        // ground that a control frame "has no frame to answer in", and the `Binary`
                        // arm below refutes it: a binary message is also outside the published
                        // client `oneOf` and is answered with a `protocol-error`. What the server
                        // sends is the *server's* vocabulary; it never required the client's message
                        // to have been a member of the client's. And ping is the half a terminal
                        // client actually crosses — `max_controls_per_window` is published for
                        // choosing a keepalive, and a keepalive slightly too fast spends it.
                        if control_rate.exceeded(
                            policy.max_controls_per_window,
                            policy.control_window,
                        ) {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                SessionProtocolErrorCode::ControlRateExceeded,
                                &format!(
                                    "A session attachment sends at most {} control frames per {} ms, \
                                     and ping shares the budget.",
                                    substrate_wire::MAX_SESSION_CONTROLS_PER_WINDOW,
                                    substrate_wire::SESSION_CONTROL_WINDOW_MS,
                                ),
                                policy,
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
                            SessionProtocolErrorCode::FrameInvalid,
                            channel_message(
                                mode,
                                "Raw-pipe client frames use the closed JSON text encoding.",
                                "Pty client frames use the closed JSON text encoding.",
                            ),
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
                        SessionProtocolErrorCode::SequenceInvalid,
                        channel_message(
                            mode,
                            "Raw-pipe client sequences must be contiguous and start at one.",
                            "Pty client sequences must be contiguous and start at one.",
                        ),
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
                                SessionProtocolErrorCode::InputClosed,
                                "Raw-pipe stdin is already closed.",
                                policy,
                            ).await;
                            return false;
                        }
                        let Ok(bytes) = content.decode() else {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                SessionProtocolErrorCode::Base64Invalid,
                                channel_message(
                                    mode,
                                    "Raw-pipe stdin content is not valid standard base64.",
                                    "Pty input content is not valid standard base64.",
                                ),
                                policy,
                            ).await;
                            return false;
                        };
                        if let Err(error) = app.driver.write_pipe_session(&exec_id, &bytes).await {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                SessionProtocolErrorCode::classify(error.code),
                                &format!(
                                    "Substrate refused or failed the {} input frame ({}).",
                                    channel_message(mode, "raw-pipe", "pty"),
                                    error.code
                                ),
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
                        // cannot become a free ioctl loop (design 13). Answered as a
                        // `protocol-error` in a published code and not as a bare WebSocket close:
                        // the close named a bound no document published, in a word no document
                        // named. Round 5 took the same answer to the ping arm above, so this
                        // budget now has exactly one answer and the 1008 close is gone from both.
                        if control_rate.exceeded(
                            policy.max_controls_per_window,
                            policy.control_window,
                        ) {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                SessionProtocolErrorCode::ControlRateExceeded,
                                &format!(
                                    "A session attachment sends at most {} control frames per {} ms, \
                                     and ping shares the budget.",
                                    substrate_wire::MAX_SESSION_CONTROLS_PER_WINDOW,
                                    substrate_wire::SESSION_CONTROL_WINDOW_MS,
                                ),
                                policy,
                            ).await;
                            return false;
                        }
                        // Two conditions, two answers. Collapsed into one they told a raw-pipe
                        // client its 80x24 window was out of range, when the window was fine and
                        // the channel was wrong — the one thing it cannot act on.
                        if mode != SessionMode::Pty {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                SessionProtocolErrorCode::NotPty,
                                "A resize frame belongs to a pty session; this attachment serves raw pipes.",
                                policy,
                            ).await;
                            return false;
                        }
                        if !window.within_bounds() {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                SessionProtocolErrorCode::ResizeInvalid,
                                "A resize names 1 to 1000 cells on each axis of a pty session.",
                                policy,
                            ).await;
                            return false;
                        }
                        if let Err(error) = app.driver.resize_pty_session(&exec_id, window).await {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                SessionProtocolErrorCode::classify(error.code),
                                &format!(
                                    "Substrate refused or failed the terminal resize ({}).",
                                    error.code
                                ),
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
                                SessionProtocolErrorCode::InputCloseUnserved,
                                "A pty session has no half-close; send the terminal's own end-of-file character as input.",
                                policy,
                            ).await;
                            return false;
                        }
                        if input_closed || app.driver.close_pipe_session_input(&exec_id).await.is_err() {
                            let _sent = send_pipe_protocol_error(
                                &mut socket,
                                &mut server_sequence,
                                SessionProtocolErrorCode::InputClosed,
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
                                SessionProtocolErrorCode::SignalInvalid,
                                channel_message(
                                    mode,
                                    "Raw-pipe signal grace exceeds the closed bound.",
                                    "Pty signal grace exceeds the closed bound.",
                                ),
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
                                    SessionProtocolErrorCode::classify(error.code),
                                    &format!(
                                        "Substrate could not terminally observe the signalled {} process ({}).",
                                        channel_message(mode, "raw-pipe", "pty"),
                                        error.code
                                    ),
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
                        // One file on a terminal, so one attribution — the same gate the
                        // `truncated` frame beside this one already has. `HostDriver` never
                        // attributes a pty frame to stderr, and a driver that did would make the
                        // daemon send what `x-b10x-one-file` and the published
                        // `"stream": {"const": "stdout"}` both forbid.
                        let stream = match (mode, frame.stream) {
                            (SessionMode::Pty, _) | (_, PipeStream::Stdout) => OutputStream::Stdout,
                            (_, PipeStream::Stderr) => OutputStream::Stderr,
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
                    // Bound, not written out: renaming the constant used to compile here and
                    // silently turn every 250 ms poll timeout into a `protocol-error` that ends the
                    // attachment.
                    Err(error) if error.code == substrate_wire::SESSION_READ_TIMEOUT => {}
                    Err(error) => {
                        let _sent = send_pipe_protocol_error(
                            &mut socket,
                            &mut server_sequence,
                            SessionProtocolErrorCode::classify(error.code),
                            &format!(
                                "Substrate could not continue {} output observation ({}).",
                                channel_message(mode, "raw-pipe", "pty"),
                                error.code
                            ),
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

/// The sentence for the channel this attachment actually serves.
///
/// `SESSION_FRAME_INVALID`'s own definition is "outside the closed vocabulary of **the mode this
/// attachment serves**", and eight messages said "raw-pipe" to whoever was listening. Same class as
/// the raw-pipe truncation sentence an earlier pass took out of a terminal transcript: a message is
/// the half of a refusal a human reads, and telling a terminal client about raw pipes is telling it
/// about a channel it is not on.
const fn channel_message(
    mode: SessionMode,
    pipes: &'static str,
    pty: &'static str,
) -> &'static str {
    match mode {
        SessionMode::Pipes => pipes,
        SessionMode::Pty => pty,
    }
}

/// One `protocol-error` frame, in a code the contract publishes.
///
/// The parameter is [`SessionProtocolErrorCode`] and not a `&str` **on purpose**: that is what
/// makes "every code a session attachment can send is one the bundle publishes" a property of the
/// type system rather than of a list somebody keeps up to date. Before it, four codes reached a
/// pty client that `x-b10x-codes` did not name, and a forwarded `DriverError::code` could put an
/// `exec.*` word into a frame whose published `code` is `^session\.[a-z0-9-]+$`.
async fn send_pipe_protocol_error(
    socket: &mut WebSocket,
    sequence: &mut u64,
    code: SessionProtocolErrorCode,
    message: &str,
    policy: PipeSessionPolicy,
) -> Result<(), ()> {
    let frame = PipeServerFrame::ProtocolError {
        sequence: *sequence,
        code,
        message: message.to_owned(),
    };
    *sequence = sequence.saturating_add(1);
    send_pipe_server_frame(socket, &frame, policy).await
}

/// The terminal frames one attachment is owed, in the vocabulary its own mode publishes.
///
/// A `pty` attachment gets no `truncated` frame: the published vocabulary has no branch for one
/// (`contracts/substrate-wire/0.10.0/schemas/pty-channel-frame.json`, `x-b10x-no-truncated`),
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
        && refusal.code == substrate_wire::SESSION_OUTPUT_BACKPRESSURE
    {
        send_pipe_protocol_error(
            socket,
            sequence,
            SessionProtocolErrorCode::classify(&refusal.code),
            &refusal.message,
            policy,
        )
        .await?;
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

#[cfg(test)]
mod authority_tests {
    use axum::http::{HeaderMap, HeaderValue};
    use ed25519_dalek::{Signer as _, SigningKey};

    use super::{exact_header, valid_authority_bearer, valid_authority_id, valid_channel_proof};

    #[test]
    fn an_attachment_proof_is_bound_to_key_authority_channel_and_timestamp() {
        let signing_key = SigningKey::from_bytes(&[3; 32]);
        let authority_id = "sa_01JSESSIONAUTHORITYTEST";
        let exporter = [4; 32];
        let timestamp = 1_788_246_000_000;
        let transcript =
            substrate_wire::session_authority_transcript(authority_id, &exporter, timestamp);
        let signature = signing_key.sign(&transcript).to_bytes();
        let public_key = signing_key.verifying_key().to_bytes();
        assert!(valid_channel_proof(
            &public_key,
            &signature,
            authority_id,
            &exporter,
            timestamp
        ));
        assert!(!valid_channel_proof(
            &public_key,
            &signature,
            authority_id,
            &[5; 32],
            timestamp
        ));
        assert!(!valid_channel_proof(
            &public_key,
            &signature,
            "sa_other",
            &exporter,
            timestamp
        ));
        assert!(!valid_channel_proof(
            &public_key,
            &signature,
            authority_id,
            &exporter,
            timestamp + 1
        ));
    }

    #[test]
    fn authority_bearers_and_headers_are_exact_and_bounded() {
        let bearer = format!(
            "session_authority_v1_{}",
            base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, [7; 32])
        );
        assert!(valid_authority_bearer(&bearer));
        assert!(!valid_authority_bearer(&(bearer.clone() + "=")));
        assert!(!valid_authority_bearer("session_authority_v1_short"));
        assert!(valid_authority_id("sa_01M1DY00000000000000000000"));
        assert!(!valid_authority_id("sa_01m1dy00000000000000000000"));
        assert!(!valid_authority_id("sa_01M1DY0000000000000000000I"));
        assert!(!valid_authority_id("sa_short"));

        let mut headers = HeaderMap::new();
        headers.insert("x-test", HeaderValue::from_static("one"));
        assert_eq!(exact_header(&headers, "x-test"), Some("one"));
        headers.append("x-test", HeaderValue::from_static("two"));
        assert_eq!(exact_header(&headers, "x-test"), None);
    }
}
