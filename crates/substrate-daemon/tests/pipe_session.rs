#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use base64::Engine as _;
use chrono::Utc;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use serde_json::{Value, json};
use substrate_daemon::{App, Identity, router};
use substrate_host::{
    DispatchOutcome, Driver, DriverError, ExecObservation, HostConfig, HostDriver, PipeFrame,
    PipeStream, WorkspaceDestroyProgress,
};
use substrate_wire::{
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, CapabilitySnapshot, Exec,
    ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState, FileAbsence, FileObservation,
    FileReadQuery, FileReadResult, HostDriverKind, LeaseObservation, OutputSlice, PipeServerFrame,
    PipeSessionStartInput, SandboxProfile, Workspace, WorkspaceCreateInput,
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use tokio::task::JoinHandle;
use tower::ServiceExt as _;

const SUBJECT: &str = "local:1000";
const DEPLOYMENT: &str = "dep_pipe_session_test";
const SNAPSHOT: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HANDSHAKE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

struct FixturePipe {
    observation: ExecObservation,
    sender: Option<mpsc::Sender<PipeFrame>>,
    receiver: Arc<AsyncMutex<mpsc::Receiver<PipeFrame>>>,
}

struct PipeFixtureDriver {
    host: Arc<HostDriver>,
    pipes: Mutex<HashMap<String, FixturePipe>>,
}

impl PipeFixtureDriver {
    fn open(root: &std::path::Path) -> Arc<Self> {
        Arc::new(Self {
            host: HostDriver::open(HostConfig::minimum(root)).expect("host fixture driver"),
            pipes: Mutex::new(HashMap::new()),
        })
    }

    fn terminal(&self, id: &str, state: ExecState, signal: Option<substrate_wire::Signal>) {
        let mut pipes = self.pipes.lock().expect("pipe fixture lock");
        let pipe = pipes.get_mut(id).expect("known pipe fixture");
        pipe.sender.take();
        pipe.observation.output_complete = true;
        pipe.observation.resource.state = state;
        pipe.observation.resource.observed_at = Utc::now();
        pipe.observation.resource.exit = Some(substrate_wire::ExecExit {
            code: signal.is_none().then_some(0),
            signal,
        });
    }
}

#[async_trait]
impl Driver for PipeFixtureDriver {
    fn machine(&self) -> CapabilitySnapshot {
        let mut capability = self.host.machine();
        SNAPSHOT.clone_into(&mut capability.snapshot);
        capability.driver = HostDriverKind::Host;
        "pipe-semantic-fixture".clone_into(&mut capability.driver_version);
        capability.facts.exec_namespaces = Some(substrate_wire::NamespaceFacts {
            user: true,
            mount: true,
            pid: true,
            ipc: true,
            uts: true,
            network: true,
        });
        capability.facts.exec_cgroup_limits = Some(substrate_wire::CgroupLimitFacts {
            processes: true,
            memory: true,
            cpu: true,
        });
        capability.facts.exec_cgroup_kill = Some(true);
        capability.facts.exec_no_egress = Some(true);
        capability
    }

    fn workspace_root_identity(&self, id: &str) -> Result<String, DriverError> {
        self.host.workspace_root_identity(id)
    }

    async fn create_workspace(
        &self,
        id: &str,
        root_name: &str,
        input: &WorkspaceCreateInput,
    ) -> DispatchOutcome<Workspace> {
        self.host.create_workspace(id, root_name, input).await
    }

    async fn observe_workspace(
        &self,
        id: &str,
        root_name: &str,
        previous: &Workspace,
    ) -> Result<Workspace, DriverError> {
        self.host.observe_workspace(id, root_name, previous).await
    }

    async fn read_workspace_path(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<FileReadResult, DriverError> {
        self.host
            .read_workspace_path(workspace_id, root_name, path, query)
            .await
    }

    async fn write_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        content: &[u8],
    ) -> Result<FileObservation, DriverError> {
        self.host
            .write_workspace_file(workspace_id, root_name, path, content)
            .await
    }

    async fn delete_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
    ) -> Result<FileAbsence, DriverError> {
        self.host
            .delete_workspace_file(workspace_id, root_name, path)
            .await
    }

    async fn destroy_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<WorkspaceDestroyProgress, DriverError> {
        self.host.destroy_workspace(workspace_id, root_name).await
    }

    async fn start_exec(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.host.start_exec(id, workspace_root_name, input).await
    }

    async fn start_pipe_session(
        &self,
        id: &str,
        _workspace_root_name: &str,
        input: &PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        let (sender, receiver) = mpsc::channel(16);
        let observation = ExecObservation {
            resource: Exec {
                id: id.to_owned(),
                kind: substrate_wire::ExecKind::Exec,
                workspace: input.exec.workspace.clone(),
                state: ExecState::Running,
                observed_at: Utc::now(),
                requested: input.exec.sandbox.clone(),
                applied: Some(AppliedConfinement {
                    capability_snapshot: SNAPSHOT.to_owned(),
                    cgroup: format!("fixture-{id}"),
                    filesystem: AppliedFilesystem::WorkspaceReadWriteSystemReadOnly,
                    network: AppliedNetwork::None,
                    profile: SandboxProfile::Workspace,
                    capsule: None,
                }),
                exit: None,
                lease: None,
            },
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            output_complete: false,
            cgroup: Some(format!("fixture-{id}")),
            leader_pid: Some(1234),
        };
        self.pipes.lock().expect("pipe fixture lock").insert(
            id.to_owned(),
            FixturePipe {
                observation: observation.clone(),
                sender: Some(sender),
                receiver: Arc::new(AsyncMutex::new(receiver)),
            },
        );
        DispatchOutcome::Observed(observation)
    }

    async fn write_pipe_session(&self, id: &str, bytes: &[u8]) -> Result<(), DriverError> {
        let sender = self
            .pipes
            .lock()
            .expect("pipe fixture lock")
            .get(id)
            .and_then(|pipe| pipe.sender.clone())
            .ok_or_else(DriverError::not_found)?;
        sender
            .send(PipeFrame {
                stream: PipeStream::Stdout,
                bytes: bytes.to_vec(),
            })
            .await
            .map_err(|_| DriverError::failed("session.write-failed", "fixture output closed"))
    }

    async fn read_pipe_session(
        &self,
        id: &str,
        timeout: Duration,
    ) -> Result<Option<PipeFrame>, DriverError> {
        let receiver = self
            .pipes
            .lock()
            .expect("pipe fixture lock")
            .get(id)
            .map(|pipe| Arc::clone(&pipe.receiver))
            .ok_or_else(DriverError::not_found)?;
        let mut receiver = receiver.lock().await;
        tokio::time::timeout(timeout, receiver.recv())
            .await
            .map_err(|_| DriverError::failed("session.read-timeout", "fixture read timeout"))
    }

    async fn close_pipe_session_input(&self, id: &str) -> Result<(), DriverError> {
        if !self
            .pipes
            .lock()
            .expect("pipe fixture lock")
            .contains_key(id)
        {
            return Err(DriverError::not_found());
        }
        self.terminal(id, ExecState::Exited, None);
        Ok(())
    }

    async fn observe_exec(&self, id: &str) -> Result<ExecObservation, DriverError> {
        self.pipes
            .lock()
            .expect("pipe fixture lock")
            .get(id)
            .map(|pipe| pipe.observation.clone())
            .or_else(|| {
                self.host
                    .completed_execs()
                    .into_iter()
                    .find(|item| item.resource.id == id)
            })
            .ok_or_else(DriverError::not_found)
    }

    async fn output(&self, id: &str, query: &ExecOutputQuery) -> Result<OutputSlice, DriverError> {
        self.host.output(id, query).await
    }

    async fn signal(
        &self,
        id: &str,
        input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        if !self
            .pipes
            .lock()
            .expect("pipe fixture lock")
            .contains_key(id)
        {
            return self.host.signal(id, input).await;
        }
        self.terminal(id, ExecState::Cancelled, Some(input.signal));
        self.observe_exec(id).await
    }

    fn completed_execs(&self) -> Vec<ExecObservation> {
        self.pipes
            .lock()
            .expect("pipe fixture lock")
            .values()
            .filter(|pipe| {
                matches!(
                    pipe.observation.resource.state,
                    ExecState::Exited | ExecState::Cancelled | ExecState::Unknown
                )
            })
            .map(|pipe| pipe.observation.clone())
            .chain(self.host.completed_execs())
            .collect()
    }

    fn set_exec_lease(&self, id: &str, lease: Option<LeaseObservation>) {
        if let Some(pipe) = self.pipes.lock().expect("pipe fixture lock").get_mut(id) {
            pipe.observation.resource.lease = lease;
        } else {
            self.host.set_exec_lease(id, lease);
        }
    }

    fn acknowledge_exec(&self, _persisted: &ExecObservation) {}

    fn discard_superseded_exec(&self, id: &str) {
        self.pipes.lock().expect("pipe fixture lock").remove(id);
        self.host.discard_superseded_exec(id);
    }
}

struct Harness {
    _directory: TempDir,
    app: Arc<App>,
    server: TestServer,
}

impl Harness {
    async fn open() -> Self {
        let directory = tempfile::tempdir().expect("temporary pipe harness");
        let store = Arc::new(
            substrate_store::Store::open(directory.path().join("state.db")).expect("state store"),
        );
        let driver = PipeFixtureDriver::open(&directory.path().join("workspaces"));
        let app = App::new(store, driver, DEPLOYMENT);
        let server = TestServer::spawn(Arc::clone(&app)).await;
        Self {
            _directory: directory,
            app,
            server,
        }
    }

    async fn call(&self, method: Method, uri: &str, body: Body) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body)
            .expect("request");
        let response = router(Arc::clone(&self.app))
            .layer(Extension(identity()))
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2_097_152)
            .await
            .expect("response body");
        let value = serde_json::from_slice(&bytes).expect("JSON response");
        (status, value)
    }

    async fn start_pipe(&self) -> (String, String) {
        let (status, workspace) = self
            .call(
                Method::POST,
                "/v1/workspaces",
                mutation(
                    "01JPIPEWORKSPACECREATE01",
                    json!({"source": "empty", "labels": {"fixture": "pipe"}}),
                ),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let workspace = workspace["result"]["id"].as_str().expect("workspace id");
        let (status, session) = self
            .call(
                Method::POST,
                "/v1/pipe-sessions",
                mutation(
                    "01JPIPESESSIONSTART00001",
                    json!({
                        "exec": {
                            "workspace": workspace,
                            "argv": ["/usr/bin/fixture-app-server"],
                            "env": {"allow": [], "set": {}},
                            "limits": {
                                "timeout_ms": 10_000,
                                "output_bytes": 1_048_576,
                                "processes": 8,
                                "memory_bytes": 67_108_864,
                                "cpu_millis": 1_000
                            },
                            "sandbox": {
                                "require": true,
                                "profile": "workspace",
                                "network": "none",
                                "capability_snapshot": SNAPSHOT
                            },
                            "wait": false,
                            "lease_ttl_ms": 60_000
                        },
                        "input_limit_bytes": 1_048_576,
                        "frame_limit_bytes": 65_536,
                        "queued_frames": 16
                    }),
                ),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{session}");
        (
            session["result"]["id"]
                .as_str()
                .expect("pipe session id")
                .to_owned(),
            session["result"]["exec"]
                .as_str()
                .expect("pipe exec id")
                .to_owned(),
        )
    }
}

#[allow(clippy::needless_pass_by_value)] // Test call sites construct one-shot JSON values.
fn mutation(operation: &str, input: Value) -> Body {
    Body::from(
        serde_json::to_vec(&json!({"op": operation, "input": input})).expect("mutation JSON"),
    )
}

fn identity() -> Identity {
    Identity {
        subject: SUBJECT.to_owned(),
        actor: "pipe-session-test".to_owned(),
        principal: None,
    }
}

struct TestServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl TestServer {
    async fn spawn(app: Arc<App>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind pipe websocket test server");
        let address = listener.local_addr().expect("test server address");
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    return;
                };
                let service = router(Arc::clone(&app)).layer(Extension(identity()));
                tokio::spawn(async move {
                    let connection = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), TowerToHyperService::new(service))
                        .with_upgrades();
                    let _result = connection.await;
                });
            }
        });
        Self { address, task }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Handshake {
    status: u16,
    stream: TcpStream,
}

impl Handshake {
    async fn open(address: SocketAddr, path: &str) -> Self {
        let mut stream = TcpStream::connect(address)
            .await
            .expect("connect pipe websocket client");
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write websocket handshake");
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            assert!(response.len() < 16 * 1_024, "bounded handshake response");
            let mut byte = [0_u8; 1];
            stream
                .read_exact(&mut byte)
                .await
                .expect("read websocket handshake");
            response.push(byte[0]);
        }
        let response = std::str::from_utf8(&response).expect("ASCII handshake response");
        let status = response
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .expect("HTTP handshake status");
        Self { status, stream }
    }

    fn upgraded(self) -> WebSocketClient {
        assert_eq!(self.status, 101, "websocket upgrade must succeed");
        WebSocketClient {
            stream: self.stream,
        }
    }
}

struct ServerFrame {
    opcode: u8,
    payload: Vec<u8>,
}

struct WebSocketClient {
    stream: TcpStream,
}

impl WebSocketClient {
    async fn send_text(&mut self, payload: &[u8]) {
        let mut encoded = Vec::with_capacity(payload.len().saturating_add(14));
        encoded.push(0x81);
        match payload.len() {
            length @ 0..=125 => encoded.push(0x80 | u8::try_from(length).expect("short length")),
            length @ 126..=65_535 => {
                encoded.push(0x80 | 0x7e);
                encoded.extend_from_slice(
                    &u16::try_from(length).expect("medium length").to_be_bytes(),
                );
            }
            length => {
                encoded.push(0x80 | 127);
                encoded
                    .extend_from_slice(&u64::try_from(length).expect("large length").to_be_bytes());
            }
        }
        let mask = [0x11, 0x22, 0x33, 0x44];
        encoded.extend_from_slice(&mask);
        encoded.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % mask.len()]),
        );
        self.stream
            .write_all(&encoded)
            .await
            .expect("write websocket text frame");
    }

    async fn next_frame(&mut self) -> Option<ServerFrame> {
        let mut header = [0_u8; 2];
        if let Err(error) = self.stream.read_exact(&mut header).await {
            assert!(
                matches!(
                    error.kind(),
                    ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset
                ),
                "unexpected websocket read error: {error}"
            );
            return None;
        }
        let opcode = header[0] & 0x0f;
        let mut length = u64::from(header[1] & 0x7f);
        if length == 126 {
            let mut bytes = [0_u8; 2];
            self.stream
                .read_exact(&mut bytes)
                .await
                .expect("read medium length");
            length = u64::from(u16::from_be_bytes(bytes));
        } else if length == 127 {
            let mut bytes = [0_u8; 8];
            self.stream
                .read_exact(&mut bytes)
                .await
                .expect("read large length");
            length = u64::from_be_bytes(bytes);
        }
        let mut payload = vec![0_u8; usize::try_from(length).expect("frame length in memory")];
        self.stream
            .read_exact(&mut payload)
            .await
            .expect("read server payload");
        Some(ServerFrame { opcode, payload })
    }

    async fn next_json(&mut self) -> Value {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(2), self.next_frame())
                .await
                .expect("bounded server frame")
                .expect("server frame");
            if frame.opcode == 0x1 {
                return serde_json::from_slice(&frame.payload).expect("server JSON frame");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One end-to-end session lifecycle shares a single daemon.
async fn durable_pipe_start_single_attachment_and_terminal_output_are_scoped() {
    let harness = Harness::open().await;
    let (status, capabilities) = harness
        .call(Method::GET, "/v1/pipe-sessions", Body::empty())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(capabilities["result"]["contract"], "substrate-wire/0.4.0");
    assert_eq!(capabilities["result"]["single_attachment"], true);
    assert_eq!(capabilities["result"]["network"], "none");
    let (session_id, exec_id) = harness.start_pipe().await;
    let (status, renewed) = harness
        .call(
            Method::POST,
            &format!("/v1/pipe-sessions/{session_id}/lease/renew"),
            mutation("01JPIPESESSIONRENEW00001", json!({"ttl_ms": 90_000})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    assert_eq!(
        renewed["result"]["lease"]["authorizing_operation"],
        "01JPIPESESSIONRENEW00001"
    );
    let path = format!("/v1/pipe-sessions/{session_id}/attach");
    let mut client = Handshake::open(harness.server.address, &path)
        .await
        .upgraded();
    assert_eq!(
        Handshake::open(harness.server.address, &path).await.status,
        409,
        "a second attachment must fail before upgrade"
    );

    client
        .send_text(
            &serde_json::to_vec(&json!({
                "kind": "stdin",
                "sequence": 1,
                "content": {
                    "encoding": "base64",
                    "data": base64::engine::general_purpose::STANDARD.encode(b"{\"id\":1}\n")
                }
            }))
            .expect("client frame"),
        )
        .await;
    let output = client.next_json().await;
    let output: PipeServerFrame = serde_json::from_value(output).expect("closed server frame");
    assert!(matches!(
        output,
        PipeServerFrame::Output {
            sequence: 1,
            stream: substrate_wire::OutputStream::Stdout,
            content,
        } if content.decode().expect("base64 output") == b"{\"id\":1}\n"
    ));

    client
        .send_text(
            &serde_json::to_vec(&json!({"kind": "close-input", "sequence": 2}))
                .expect("close input frame"),
        )
        .await;
    let exit: PipeServerFrame =
        serde_json::from_value(client.next_json().await).expect("closed exit frame");
    assert!(matches!(
        exit,
        PipeServerFrame::Exit {
            sequence: 2,
            state: ExecState::Exited,
            ..
        }
    ));

    let (status, observed) = harness
        .call(Method::GET, &format!("/v1/execs/{exec_id}"), Body::empty())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(observed["result"]["state"], "exited");
    assert_eq!(observed["result"]["applied"]["network"], "none");
    let (status, session) = harness
        .call(
            Method::GET,
            &format!("/v1/pipe-sessions/{session_id}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["result"]["state"], "exited");
    assert_eq!(session["result"]["attachment"], "consumed");
    assert_eq!(session["result"]["exec"], exec_id);

    let (status, refusal) = harness
        .call(
            Method::DELETE,
            &format!("/v1/execs/{exec_id}"),
            mutation("01JPIPEEXECRETIRE000001", json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refusal}");
    assert_eq!(refusal["error"]["code"], "exec.session-owned");

    let (status, absence) = harness
        .call(
            Method::DELETE,
            &format!("/v1/pipe-sessions/{session_id}"),
            mutation("01JPIPESESSIONRETIRE001", json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{absence}");
    assert_eq!(absence["result"]["absent"], true);
    assert_eq!(
        harness
            .call(
                Method::GET,
                &format!("/v1/pipe-sessions/{session_id}"),
                Body::empty(),
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        harness
            .call(Method::GET, &format!("/v1/execs/{exec_id}"), Body::empty(),)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_sequence_fails_closed_and_disconnect_cancels_the_session() {
    let harness = Harness::open().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    let path = format!("/v1/pipe-sessions/{session_id}/attach");
    let mut client = Handshake::open(harness.server.address, &path)
        .await
        .upgraded();
    client
        .send_text(
            &serde_json::to_vec(&json!({"kind": "close-input", "sequence": 2}))
                .expect("invalid sequence frame"),
        )
        .await;
    let error: PipeServerFrame =
        serde_json::from_value(client.next_json().await).expect("closed error frame");
    assert!(matches!(
        error,
        PipeServerFrame::ProtocolError {
            sequence: 1,
            ref code,
            ..
        } if code == "session.sequence-invalid"
    ));
    drop(client);

    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let (status, observed) = harness
                .call(Method::GET, &format!("/v1/execs/{exec_id}"), Body::empty())
                .await;
            assert_eq!(status, StatusCode::OK);
            if observed["result"]["state"] == "cancelled" {
                return observed;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("disconnect cancellation must become durable");
    assert_eq!(observed["result"]["exit"]["signal"], "KILL");
}

#[tokio::test(flavor = "multi_thread")]
async fn capability_inspection_refuses_without_delegated_confinement() {
    let directory = tempfile::tempdir().expect("portable refusal directory");
    let store = Arc::new(
        substrate_store::Store::open(directory.path().join("state.db")).expect("state store"),
    );
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("portable host driver");
    let app = App::new(store, driver, "dep_pipe_refusal");
    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/pipe-sessions")
        .body(Body::empty())
        .expect("capability request");
    let response = router(app)
        .layer(Extension(identity()))
        .oneshot(request)
        .await
        .expect("capability response");
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let bytes = to_bytes(response.into_body(), 2_097_152)
        .await
        .expect("refusal body");
    let refusal: Value = serde_json::from_slice(&bytes).expect("refusal JSON");
    assert_eq!(refusal["error"]["code"], "session.confinement-unavailable");
}
