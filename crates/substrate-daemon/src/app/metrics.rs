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

use super::events::EventStreamPermit;
use super::operations::{driver_failure, stored_exec};
use super::responses::{failure, not_found, request_id, schema_invalid, store_failure, success};
use super::{App, BODY_LIMIT, Identity};

/// The named refusal a metrics stream over the published capacity earns, in the vocabulary
/// `app/events.rs` already publishes for the same bound on the event stream.
pub(super) const METRICS_STREAM_CAPACITY: &str = "metrics.stream-capacity";

/// What a metrics stream may hold, in the shape `EventStreamPolicy` publishes: the same permit
/// bounds, the same client-frame ceiling and the same lifetime. Metrics streams were the one
/// upgrade with none of them, and each open stream costs one `observe_exec` plus one `put_exec`
/// durable write per sample interval until the exec ends.
#[derive(Clone, Copy)]
pub(super) struct MetricsStreamPolicy {
    pub(super) global_streams: usize,
    pub(super) streams_per_subject: usize,
    max_input_bytes: usize,
    max_output_bytes: usize,
    write_buffer_bytes: usize,
    send_timeout: Duration,
    lifetime: Duration,
}

impl MetricsStreamPolicy {
    pub(super) const fn production() -> Self {
        Self {
            global_streams: 64,
            streams_per_subject: 4,
            max_input_bytes: 1_024,
            max_output_bytes: BODY_LIMIT,
            write_buffer_bytes: 16 * 1_024,
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
                    let session =
                        run_stream(app, scope, query.exec_id, policy, stream_permit, socket);
                    let _completed = tokio::time::timeout(policy.lifetime, session).await;
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

    use super::super::events::EventStreamPolicy;
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
            let server = tokio::spawn(async move {
                loop {
                    let Ok((stream, _peer)) = listener.accept().await else {
                        return;
                    };
                    let service = router(Arc::clone(&app)).layer(Extension(identity()));
                    tokio::spawn(async move {
                        let connection = http1::Builder::new()
                            .serve_connection(
                                TokioIo::new(stream),
                                TowerToHyperService::new(service),
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
        let cap = EventStreamPolicy::production().streams_per_subject;

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
        let cap = EventStreamPolicy::production().streams_per_subject;

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

    /// Finding 3's class, not its instance: an upgrade that runs on the library's default frame
    /// and message bounds with no lifetime. The metrics stream was the one instance; this reads
    /// every upgrade the app serves, so a fourth route cannot be added without its bounds.
    #[test]
    fn every_websocket_upgrade_declares_its_frame_message_and_lifetime_bounds() {
        // Spelled in pieces so the scanner does not find its own needle in this file.
        const UPGRADE: &str = concat!(".on_", "upgrade", "(");
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
        let mut checked = 0_usize;
        for entry in std::fs::read_dir(&directory).expect("app module directory") {
            let path = entry.expect("app module entry").path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("app module source");
            let mut from = 0;
            while let Some(offset) = source[from..].find(UPGRADE) {
                let index = from + offset;
                let mut start = index.saturating_sub(800);
                while !source.is_char_boundary(start) {
                    start -= 1;
                }
                let mut end = index.saturating_add(1_200).min(source.len());
                while !source.is_char_boundary(end) {
                    end -= 1;
                }
                let declared = &source[start..index];
                let body = &source[index..end];
                let file = path.display();
                assert!(
                    declared.contains(".max_frame_size("),
                    "{file}: the upgrade at byte {index} runs on the library's default frame bound"
                );
                assert!(
                    declared.contains(".max_message_size("),
                    "{file}: the upgrade at byte {index} runs on the library's default message bound"
                );
                assert!(
                    body.contains("policy.lifetime"),
                    "{file}: the upgrade at byte {index} runs with no lifetime"
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
}
