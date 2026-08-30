use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use substrate_host::{DispatchOutcome, DriverErrorClass};
use substrate_store::{ExecRetireReservation, ExecWrite, NewLease, StoredExec, WorkspaceAdmission};
use substrate_wire::{
    EmptyInput, Exec, ExecKind, ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState,
    LeaseRenewInput, MAX_LEASE_TTL_MS, MIN_LEASE_TTL_MS, Success,
};

use super::operations::{
    begin, decode_mutation, driver_failure, finish_dispatch_absence, finish_dispatch_unknown,
    finish_driver_error, finish_exec, finish_exec_leased, finish_lease_store_error, new_lease,
    new_operation, observation_from_stored, refuse_before_dispatch_response, replay,
    reservation_response, stored_exec, stored_output, validate_exec_input,
};
use super::responses::{
    not_found, not_found_with_operation, operation_ledger_capacity, outcome_unknown,
    query_is_empty, request_id, schema_invalid, store_failure, success, workspace_frozen_refusal,
};
use super::{App, Identity};

#[allow(clippy::too_many_lines)] // Admission, durable reservation, and dispatch stay adjacent.
pub(super) async fn exec_start(
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
            // An accepted exec has hit no bound: nothing has run yet.
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

pub(super) async fn exec_get(
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

pub(super) async fn exec_retire(
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

pub(super) async fn exec_output_get(
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
pub(super) async fn exec_signal(
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
pub(super) async fn exec_lease_renew(
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
