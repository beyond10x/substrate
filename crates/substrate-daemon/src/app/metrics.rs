use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use futures_util::{SinkExt as _, StreamExt as _};
use substrate_store::{
    ExecWrite, Scope, StoredExec, WorkspaceAdmission, WorkspaceObservationWrite,
};
use substrate_wire::{
    ErrorClass, ExecState, ExecUsage, MetricsObservation, MetricsQuery, MetricsResourceKind,
    MetricsStreamFrame, MetricsStreamQuery, Success,
};

use super::operations::{driver_failure, stored_exec};
use super::responses::{failure, not_found, request_id, schema_invalid, store_failure, success};
use super::{App, Identity};

pub(super) async fn metrics_get(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    let Ok(query) = serde_urlencoded::from_str::<MetricsQuery>(raw_query.as_deref().unwrap_or(""))
    else {
        return schema_invalid(&request_id, None, "query");
    };
    let scope = app.scope(&identity);
    match query.resource_kind {
        MetricsResourceKind::Exec => {
            match load_exec_usage(&app, &scope, &query.resource_id).await {
                Ok(usage) => success(
                    StatusCode::OK,
                    Success::observed(
                        request_id,
                        MetricsObservation::Exec {
                            exec: query.resource_id,
                            usage,
                        },
                    ),
                ),
                Err(MetricsLoadError::NotFound) => not_found(&request_id),
                Err(MetricsLoadError::NotRequested) => metrics_not_requested(&request_id),
                Err(MetricsLoadError::Driver(error)) => driver_failure(&request_id, None, &error),
                Err(MetricsLoadError::Store(error)) => store_failure(&request_id, None, &error),
            }
        }
        MetricsResourceKind::Workspace => {
            let _workspace_guard = app.lock_workspace(&scope, &query.resource_id).await;
            match load_workspace_storage(&app, &scope, &query.resource_id).await {
                Ok(storage) => success(
                    StatusCode::OK,
                    Success::observed(
                        request_id,
                        MetricsObservation::Workspace {
                            workspace: query.resource_id,
                            storage,
                        },
                    ),
                ),
                Err(MetricsLoadError::NotFound) => not_found(&request_id),
                Err(MetricsLoadError::NotRequested) => workspace_metrics_not_requested(&request_id),
                Err(MetricsLoadError::Driver(error)) => driver_failure(&request_id, None, &error),
                Err(MetricsLoadError::Store(error)) => store_failure(&request_id, None, &error),
            }
        }
    }
}

pub(super) async fn metrics_stream(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    ws: WebSocketUpgrade,
) -> Response {
    let request_id = request_id(&app, &headers);
    let Ok(query) =
        serde_urlencoded::from_str::<MetricsStreamQuery>(raw_query.as_deref().unwrap_or(""))
    else {
        return schema_invalid(&request_id, None, "query");
    };
    let scope = app.scope(&identity);
    match load_exec_usage(&app, &scope, &query.exec_id).await {
        Ok(_) => ws.on_upgrade(move |socket| run_stream(app, scope, query.exec_id, socket)),
        Err(MetricsLoadError::NotFound) => not_found(&request_id),
        Err(MetricsLoadError::NotRequested) => metrics_not_requested(&request_id),
        Err(MetricsLoadError::Driver(error)) => driver_failure(&request_id, None, &error),
        Err(MetricsLoadError::Store(error)) => store_failure(&request_id, None, &error),
    }
}

async fn run_stream(app: Arc<App>, scope: Scope, exec_id: String, mut socket: WebSocket) {
    let mut interval = tokio::time::interval(Duration::from_millis(
        substrate_wire::RESOURCE_USAGE_SAMPLE_INTERVAL_MS,
    ));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let Ok(usage) = load_exec_usage(&app, &scope, &exec_id).await else {
            let _ = socket.close().await;
            return;
        };
        let terminal = matches!(
            usage,
            ExecUsage::Observed(substrate_wire::ResourceUsage { complete: true, .. })
                | ExecUsage::Unavailable { .. }
        );
        let Ok(encoded) = serde_json::to_string(&MetricsStreamFrame::Usage {
            exec: exec_id.clone(),
            usage,
        }) else {
            return;
        };
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(5),
                socket.send(Message::Text(encoded.into())),
            )
            .await,
            Ok(Ok(()))
        ) {
            return;
        }
        if terminal {
            let _ = socket.close().await;
            return;
        }
        tokio::select! {
            _ = interval.tick() => {}
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(Message::Ping(bytes))) => {
                        let _ = socket.send(Message::Pong(bytes)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    _ => return,
                }
            }
        }
    }
}

async fn load_workspace_storage(
    app: &App,
    scope: &Scope,
    workspace_id: &str,
) -> Result<substrate_wire::StorageUsage, MetricsLoadError> {
    let admission = app
        .admit_workspace(scope, workspace_id)
        .await
        .map_err(MetricsLoadError::Store)?;
    let workspace = match admission {
        WorkspaceAdmission::Missing => return Err(MetricsLoadError::NotFound),
        WorkspaceAdmission::Frozen { resource, .. } => resource,
        WorkspaceAdmission::Admitted {
            root_name,
            resource,
        } => {
            let observation = app
                .driver
                .observe_workspace(workspace_id, &root_name, &resource)
                .await
                .map_err(MetricsLoadError::Driver)?;
            match app
                .store_io(|| {
                    app.store
                        .merge_workspace_observation(scope, &root_name, &observation)
                })
                .await
                .map_err(MetricsLoadError::Store)?
            {
                WorkspaceObservationWrite::Authoritative(authoritative) => *authoritative,
                WorkspaceObservationWrite::Missing => return Err(MetricsLoadError::NotFound),
            }
        }
    };
    workspace.storage.ok_or(MetricsLoadError::NotRequested)
}

enum MetricsLoadError {
    NotFound,
    NotRequested,
    Driver(substrate_host::DriverError),
    Store(substrate_store::StoreError),
}

async fn load_exec_usage(
    app: &App,
    scope: &Scope,
    exec_id: &str,
) -> Result<ExecUsage, MetricsLoadError> {
    let stored = app
        .store_io(|| app.store.exec(scope, exec_id))
        .await
        .map_err(MetricsLoadError::Store)?
        .ok_or(MetricsLoadError::NotFound)?;
    if matches!(
        stored.resource.state,
        ExecState::Exited | ExecState::Cancelled | ExecState::Expired | ExecState::Unknown
    ) {
        return stored.resource.usage.ok_or(MetricsLoadError::NotRequested);
    }
    let observation = app
        .driver
        .observe_exec(exec_id)
        .await
        .map_err(MetricsLoadError::Driver)?;
    let write = app
        .store_io(|| app.store.put_exec(scope, &stored_exec(&observation)))
        .await
        .map_err(MetricsLoadError::Store)?;
    let authoritative: StoredExec = match write {
        ExecWrite::PersistedExact(stored)
        | ExecWrite::PersistedTransformed(stored)
        | ExecWrite::Superseded(stored) => stored,
        ExecWrite::Retired => return Err(MetricsLoadError::NotFound),
    };
    if matches!(
        authoritative.resource.state,
        ExecState::Exited | ExecState::Cancelled | ExecState::Expired | ExecState::Unknown
    ) {
        app.driver.acknowledge_exec(&observation);
    }
    authoritative
        .resource
        .usage
        .ok_or(MetricsLoadError::NotRequested)
}

fn metrics_not_requested(request_id: &str) -> Response {
    failure(
        StatusCode::CONFLICT,
        request_id,
        None,
        ErrorClass::Conflict,
        "exec.metrics-not-requested",
        "The exec did not request resource-usage measurement.",
        Some("resource_id"),
        false,
    )
}

fn workspace_metrics_not_requested(request_id: &str) -> Response {
    failure(
        StatusCode::CONFLICT,
        request_id,
        None,
        ErrorClass::Conflict,
        "workspace.metrics-not-requested",
        "The workspace was created without storage accounting.",
        Some("resource_id"),
        false,
    )
}
