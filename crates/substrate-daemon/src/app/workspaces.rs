use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use base64::Engine as _;
use substrate_host::{
    DispatchOutcome, DriverError, DriverErrorClass, Listed, WorkspaceDestroyProgress,
};
use substrate_store::{
    NewLease, WorkspaceAdmission, WorkspaceAdoptReservation, WorkspaceDestroyReservation,
    WorkspaceObservationWrite, WorkspaceRootHolder, adopt_not_found,
};
use substrate_wire::{
    EmptyInput, ErrorClass, FileEditInput, FilePatchInput, FileReadQuery, FileReplaceInput,
    FileWriteInput, GitBaselineFileQuery, GitChangesQuery, LeaseRenewInput, MAX_LEASE_TTL_MS,
    MIN_LEASE_TTL_MS, OMITTED_ENTRIES_HEADER, Success, WorkspaceAdoptInput, WorkspaceCreateInput,
    WorkspaceKind, WorkspaceOrigin, WorkspaceState, WorkspaceTreeQuery, validate_relative_path,
};

use super::operations::{
    BoundMutation, begin, decode_mutation, driver_failure, finish_dispatch_absence,
    finish_dispatch_unknown, finish_driver_error, finish_lease_store_error, finish_success,
    labels_within_bounds, new_lease, new_operation, refuse_before_dispatch,
    refuse_before_dispatch_response, refuse_workspace_mutation, replay, reservation_response,
    validate_workspace_input,
};
use super::responses::{
    failure, failure_detail, normalized_file_address, not_found, not_found_with_operation,
    operation_ledger_capacity, outcome_unknown, path_refusal, query_is_empty, request_id,
    schema_invalid, store_failure, success, workspace_frozen_refusal, workspace_missing_refusal,
};
use super::service::{end_workspace, run_maintenance_driver};
use super::{App, Identity};

#[allow(clippy::too_many_lines)] // Durable admission and driver dispatch stay auditable together.
pub(super) async fn workspace_create(
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
    if let Err(response) = validate_workspace_input(&app, &mutation, &request_id) {
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
    let source_authorization = match headers
        .get("x-b10x-workspace-source-authorization")
        .map(|value| value.to_str().ok())
    {
        None => None,
        Some(Some(value)) => {
            if value.is_empty()
                || value.len() > 512
                || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
            {
                let error = DriverError::refused(
                    "workspace.git-authority-invalid",
                    "Workspace source authority is invalid.",
                    "workspace.git",
                );
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
            Some(zeroize::Zeroizing::new(value.to_owned()))
        }
        Some(None) => {
            return refuse_before_dispatch(
                &app,
                &identity,
                &request_id,
                "workspace.create",
                "POST",
                "/v1/workspaces",
                &mutation,
                &DriverError::refused(
                    "workspace.git-authority-invalid",
                    "Workspace source authority is invalid.",
                    "workspace.git",
                ),
            )
            .await;
        }
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
        origin: WorkspaceOrigin::Created,
        root_name: root_name.as_str().into(),
        storage: None,
        lease: lease.as_ref().map(NewLease::observation),
    };
    let backend = crate::backend_ledger::workspace_reservation(
        app.driver.as_ref(),
        &id,
        &scope.subject,
        &operation,
    );
    if let Some(response) = reservation_response(
        app.store_io(|| {
            app.store.reserve_workspace_create_with_backend(
                &new,
                &root_name,
                &provisional,
                lease.as_ref(),
                &backend,
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
        .create_workspace_authorized(
            &id,
            &root_name,
            &mutation.input,
            source_authorization.as_ref().map(|value| value.as_str()),
        )
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

/// `workspace.adopt` (ADR 0035): serve a directory the operator placed under the workspace root.
///
/// Refusals run in the decided order and every one is a recorded ledger row a retry replays:
/// an out-of-set name, a name another subject's row holds (answered exactly as an absent
/// directory), a name the caller's own row holds, the host observation (absent, not a directory,
/// or not owned by the daemon's effective uid), then the workspace partition and the ledger. The
/// held-name and capacity checks run again inside the reservation transaction, so a row written
/// between the read and the reservation is still refused by owner, and a name held at the read
/// but freed before the reservation is answered from the read, never admitted unobserved.
#[allow(clippy::too_many_lines)] // Admission order and recording stay auditable together.
pub(super) async fn workspace_adopt(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    const KIND: &str = "workspace.adopt";
    const ADDRESS: &str = "/v1/workspaces/adopt";
    let request_id = request_id(&app, &headers);
    let mutation = match decode_mutation::<WorkspaceAdoptInput>(
        &app,
        &identity,
        KIND,
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
    let refusal = if mutation.input.root_name.is_valid() {
        (!labels_within_bounds(&mutation.input.labels))
            .then(|| schema_invalid(&request_id, Some(&mutation.op), "input"))
    } else {
        Some(schema_invalid(&request_id, Some(&mutation.op), "root_name"))
    };
    if let Some(response) = refusal {
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            KIND,
            "POST",
            ADDRESS,
            &mutation,
            response,
        )
        .await;
    }
    let root_name = mutation.input.root_name.as_str().to_owned();
    let id = app.authority.workspace_id();
    let scope = app.scope(&identity);
    let _workspace_guard = app.lock_workspace(&scope, &id).await;
    let new = new_operation(
        &app,
        &identity,
        KIND,
        "POST",
        ADDRESS,
        &mutation,
        None,
        Some(id.clone()),
    );
    let provisional = substrate_wire::Workspace {
        id: id.clone(),
        kind: WorkspaceKind::Workspace,
        labels: mutation.input.labels.clone(),
        observed_at: app.authority.now(),
        state: WorkspaceState::Ready,
        origin: WorkspaceOrigin::Adopted,
        root_name: mutation.input.root_name.clone(),
        storage: None,
        lease: None,
    };
    let holder = match app
        .store_io(|| app.store.workspace_root_holder(&scope, &root_name))
        .await
    {
        Ok(value) => value,
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    };
    // A held name is refused by the reservation without asking the host anything, so the answer
    // to another subject's name is decided before, and independently of, the directory's state.
    // The reservation is handed the holder this read found, so a name freed in between is still
    // answered from it: the provisional resource below is never written as an observed row.
    let observed = if holder == WorkspaceRootHolder::Free {
        match app
            .driver
            .observe_workspace(&id, &root_name, &provisional)
            .await
        {
            Ok(mut observed) => {
                observed.storage = None;
                observed.lease = None;
                observed
            }
            Err(error) if error.class == DriverErrorClass::NotFound => {
                return refuse_before_dispatch_response(
                    &app,
                    &identity,
                    &request_id,
                    KIND,
                    "POST",
                    ADDRESS,
                    &mutation,
                    failure_detail(
                        StatusCode::NOT_FOUND,
                        &request_id,
                        adopt_not_found(&mutation.op),
                    ),
                )
                .await;
            }
            Err(error) => {
                return refuse_before_dispatch(
                    &app,
                    &identity,
                    &request_id,
                    KIND,
                    "POST",
                    ADDRESS,
                    &mutation,
                    &error,
                )
                .await;
            }
        }
    } else {
        provisional
    };
    let terminal_at = app.authority.now().to_rfc3339();
    match app
        .store_io(|| {
            app.store
                .reserve_workspace_adopt(&new, &root_name, holder, &observed, &terminal_at)
        })
        .await
    {
        Ok(WorkspaceAdoptReservation::Adopted(workspace)) => success(
            StatusCode::CREATED,
            Success::mutation(request_id, mutation.op, workspace),
        ),
        Ok(WorkspaceAdoptReservation::Answered(reservation)) => {
            reservation_response(Ok(reservation), &request_id, &mutation.op)
                .unwrap_or_else(|| outcome_unknown(&request_id, &mutation.op))
        }
        Err(error) => store_failure(&request_id, Some(&mutation.op), &error),
    }
}

pub(super) async fn workspace_get(
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
                Ok(WorkspaceObservationWrite::Authoritative(authoritative)) => success(
                    StatusCode::OK,
                    Success::observed(request_id, *authoritative),
                ),
                Ok(WorkspaceObservationWrite::Missing) => not_found(&request_id),
                Err(error) => store_failure(&request_id, None, &error),
            }
        }
        Err(error) => driver_failure(&request_id, None, &error),
    }
}

pub(super) async fn workspace_file_read(
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
        Ok(listed) => listed_success(request_id, listed),
        Err(error) => driver_failure(&request_id, None, &error),
    }
}

pub(super) async fn workspace_file_read_v2(
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
        .read_workspace_file_v2(&workspace_id, &root_name, &path, &query)
        .await
    {
        Ok(result) => success(StatusCode::OK, Success::observed(request_id, result)),
        Err(error) => driver_failure(&request_id, None, &error),
    }
}

pub(super) async fn workspace_tree_read_v2(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(workspace_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    let query: WorkspaceTreeQuery = match serde_urlencoded::from_str::<WorkspaceTreeQuery>(
        raw_query.as_deref().unwrap_or(""),
    ) {
        Ok(value) if value.validate().is_ok() => value,
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
        .list_workspace_tree_v2(&workspace_id, &root_name, &query)
        .await
    {
        Ok(listed) => listed_success(request_id, listed),
        Err(error) => driver_failure(&request_id, None, &error),
    }
}

/// A listing's `200`, naming the entries it omitted in `x-b10x-omitted-entries` exactly when it
/// omitted at least one (ADR 0043). The body is the closed wire object, unchanged.
fn listed_success<T: serde::Serialize>(request_id: String, listed: Listed<T>) -> Response {
    let mut response = success(StatusCode::OK, Success::observed(request_id, listed.value));
    if listed.omitted_entries > 0 {
        response.headers_mut().insert(
            OMITTED_ENTRIES_HEADER,
            HeaderValue::from(listed.omitted_entries),
        );
    }
    response
}

pub(super) async fn workspace_git_baseline_file_read_v2(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path((workspace_id, path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    let query: GitBaselineFileQuery = match serde_urlencoded::from_str::<GitBaselineFileQuery>(
        raw_query.as_deref().unwrap_or(""),
    ) {
        Ok(value) if value.validate().is_ok() => value,
        _ => return schema_invalid(&request_id, None, "query"),
    };
    let path = match validate_relative_path(&path) {
        Ok(()) => path,
        Err(error) => return path_refusal(&request_id, None, &error),
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
        .read_workspace_git_baseline_file_v2(&workspace_id, &root_name, &path, query.max_bytes)
        .await
    {
        Ok(result) => success(StatusCode::OK, Success::observed(request_id, result)),
        Err(error) => driver_failure(&request_id, None, &error),
    }
}

pub(super) async fn workspace_git_changes_read_v2(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(workspace_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    let query: GitChangesQuery =
        match serde_urlencoded::from_str::<GitChangesQuery>(raw_query.as_deref().unwrap_or("")) {
            Ok(value) if value.validate().is_ok() => value,
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
        .read_workspace_git_changes_v2(&workspace_id, &root_name, &query)
        .await
    {
        Ok(result) => success(StatusCode::OK, Success::observed(request_id, result)),
        Err(error) => driver_failure(&request_id, None, &error),
    }
}

#[allow(clippy::too_many_lines)] // Durable refusal and atomic host write stay adjacent.
pub(super) async fn workspace_file_write(
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

enum V2FileMutation {
    Replace(FileReplaceInput),
    Edit(FileEditInput),
    Patch(FilePatchInput),
}

pub(super) async fn workspace_file_replace_v2(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path((workspace_id, path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v2/workspaces/{workspace_id}/files/{path}");
    let mutation = match decode_mutation::<FileReplaceInput>(
        &app,
        &identity,
        "workspace.file.replace-v2",
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
    workspace_file_mutation_v2(
        app,
        identity,
        request_id,
        workspace_id,
        path,
        address,
        "workspace.file.replace-v2",
        "PUT",
        BoundMutation {
            op: mutation.op,
            input: V2FileMutation::Replace(mutation.input),
            request_hash: mutation.request_hash,
            attribution: mutation.attribution,
        },
    )
    .await
}

pub(super) async fn workspace_file_edit_v2(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path((workspace_id, path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v2/workspaces/{workspace_id}/file-edits/{path}");
    let mutation = match decode_mutation::<FileEditInput>(
        &app,
        &identity,
        "workspace.file.edit-v2",
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
    workspace_file_mutation_v2(
        app,
        identity,
        request_id,
        workspace_id,
        path,
        address,
        "workspace.file.edit-v2",
        "POST",
        BoundMutation {
            op: mutation.op,
            input: V2FileMutation::Edit(mutation.input),
            request_hash: mutation.request_hash,
            attribution: mutation.attribution,
        },
    )
    .await
}

pub(super) async fn workspace_file_patch_v2(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path((workspace_id, path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    let address = format!("/v2/workspaces/{workspace_id}/file-patches/{path}");
    let mutation = match decode_mutation::<FilePatchInput>(
        &app,
        &identity,
        "workspace.file.patch-v2",
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
    workspace_file_mutation_v2(
        app,
        identity,
        request_id,
        workspace_id,
        path,
        address,
        "workspace.file.patch-v2",
        "POST",
        BoundMutation {
            op: mutation.op,
            input: V2FileMutation::Patch(mutation.input),
            request_hash: mutation.request_hash,
            attribution: mutation.attribution,
        },
    )
    .await
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn workspace_file_mutation_v2(
    app: Arc<App>,
    identity: Identity,
    request_id: String,
    workspace_id: String,
    path: String,
    address: String,
    operation_kind: &'static str,
    method: &'static str,
    mutation: BoundMutation<V2FileMutation>,
) -> Response {
    if let Err(error) = validate_relative_path(&path) {
        let response = path_refusal(&request_id, Some(&mutation.op), &error);
        return refuse_before_dispatch_response(
            &app,
            &identity,
            &request_id,
            operation_kind,
            method,
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
                operation_kind,
                method,
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
                operation_kind,
                method,
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
        operation_kind,
        method,
        &address,
        &mutation,
        None,
        Some(workspace_id.clone()),
    )
    .await
    {
        return response;
    }
    let result = match &mutation.input {
        V2FileMutation::Replace(input) => {
            app.driver
                .replace_workspace_file_v2(&workspace_id, &root_name, &path, input)
                .await
        }
        V2FileMutation::Edit(input) => {
            app.driver
                .edit_workspace_file_v2(&workspace_id, &root_name, &path, input)
                .await
        }
        V2FileMutation::Patch(input) => {
            app.driver
                .patch_workspace_file_v2(&workspace_id, &root_name, &path, input)
                .await
        }
    };
    match result {
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
pub(super) async fn workspace_file_delete(
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
pub(super) async fn workspace_destroy(
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
    let (root_name, origin) = match app
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
        Ok(WorkspaceDestroyReservation::Admitted {
            root_name,
            resource,
        }) => {
            super::health::dispatch_admitted();
            (root_name, resource.origin)
        }
        Err(error) => return store_failure(&request_id, Some(&operation), &error),
    };
    match run_maintenance_driver(end_workspace(
        app.driver.as_ref(),
        &workspace_id,
        &root_name,
        origin,
    ))
    .await
    {
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
                origin,
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
                origin,
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

#[allow(clippy::too_many_lines)] // Durable refusal and renewal stay in one mutation path.
pub(super) async fn workspace_lease_renew(
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
