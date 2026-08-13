#![allow(clippy::result_large_err)] // Axum responses are the natural typed rejection at this seam.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use substrate_host::{Driver, DriverError, DriverErrorClass, ExecObservation};
use substrate_store::{NewOperation, Reservation, Scope, Store, StoredAnswer, StoredExec};
use substrate_wire::{
    Base64Content, Base64Encoding, EmptyInput, ErrorClass, ErrorDetail, ExecOutputQuery,
    ExecSignalInput, ExecStartInput, Failure, FileReadQuery, FileWriteInput, Mutation, NetworkMode,
    OperationOutcome, OutputSlice, Success, WorkspaceCreateInput, WorkspaceSource,
    canonical_request_hash, validate_operation_id, validate_relative_path,
};
use ulid::Ulid;

const BODY_LIMIT: usize = 1_048_576;

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
}

pub struct App {
    pub store: Arc<Store>,
    pub driver: Arc<dyn Driver>,
    pub deployment: String,
    authority: Arc<dyn Authority>,
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
        Arc::new(Self {
            store,
            driver,
            deployment: deployment.into(),
            authority,
        })
    }

    fn scope(&self, identity: &Identity) -> Scope {
        Scope {
            deployment: self.deployment.clone(),
            subject: identity.subject.clone(),
        }
    }
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
        .route("/v1/execs/{exec_id}", get(exec_get))
        .route("/v1/execs/{exec_id}/output", get(exec_output_get))
        .route("/v1/execs/{exec_id}/signal", post(exec_signal))
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

async fn workspace_create(
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
    let mutation = match decode_mutation::<WorkspaceCreateInput>(body, &request_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = validate_workspace_input(&mutation, &request_id) {
        return response;
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
        );
    }
    let operation = mutation.op.clone();
    let id = app.authority.workspace_id();
    let scope = app.scope(&identity);
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        "workspace.create",
        "POST",
        "/v1/workspaces",
        &mutation,
        None,
        Some(id.clone()),
    ) {
        return response;
    }
    match app.driver.create_workspace(&id, &mutation.input).await {
        Ok((root_name, workspace)) => {
            if let Err(error) = app.store.complete_workspace(
                &scope,
                &operation,
                &app.authority.now().to_rfc3339(),
                201,
                &root_name,
                &workspace,
            ) {
                return store_failure(&request_id, Some(&operation), &error);
            }
            success(
                StatusCode::CREATED,
                Success::mutation(request_id, operation, workspace),
            )
        }
        Err(error) => finish_driver_error(&app, &scope, &request_id, &operation, Some(&id), &error),
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
    let Some((root_name, previous)) =
        store_or_response!(app.store.workspace(&scope, &workspace_id), request_id, None)
    else {
        return not_found(&request_id);
    };
    match app
        .driver
        .observe_workspace(&workspace_id, &root_name, &previous)
        .await
    {
        Ok(workspace) => {
            if let Err(error) = app.store.put_workspace(&scope, &root_name, &workspace) {
                return store_failure(&request_id, None, &error);
            }
            success(StatusCode::OK, Success::observed(request_id, workspace))
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
    if validate_relative_path(&path).is_err() {
        return path_escape(&request_id, None);
    }
    let query: FileReadQuery =
        match serde_urlencoded::from_str::<FileReadQuery>(raw_query.as_deref().unwrap_or("")) {
            Ok(value) if value.validate_shape().is_ok() => value,
            _ => return schema_invalid(&request_id, None, "query"),
        };
    let scope = app.scope(&identity);
    let Some((root_name, _)) =
        store_or_response!(app.store.workspace(&scope, &workspace_id), request_id, None)
    else {
        return not_found(&request_id);
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

async fn workspace_file_write(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path((workspace_id, path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let mutation = match decode_mutation::<FileWriteInput>(body, &request_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if validate_relative_path(&path).is_err() {
        return path_escape(&request_id, Some(&mutation.op));
    }
    let content = match mutation.input.content.decode() {
        Ok(value)
            if base64::engine::general_purpose::STANDARD.encode(&value)
                == mutation.input.content.data =>
        {
            value
        }
        _ => return schema_invalid(&request_id, Some(&mutation.op), "input"),
    };
    let scope = app.scope(&identity);
    let Some((root_name, _)) = store_or_response!(
        app.store.workspace(&scope, &workspace_id),
        request_id,
        Some(&mutation.op)
    ) else {
        return not_found_with_operation(&request_id, &mutation.op);
    };
    let address = normalized_file_address(&workspace_id, &path);
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
    ) {
        return response;
    }
    match app
        .driver
        .write_workspace_file(&workspace_id, &root_name, &path, &content)
        .await
    {
        Ok(observation) => finish_success(
            &app,
            &scope,
            &request_id,
            &operation,
            StatusCode::OK,
            Some(&workspace_id),
            observation,
        ),
        Err(error) => finish_driver_error(
            &app,
            &scope,
            &request_id,
            &operation,
            Some(&workspace_id),
            &error,
        ),
    }
}

async fn workspace_file_delete(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path((workspace_id, path)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let mutation = match decode_mutation::<EmptyInput>(body, &request_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if validate_relative_path(&path).is_err() {
        return path_escape(&request_id, Some(&mutation.op));
    }
    let scope = app.scope(&identity);
    let Some((root_name, _)) = store_or_response!(
        app.store.workspace(&scope, &workspace_id),
        request_id,
        Some(&mutation.op)
    ) else {
        return not_found_with_operation(&request_id, &mutation.op);
    };
    let address = normalized_file_address(&workspace_id, &path);
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
    ) {
        return response;
    }
    match app
        .driver
        .delete_workspace_file(&workspace_id, &root_name, &path)
        .await
    {
        Ok(absence) => finish_success(
            &app,
            &scope,
            &request_id,
            &operation,
            StatusCode::OK,
            Some(&workspace_id),
            absence,
        ),
        Err(error) => finish_driver_error(
            &app,
            &scope,
            &request_id,
            &operation,
            Some(&workspace_id),
            &error,
        ),
    }
}

async fn workspace_destroy(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(workspace_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let mutation = match decode_mutation::<EmptyInput>(body, &request_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let scope = app.scope(&identity);
    let Some((root_name, _)) = store_or_response!(
        app.store.workspace(&scope, &workspace_id),
        request_id,
        Some(&mutation.op)
    ) else {
        return not_found_with_operation(&request_id, &mutation.op);
    };
    match app
        .store
        .workspace_has_nonterminal_execs(&scope, &workspace_id)
    {
        Ok(true) => {
            return conflict(
                &request_id,
                &mutation.op,
                "workspace.execs-active",
                "Workspace has nonterminal execs.",
                "workspace",
            );
        }
        Ok(false) => {}
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    }
    let address = format!("/v1/workspaces/{workspace_id}");
    let operation = mutation.op.clone();
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        "workspace.destroy",
        "DELETE",
        &address,
        &mutation,
        None,
        Some(workspace_id.clone()),
    ) {
        return response;
    }
    match app
        .driver
        .destroy_workspace(&workspace_id, &root_name)
        .await
    {
        Ok(absence) => {
            if let Err(error) = app.store.complete_workspace_absence(
                &scope,
                &operation,
                &app.authority.now().to_rfc3339(),
                200,
                &workspace_id,
                &absence,
            ) {
                return store_failure(&request_id, Some(&operation), &error);
            }
            success(
                StatusCode::OK,
                Success::mutation(request_id, operation, absence),
            )
        }
        Err(error) => finish_driver_error(
            &app,
            &scope,
            &request_id,
            &operation,
            Some(&workspace_id),
            &error,
        ),
    }
}

async fn exec_start(
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
    let mutation = match decode_mutation::<ExecStartInput>(body, &request_id).await {
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
        );
    }
    let scope = app.scope(&identity);
    let Some((root_name, _)) = store_or_response!(
        app.store.workspace(&scope, &mutation.input.workspace),
        request_id,
        Some(&mutation.op)
    ) else {
        return not_found_with_operation(&request_id, &mutation.op);
    };
    let operation = mutation.op.clone();
    let id = app.authority.exec_id();
    let capability = Some(mutation.input.sandbox.capability_snapshot.clone());
    if let Some(response) = begin(
        &app,
        &identity,
        &request_id,
        "exec.start",
        "POST",
        "/v1/execs",
        &mutation,
        capability,
        Some(id.clone()),
    ) {
        return response;
    }
    match app
        .driver
        .start_exec(&id, &root_name, &mutation.input)
        .await
    {
        Ok(observation) => finish_exec(
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
        ),
        Err(error) => finish_driver_error(&app, &scope, &request_id, &operation, Some(&id), &error),
    }
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
    if store_or_response!(app.store.exec(&scope, &exec_id), request_id, None).is_none() {
        return not_found(&request_id);
    }
    match app.driver.observe_exec(&exec_id).await {
        Ok(observation) => {
            if let Err(error) = app.store.put_exec(&scope, &stored_exec(&observation)) {
                return store_failure(&request_id, None, &error);
            }
            success(
                StatusCode::OK,
                Success::observed(request_id, observation.resource),
            )
        }
        Err(_) => match app.store.exec(&scope, &exec_id) {
            Ok(Some(stored)) => success(
                StatusCode::OK,
                Success::observed(request_id, stored.resource),
            ),
            Ok(None) => not_found(&request_id),
            Err(error) => store_failure(&request_id, None, &error),
        },
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
    let mut stored = match app.store.exec(&scope, &exec_id) {
        Ok(Some(value)) => value,
        Ok(None) => return not_found(&request_id),
        Err(error) => return store_failure(&request_id, None, &error),
    };
    if let Ok(observation) = app.driver.observe_exec(&exec_id).await {
        stored = stored_exec(&observation);
        if let Err(error) = app.store.put_exec(&scope, &stored) {
            return store_failure(&request_id, None, &error);
        }
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

async fn exec_signal(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(exec_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let mutation = match decode_mutation::<ExecSignalInput>(body, &request_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if mutation.input.grace_ms > 30_000 {
        return schema_invalid(&request_id, Some(&mutation.op), "input");
    }
    let scope = app.scope(&identity);
    let stored = match app.store.exec(&scope, &exec_id) {
        Ok(Some(value)) => value,
        Ok(None) => return not_found_with_operation(&request_id, &mutation.op),
        Err(error) => return store_failure(&request_id, Some(&mutation.op), &error),
    };
    let address = format!("/v1/execs/{exec_id}/signal");
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
    ) {
        return response;
    }
    if matches!(
        stored.resource.state,
        substrate_wire::ExecState::Exited | substrate_wire::ExecState::Cancelled
    ) {
        return finish_exec(
            &app,
            &scope,
            &request_id,
            &operation,
            StatusCode::OK,
            observation_from_stored(stored),
        );
    }
    match app.driver.signal(&exec_id, &mutation.input).await {
        Ok(observation) => finish_exec(
            &app,
            &scope,
            &request_id,
            &operation,
            StatusCode::OK,
            observation,
        ),
        Err(error) => finish_driver_error(
            &app,
            &scope,
            &request_id,
            &operation,
            Some(&exec_id),
            &error,
        ),
    }
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
    match app.store.operation(&app.scope(&identity), &operation_id) {
        Ok(Some(operation)) => success(StatusCode::OK, Success::observed(request_id, operation)),
        Ok(None) => not_found(&request_id),
        Err(error) => store_failure(&request_id, None, &error),
    }
}

async fn route_not_found(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let request_id = request_id(&app, &headers);
    not_found(&request_id)
}

macro_rules! store_or_response {
    ($result:expr, $request_id:expr, $operation:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => return store_failure(&$request_id, $operation, &error),
        }
    };
}
use store_or_response;

async fn decode_mutation<T: DeserializeOwned>(
    body: Body,
    request_id: &str,
) -> Result<Mutation<T>, Response> {
    let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            request_id,
            None,
            ErrorClass::Exhausted,
            "request.body-limit",
            "Request body exceeds the configured byte limit.",
            Some("body"),
            false,
        ));
    };
    let raw: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Err(schema_invalid(request_id, None, "input")),
    };
    let operation = raw.get("op").and_then(Value::as_str).map(ToOwned::to_owned);
    match serde_json::from_value(raw) {
        Ok(mutation) if validate_operation_id(operation.as_deref().unwrap_or_default()).is_ok() => {
            Ok(mutation)
        }
        _ => Err(schema_invalid(request_id, operation.as_deref(), "input")),
    }
}

fn validate_workspace_input(
    mutation: &Mutation<WorkspaceCreateInput>,
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
    mutation: &Mutation<ExecStartInput>,
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
fn begin<T: Serialize>(
    app: &App,
    identity: &Identity,
    request_id: &str,
    operation_kind: &str,
    method: &str,
    address: &str,
    mutation: &Mutation<T>,
    capability_snapshot: Option<String>,
    resource: Option<String>,
) -> Option<Response> {
    let Ok(input) = serde_json::to_value(&mutation.input) else {
        return Some(schema_invalid(request_id, Some(&mutation.op), "input"));
    };
    let Ok(request_hash) = canonical_request_hash(method, address, &input) else {
        return Some(schema_invalid(request_id, Some(&mutation.op), "input"));
    };
    let new = NewOperation {
        scope: app.scope(identity),
        operation: mutation.op.clone(),
        operation_kind: operation_kind.to_owned(),
        request_hash,
        accepted_at: app.authority.now().to_rfc3339(),
        capability_snapshot,
        actor: identity.actor.clone(),
        principal: identity.principal.clone(),
        resource,
    };
    match app.store.reserve(&new) {
        Ok(Reservation::Accepted) => None,
        Ok(Reservation::Replay(answer)) => Some(replay(request_id, &mutation.op, answer)),
        Ok(Reservation::Conflict) => Some(conflict(
            request_id,
            &mutation.op,
            "operation.request-conflict",
            "Operation id is already bound to different input.",
            "operation",
        )),
        Ok(Reservation::Pending(_)) => Some(failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            request_id,
            Some(&mutation.op),
            ErrorClass::Failed,
            "operation.outcome-unknown",
            "Operation was accepted but its terminal outcome is unknown.",
            Some("operation"),
            true,
        )),
        Err(error) => Some(store_failure(request_id, Some(&mutation.op), &error)),
    }
}

#[allow(clippy::too_many_arguments)]
fn refuse_before_dispatch<T: Serialize>(
    app: &App,
    identity: &Identity,
    request_id: &str,
    operation_kind: &str,
    method: &str,
    address: &str,
    mutation: &Mutation<T>,
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
}

#[allow(clippy::too_many_arguments)]
fn refuse_before_dispatch_response<T: Serialize>(
    app: &App,
    identity: &Identity,
    request_id: &str,
    operation_kind: &str,
    method: &str,
    address: &str,
    mutation: &Mutation<T>,
    response: Response,
) -> Response {
    let Ok(input) = serde_json::to_value(&mutation.input) else {
        return response;
    };
    let Ok(request_hash) = canonical_request_hash(method, address, &input) else {
        return response;
    };
    let detail = response.extensions().get::<ErrorDetail>().cloned();
    let Some(detail) = detail else {
        return response;
    };
    let new = NewOperation {
        scope: app.scope(identity),
        operation: mutation.op.clone(),
        operation_kind: operation_kind.to_owned(),
        request_hash,
        accepted_at: app.authority.now().to_rfc3339(),
        capability_snapshot: None,
        actor: identity.actor.clone(),
        principal: identity.principal.clone(),
        resource: None,
    };
    match app.store.record_refusal(
        &new,
        &app.authority.now().to_rfc3339(),
        response.status().as_u16(),
        &detail,
    ) {
        Ok(Reservation::Replay(answer)) => replay(request_id, &mutation.op, answer),
        Ok(Reservation::Conflict) => conflict(
            request_id,
            &mutation.op,
            "operation.request-conflict",
            "Operation id is already bound to different input.",
            "operation",
        ),
        Ok(Reservation::Pending(_) | Reservation::Accepted) | Err(_) => response,
    }
}

fn finish_success<T: Serialize>(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    status: StatusCode,
    resource_id: Option<&str>,
    result: T,
) -> Response {
    if let Err(error) = app.store.complete_success(
        scope,
        operation,
        &app.authority.now().to_rfc3339(),
        status.as_u16(),
        resource_id,
        &result,
    ) {
        return store_failure(request_id, Some(operation), &error);
    }
    success(status, Success::mutation(request_id, operation, result))
}

fn finish_exec(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    status: StatusCode,
    observation: ExecObservation,
) -> Response {
    if let Err(error) = app.store.complete_exec(
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
    ) {
        return store_failure(request_id, Some(operation), &error);
    }
    success(
        status,
        Success::mutation(request_id, operation, observation.resource),
    )
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

fn finish_driver_error(
    app: &App,
    scope: &Scope,
    request_id: &str,
    operation: &str,
    resource_id: Option<&str>,
    error: &DriverError,
) -> Response {
    let (status, detail) = driver_detail(Some(operation), error);
    if let Err(store_error) = app.store.complete_error(
        scope,
        operation,
        &app.authority.now().to_rfc3339(),
        status.as_u16(),
        resource_id,
        &detail,
    ) {
        return store_failure(request_id, Some(operation), &store_error);
    }
    failure_detail(status, request_id, detail)
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

fn path_escape(request_id: &str, operation: Option<&str>) -> Response {
    failure(
        StatusCode::UNPROCESSABLE_ENTITY,
        request_id,
        operation,
        ErrorClass::Refused,
        "workspace.path-escape",
        "Workspace path is outside the confined root.",
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

fn not_found(request_id: &str) -> Response {
    failure(
        StatusCode::NOT_FOUND,
        request_id,
        None,
        ErrorClass::Refused,
        "resource.not-found",
        "Resource was not found.",
        Some("resource"),
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
