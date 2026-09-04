use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use futures_util::{SinkExt as _, StreamExt as _};
use substrate_store::{
    ExecWrite, Scope, StoredExec, WorkspaceAdmission, WorkspaceObservationWrite,
};
use substrate_wire::{
    ErrorClass, ExecState, ExecUsage, MetricsObservation, MetricsQuery, MetricsResourceKind,
    MetricsStreamFrame, MetricsStreamQuery, Success,
};

use crate::runtime::TransportPermit;

use super::events::{
    ClientFrame, ControlRate, EventStreamPermit, classify_client_frame,
    enforce_event_stream_lifetime, enforce_stream_send_deadline, send_protocol_close,
};
use super::operations::{driver_failure, stored_exec};
use super::responses::{failure, not_found, request_id, schema_invalid, store_failure, success};
use super::{App, BODY_LIMIT, Identity};

/// The named refusal a metrics stream over the published capacity earns, in the vocabulary
/// `app/events.rs` already publishes for the same bound on the event stream.
pub(super) const METRICS_STREAM_CAPACITY: &str = "metrics.stream-capacity";

/// The close a client that sends a data frame earns — RFC 6455's "unacceptable data", the code
/// `app/events.rs:505` gives on the sibling stream. Named rather than written twice, because
/// `the_published_cap_and_refusal_are_the_ones_the_public_guide_states` derives the guide's
/// sentence from it.
pub(super) const METRICS_STREAM_DATA_CLOSE: u16 = 1003;

/// The close a client that outruns the control-frame budget earns — RFC 6455's "policy
/// violation", the code `app/events.rs:521` gives on the sibling stream.
pub(super) const METRICS_STREAM_CONTROL_RATE_CLOSE: u16 = 1008;

/// What a metrics stream may hold, in the shape `EventStreamPolicy` publishes: the same permit
/// bounds, the same client-frame ceiling, the same control-frame budget and the same lifetime.
/// Metrics streams were the one upgrade with none of them, and each open stream costs one
/// `observe_exec` plus one `put_exec` durable write per sample interval until the exec ends.
///
/// Every field is compared against its `EventStreamPolicy` original by
/// `metrics_stream_policy_publishes_the_event_stream_bounds` (`app/tests.rs`), so this docstring's
/// claim is a check rather than a promise.
#[derive(Clone, Copy)]
pub(super) struct MetricsStreamPolicy {
    pub(super) global_streams: usize,
    pub(super) streams_per_subject: usize,
    pub(super) max_input_bytes: usize,
    pub(super) max_output_bytes: usize,
    pub(super) write_buffer_bytes: usize,
    pub(super) max_controls_per_window: u32,
    pub(super) control_window: Duration,
    pub(super) send_timeout: Duration,
    pub(super) lifetime: Duration,
}

impl MetricsStreamPolicy {
    pub(super) const fn production() -> Self {
        Self {
            global_streams: 64,
            streams_per_subject: 4,
            max_input_bytes: 1_024,
            max_output_bytes: BODY_LIMIT,
            write_buffer_bytes: 16 * 1_024,
            max_controls_per_window: 120,
            control_window: Duration::from_mins(1),
            send_timeout: Duration::from_secs(5),
            lifetime: Duration::from_hours(1),
        }
    }
}

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
    transport: Option<Extension<TransportPermit>>,
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
    // The permit is taken before the first durable read, so a flood of upgrades cannot buy the
    // `observe_exec` and `put_exec` that a stream costs. Every refusal below drops it again.
    let Some(stream_permit) = app.metrics_stream_limits.acquire(&scope) else {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            &request_id,
            None,
            ErrorClass::Exhausted,
            METRICS_STREAM_CAPACITY,
            "The bounded metrics stream capacity is exhausted.",
            Some("stream"),
            true,
        );
    };
    match load_exec_usage(&app, &scope, &query.exec_id).await {
        Ok(_) => {
            let policy = app.metrics_stream_policy;
            // The transport admission this connection was accepted under, moved into the upgraded
            // task below. hyper resolves an upgradeable connection future when it hands the socket
            // over, so an admission left with the connection stops counting a socket that is still
            // serving. Absent when no listener published one — the crate's own tests drive this
            // route without a transport.
            let transport_admission = transport.map(|Extension(permit)| permit);
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
                    // Held for as long as this socket serves, so the transport budget counts it.
                    let _transport_admission = transport_admission;
                    let session =
                        run_stream(app, scope, query.exec_id, policy, stream_permit, socket);
                    let _completed = enforce_event_stream_lifetime(policy.lifetime, session).await;
                })
                .into_response()
        }
        Err(MetricsLoadError::NotFound) => not_found(&request_id),
        Err(MetricsLoadError::NotRequested) => metrics_not_requested(&request_id),
        Err(MetricsLoadError::Driver(error)) => driver_failure(&request_id, None, &error),
        Err(MetricsLoadError::Store(error)) => store_failure(&request_id, None, &error),
    }
}

async fn run_stream(
    app: Arc<App>,
    scope: Scope,
    exec_id: String,
    policy: MetricsStreamPolicy,
    _stream_permit: EventStreamPermit,
    mut socket: WebSocket,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(
        substrate_wire::RESOURCE_USAGE_SAMPLE_INTERVAL_MS,
    ));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // A `tokio` interval's first tick is immediate, and the contract's first sample is too
    // ("one immediate observation, then the latest sample approximately once per second",
    // `website/docs/guides/storage-and-metrics.md`). Spending it here leaves exactly one tick
    // between every pair of frames the loop sends.
    interval.tick().await;
    // The budget spans the stream, not one sample period: the inner loop below answers every
    // control frame that arrives while it waits, so without a counter carried across iterations
    // a client can spend the route's read-match-write for as long as it holds the permit.
    let mut control_rate = ControlRate::new();
    loop {
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
                policy.send_timeout,
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
        // One tick per sample, and only here. `Interval::tick` is cancel-safe, so a control
        // frame that wins this race consumes no tick: the client's traffic is answered without
        // moving the cadence the route advertises, and the next sample still lands on the
        // interval's own schedule. Every ending below is a named one, as `app/events.rs:499-527`
        // gives on the sibling stream — a client that hits a bound is told which.
        loop {
            tokio::select! {
                _ = interval.tick() => break,
                incoming = socket.next() => {
                    match incoming {
                        Some(Err(_)) | None => return,
                        Some(Ok(message)) => match classify_client_frame(&message) {
                            ClientFrame::Close => return,
                            ClientFrame::Data => {
                                let _ = send_protocol_close(
                                    &mut socket,
                                    METRICS_STREAM_DATA_CLOSE,
                                    "metrics streams accept control frames only",
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
                                        METRICS_STREAM_CONTROL_RATE_CLOSE,
                                        "metrics stream control-frame rate exceeded",
                                        policy.send_timeout,
                                    )
                                    .await;
                                    return;
                                }
                                if let Message::Ping(bytes) = message
                                    && enforce_stream_send_deadline(
                                        policy.send_timeout,
                                        socket.send(Message::Pong(bytes)),
                                    )
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        },
                    }
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

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::Extension;
    use chrono::Utc;
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;
    use hyper_util::service::TowerToHyperService;
    use serde_json::Value;
    use substrate_host::{
        DispatchOutcome, Driver, DriverError, ExecObservation, WorkspaceDestroyProgress,
    };
    use substrate_store::{ExecWrite, NewOperation, Reservation, Scope, Store, StoredExec};
    use substrate_wire::{
        CapabilitySnapshot, ConfinementRequest, Exec, ExecExit, ExecKind, ExecOutputQuery,
        ExecSignalInput, ExecStartInput, ExecState, ExecUsage, FileAbsence, FileObservation,
        FileReadQuery, FileReadResult, LeaseObservation, NetworkMode, OutputSlice, ResourceUsage,
        SandboxProfile, Workspace, WorkspaceCreateInput, WorkspaceKind, WorkspaceState,
    };
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;

    use super::{
        METRICS_STREAM_CAPACITY, METRICS_STREAM_CONTROL_RATE_CLOSE, METRICS_STREAM_DATA_CLOSE,
        MetricsStreamPolicy,
    };
    use crate::runtime::{TcpConnectionLimits, admitted_service};
    use crate::{App, Identity, router};

    const SUBJECT: &str = "local:1000";
    const DEPLOYMENT: &str = "dep_metrics_stream_test";
    const WORKSPACE_ID: &str = "ws_metrics_stream";
    const EXEC_ID: &str = "ex_metrics_stream";
    const HANDSHAKE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

    fn identity() -> Identity {
        Identity {
            subject: SUBJECT.to_owned(),
            actor: "metrics-stream-test".to_owned(),
            principal: None,
        }
    }

    fn scope() -> Scope {
        Scope {
            deployment: DEPLOYMENT.to_owned(),
            subject: SUBJECT.to_owned(),
        }
    }

    /// A driver that answers nothing. Every metrics sample in these cases is served from the
    /// store, because `load_exec_usage` reads a terminal exec's recorded usage without observing
    /// it again — so a driver call on this path is a bug, and this fake makes it a panic rather
    /// than a silent host read. It also keeps the host *implementation* out of a file that is not
    /// a composition root (`crates/substrate-daemon/tests/driver_port.rs`).
    struct NoDriver;

    #[async_trait::async_trait]
    impl Driver for NoDriver {
        fn machine(&self) -> CapabilitySnapshot {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn shutdown(&self) -> Result<(), DriverError> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        fn workspace_root_identity(&self, _id: &str) -> Result<String, DriverError> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn create_workspace(
            &self,
            _id: &str,
            _root_name: &str,
            _input: &WorkspaceCreateInput,
        ) -> DispatchOutcome<Workspace> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn observe_workspace(
            &self,
            _id: &str,
            _root_name: &str,
            _previous: &Workspace,
        ) -> Result<Workspace, DriverError> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn read_workspace_path(
            &self,
            _workspace_id: &str,
            _root_name: &str,
            _path: &str,
            _query: &FileReadQuery,
        ) -> Result<FileReadResult, DriverError> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn write_workspace_file(
            &self,
            _workspace_id: &str,
            _root_name: &str,
            _path: &str,
            _content: &[u8],
        ) -> Result<FileObservation, DriverError> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn delete_workspace_file(
            &self,
            _workspace_id: &str,
            _root_name: &str,
            _path: &str,
        ) -> Result<FileAbsence, DriverError> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn destroy_workspace(
            &self,
            _workspace_id: &str,
            _root_name: &str,
        ) -> Result<WorkspaceDestroyProgress, DriverError> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn start_exec(
            &self,
            _id: &str,
            _workspace_root_name: &str,
            _input: &ExecStartInput,
        ) -> DispatchOutcome<ExecObservation> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn observe_exec(&self, _id: &str) -> Result<ExecObservation, DriverError> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn output(
            &self,
            _id: &str,
            _query: &ExecOutputQuery,
        ) -> Result<OutputSlice, DriverError> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        async fn signal(
            &self,
            _id: &str,
            _input: &ExecSignalInput,
        ) -> Result<ExecObservation, DriverError> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        fn completed_execs(&self) -> Vec<ExecObservation> {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        fn set_exec_lease(&self, _id: &str, _lease: Option<LeaseObservation>) {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        fn acknowledge_exec(&self, _persisted: &ExecObservation) {
            unreachable!("the metrics stream fixture makes no driver call")
        }

        fn discard_superseded_exec(&self, _id: &str) {
            unreachable!("the metrics stream fixture makes no driver call")
        }
    }

    struct Harness {
        address: SocketAddr,
        server: JoinHandle<()>,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    impl Harness {
        async fn open() -> Self {
            let store = Arc::new(Store::open(":memory:").expect("state store"));
            let app = App::new(Arc::clone(&store), Arc::new(NoDriver), DEPLOYMENT);
            seed_streaming_exec(&store);

            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind metrics stream test server");
            let address = listener.local_addr().expect("test server address");
            // Admitted like the production TCP listener, because a fixture that serves an
            // upgradeable connection outside a transport budget does not resemble the thing it
            // stands in for: the route under test reads its admission off the request, and a
            // harness that published none would exercise the absent half of that and call it the
            // served one.
            let limits = TcpConnectionLimits::production();
            let server = tokio::spawn(async move {
                loop {
                    let Ok((stream, peer)) = listener.accept().await else {
                        return;
                    };
                    let Some(permit) = limits.acquire(peer.ip()) else {
                        // The production refusal, in the shape `accept_authorized` gives it: the
                        // stream is dropped unserved and the loop continues. It is unreached here
                        // — the suite's widest case holds five of this source's sixteen slots —
                        // and a harness that made it louder would be inventing a vocabulary no
                        // production reader shares.
                        continue;
                    };
                    let service = router(Arc::clone(&app)).layer(Extension(identity()));
                    tokio::spawn(async move {
                        let connection = http1::Builder::new()
                            .serve_connection(
                                TokioIo::new(stream),
                                TowerToHyperService::new(admitted_service(&permit, service)),
                            )
                            .with_upgrades();
                        let _result = connection.await;
                    });
                }
            });
            Self { address, server }
        }

        async fn stream(&self) -> Handshake {
            Handshake::open(
                self.address,
                &format!("/v1/metrics/stream?exec_id={EXEC_ID}"),
            )
            .await
        }
    }

    /// A terminal exec whose usage observation is *not* complete keeps `run_stream` sampling, so
    /// the upgraded socket stays open exactly as it does for a live exec — and it reaches that
    /// state through the durable store rather than through a driver.
    #[allow(clippy::too_many_lines)] // One durable fixture; splitting it hides the shape.
    fn seed_streaming_exec(store: &Arc<Store>) {
        let workspace = Workspace {
            id: WORKSPACE_ID.to_owned(),
            kind: WorkspaceKind::Workspace,
            labels: std::collections::BTreeMap::new(),
            observed_at: Utc::now(),
            state: WorkspaceState::Ready,
            storage: None,
            lease: None,
        };
        let workspace_operation = NewOperation {
            scope: scope(),
            operation: "01JMETRICSSTREAMSEEDWS".to_owned(),
            operation_kind: "workspace.create".to_owned(),
            request_hash: "8".repeat(64),
            accepted_at: Utc::now().to_rfc3339(),
            capability_snapshot: None,
            actor: "metrics-stream-test".to_owned(),
            principal: None,
            grant_ref: None,
            platform_principal: None,
            resource: Some(WORKSPACE_ID.to_owned()),
        };
        let root_name = "root_metrics_stream";
        assert_eq!(
            store
                .reserve_workspace_create(&workspace_operation, root_name, &workspace, None)
                .expect("reserve workspace fixture"),
            Reservation::Accepted
        );
        store
            .complete_workspace(
                &scope(),
                &workspace_operation.operation,
                &Utc::now().to_rfc3339(),
                201,
                root_name,
                &workspace,
            )
            .expect("complete workspace fixture");

        let exec = Exec {
            id: EXEC_ID.to_owned(),
            kind: ExecKind::Exec,
            workspace: WORKSPACE_ID.to_owned(),
            state: ExecState::Exited,
            observed_at: Utc::now(),
            requested: ConfinementRequest {
                capability_snapshot: "cap_metrics_stream".to_owned(),
                network: NetworkMode::None,
                aperture: None,
                profile: SandboxProfile::Workspace,
                required: true,
            },
            applied: None,
            exit: Some(ExecExit {
                code: Some(0),
                signal: None,
            }),
            usage: Some(ExecUsage::Observed(ResourceUsage {
                complete: false,
                observed_at: Utc::now(),
                wall_time_us: 1_000,
                cpu_time_us: 500,
                memory_current_bytes: Some(1_024),
                memory_peak_bytes: 2_048,
                processes_current: Some(1),
                processes_peak: 1,
                process_limit_hits: 0,
                memory_oom_kills: 0,
                io_read_bytes: 0,
                io_write_bytes: 0,
                scratch: None,
            })),
            lease: None,
            refusal: None,
        };
        let exec_operation = NewOperation {
            scope: scope(),
            operation: "01JMETRICSSTREAMSEEDEX".to_owned(),
            operation_kind: "exec.start".to_owned(),
            request_hash: "9".repeat(64),
            accepted_at: Utc::now().to_rfc3339(),
            capability_snapshot: Some("cap_metrics_stream".to_owned()),
            actor: "metrics-stream-test".to_owned(),
            principal: None,
            grant_ref: None,
            platform_principal: None,
            resource: Some(EXEC_ID.to_owned()),
        };
        let mut provisional = exec.clone();
        provisional.state = ExecState::Accepted;
        provisional.exit = None;
        assert_eq!(
            store
                .reserve_exec_start(
                    &exec_operation,
                    &StoredExec {
                        resource: provisional,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                        stdout_truncated: false,
                        stderr_truncated: false,
                        output_complete: false,
                        cgroup: None,
                        leader_pid: None,
                    },
                    None,
                    None,
                )
                .expect("reserve streaming exec fixture"),
            Reservation::Accepted
        );
        assert!(matches!(
            store
                .complete_exec(
                    &scope(),
                    &exec_operation.operation,
                    &Utc::now().to_rfc3339(),
                    201,
                    &exec,
                    b"",
                    &[],
                    false,
                    false,
                    true,
                    None,
                    None,
                )
                .expect("complete streaming exec fixture"),
            ExecWrite::PersistedExact(_)
        ));
    }

    struct Handshake {
        status: u16,
        body: Vec<u8>,
        /// Held, never read: an open metrics stream is exactly a peer that has not gone away.
        _stream: TcpStream,
    }

    impl Handshake {
        async fn open(address: SocketAddr, path: &str) -> Self {
            let mut stream = TcpStream::connect(address)
                .await
                .expect("connect metrics stream client");
            let request = format!(
                "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
            );
            stream
                .write_all(request.as_bytes())
                .await
                .expect("write metrics stream handshake");
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 16 * 1_024, "bounded handshake response");
                let mut byte = [0_u8; 1];
                stream
                    .read_exact(&mut byte)
                    .await
                    .expect("read metrics stream handshake");
                head.push(byte[0]);
            }
            let head = std::str::from_utf8(&head).expect("ASCII handshake response");
            let status = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse::<u16>().ok())
                .expect("HTTP handshake status");
            let length = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = vec![0_u8; length];
            if length > 0 {
                stream
                    .read_exact(&mut body)
                    .await
                    .expect("read refusal body");
            }
            Self {
                status,
                body,
                _stream: stream,
            }
        }

        fn refusal(&self) -> Value {
            serde_json::from_slice(&self.body).expect("refusal JSON")
        }
    }

    /// The acceptance: opening one more metrics stream than the published per-subject cap answers
    /// `429` with a named `exhausted` refusal, rather than the unbounded upgrade of finding 3.
    #[tokio::test(flavor = "multi_thread")]
    async fn one_metrics_stream_over_the_published_subject_cap_is_refused_as_exhausted() {
        let harness = Harness::open().await;
        let cap = MetricsStreamPolicy::production().streams_per_subject;

        let mut open = Vec::new();
        for index in 0..cap {
            let stream = harness.stream().await;
            assert_eq!(
                stream.status, 101,
                "metrics stream {index} inside the cap must upgrade"
            );
            open.push(stream);
        }

        let refused = harness.stream().await;
        assert_eq!(
            refused.status, 429,
            "one metrics stream over the published per-subject cap must be refused"
        );
        let refusal = refused.refusal();
        assert_eq!(refusal["error"]["code"], "metrics.stream-capacity");
        assert_eq!(refusal["error"]["class"], "exhausted");
        assert_eq!(refusal["error"]["retriable"], true);
        assert_eq!(refusal["error"]["address"], "stream");
    }

    /// The cap is a live bound, not a one-way ratchet: a stream that ends gives its permit back.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_closed_metrics_stream_returns_its_subject_permit() {
        let harness = Harness::open().await;
        let cap = MetricsStreamPolicy::production().streams_per_subject;

        let mut open = Vec::new();
        for _ in 0..cap {
            open.push(harness.stream().await);
        }
        assert_eq!(
            harness.stream().await.status,
            429,
            "the cap must be reached before a permit can be returned"
        );

        drop(open.pop().expect("one open metrics stream"));
        let mut status = 0;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            status = harness.stream().await.status;
            if status == 101 {
                break;
            }
        }
        assert_eq!(
            status, 101,
            "a closed metrics stream must return its subject permit"
        );
    }

    /// Every `.rs` file under the crate's `src/`, recursively — the walk
    /// `tests/session_refusal_literals.rs:39` uses.
    fn crate_sources() -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|value| value == "rs") {
                    found.push(path);
                }
            }
        }
        found.sort();
        found
    }

    /// `source` with `//` and `/* */` comments and `"…"`, `r#"…"#` and character literals
    /// replaced by spaces, so a mention of a bound in prose, in an assertion message or in a test
    /// fixture cannot satisfy a check that the bound is *set*. Byte-for-byte in length, so offsets
    /// still address the original file, and newline-preserving, so line numbers survive.
    ///
    /// A character literal is masked as well as a string, because this file compares against the
    /// double-quote byte and an unmasked one would open a string that never closes. An
    /// unterminated literal panics rather than silently swallowing the rest of the file: a
    /// desynced masker hides real code, and a structural check that reports a false *negative* is
    /// worse than one that stops.
    fn masked(source: &str, file: &str) -> String {
        const QUOTE: u8 = 0x22;
        const APOSTROPHE: u8 = 0x27;
        let bytes = source.as_bytes();
        let mut masked = vec![b' '; bytes.len()];
        let mut cursor = 0;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if byte == b'\n' {
                masked[cursor] = b'\n';
                cursor += 1;
            } else if byte == b'/' && bytes.get(cursor + 1) == Some(&b'/') {
                while cursor < bytes.len() && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
            } else if byte == b'/' && bytes.get(cursor + 1) == Some(&b'*') {
                let mut depth = 1_usize;
                cursor += 2;
                while cursor < bytes.len() && depth > 0 {
                    if bytes[cursor] == b'\n' {
                        masked[cursor] = b'\n';
                        cursor += 1;
                    } else if bytes[cursor] == b'/' && bytes.get(cursor + 1) == Some(&b'*') {
                        depth += 1;
                        cursor += 2;
                    } else if bytes[cursor] == b'*' && bytes.get(cursor + 1) == Some(&b'/') {
                        depth -= 1;
                        cursor += 2;
                    } else {
                        cursor += 1;
                    }
                }
            } else if let Some(hashes) = raw_string_hashes(bytes, cursor) {
                cursor += hashes + 2;
                loop {
                    assert!(
                        cursor < bytes.len(),
                        "{file}: an unterminated raw string is outside this masker"
                    );
                    if bytes[cursor] == b'\n' {
                        masked[cursor] = b'\n';
                    } else if bytes[cursor] == QUOTE
                        && bytes[cursor + 1..]
                            .iter()
                            .take(hashes)
                            .filter(|byte| **byte == b'#')
                            .count()
                            == hashes
                    {
                        cursor += hashes + 1;
                        break;
                    }
                    cursor += 1;
                }
            } else if byte == QUOTE {
                cursor += 1;
                loop {
                    assert!(
                        cursor < bytes.len(),
                        "{file}: an unterminated string is outside this masker"
                    );
                    if bytes[cursor] == QUOTE {
                        cursor += 1;
                        break;
                    }
                    if bytes[cursor] == b'\n' {
                        masked[cursor] = b'\n';
                    }
                    cursor += usize::from(bytes[cursor] == b'\\') + 1;
                }
            } else if byte == APOSTROPHE && character_literal_end(bytes, cursor).is_some() {
                cursor = character_literal_end(bytes, cursor).expect("just measured") + 1;
            } else {
                masked[cursor] = byte;
                cursor += 1;
            }
        }
        String::from_utf8(masked).expect("masking replaces whole ASCII delimiters only")
    }

    /// The `#` count when `start` opens a raw string — `r"`, `r#"`, `r##"` — and `None` otherwise,
    /// including for the raw *identifier* `r#type` and for the `r` at the end of a word.
    fn raw_string_hashes(code: &[u8], start: usize) -> Option<usize> {
        if code[start] != b'r' {
            return None;
        }
        if start > 0 && (code[start - 1].is_ascii_alphanumeric() || code[start - 1] == b'_') {
            return None;
        }
        let hashes = code[start + 1..]
            .iter()
            .take_while(|byte| **byte == b'#')
            .count();
        (code.get(start + 1 + hashes) == Some(&0x22)).then_some(hashes)
    }

    /// The index of the closing quote when `start` opens a character literal, and `None` when it
    /// opens a lifetime. `'a'` closes two bytes on; an escape such as `'\n'` or `'\u{27}'` closes
    /// at the first unescaped quote within the literal's maximum length. A lifetime — `'a,`,
    /// `'static` — has neither.
    fn character_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
        const APOSTROPHE: u8 = 0x27;
        if bytes.get(start + 1) == Some(&b'\\') {
            return (start + 2..(start + 12).min(bytes.len()))
                .find(|index| bytes[*index] == APOSTROPHE);
        }
        (bytes.get(start + 2) == Some(&APOSTROPHE)).then_some(start + 2)
    }

    /// The field names declared by `struct <name>` in `source`, in order.
    fn policy_fields(source: &str, name: &str) -> Vec<String> {
        let head = source
            .find(&format!("struct {name} {{"))
            .unwrap_or_else(|| panic!("{name} is declared"));
        let open = head + source[head..].find('{').expect("struct body");
        let close = open + source[open..].find('}').expect("struct body ends");
        source[open + 1..close]
            .lines()
            .filter_map(|line| line.split(':').next())
            .map(|name| name.trim().trim_start_matches("pub(super) ").trim())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The class this round's blocker belongs to: a bound `EventStreamPolicy` declares and
    /// `MetricsStreamPolicy` simply does not have.
    ///
    /// `max_controls_per_window` and `control_window` were on the event policy from the start and
    /// absent from the metrics one, so the metrics stream answered client control frames with no
    /// budget at all. Nothing saw it, because
    /// `metrics_stream_policy_publishes_the_event_stream_bounds` (`app/tests.rs`) compares the
    /// fields the two policies *share* — a field missing from one side is a comparison nobody
    /// writes. This reads both declarations instead, so the next bound added to the event stream
    /// forces an answer here: restate it, or name it and say why it does not apply.
    #[test]
    fn every_event_stream_bound_is_restated_or_named_as_inapplicable() {
        /// Fields of `EventStreamPolicy` the metrics stream deliberately does not restate, each
        /// with the reason it cannot apply.
        const NOT_RESTATED: &[(&str, &str)] = &[
            (
                "max_catch_up_pages",
                "the metrics stream has no cursor and no catch-up: it samples current usage and \
                 publishes no history (website/docs/guides/storage-and-metrics.md)",
            ),
            (
                "max_page_items",
                "the metrics stream sends one usage frame per sample, never a page",
            ),
        ];
        let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
        let events = std::fs::read_to_string(app.join("events.rs")).expect("events module");
        let metrics = std::fs::read_to_string(app.join("metrics.rs")).expect("metrics module");
        let events = policy_fields(&masked(&events, "events.rs"), "EventStreamPolicy");
        let metrics = policy_fields(&masked(&metrics, "metrics.rs"), "MetricsStreamPolicy");

        assert!(events.len() >= 9, "read {events:?} from EventStreamPolicy");
        for (field, reason) in NOT_RESTATED {
            assert!(
                events.contains(&(*field).to_owned()),
                "EventStreamPolicy no longer declares {field}; drop it from NOT_RESTATED with \
                 its reason ({reason})"
            );
        }
        for field in &events {
            assert!(
                metrics.contains(field) || NOT_RESTATED.iter().any(|(named, _)| named == field),
                "EventStreamPolicy bounds the event stream with {field} and MetricsStreamPolicy \
                 does not restate it. Restate it and enforce it in run_stream, or add it to \
                 NOT_RESTATED with the reason it cannot apply — an absent bound is the defect \
                 that let the metrics stream answer control frames without a budget."
            );
        }
    }

    /// Spelled in pieces so the scan does not find its own needle; masking hides it too, and
    /// neither is relied on alone.
    const UPGRADE: &str = concat!(".on_", "upgrade", "(");

    /// The index of the bracket matching the closer at `at`, scanning left. `code` must be masked,
    /// so every bracket it sees is a real one.
    fn opening_bracket(code: &[u8], at: usize) -> Option<usize> {
        let mut depth = 0_usize;
        let mut cursor = at;
        loop {
            match code[cursor] {
                b')' | b']' | b'}' => depth += 1,
                b'(' | b'[' | b'{' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(cursor);
                    }
                }
                _ => {}
            }
            cursor = cursor.checked_sub(1)?;
        }
    }

    /// The index of the bracket matching the opener at `at`, scanning right.
    fn closing_bracket(code: &[u8], at: usize) -> Option<usize> {
        let mut depth = 0_usize;
        for (cursor, byte) in code.iter().enumerate().skip(at) {
            match byte {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(cursor);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The method names of the call chain whose last link starts at `index`, nearest first.
    ///
    /// Walks left over `receiver.method(args)` links, skipping each argument list as one balanced
    /// bracket group, so an argument of any length — a 24-line closure, say — is one step rather
    /// than a distance. This is what a byte or line window around the call cannot do, and what
    /// made the check accuse a correctly bounded `sessions.rs` on the integration branch.
    fn receiver_chain(code: &str, index: usize) -> Vec<String> {
        let bytes = code.as_bytes();
        let mut names = Vec::new();
        let mut cursor = index;
        loop {
            while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
                cursor -= 1;
            }
            if cursor == 0 || bytes[cursor - 1] != b')' {
                return names;
            }
            let Some(open) = opening_bracket(bytes, cursor - 1) else {
                return names;
            };
            cursor = open;
            let name_end = cursor;
            while cursor > 0
                && (bytes[cursor - 1].is_ascii_alphanumeric() || bytes[cursor - 1] == b'_')
            {
                cursor -= 1;
            }
            if cursor == name_end {
                return names;
            }
            names.push(code[cursor..name_end].to_owned());
            while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
                cursor -= 1;
            }
            if cursor == 0 || bytes[cursor - 1] != b'.' {
                return names;
            }
            cursor -= 1;
        }
    }

    /// What the upgrade whose `.on_upgrade(` begins at `index` declares, or the first bound it
    /// does not. `code` must be masked.
    fn upgrade_bounds(code: &str, index: usize) -> Result<(), String> {
        let chain = receiver_chain(code, index);
        for bound in ["max_frame_size", "max_message_size"] {
            if !chain.iter().any(|method| method == bound) {
                return Err(format!(
                    "the upgrade at byte {index} runs on the library's default {bound}; its \
                     builder chain is {chain:?}"
                ));
            }
        }
        let open = index + UPGRADE.len() - 1;
        let Some(close) = closing_bracket(code.as_bytes(), open) else {
            return Err(format!("the upgrade at byte {index} has no argument list"));
        };
        if !code[open..=close].contains("policy.lifetime") {
            return Err(format!("the upgrade at byte {index} runs with no lifetime"));
        }
        Ok(())
    }

    /// Finding 3's class, not its instance: an upgrade that runs on the library's default frame
    /// and message bounds with no lifetime. The metrics stream was the one instance; this reads
    /// every upgrade the crate serves, so a fourth route cannot be added without its bounds.
    ///
    /// Three properties, each an answer to a way an earlier version of this check could be
    /// satisfied without the bound being set:
    ///
    /// 1. It walks `src/` **recursively**, not one `read_dir` of `src/app/`, so `src/runtime.rs`,
    ///    `src/hosted.rs` and `src/tls.rs` are inside it rather than beside it.
    /// 2. It reads the **masked** source and scopes to the **receiver chain** — the method calls
    ///    the upgrade is chained onto, each argument list skipped as one balanced group. A
    ///    narrated `.max_frame_size(` is masked away, one belonging to the statement above is not
    ///    on the chain, and an argument of any length between the bounds and the call is one step
    ///    rather than a distance.
    /// 3. It fails a raw `hyper::upgrade::on`, which takes the upgrade past the builder that
    ///    carries the bounds entirely.
    ///
    /// **This case reads files no other case in this unit touches**, including
    /// `app/sessions.rs` and `app/events.rs`. That is deliberate — a class check that reads only
    /// its own file checks nothing — but it is a coupling: a change to how *those* routes spell
    /// their upgrade can fail a test declared in this one. It has happened once, on the
    /// integration branch of the 2026-09-04 wave, when a `.on_failed_upgrade(…)` with a 24-line
    /// closure was added between `sessions.rs`'s bounds and its `.on_upgrade(`. The answer was to
    /// make the matcher exact, not to stop reading the file.
    ///
    /// **Two limits, as rules.** It proves the bounds are *declared*, never that they are the
    /// published values — what observes one firing is
    /// `an_oversized_client_frame_ends_the_metrics_stream_and_returns_its_permit`
    /// (`crates/substrate-daemon/tests/metrics_stream_adversary.rs`), and a route added without
    /// that partner case is bounded on paper only. And bracket matching assumes the brackets in
    /// `src/` balance; a macro invocation carrying unbalanced token trees between a bound and an
    /// upgrade would desync it. There is none in this crate, and one would have to be written on
    /// purpose in exactly that place.
    #[test]
    fn every_websocket_upgrade_declares_its_frame_message_and_lifetime_bounds() {
        const RAW_UPGRADE: &str = concat!("upgrade", "::on(");
        let mut checked = 0_usize;
        for path in crate_sources() {
            let file = path.display().to_string();
            let source = std::fs::read_to_string(&path).expect("crate source");
            let code = masked(&source, &file);
            assert!(
                !code.contains(RAW_UPGRADE),
                "{file}: an upgrade taken outside the bounded builder has no frame, message or \
                 lifetime bound at all"
            );
            let mut from = 0;
            while let Some(offset) = code[from..].find(UPGRADE) {
                let index = from + offset;
                if let Err(fault) = upgrade_bounds(&code, index) {
                    panic!("{file}: {fault}");
                }
                checked += 1;
                from = index + UPGRADE.len();
            }
        }
        assert!(
            checked >= 3,
            "the events, sessions and metrics upgrades must all be read, saw {checked}"
        );
    }

    /// The statement every upgraded task holds the connection's transport admission with.
    ///
    /// One spelling for one thing, so the class check below is a whole-crate rule rather than a
    /// list of routes somebody has to remember to extend — and the whole binding rather than the
    /// identifier, because matching the identifier alone passed `drop(transport_admission);`,
    /// which releases the slot at the handshake and is exactly the defect. Matched against the
    /// argument list with its whitespace normalised, so a wrapped line is not a failure.
    const TRANSPORT_ADMISSION: &str =
        concat!("let _transport", "_admission = transport", "_admission;");

    /// Finding 4's class, not its instance: an upgraded socket that serves outside the transport
    /// budget that let its connection in.
    ///
    /// hyper resolves an upgradeable connection future when it hands the socket to the upgrade
    /// (`crates/substrate-daemon/src/runtime.rs`, the three `.with_upgrades()` listeners), so an
    /// admission left with the connection is released at the handshake and the socket that is
    /// still serving stops being counted — 128 global and 32 per uid on unix, 128 and 16 per
    /// source over TCP and TLS. The remedy is one line in every upgraded task: move the
    /// connection's `TransportPermit` in, so the slot returns when the socket ends rather than
    /// when the handshake succeeds.
    ///
    /// The instance was the per-uid budget, observed through the unix listener by
    /// `runtime::tests::an_upgraded_websocket_keeps_its_per_uid_connection_permit`. This reads
    /// every upgrade the crate serves, on the same recursive walk of `src/` and the same masked
    /// source as its sibling above, so a fourth route cannot be added without one.
    ///
    /// **What it checks is a spelling convention, and that cuts both ways.** It requires the
    /// statement `let _transport_admission = transport_admission;` inside the `.on_upgrade(`
    /// argument list, and it is worth being exact about what that does and does not establish:
    ///
    /// 1. It proves the admission is *bound for the task's life* — a binding, not a mention, so
    ///    `drop(transport_admission);` fails it. That matters most where no case can catch the
    ///    difference: `app/sessions.rs`'s attach route holds the admission on a path no in-crate
    ///    admitted listener drives, so this check is the whole of what stands behind that line.
    /// 2. It does not prove the value bound is a live permit. What observes one holding a slot end
    ///    to end is `runtime::tests::an_upgraded_websocket_keeps_its_per_uid_connection_permit` on
    ///    the unix listener and `app::tests::upgraded_transport_slot` on a TCP one, both over the
    ///    event stream; a route added without a partner case is counted on paper only.
    /// 3. It **fails correct code** that holds the admission some other way — under another name,
    ///    or inside a tuple or struct built above the chain. The convention is the check: one
    ///    statement for the thing, spelled inside the task that holds it. A route with a reason to
    ///    hold it differently has a reason to change this check with it, and that is the
    ///    conversation this failure buys.
    ///
    /// A structural check — the value's *type* traced into the task — needs a parser this crate
    /// does not have and would not gain for one rule.
    #[test]
    fn every_websocket_upgrade_keeps_its_transport_admission() {
        let mut checked = 0_usize;
        for path in crate_sources() {
            let file = path.display().to_string();
            let source = std::fs::read_to_string(&path).expect("crate source");
            let code = masked(&source, &file);
            let mut from = 0;
            while let Some(offset) = code[from..].find(UPGRADE) {
                let index = from + offset;
                let open = index + UPGRADE.len() - 1;
                let close = closing_bracket(code.as_bytes(), open).unwrap_or_else(|| {
                    panic!("{file}: the upgrade at byte {index} has no argument list")
                });
                let held = code[open..=close]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                assert!(
                    held.contains(TRANSPORT_ADMISSION),
                    "{file}: the upgraded task at byte {index} does not take the connection's \
                     transport admission with it, so the transport budget stops counting the \
                     socket at the handshake"
                );
                checked += 1;
                from = index + UPGRADE.len();
            }
        }
        assert!(
            checked >= 3,
            "the events, sessions and metrics upgrades must all be read, saw {checked}"
        );
    }

    /// The other half of finding 4's class: a listener that serves an upgradeable connection
    /// without publishing the admission that connection was accepted under.
    ///
    /// The sibling above reads every upgraded task and asserts it keeps the admission. It cannot
    /// see whether one was ever handed over, and an absent one is silent: the route extracts
    /// `Option<Extension<TransportPermit>>`, because the crate's own tests drive these routes
    /// without a transport, so a listener that dropped the layer would go on serving and simply
    /// stop counting. Invariant 3 does not allow a guarantee to go missing quietly, and this is
    /// what makes it loud — at `cargo test`, for the two listeners no case drives end to end.
    ///
    /// It reads each `.with_upgrades()` in `src/`, walks back to the `.serve_connection(` it is
    /// chained onto, and asserts that call serves `admitted_service(…)`
    /// (`crates/substrate-daemon/src/runtime.rs`) rather than a bare service. Today that set is
    /// six, and the check asserts the whole of it *by file* rather than as a floor on the total:
    /// four in `runtime.rs` — the unix, TCP and TLS listeners and the unix fixture in its own
    /// tests — one in `app/metrics.rs`, the metrics stream harness below, and one in
    /// `app/tests.rs`, the admitted TCP listener in `upgraded_transport_slot`. A floor is what
    /// this had, and a floor of three over a set of six is satisfied by reading the three fixtures
    /// and no production listener at all.
    ///
    /// The behavioural partners are
    /// `runtime::tests::an_upgraded_websocket_keeps_its_per_uid_connection_permit` on the unix
    /// listener and `app::tests::upgraded_transport_slot` on a production-shaped TCP one, and both
    /// drive the event stream. **The TLS listener has neither, and neither does any route but the
    /// event stream**: the harness below upgrades the metrics stream through an admitted listener
    /// but observes no slot, and `app/sessions.rs`'s attach route is driven through an admitted
    /// listener by nothing at all. What stands behind those is this check and the sibling above —
    /// which is why the sibling matches the whole hold statement rather than a mention of it.
    ///
    /// **`src/` is the whole of its reach, and three listeners live outside it.**
    /// `tests/websocket.rs:110-111`, `tests/metrics_stream_adversary.rs:94-95` and
    /// `tests/pipe_session.rs:662-664` each serve the same production `router` with
    /// `.with_upgrades()` and publish no admission, and nothing here reads them. The reason is not
    /// that they matter less — the argument for admitting the harness below applies to them word
    /// for word — but that `TransportPermit`, `TcpConnectionLimits` and `admitted_service` are
    /// `pub(crate)`, so an integration test cannot reach them: including those three means making
    /// the transport's admission surface public API for the benefit of tests, on a crate whose
    /// library deliberately exposes configuration and nothing else. `upgraded_transport_slot`
    /// narrows that gap rather than closing it: it drives an admitted listener over the same
    /// router, but over one route of the three those files drive bare — the event stream —
    /// and `tests/pipe_session.rs`'s attach route has no in-crate admitted listener at all.
    /// Closing it means either that public surface, or a pipe-session driver double inside the
    /// crate. Widen this walk on the day one of those exists for a reason of its own, and not
    /// before.
    ///
    /// **One limit, as a rule.** It requires the two calls to be adjacent on the chain, so a step
    /// inserted between them fails it. That is the sibling's lesson taken deliberately the other
    /// way: this check reports what it read, and a chain it cannot follow is a chain a reader
    /// cannot follow either.
    #[test]
    fn every_upgradeable_connection_publishes_its_transport_admission() {
        const UPGRADES: &str = concat!(".with_", "upgrades()");
        const SERVE: &str = concat!(".serve_", "connection(");
        const ADMITTED: &str = concat!("admitted_", "service(");
        // The enumeration this docstring gives, as the set the walk must read — by file, because
        // a floor on the total is satisfied by the three fixtures alone, and it is the three
        // production listeners in `runtime.rs` that nothing else holds. A legitimate change to
        // the set changes this map and the sentence above together.
        let expected = std::collections::BTreeMap::from([
            ("app/metrics.rs", 1_usize),
            ("app/tests.rs", 1),
            ("runtime.rs", 4),
        ]);
        let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut read = std::collections::BTreeMap::<&str, usize>::new();
        let sources = crate_sources();
        let relative = sources
            .iter()
            .map(|path| {
                path.strip_prefix(&source_root)
                    .expect("every crate source is under src/")
                    .to_str()
                    .expect("UTF-8 crate source path")
            })
            .collect::<Vec<_>>();
        for (path, name) in sources.iter().zip(relative) {
            let file = path.display().to_string();
            let source = std::fs::read_to_string(path).expect("crate source");
            let code = masked(&source, &file);
            let mut from = 0;
            while let Some(offset) = code[from..].find(UPGRADES) {
                let index = from + offset;
                let served = code[..index]
                    .rfind(SERVE)
                    .unwrap_or_else(|| panic!("{file}: the upgradeable connection at byte {index} is not served by this crate"));
                let open = served + SERVE.len() - 1;
                let close = closing_bracket(code.as_bytes(), open).unwrap_or_else(|| {
                    panic!("{file}: the connection served at byte {served} has no argument list")
                });
                assert!(
                    close < index && code[close + 1..index].trim().is_empty(),
                    "{file}: the upgradeable connection at byte {index} is not chained onto the \
                     connection served at byte {served}; this check reads the two as one chain"
                );
                assert!(
                    code[open..=close].contains(ADMITTED),
                    "{file}: the listener at byte {served} serves an upgradeable connection \
                     without publishing the transport admission it was accepted under, so nothing \
                     an upgrade produces can keep it and the budget stops counting at the handshake"
                );
                *read.entry(name).or_default() += 1;
                from = index + UPGRADES.len();
            }
        }
        assert_eq!(
            read, expected,
            "the upgradeable listeners under src/ are not the ones this check documents; a walk \
             that read only the fixtures would satisfy a floor on the total while reading no \
             production listener at all, so the set is asserted by file. If the change is \
             deliberate, move the enumeration in this docstring with it"
        );
    }

    /// The shape that made the check accuse a correctly bounded file: bounds set, then a long
    /// closure argument, then the upgrade. Reduced from `app/sessions.rs:1281-1314` as it stands
    /// on the integration branch, and kept here so the false positive cannot come back by a
    /// different route than the one it took.
    const BOUNDED_THROUGH_A_LONG_ARGUMENT: &str = r#"
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
            tokio::spawn(async move {
                if terminate_pipe_session(&app, &scope, &exec).await {
                    tracing::info!(exec = %exec, %error, "terminated a claimed session");
                } else {
                    // A brace-heavy body, because braces are what a statement-scoped
                    // matcher mistook for the start of the chain: } { } {
                    tracing::warn!(exec = %exec, %error, "could not terminate");
                }
            });
        })
        .on_upgrade(move |socket| async move {
            let completed = tokio::time::timeout(policy.lifetime, run(socket)).await;
            let _ = completed;
        })
"#;

    /// The same chain with its frame bound removed, and nothing else changed. Spelled `r"` where
    /// its partner above is `r#"`, so the masker's zero-hash and hashed raw-string paths are both
    /// walked by the case that depends on them.
    const UNBOUNDED_THROUGH_A_LONG_ARGUMENT: &str = r"
    ws.read_buffer_size(policy.max_message_bytes)
        .write_buffer_size(policy.write_buffer_bytes)
        .max_message_size(policy.max_message_bytes)
        .on_failed_upgrade(move |error: axum::Error| {
            tokio::spawn(async move { let _ = error; });
        })
        .on_upgrade(move |socket| async move {
            let completed = tokio::time::timeout(policy.lifetime, run(socket)).await;
            let _ = completed;
        })
";

    /// A bound is on the chain or it is not; the distance from it to the call is not evidence
    /// either way.
    ///
    /// The first fixture is the shape that failed `bash scripts/gate.sh` on
    /// `wave/security-2026-09-04` while being correctly bounded. The second is the same chain
    /// with the frame bound taken away, so a matcher that passed the first by giving up rather
    /// than by reading the chain does not pass this case.
    #[test]
    fn a_long_argument_between_a_bound_and_its_upgrade_hides_neither() {
        for (label, fixture, expected) in [
            ("bounded", BOUNDED_THROUGH_A_LONG_ARGUMENT, true),
            ("unbounded", UNBOUNDED_THROUGH_A_LONG_ARGUMENT, false),
        ] {
            let code = masked(fixture, label);
            let index = code.find(UPGRADE).expect("the fixture upgrades");
            assert_eq!(
                upgrade_bounds(&code, index).is_ok(),
                expected,
                "{label}: {:?}",
                upgrade_bounds(&code, index)
            );
            assert!(
                receiver_chain(&code, index).contains(&"max_message_size".to_owned()),
                "{label}: the chain must be read through the closure argument, not around it"
            );
        }
    }

    /// "One more than a **published** per-subject cap" — and the cap, the refusal code and the
    /// bounds beside them live in daemon source, where no client can read them.
    ///
    /// `website/docs/guides/storage-and-metrics.md` now states all four, and this is the tie:
    /// every number in that paragraph is derived here from `MetricsStreamPolicy::production()` and
    /// `METRICS_STREAM_CAPACITY`, so moving a bound without moving the guide fails. Whitespace is
    /// normalised first, so re-wrapping the paragraph is not a failure — changing what it says is.
    ///
    /// **The limit, as a rule:** the guide is the only public statement of these bounds. They are
    /// deliberately *not* in a contract bundle: `contracts/substrate-wire/0.15.0/refusals.json` is
    /// `b10x.substrate-session-refusals.v1` and carries session codes only, and invariant 6 freezes
    /// every released bundle, so naming a metrics refusal there is a successor bundle and an ADR,
    /// not a line. A client that needs the cap machine-readably still has nowhere to read it.
    #[test]
    fn the_published_cap_and_refusal_are_the_ones_the_public_guide_states() {
        let guide = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../website/docs/guides/storage-and-metrics.md");
        let text = std::fs::read_to_string(&guide).expect("public metrics guide");
        let prose = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let policy = MetricsStreamPolicy::production();
        assert_eq!(
            policy.control_window,
            Duration::from_mins(1),
            "the guide states the control budget per minute; the policy must be a minute"
        );
        for statement in [
            format!(
                "hold {} metrics streams at once and a deployment {};",
                policy.streams_per_subject, policy.global_streams
            ),
            format!("refused `429` with the code `{METRICS_STREAM_CAPACITY}`"),
            format!(
                "send {} control frames a minute, and the one after that closes the stream \
                 `{METRICS_STREAM_CONTROL_RATE_CLOSE}`",
                policy.max_controls_per_window
            ),
            format!("a data frame closes it `{METRICS_STREAM_DATA_CLOSE}`"),
            format!("cut after {} hour", policy.lifetime.as_secs() / 3_600),
            format!(
                "client frame larger than {} bytes is refused by the socket",
                policy.max_input_bytes
            ),
        ] {
            assert!(
                prose.contains(&statement),
                "{} does not state \"{statement}\"; a bound a client cannot read is not published",
                guide.display()
            );
        }
    }

    /// The cadence the loop paces on is a value five released bundles also state, and nothing tied
    /// them together.
    ///
    /// `run_stream` paces on `substrate_wire::RESOURCE_USAGE_SAMPLE_INTERVAL_MS`
    /// (`crates/substrate-wire/src/lib.rs:37`), and the host probe publishes that same constant as
    /// the machine fact `metrics.stream.sample_interval_ms`
    /// (`crates/substrate-host/src/probe.rs:153`). Every released bundle from `0.11.0` on makes
    /// `metrics.stream` callable only where that fact equals a literal `1000`
    /// (`contracts/substrate-wire/0.15.0/operations.json:936`), and those bytes are frozen by
    /// invariant 6. No Rust file names `sample_interval_ms` beside them, so moving the constant
    /// would leave the daemon advertising a fact its own released contract refuses — silently.
    /// This is that tie, and it is the class the sampling defect belongs to: a paced value the
    /// daemon and a frozen bundle both state, with nothing comparing them.
    #[test]
    fn the_advertised_sample_interval_is_the_one_every_released_bundle_requires() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/substrate-wire");
        let mut checked = Vec::new();
        for entry in std::fs::read_dir(&root).expect("released bundle root") {
            let bundle = entry.expect("bundle directory").path();
            let operations = bundle.join("operations.json");
            if !operations.is_file() {
                continue;
            }
            let document: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&operations).expect("operations.json"))
                    .expect("operations JSON");
            let version = bundle
                .file_name()
                .expect("bundle version")
                .to_string_lossy()
                .into_owned();
            for operation in document
                .get("operations")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                for predicate in operation
                    .get("capability_predicates")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(interval) = predicate
                        .get("value")
                        .and_then(|value| value.get("sample_interval_ms"))
                        .and_then(serde_json::Value::as_u64)
                    else {
                        continue;
                    };
                    assert_eq!(
                        interval,
                        substrate_wire::RESOURCE_USAGE_SAMPLE_INTERVAL_MS,
                        "released bundle {version} makes {} callable only at a \
                         sample_interval_ms of {interval}, and the daemon paces on {}",
                        operation
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("<unnamed>"),
                        substrate_wire::RESOURCE_USAGE_SAMPLE_INTERVAL_MS
                    );
                    checked.push(version.clone());
                }
            }
        }
        checked.sort();
        checked.dedup();
        assert!(
            checked.len() >= 6,
            "0.11.0 through 0.16.0 each gate metrics.stream on the sample interval; read {checked:?}"
        );
    }
}
