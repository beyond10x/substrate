use std::collections::BTreeSet;

use axum::body::{Body, to_bytes};
use axum::http::StatusCode;
use axum::response::Response;
use base64::Engine as _;
use chrono::Utc;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use substrate_host::{DriverError, DriverErrorClass, ExecObservation};
use substrate_store::{
    ExecWrite, LeaseClock, NewLease, NewOperation, Reservation, Scope, StoreError, StoredAnswer,
    StoredExec,
};
use substrate_wire::{
    Base64Content, Base64Encoding, ErrorClass, ErrorDetail, ExecOutputQuery, ExecStartInput,
    MAX_LEASE_TTL_MS, MIN_LEASE_TTL_MS, OperationOutcome, OutputSlice, PipeSession,
    PipeSessionStartInput, SessionAttachmentState, SessionState, Success, WorkspaceCreateInput,
    WorkspaceSource, canonical_request_hash_v2, validate_operation_id,
};

use crate::delegation::{ContextRefusal, VerifiedContext};

use super::responses::{
    conflict, delegated_context_refusal, failure, failure_detail, operation_ledger_capacity,
    outcome_unknown, query_is_empty, schema_invalid, store_failure, success,
};
use super::{
    App, BODY_LIMIT, Identity, PIPE_MAX_FRAME_BYTES, PIPE_MAX_INPUT_BYTES, PIPE_MAX_QUEUED_FRAMES,
    REQUEST_BODY_READ_TIMEOUT,
};

#[derive(Debug)]
pub(super) struct BoundMutation<T> {
    pub(super) op: String,
    pub(super) input: T,
    pub(super) request_hash: String,
    /// What a verified delegated context contributed, or `None` when none was presented.
    ///
    /// Carried on the bound mutation rather than re-derived per route, so every ledger row this
    /// request writes — accepted, refused before dispatch, or terminal — records the same
    /// attribution (ADR 0011).
    pub(super) attribution: Option<VerifiedContext>,
}

pub(super) async fn read_bounded_body(
    body: Body,
    request_id: &str,
) -> Result<axum::body::Bytes, Response> {
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
pub(super) async fn decode_mutation<T: DeserializeOwned>(
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
    let presented = presented_context(object, request_id, operation)?;
    let request_hash = canonical_request_hash_v2(method, address, raw_input, raw_query)
        .map_err(|_| schema_invalid(request_id, Some(operation), "input"))?;
    let operation = operation.to_owned();
    let scope = app.scope(identity);
    // Pure computation, before any store read and before any driver authority exists (design 09
    // § 3). The refusal is held rather than returned, because it is owed a durable ledger row under
    // this operation id and `new` does not exist yet.
    let (attribution, refusal) = match app.delegated_context.verify(
        presented.as_deref(),
        &identity.subject,
        &app.deployment,
        app.authority.now(),
    ) {
        Ok(verified) => (verified, None),
        Err(refusal) => (None, Some(refusal)),
    };
    let (invalid_query, invalid_envelope) = invalid_envelope(object, raw_query);
    match app
        .store_io(|| {
            app.store
                .inspect_reservation(&scope, &operation, &request_hash)
        })
        .await
    {
        Ok(None) => {}
        Ok(Some(reservation)) => {
            // Replay is still a request in the current trust posture. An existing ledger row is
            // immutable, so these refusals are returned without trying to replace its outcome.
            if invalid_query {
                return Err(schema_invalid(request_id, Some(&operation), "query"));
            }
            if invalid_envelope {
                return Err(schema_invalid(request_id, Some(&operation), "input"));
            }
            if let Some(refusal) = refusal {
                return Err(delegated_context_refusal(
                    request_id,
                    Some(&operation),
                    refusal,
                ));
            }
            if let Some(response) =
                grant_conflict(app, &scope, &operation, request_id, attribution.as_ref()).await
            {
                return Err(response);
            }
            return Err(
                reservation_response(Ok(reservation), request_id, &operation)
                    .unwrap_or_else(|| outcome_unknown(request_id, &operation)),
            );
        }
        Err(error) => return Err(store_failure(request_id, Some(&operation), &error)),
    }
    let new = bound_new_operation(
        app,
        identity,
        scope,
        &operation,
        operation_kind,
        &request_hash,
        attribution.as_ref(),
    );
    if invalid_query {
        let response = schema_invalid(request_id, Some(&operation), "query");
        return Err(record_bound_refusal(app, request_id, &new, response).await);
    }
    // Still closed: `op`, `input`, and the one optional sibling. Anything else is the same
    // strict-request refusal `0.6.0` gave, at the same address.
    if invalid_envelope {
        let response = schema_invalid(request_id, Some(&operation), "input");
        return Err(record_bound_refusal(app, request_id, &new, response).await);
    }
    if let Some(refusal) = refusal {
        let response = delegated_context_refusal(request_id, Some(&operation), refusal);
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
        attribution,
    })
}

fn invalid_envelope(
    object: &serde_json::Map<String, Value>,
    raw_query: Option<&str>,
) -> (bool, bool) {
    let invalid_member = object
        .keys()
        .any(|member| !matches!(member.as_str(), "op" | "input" | "delegated_context"));
    (!query_is_empty(raw_query), invalid_member)
}

fn bound_new_operation(
    app: &App,
    identity: &Identity,
    scope: Scope,
    operation: &str,
    operation_kind: &str,
    request_hash: &str,
    attribution: Option<&VerifiedContext>,
) -> NewOperation {
    NewOperation {
        scope,
        operation: operation.to_owned(),
        operation_kind: operation_kind.to_owned(),
        request_hash: request_hash.to_owned(),
        accepted_at: app.authority.now().to_rfc3339(),
        capability_snapshot: None,
        actor: identity.actor.clone(),
        principal: identity.principal.clone(),
        grant_ref: attribution.map(|value| value.grant_ref.clone()),
        platform_principal: attribution.map(|value| value.platform_principal.clone()),
        resource: None,
    }
}

/// The one new envelope member (ADR 0011), read as a raw string before anything interprets it.
///
/// A structure shaped like something else is refused here rather than coerced: the member is a
/// compact JWS or it is not present, and there is no third reading.
fn presented_context(
    object: &serde_json::Map<String, Value>,
    request_id: &str,
    operation: &str,
) -> Result<Option<String>, Response> {
    match object.get("delegated_context") {
        None => Ok(None),
        Some(Value::String(token)) => Ok(Some(token.clone())),
        Some(_) => Err(schema_invalid(
            request_id,
            Some(operation),
            "delegated_context",
        )),
    }
}

/// The one conflict a verified context can raise, on an operation id that already exists.
///
/// `delegated_context` is outside the canonical request hash, so replaying an `op` with a *fresh*
/// context is the same operation and returns the original outcome. Replaying it under a *different*
/// grant is not: first write wins on the recorded one (design 09 § 4). The extra ledger read costs
/// nothing on the common path — it runs only when a reservation already exists *and* this request
/// carried a verified grant.
async fn grant_conflict(
    app: &App,
    scope: &Scope,
    operation: &str,
    request_id: &str,
    attribution: Option<&VerifiedContext>,
) -> Option<Response> {
    let attribution = attribution?;
    let existing = match app.store_io(|| app.store.operation(scope, operation)).await {
        Ok(existing) => existing?,
        Err(error) => return Some(store_failure(request_id, Some(operation), &error)),
    };
    existing
        .grant_ref
        .is_some_and(|recorded| recorded != attribution.grant_ref)
        .then(|| {
            delegated_context_refusal(request_id, Some(operation), ContextRefusal::GRANT_CONFLICT)
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

pub(super) fn validate_workspace_input(
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

pub(super) fn validate_exec_input(
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
    check_egress_aperture(&facts, &input.sandbox, mutation, request_id)?;
    check_secret_slots(&facts, mutation, request_id)?;
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

/// Daemon-side admission for the aperture a start selects, by name (ADR 0013).
///
/// Shape, then capability, then declaration — the same order the driver uses independently, so a
/// caller hears the same answer whichever layer sees the request first. The published fact is the
/// declared names and their pinned destinations, so this needs no access to daemon configuration
/// and gets none.
fn check_egress_aperture(
    facts: &substrate_wire::CapabilityFacts,
    sandbox: &substrate_wire::ConfinementRequest,
    mutation: &BoundMutation<ExecStartInput>,
    request_id: &str,
) -> Result<(), Response> {
    if let Err(error) =
        substrate_wire::validate_aperture_request(sandbox.network, sandbox.aperture.as_deref())
    {
        let (code, message, address) = match error {
            substrate_wire::WireValidationError::ApertureDestinationInRequest => (
                "exec.aperture-destination-in-request",
                "An egress aperture is selected by name; a destination may not appear in a request.",
                "sandbox.network.aperture",
            ),
            // Named apart from the destination refusal for the same reason that one exists: an
            // escalation an operator can read is worth more than a schema complaint (ADR 0014).
            substrate_wire::WireValidationError::ApertureCeilingInRequest => (
                "exec.aperture-ceiling-in-request",
                "An egress aperture byte ceiling is declared by the operator; it may not appear in a request.",
                "sandbox.network.aperture",
            ),
            substrate_wire::WireValidationError::InvalidApertureName => (
                "exec.aperture-name-invalid",
                "An egress aperture name must match [a-z][a-z0-9_]{0,63}.",
                "sandbox.network.aperture",
            ),
            _ => (
                "exec.aperture-mode-mismatch",
                "A sandbox asks for network \"aperture\" with a name, or \"none\" without one.",
                "sandbox.network",
            ),
        };
        return Err(failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            request_id,
            Some(&mutation.op),
            ErrorClass::Refused,
            code,
            message,
            Some(address),
            false,
        ));
    }
    let Some(name) = sandbox.aperture.as_deref() else {
        return Ok(());
    };
    let Some(published) = facts.exec_egress_apertures.as_ref() else {
        return Err(failure(
            StatusCode::NOT_IMPLEMENTED,
            request_id,
            Some(&mutation.op),
            ErrorClass::Unserved,
            "exec.egress-apertures-unserved",
            "Egress apertures are not served by this host.",
            Some("exec.network-aperture"),
            false,
        ));
    };
    if !published.iter().any(|fact| fact.name == name) {
        return Err(failure(
            StatusCode::NOT_IMPLEMENTED,
            request_id,
            Some(&mutation.op),
            ErrorClass::Unserved,
            "exec.aperture-undeclared",
            // The name, never a destination: an operator debugging a harness needs to know which
            // aperture was asked for, and a name is deployment vocabulary.
            &format!("Egress aperture {name} is not declared on this host."),
            Some("exec.network-aperture"),
            false,
        ));
    }
    Ok(())
}

/// Daemon-side admission for the slots a start names (ADR 0012).
///
/// Shape, then capability, then declaration — the same order the driver uses, so a caller hears the
/// same answer whichever layer sees the request first. The published fact is the sorted list of
/// declared **names**, so this needs no access to any path and gets none.
fn check_secret_slots(
    facts: &substrate_wire::CapabilityFacts,
    mutation: &BoundMutation<ExecStartInput>,
    request_id: &str,
) -> Result<(), Response> {
    let requested = &mutation.input.secret_slots;
    if requested.is_empty() {
        return Ok(());
    }
    if substrate_wire::validate_secret_slots(requested).is_err() {
        return Err(failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            request_id,
            Some(&mutation.op),
            ErrorClass::Refused,
            "exec.secret-slot-descriptor-invalid",
            "A named secret slot is outside the closed descriptor bounds.",
            Some("secret_slots"),
            false,
        ));
    }
    let Some(published) = facts.secrets_slots.as_ref() else {
        return Err(failure(
            StatusCode::NOT_IMPLEMENTED,
            request_id,
            Some(&mutation.op),
            ErrorClass::Unserved,
            "exec.secret-slots-unserved",
            "Sealed secret slots are not served by this host.",
            Some("secret_slots"),
            false,
        ));
    };
    for slot in requested {
        if !published.contains(&slot.slot) {
            return Err(failure(
                StatusCode::UNPROCESSABLE_ENTITY,
                request_id,
                Some(&mutation.op),
                ErrorClass::Refused,
                "exec.secret-slot-unknown",
                // The slot name, never its material: an error may say which slot and nothing about
                // what is in it (`docs/design/04-security-and-isolation.md`).
                &format!("Secret slot {} is not declared on this host.", slot.slot),
                Some("secret_slots"),
                false,
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_pipe_session_input(
    app: &App,
    mutation: &BoundMutation<PipeSessionStartInput>,
    request_id: &str,
) -> Result<(), Response> {
    let exec_mutation = BoundMutation {
        op: mutation.op.clone(),
        input: mutation.input.exec.clone(),
        request_hash: mutation.request_hash.clone(),
        attribution: mutation.attribution.clone(),
    };
    // The mode gate is outermost, because the mode decides which contract the rest of the request
    // is read under. A terminal this deployment never proved it can give is refused by name — not
    // as the confinement refusal a pipe session would also get, and never served as pipes instead
    // (design 13, invariant 3).
    //
    // **The fact outranks the window shape**, and the order is asserted rather than incidental
    // (`vectors/http/pty-session-unserved-outranks-a-missing-window.json`). Both refusals can apply
    // to one request; only one of them is worth acting on. `session.window-invalid` invites the
    // client to add a window and try again, which on a deployment with no terminals is a retry that
    // can never succeed. `session.pty-unserved` says *stop*, which is the true answer.
    let input = &mutation.input;
    if input.mode == substrate_wire::SessionMode::Pty
        && app.driver.machine().facts.sessions_pty != Some(true)
    {
        return Err(failure(
            StatusCode::NOT_IMPLEMENTED,
            request_id,
            Some(&mutation.op),
            ErrorClass::Unserved,
            substrate_wire::SESSION_PTY_UNSERVED,
            "This deployment did not prove it can give a confined process a controlling terminal.",
            Some("mode"),
            false,
        ));
    }
    if substrate_wire::validate_session_window(input.mode, input.window.as_ref()).is_err() {
        return Err(failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            request_id,
            Some(&mutation.op),
            ErrorClass::Refused,
            substrate_wire::SESSION_WINDOW_INVALID,
            "A pty session declares an initial window within the closed cell bounds, and a raw-pipe session declares none.",
            Some("window"),
            false,
        ));
    }
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

pub(super) fn pipe_confinement_available(facts: &substrate_wire::CapabilityFacts) -> bool {
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
pub(super) async fn begin<T>(
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
pub(super) fn new_operation<T>(
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
        grant_ref: mutation
            .attribution
            .as_ref()
            .map(|value| value.grant_ref.clone()),
        platform_principal: mutation
            .attribution
            .as_ref()
            .map(|value| value.platform_principal.clone()),
        resource,
    }
}

pub(super) fn reservation_response(
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
pub(super) async fn refuse_before_dispatch<T>(
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
pub(super) async fn refuse_workspace_mutation<T>(
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
pub(super) async fn refuse_before_dispatch_response<T>(
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
        grant_ref: mutation
            .attribution
            .as_ref()
            .map(|value| value.grant_ref.clone()),
        platform_principal: mutation
            .attribution
            .as_ref()
            .map(|value| value.platform_principal.clone()),
        resource: None,
    };
    let _ = (method, address);
    record_bound_refusal(app, request_id, &new, response).await
}

pub(super) async fn finish_success<T: Serialize + Sync>(
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

pub(super) async fn finish_exec(
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
pub(super) async fn finish_exec_leased(
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

pub(super) async fn finish_pipe_session_start(
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

pub(super) async fn finish_pipe_session_observation(
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

pub(super) fn stored_exec(observation: &ExecObservation) -> StoredExec {
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

pub(super) fn observation_from_stored(stored: StoredExec) -> ExecObservation {
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

pub(super) fn stored_output(
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

pub(super) async fn finish_driver_error(
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

pub(super) async fn finish_dispatch_absence(
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
pub(super) async fn finish_pipe_session_dispatch_absence(
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
pub(super) async fn finish_pipe_session_dispatch_unknown(
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

pub(super) async fn finish_dispatch_unknown(
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

pub(super) fn replay(request_id: &str, operation: &str, answer: StoredAnswer) -> Response {
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

pub(super) fn driver_failure(
    request_id: &str,
    operation: Option<&str>,
    error: &DriverError,
) -> Response {
    let (status, detail) = driver_detail(operation, error);
    failure_detail(status, request_id, detail)
}

pub(super) fn driver_detail(
    operation: Option<&str>,
    error: &DriverError,
) -> (StatusCode, ErrorDetail) {
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

pub(super) fn new_lease(
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

pub(super) async fn finish_lease_store_error(
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

pub(super) fn linux_lease_clock() -> Result<LeaseClock, String> {
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
