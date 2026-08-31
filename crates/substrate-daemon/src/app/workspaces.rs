use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use base64::Engine as _;
use substrate_host::{DispatchOutcome, DriverError, WorkspaceDestroyProgress};
use substrate_store::{
    NewLease, WorkspaceAdmission, WorkspaceDestroyReservation, WorkspaceObservationWrite,
};
use substrate_wire::{
    EmptyInput, ErrorClass, FileEditInput, FilePatchInput, FileReadQuery, FileReplaceInput,
    FileWriteInput, LeaseRenewInput, MAX_LEASE_TTL_MS, MIN_LEASE_TTL_MS, Success,
    WorkspaceCreateInput, WorkspaceKind, WorkspaceState, WorkspaceTreeQuery,
    validate_relative_path,
};

use super::operations::{
    BoundMutation, begin, decode_mutation, driver_failure, finish_dispatch_absence,
    finish_dispatch_unknown, finish_driver_error, finish_lease_store_error, finish_success,
    new_lease, new_operation, refuse_before_dispatch, refuse_before_dispatch_response,
    refuse_workspace_mutation, replay, reservation_response, validate_workspace_input,
};
use super::responses::{
    failure, normalized_file_address, not_found, not_found_with_operation,
    operation_ledger_capacity, outcome_unknown, path_refusal, query_is_empty, request_id,
    schema_invalid, store_failure, success, workspace_frozen_refusal, workspace_missing_refusal,
};
use super::service::run_maintenance_driver;
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
        storage: None,
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
        Ok(result) => success(StatusCode::OK, Success::observed(request_id, result)),
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
