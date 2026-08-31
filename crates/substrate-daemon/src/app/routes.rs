use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{get, post};
use axum::{Extension, Router};
use substrate_wire::{ErrorClass, ErrorDetail, Failure, Success, validate_operation_id};

use super::events::{
    event_list, event_stream, reconciliation_snapshot_create, reconciliation_snapshot_get,
};
use super::execs::{
    exec_get, exec_lease_renew, exec_output_get, exec_retire, exec_signal, exec_start,
};
use super::responses::{
    not_found, not_found_at, query_is_empty, request_id, schema_invalid, store_failure, success,
};
use super::sessions::{
    pipe_session_attach, pipe_session_capabilities, pipe_session_get, pipe_session_lease_renew,
    pipe_session_retire, pipe_session_signal, pipe_session_start,
};
use super::workspaces::{
    workspace_create, workspace_destroy, workspace_file_delete, workspace_file_edit_v2,
    workspace_file_patch_v2, workspace_file_read, workspace_file_read_v2,
    workspace_file_replace_v2, workspace_file_write, workspace_get, workspace_lease_renew,
    workspace_tree_read_v2,
};
use super::{App, CONTRACT_BUNDLE, CONTRACT_BUNDLE_SHA256, Identity};

// A mutation diff can contain two maximum-sized files and JSON escaping can expand its bytes.
// Keep envelope rewriting bounded, but above the largest response the handlers can produce.
const V2_ENVELOPE_LIMIT: usize = super::BODY_LIMIT * 8;

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
        .merge(
            Router::new()
                .route(
                    "/v2/workspaces/{workspace_id}/files/{*path}",
                    get(workspace_file_read_v2).put(workspace_file_replace_v2),
                )
                .route(
                    "/v2/workspaces/{workspace_id}/tree",
                    get(workspace_tree_read_v2),
                )
                .route(
                    "/v2/workspaces/{workspace_id}/file-edits/{*path}",
                    post(workspace_file_edit_v2),
                )
                .route(
                    "/v2/workspaces/{workspace_id}/file-patches/{*path}",
                    post(workspace_file_patch_v2),
                )
                .route_layer(middleware::from_fn(v2_envelope)),
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
        .layer(middleware::from_fn(contract_identity))
        .with_state(app)
}

/// V2 was added after the shared durable-operation machinery, whose stored answers intentionally
/// preserve the v1 bytes frozen in every released bundle. Keep that machinery byte-identical and
/// version the route-selected envelope at the HTTP boundary, including refusals and replays.
async fn v2_envelope(request: Request<Body>, next: Next) -> Response {
    let response = next.run(request).await;
    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, V2_ENVELOPE_LIMIT).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(%error, "v2 response exceeded the closed envelope bound");
            return v2_envelope_failure();
        }
    };
    let mut document: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(document) => document,
        Err(error) => {
            tracing::error!(%error, "v2 handler returned a non-JSON envelope");
            return v2_envelope_failure();
        }
    };
    let Some(object) = document.as_object_mut() else {
        return v2_envelope_failure();
    };
    object.insert("api_version".to_owned(), serde_json::json!("v2"));
    let encoded = match serde_json::to_vec(&document) {
        Ok(encoded) => encoded,
        Err(error) => {
            tracing::error!(%error, "v2 envelope serialization failed");
            return v2_envelope_failure();
        }
    };
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(encoded))
}

fn v2_envelope_failure() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(Failure {
            api_version: "v2".to_owned(),
            request_id: "v2-envelope-failed".to_owned(),
            error: ErrorDetail {
                class: ErrorClass::Failed,
                code: "response.envelope-failed".to_owned(),
                message: "Response could not be encoded in the selected API envelope.".to_owned(),
                retriable: false,
                address: Some("response".to_owned()),
                operation: None,
            },
        }),
    )
        .into_response()
}

async fn contract_identity(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "x-b10x-contract",
        CONTRACT_BUNDLE.parse().expect("static contract header"),
    );
    response.headers_mut().insert(
        "x-b10x-contract-bundle-sha256",
        CONTRACT_BUNDLE_SHA256
            .parse()
            .expect("static contract digest header"),
    );
    response
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
