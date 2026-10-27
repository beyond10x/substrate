#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
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
/// A second subject on the same daemon, for the per-subject attachment cap (ADR 0042 decision 4).
const OTHER_SUBJECT: &str = "local:1001";
/// A third subject, which finds the deployment-wide attachment cap full when the first two hold
/// their per-subject shares.
const THIRD_SUBJECT: &str = "local:1002";
/// The per-subject attachment cap ADR 0042 decision 4 states; half of the global 32.
const ATTACHMENTS_PER_SUBJECT: u32 = 16;
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
    /// What this fixture's capability document publishes for `sessions.pty`.
    terminals: bool,
    /// Every window that reached the driver, in order. A refused resize leaves nothing here.
    resizes: Mutex<HashMap<String, Vec<substrate_wire::PtyWindow>>>,
    /// When set, `signal` refuses.
    ///
    /// That is exactly the state `terminate_pipe_session` reports as `false` -- a cancellation it
    /// could not prove -- which is the only way to reach the failure branch of either containment
    /// hand-off out of the attach handler. Off by default, so no case written before this field
    /// changes behaviour.
    refuse_signal: std::sync::atomic::AtomicBool,
    /// How many of the next `signal` calls refuse, each spending one; zero by default.
    ///
    /// A *transient* refusal: the first containment kill is refused and the next is allowed,
    /// which is the driver recovering between two containment rounds.
    refuse_signals: AtomicU32,
    /// When set, `signal` never answers: the kill is in the driver and stays there.
    hang_signal: std::sync::atomic::AtomicBool,
    /// Every `signal` call that reached this driver, answered or not.
    signal_calls: AtomicU32,
    /// The exec id of every `signal` call that reached this driver, in arrival order.
    signalled: Mutex<Vec<String>>,
    /// Execs whose kill this driver never proves: `signal` refuses for each, every time.
    refuse_signal_for: Mutex<std::collections::HashSet<String>>,
    /// When set, a kill is answered with an `unknown` terminal observation: the driver ended the
    /// exec but could not prove the whole tree went with it.
    kill_ends_unknown: std::sync::atomic::AtomicBool,
    /// Every maintenance-round retry of unproven exec trees that reached this driver.
    unproven_retries: AtomicU32,
    /// Execs whose tree the driver has not proven gone: `signal` answers each the way the host
    /// driver answers a carried tree, `exec.cgroup-not-empty`, never `NotFound`.
    unproven_tree_for: Mutex<std::collections::HashSet<String>>,
}

impl PipeFixtureDriver {
    fn with_terminals(root: &std::path::Path, terminals: bool) -> Arc<Self> {
        Arc::new(Self {
            host: HostDriver::open(HostConfig::minimum(root)).expect("host fixture driver"),
            pipes: Mutex::new(HashMap::new()),
            terminals,
            resizes: Mutex::new(HashMap::new()),
            refuse_signal: std::sync::atomic::AtomicBool::new(false),
            refuse_signals: AtomicU32::new(0),
            hang_signal: std::sync::atomic::AtomicBool::new(false),
            signal_calls: AtomicU32::new(0),
            signalled: Mutex::new(Vec::new()),
            refuse_signal_for: Mutex::new(std::collections::HashSet::new()),
            kill_ends_unknown: std::sync::atomic::AtomicBool::new(false),
            unproven_retries: AtomicU32::new(0),
            unproven_tree_for: Mutex::new(std::collections::HashSet::new()),
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

    /// Exactly what a host records when a `pty` session reaches its declared output bound.
    ///
    /// `drain_capped` raises `truncated` on the same branch that raises the bound flag, `run_child`
    /// copies it onto `observation.stdout_truncated`, and `record_terminal_output_bound` names the
    /// bound on the refusal field — `crates/substrate-host/src/process.rs:1554-1557`, `:1579` and
    /// `:1675-1683`. Written by the adversary pass; no case that shipped calls it.
    fn output_bound_reached(&self, id: &str) {
        self.terminal(id, ExecState::Cancelled, Some(substrate_wire::Signal::Kill));
        let mut pipes = self.pipes.lock().expect("pipe fixture lock");
        let pipe = pipes.get_mut(id).expect("known pipe fixture");
        pipe.observation.stdout_truncated = true;
        pipe.observation.resource.refusal = Some(substrate_wire::ExecRefusal {
            class: substrate_wire::ErrorClass::Exhausted,
            code: "session.output-limit".to_owned(),
            message: "The declared output bound ended the terminal session.".to_owned(),
        });
    }
}

#[async_trait]
impl Driver for PipeFixtureDriver {
    async fn shutdown(&self) -> Result<(), DriverError> {
        self.host.shutdown().await
    }

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
        capability.facts.sessions_pty = self.terminals.then_some(true);
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
    ) -> Result<substrate_host::Listed<FileReadResult>, DriverError> {
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
                unrecorded_output: None,
                id: id.to_owned(),
                kind: substrate_wire::ExecKind::Exec,
                workspace: input.exec.workspace.clone(),
                state: ExecState::Running,
                observed_at: Utc::now(),
                requested: input.exec.sandbox.clone(),
                applied: Some(AppliedConfinement {
                    writable_roots: Vec::new(),
                    workspace_access: substrate_wire::WorkspaceAccess::ReadWrite,
                    read_only_roots: Vec::new(),
                    read_only_files: Vec::new(),
                    secret_slots: Vec::new(),
                    workdir: None,
                    capability_snapshot: SNAPSHOT.to_owned(),
                    cgroup: format!("fixture-{id}"),
                    filesystem: AppliedFilesystem::WorkspaceReadWriteSystemReadOnly,
                    network: AppliedNetwork::None,
                    profile: SandboxProfile::Workspace,
                    capsule: None,
                }),
                exit: None,
                usage: None,
                lease: None,
                refusal: None,
            },
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            stdout_kept: None,
            stderr_kept: None,
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

    async fn resize_pty_session(
        &self,
        id: &str,
        window: substrate_wire::PtyWindow,
    ) -> Result<(), DriverError> {
        if !self
            .pipes
            .lock()
            .expect("pipe fixture lock")
            .contains_key(id)
        {
            return Err(DriverError::not_found());
        }
        self.resizes
            .lock()
            .expect("pipe fixture lock")
            .entry(id.to_owned())
            .or_default()
            .push(window);
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
        self.signal_calls.fetch_add(1, Ordering::SeqCst);
        self.signalled
            .lock()
            .expect("signalled fixture lock")
            .push(id.to_owned());
        if self.hang_signal.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self
            .unproven_tree_for
            .lock()
            .expect("unproven_tree_for fixture lock")
            .contains(id)
        {
            return Err(DriverError::failed(
                "exec.cgroup-not-empty",
                "The exec's cgroup has not been proven empty yet.",
            ));
        }
        if self
            .refuse_signal_for
            .lock()
            .expect("refuse_signal_for fixture lock")
            .contains(id)
        {
            return Err(DriverError::failed(
                "session.signal-refused",
                "fixture driver cannot prove this exec's cancellation",
            ));
        }
        let transient = self
            .refuse_signals
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if transient
            || self
                .refuse_signal
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(DriverError::failed(
                "session.signal-refused",
                "fixture driver cannot prove this cancellation",
            ));
        }
        if !self
            .pipes
            .lock()
            .expect("pipe fixture lock")
            .contains_key(id)
        {
            return self.host.signal(id, input).await;
        }
        if self.kill_ends_unknown.load(Ordering::SeqCst) {
            self.terminal(id, ExecState::Unknown, Some(input.signal));
        } else {
            self.terminal(id, ExecState::Cancelled, Some(input.signal));
        }
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

    async fn retry_unproven_exec_trees(&self) -> Result<(), DriverError> {
        self.unproven_retries.fetch_add(1, Ordering::SeqCst);
        self.host.retry_unproven_exec_trees().await
    }
}

struct Harness {
    driver: Arc<PipeFixtureDriver>,
    _directory: TempDir,
    app: Arc<App>,
    server: TestServer,
    _containment: Option<ContainmentTask>,
}

/// The daemon's containment step, run the way `runtime.rs` runs it: `App::contain_pending` on a
/// 250 ms tick and whenever a containment is queued. Aborted when the harness drops.
struct ContainmentTask(JoinHandle<()>);

impl ContainmentTask {
    fn spawn(app: Arc<App>) -> Self {
        Self(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    () = app.containment_notified() => {}
                }
                app.contain_pending().await;
            }
        }))
    }
}

impl Drop for ContainmentTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Harness {
    async fn open() -> Self {
        Self::opened(false, true).await
    }

    /// A daemon whose driver published `sessions.pty` after a verified allocation.
    async fn with_terminals() -> Self {
        Self::opened(true, true).await
    }

    /// A daemon with no containment task: each containment round happens only when the case
    /// calls `App::contain_pending`, so a case can count rounds.
    async fn driven() -> Self {
        Self::opened(false, false).await
    }

    async fn opened(terminals: bool, containment_task: bool) -> Self {
        let directory = tempfile::tempdir().expect("temporary pipe harness");
        let store = Arc::new(
            substrate_store::Store::open(directory.path().join("state.db")).expect("state store"),
        );
        let driver =
            PipeFixtureDriver::with_terminals(&directory.path().join("workspaces"), terminals);
        let app = App::new(store, Arc::clone(&driver) as Arc<dyn Driver>, DEPLOYMENT);
        let server = TestServer::spawn(Arc::clone(&app)).await;
        let containment = containment_task.then(|| ContainmentTask::spawn(Arc::clone(&app)));
        Self {
            driver,
            _directory: directory,
            app,
            server,
            _containment: containment,
        }
    }

    async fn exec(&self, exec_id: &str) -> Value {
        let (status, observed) = self
            .call(Method::GET, &format!("/v1/execs/{exec_id}"), Body::empty())
            .await;
        assert_eq!(status, StatusCode::OK, "{observed}");
        observed
    }

    /// Waits until exactly `count` containments are pending, or fails naming what it waited for.
    async fn await_pending_containments(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.app.pending_containments() != count {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "{} containments are pending ten seconds on, not {count}",
                self.app.pending_containments()
            )
        });
    }

    /// Drives containment rounds until every queued transient refusal has been spent.
    ///
    /// Whichever hand-off made the first, refused attempt — the attach handler itself or a
    /// containment round — the case continues from the same state: the refusal is spent, the
    /// exec is still running and its containment is still pending.
    async fn spend_refused_kills(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.driver.refuse_signals.load(Ordering::SeqCst) != 0 {
                self.app.contain_pending().await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a pending containment reaches the driver within ten seconds of rounds");
    }

    /// Strands a claimed pipe session the way `on_failed_upgrade` sees it: `101`, then no upgrade.
    async fn strand(&self, session_id: &str) -> TestServer {
        let stranding = TestServer::spawn_without_upgrades(Arc::clone(&self.app)).await;
        let handshake = Handshake::open(
            stranding.address,
            &format!("/v1/sessions/{session_id}/attach"),
        )
        .await;
        assert_eq!(
            handshake.status, 101,
            "the attach handshake is answered before the upgrade can fail"
        );
        drop(handshake);
        stranding
    }

    fn resizes(&self, exec_id: &str) -> Vec<substrate_wire::PtyWindow> {
        self.driver
            .resizes
            .lock()
            .expect("pipe fixture lock")
            .get(exec_id)
            .cloned()
            .unwrap_or_default()
    }

    async fn create_workspace(&self, operation: &str) -> String {
        self.create_workspace_for(&identity(), operation).await
    }

    /// [`Harness::create_workspace`] in the scope of `identity`'s subject.
    async fn create_workspace_for(&self, identity: &Identity, operation: &str) -> String {
        let (status, workspace) = self
            .call_as(
                identity,
                Method::POST,
                "/v1/workspaces",
                mutation(
                    operation,
                    json!({"source": "empty", "labels": {"fixture": "pty"}}),
                ),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{workspace}");
        workspace["result"]["id"]
            .as_str()
            .expect("workspace id")
            .to_owned()
    }

    async fn start_pty(&self) -> (String, String) {
        let workspace = self.create_workspace("01JPTYWORKSPACECREATE003").await;
        let (status, session) = self
            .call(
                Method::POST,
                "/v1/sessions",
                mutation(
                    "01JPTYSESSIONSTART000001",
                    pty_start(&workspace, Some(json!({"columns": 80, "rows": 24}))),
                ),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{session}");
        assert_eq!(session["result"]["mode"], "pty", "{session}");
        (
            session["result"]["id"]
                .as_str()
                .expect("pty session id")
                .to_owned(),
            session["result"]["exec"]
                .as_str()
                .expect("pty exec id")
                .to_owned(),
        )
    }

    async fn attach(&self, path: &str) -> WebSocketClient {
        Handshake::open(self.server.address, path).await.upgraded()
    }

    async fn call(&self, method: Method, uri: &str, body: Body) -> (StatusCode, Value) {
        self.call_as(&identity(), method, uri, body).await
    }

    /// [`Harness::call`] as `identity`.
    async fn call_as(
        &self,
        identity: &Identity,
        method: Method,
        uri: &str,
        body: Body,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body)
            .expect("request");
        let response = router(Arc::clone(&self.app))
            .layer(Extension(identity.clone()))
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
                "/v1/sessions",
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

    /// [`Harness::start_pipe`] with operation ids of its own, so one harness can hold many
    /// sessions: a fresh workspace and a fresh session per `n`.
    async fn start_pipe_numbered(&self, n: u32) -> (String, String) {
        self.start_pipe_numbered_leased(n, None).await
    }

    /// [`Harness::start_pipe_numbered`] in a workspace created with `workspace_lease_ttl_ms`.
    async fn start_pipe_numbered_leased(
        &self,
        n: u32,
        workspace_lease_ttl_ms: Option<u64>,
    ) -> (String, String) {
        let mut input = json!({"source": "empty", "labels": {"fixture": "pipe"}});
        if let Some(ttl) = workspace_lease_ttl_ms {
            input["lease_ttl_ms"] = json!(ttl);
        }
        let (status, workspace) = self
            .call(
                Method::POST,
                "/v1/workspaces",
                mutation(&format!("01JU1SECWORKSPACE{n:07}"), input),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{workspace}");
        let workspace = workspace["result"]["id"].as_str().expect("workspace id");
        let (status, session) = self
            .call(
                Method::POST,
                "/v1/sessions",
                mutation(
                    &format!("01JU1SECSESSION{n:09}"),
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

/// A `pty` start body, with the window the caller wants to try.
fn pty_start(workspace: &str, window: Option<Value>) -> Value {
    let mut input = json!({
        "exec": {
            "workspace": workspace,
            "argv": ["/bin/sh"],
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
        "queued_frames": 16,
        "mode": "pty"
    });
    if let Some(window) = window {
        input["window"] = window;
    }
    input
}

fn identity() -> Identity {
    identity_of(SUBJECT)
}

fn identity_of(subject: &str) -> Identity {
    Identity {
        subject: subject.to_owned(),
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
        Self::serving(app, true).await
    }

    /// A server that answers the handshake and then never completes the upgrade.
    ///
    /// Without `with_upgrades()`, hyper flushes the `101` and then resolves the upgrade future
    /// with an error instead of handing the socket over. That is the same seam a client reaches
    /// by dropping between the switch and the hand-off, and it is reachable deterministically.
    async fn spawn_without_upgrades(app: Arc<App>) -> Self {
        Self::serving(app, false).await
    }

    async fn serving(app: Arc<App>, upgrades: bool) -> Self {
        Self::serving_as(app, upgrades, identity()).await
    }

    /// A server whose every connection is admitted as `identity`, as a listener admits one peer.
    async fn spawn_as(app: Arc<App>, identity: Identity) -> Self {
        Self::serving_as(app, true, identity).await
    }

    async fn serving_as(app: Arc<App>, upgrades: bool, identity: Identity) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind pipe websocket test server");
        let address = listener.local_addr().expect("test server address");
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    return;
                };
                let service = router(Arc::clone(&app)).layer(Extension(identity.clone()));
                tokio::spawn(async move {
                    let connection = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), TowerToHyperService::new(service));
                    if upgrades {
                        let _result = connection.with_upgrades().await;
                    } else {
                        let _result = connection.await;
                    }
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
        .call(Method::GET, "/v1/sessions", Body::empty())
        .await;
    assert_eq!(status, StatusCode::OK);
    // Round 5: the document names the bundle whose schema it conforms to, and it carries five
    // properties `0.4.0`'s closed schema forbids. `the_capability_document_is_admitted_by_the_
    // contract_it_names` is the case that decides this; here it is just read back.
    assert_eq!(
        capabilities["result"]["contract"],
        substrate_wire::PIPE_SESSION_CAPABILITY_CONTRACT
    );
    assert_eq!(capabilities["result"]["single_attachment"], true);
    assert_eq!(capabilities["result"]["network"], "none");
    let (session_id, exec_id) = harness.start_pipe().await;
    let (status, renewed) = harness
        .call(
            Method::POST,
            &format!("/v1/sessions/{session_id}/lease/renew"),
            mutation("01JPIPESESSIONRENEW00001", json!({"ttl_ms": 90_000})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    assert_eq!(
        renewed["result"]["lease"]["authorizing_operation"],
        "01JPIPESESSIONRENEW00001"
    );
    let path = format!("/v1/sessions/{session_id}/attach");
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
            &format!("/v1/sessions/{session_id}"),
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
            &format!("/v1/sessions/{session_id}"),
            mutation("01JPIPESESSIONRETIRE001", json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{absence}");
    assert_eq!(absence["result"]["absent"], true);
    assert_eq!(
        harness
            .call(
                Method::GET,
                &format!("/v1/sessions/{session_id}"),
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
    let path = format!("/v1/sessions/{session_id}/attach");
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
            code: substrate_wire::SessionProtocolErrorCode::SequenceInvalid,
            ..
        }
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

/// A `101` whose upgrade never completes must not leave a claimed session running.
///
/// `pipe_session_attach` consumes the durable attachment claim *before* the upgrade, so a client
/// that drops between the switch and the hand-off would otherwise leave the process running
/// unattached until its lease or timeout ended it, with the claim already spent. The server below
/// answers the handshake without `with_upgrades()`, which is that exact seam: hyper flushes the
/// `101`, then resolves the upgrade future with an error and the attachment closure never runs.
#[tokio::test(flavor = "multi_thread")]
async fn an_attachment_whose_upgrade_never_completes_is_terminated_and_reads_terminal() {
    let harness = Harness::open().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    let stranding = TestServer::spawn_without_upgrades(Arc::clone(&harness.app)).await;
    let handshake = Handshake::open(
        stranding.address,
        &format!("/v1/sessions/{session_id}/attach"),
    )
    .await;
    assert_eq!(
        handshake.status, 101,
        "the attach handshake is answered before the upgrade can fail"
    );
    drop(handshake);

    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (status, observed) = harness
                .call(Method::GET, &format!("/v1/execs/{exec_id}"), Body::empty())
                .await;
            assert_eq!(status, StatusCode::OK);
            if observed["result"]["state"] == "cancelled" {
                return observed;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a claimed attachment whose upgrade never completed must leave no running process");
    assert_eq!(observed["result"]["exit"]["signal"], "KILL");

    let (status, session) = harness
        .call(
            Method::GET,
            &format!("/v1/sessions/{session_id}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(session["result"]["state"], "cancelled", "{session}");
    assert_eq!(session["result"]["attachment"], "consumed", "{session}");

    let (status, refusal) = refused_upgrade(
        harness.server.address,
        &format!("/v1/sessions/{session_id}/attach"),
    )
    .await;
    assert_eq!(status, 409, "{refusal}");
    assert_eq!(
        refusal["error"]["code"].as_str(),
        Some(substrate_wire::SESSION_NOT_ATTACHABLE),
        "a contained session names its refusal rather than being quietly gone: {refusal}"
    );
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
        .uri("/v1/sessions")
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

// ------------------------------------------------------------------------------------------------
// pty sessions (design 13)
// ------------------------------------------------------------------------------------------------

/// Invariant 3, on the request side: a terminal this daemon never proved it can give is refused by
/// name, with the class and status design 13 fixes — and **never** served as a pipe session.
#[tokio::test(flavor = "multi_thread")]
async fn a_pty_start_is_unserved_when_the_capability_fact_is_absent() {
    let harness = Harness::open().await;
    let workspace = harness.create_workspace("01JPTYWORKSPACECREATE001").await;
    let (status, refusal) = harness
        .call(
            Method::POST,
            "/v1/sessions",
            mutation(
                "01JPTYSESSIONUNSERVED001",
                pty_start(&workspace, Some(json!({"columns": 80, "rows": 24}))),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{refusal}");
    assert_eq!(refusal["error"]["class"], "unserved");
    assert_eq!(refusal["error"]["code"], "session.pty-unserved");
    assert_eq!(refusal["error"]["address"], "mode");
    assert_eq!(refusal["error"]["retriable"], false);
}

/// Design 13: `mode: "pty"` without a window is refused rather than defaulted to 80x24 — substrate
/// has nothing to observe here and inventing the number would manufacture a fact — and a `pipes`
/// start carrying a window is refused for the mirror reason.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_is_required_for_pty_refused_for_pipes_and_never_defaulted() {
    let harness = Harness::with_terminals().await;
    let workspace = harness.create_workspace("01JPTYWORKSPACECREATE002").await;
    let cases = [
        ("01JPTYSESSIONNOWINDOW001", "pty", None),
        (
            "01JPTYSESSIONPIPEWINDOW1",
            "pipes",
            Some(json!({"columns": 80, "rows": 24})),
        ),
        (
            "01JPTYSESSIONZEROWINDOW1",
            "pty",
            Some(json!({"columns": 0, "rows": 24})),
        ),
        (
            "01JPTYSESSIONHUGEWINDOW1",
            "pty",
            Some(json!({"columns": 1001, "rows": 24})),
        ),
    ];
    for (operation, mode, window) in cases {
        let mut input = pty_start(&workspace, window);
        input["mode"] = json!(mode);
        let (status, refusal) = harness
            .call(Method::POST, "/v1/sessions", mutation(operation, input))
            .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{operation}: {refusal}"
        );
        assert_eq!(refusal["error"]["class"], "refused", "{operation}");
        assert_eq!(
            refusal["error"]["code"], "session.window-invalid",
            "{operation}"
        );
        assert_eq!(refusal["error"]["address"], "window", "{operation}");
    }
}

/// The capability document is the per-mode gate, because the registry's own gate cannot be: a
/// `capability_predicate` on `POST /v1/sessions` would take the route away from a daemon that
/// serves pipes perfectly well (design 13). The ceilings are derived from the wire constants and
/// are never a second source of truth.
#[tokio::test(flavor = "multi_thread")]
async fn session_capabilities_publish_the_served_modes_and_the_window_ceilings() {
    let harness = Harness::open().await;
    let (status, capabilities) = harness
        .call(Method::GET, "/v1/sessions", Body::empty())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(capabilities["result"]["modes"], json!(["pipes"]));
    assert_eq!(
        capabilities["result"]["max_window_columns"],
        json!(substrate_wire::MAX_PTY_WINDOW_COLUMNS)
    );
    assert_eq!(
        capabilities["result"]["max_window_rows"],
        json!(substrate_wire::MAX_PTY_WINDOW_ROWS)
    );

    let terminals = Harness::with_terminals().await;
    let (status, capabilities) = terminals
        .call(Method::GET, "/v1/sessions", Body::empty())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(capabilities["result"]["modes"], json!(["pipes", "pty"]));
}

/// Design 13: a resize outside 1..=1000 cells is a `protocol-error` frame carrying
/// `session.resize-invalid`, joining the vocabulary the attachment already speaks — and an admitted
/// resize reaches the driver as the window the client asked for, not as a clamped one.
#[tokio::test(flavor = "multi_thread")]
async fn a_resize_outside_the_declared_bounds_is_a_protocol_error() {
    let harness = Harness::with_terminals().await;
    let (session_id, exec_id) = harness.start_pty().await;
    let mut socket = harness
        .attach(&format!("/v1/sessions/{session_id}/attach"))
        .await;
    socket
        .send_text(
            serde_json::to_vec(&json!({
                "kind": "resize",
                "sequence": 1,
                "window": {"columns": 132, "rows": 43}
            }))
            .expect("resize frame")
            .as_slice(),
        )
        .await;
    socket
        .send_text(
            serde_json::to_vec(&json!({
                "kind": "resize",
                "sequence": 2,
                "window": {"columns": 0, "rows": 43}
            }))
            .expect("resize frame")
            .as_slice(),
        )
        .await;
    let frame = socket.next_json().await;
    assert_eq!(frame["kind"], "protocol-error", "{frame}");
    assert_eq!(frame["code"], "session.resize-invalid");
    assert_eq!(
        harness.resizes(&exec_id),
        vec![substrate_wire::PtyWindow {
            columns: 132,
            rows: 43
        }],
        "the admitted resize reached the driver and the refused one did not"
    );
}

/// Both refusals apply to one request, and the order is a decision rather than an accident.
///
/// A windowless `mode: "pty"` start on a deployment with no `sessions.pty` is refused
/// `session.pty-unserved`, not `session.window-invalid`: telling this client to add a window would
/// send it back for a retry that can never succeed. Asserted here and in
/// `vectors/http/pty-session-unserved-outranks-a-missing-window.json`, so narrowing it later is
/// something somebody does on purpose.
#[tokio::test(flavor = "multi_thread")]
async fn the_absent_pty_fact_outranks_a_missing_window() {
    let harness = Harness::open().await;
    let workspace = harness.create_workspace("01JPTYWORKSPACECREATE004").await;
    let (status, refusal) = harness
        .call(
            Method::POST,
            "/v1/sessions",
            mutation("01JPTYSESSIONORDER000001", pty_start(&workspace, None)),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{refusal}");
    assert_eq!(refusal["error"]["code"], "session.pty-unserved");
    assert_eq!(refusal["error"]["address"], "mode");

    // And the window rule still answers where it is the only thing wrong.
    let terminals = Harness::with_terminals().await;
    let workspace = terminals.create_workspace("01JPTYWORKSPACECREATE005").await;
    let (status, refusal) = terminals
        .call(
            Method::POST,
            "/v1/sessions",
            mutation("01JPTYSESSIONORDER000002", pty_start(&workspace, None)),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refusal}");
    assert_eq!(refusal["error"]["code"], "session.window-invalid");
}

/// A terminal has no `truncated` frame, and reaching the output bound ends the session instead.
///
/// `contracts/substrate-wire/0.10.0/schemas/pty-channel-frame.json` carries
/// `x-b10x-no-truncated: "reaching-the-output-bound-ends-the-session-through-the-exec-refusal"` and
/// no `truncated` branch in its `oneOf`; `xtask/src/bundle.rs:754-760` refuses a bundle whose pty
/// vocabulary grows one; `vectors/driver/pty-session-output-bound-ends-the-session.json` states
/// `/probes/truncated_frames_delivered` is 0. The attachment's terminal path
/// (`crates/substrate-daemon/src/app/sessions.rs:1188-1220`) does not read the session's mode, so a
/// pty session whose merged transcript was truncated is sent the raw-pipe `truncated` frame — a
/// frame outside the vocabulary the bundle publishes for this attachment.
#[tokio::test(flavor = "multi_thread")]
async fn a_pty_attachment_is_never_sent_a_truncated_frame() {
    let harness = Harness::with_terminals().await;
    let (session_id, exec_id) = harness.start_pty().await;
    let mut socket = harness
        .attach(&format!("/v1/sessions/{session_id}/attach"))
        .await;
    harness.driver.output_bound_reached(&exec_id);
    let frame = socket.next_json().await;
    assert_eq!(
        frame["kind"], "exit",
        "the pty frame vocabulary has no truncated frame: {frame}"
    );
    assert_eq!(frame["state"], "cancelled", "{frame}");
}

// ---------------------------------------------------------------------------
// Adversary pass 3. Added cases only; nothing above this line was altered.
// ---------------------------------------------------------------------------

/// The `protocol-error` codes `0.10.0` publishes for a pty attachment, read out of the bundle.
///
/// `schemas/pty-channel-frame.json`'s `protocol-error` branch carries `x-b10x-codes`, an annotation
/// this wave introduced — the raw-pipe vocabulary next door has none. It is the only place a client
/// can look up what a pty attachment may be told, and `xtask`'s `check_pty_refusal_class` treats a
/// code's presence in it as proof that the code is readable.
fn published_pty_protocol_error_codes() -> Vec<String> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("contracts/substrate-wire/0.10.0/schemas/pty-channel-frame.json");
    let document: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("pty frame schema")).expect("JSON");
    let codes = document
        .get("oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|branch| {
            branch
                .pointer("/properties/kind/const")
                .and_then(Value::as_str)
                == Some("protocol-error")
        })
        .and_then(|branch| branch.get("x-b10x-codes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<String>>();
    assert!(
        !codes.is_empty(),
        "the pty protocol-error branch publishes no code list to read"
    );
    codes
}

/// Drives one client frame at a fresh pty attachment and returns the `protocol-error` code it gets.
async fn pty_protocol_error_code(frame: &Value) -> String {
    let harness = Harness::with_terminals().await;
    let (session_id, _exec) = harness.start_pty().await;
    let mut socket = harness
        .attach(&format!("/v1/sessions/{session_id}/attach"))
        .await;
    socket
        .send_text(serde_json::to_vec(frame).expect("client frame").as_slice())
        .await;
    let answer = socket.next_json().await;
    assert_eq!(
        answer["kind"], "protocol-error",
        "{frame} must be refused, not served: {answer}"
    );
    answer["code"]
        .as_str()
        .expect("protocol-error code")
        .to_owned()
}

/// A pty attachment that sends `close-input` is told so in the code minted for exactly that.
///
/// `substrate_wire::SESSION_INPUT_CLOSE_UNSERVED` was minted in the round before this one for one
/// condition — "a pty has no half-close; a client ends input with the terminal's own end-of-file
/// character" (`crates/substrate-wire/src/lib.rs:106-107`) — and the driver port raises it for that
/// condition (`crates/substrate-host/src/process.rs:742-749`). `0.10.0` publishes it on the pty
/// vocabulary's `protocol-error` branch, so a client that reads the contract expects it. The
/// attachment loop never reaches the driver: it short-circuits on `mode == SessionMode::Pty` and
/// answers `session.frame-invalid`, a literal
/// (`crates/substrate-daemon/src/app/sessions.rs:1040-1052`) that appears in **no** document of any
/// released bundle. Two entry points, one condition, two codes — the shape round 1 found for the
/// refusal *order* and round 2 fixed at the driver port, one level up.
///
/// Portable lane. No confinement backend, no cgroup delegation.
#[tokio::test(flavor = "multi_thread")]
async fn a_pty_attachment_that_closes_input_is_told_in_the_code_minted_for_it() {
    let code = pty_protocol_error_code(&json!({"kind": "close-input", "sequence": 1})).await;
    assert_eq!(
        code,
        substrate_wire::SESSION_INPUT_CLOSE_UNSERVED,
        "the driver port and 0.10.0's pty protocol-error branch both name this condition \
         session.input-close-unserved; the attachment loop answers something else"
    );
}

/// Every code a pty attachment can be sent is one the bundle it speaks publishes.
///
/// `check_pty_refusal_class` (`xtask/src/bundle.rs:494`) asks the released bundle to name each entry
/// of `substrate_wire::SESSION_PTY_REFUSAL_CODES`. It cannot ask the converse — that no code
/// *outside* the set reaches a pty client — because it reads documents and not code. This asks it
/// from the other end: drive each refusal an attachment can produce and check the code against the
/// list the contract publishes. A code a client can receive and cannot look up is a code nobody can
/// handle, which is the whole argument the class rule was built on.
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn every_protocol_error_code_a_pty_attachment_can_receive_is_published() {
    let published = published_pty_protocol_error_codes();
    let mut unpublished = Vec::new();
    for frame in [
        json!({"kind": "close-input", "sequence": 1}),
        json!({"kind": "resize", "sequence": 9, "window": {"columns": 80, "rows": 24}}),
        json!({
            "kind": "stdin",
            "sequence": 1,
            "content": {"encoding": "base64", "data": "@@@@"}
        }),
        json!({"kind": "signal", "sequence": 1, "signal": "TERM", "grace_ms": 60_001}),
        json!({"kind": "resize", "sequence": 1, "window": {"columns": 0, "rows": 24}}),
    ] {
        let code = pty_protocol_error_code(&frame).await;
        if !published.contains(&code) {
            unpublished.push(format!("{frame} is refused {code}"));
        }
    }
    assert!(
        unpublished.is_empty(),
        "0.10.0/schemas/pty-channel-frame.json publishes {published:?} for a pty attachment, and \
         these reach one anyway:\n{}",
        unpublished.join("\n")
    );
}

/// The same pty start, refused by name at both entry points — with the same name.
///
/// Round 1 found the two entry points disagreeing about which of two applicable refusals answers a
/// `mode: "pty"` start, and round 2 fixed it for one pair: the `sessions.pty` fact now outranks the
/// window shape at the driver port as well as at the daemon. The pair was fixed; the *class* was
/// not. `ProcessRuntime::start_pipe` still answers `session.wait-invalid`
/// (`crates/substrate-host/src/process.rs:284-290`) before it looks at the fact, while the daemon's
/// mode gate is outermost by construction and answers `session.pty-unserved`
/// (`crates/substrate-daemon/src/app/operations.rs:557-571`).
///
/// So a `wait: true` pty start on a deployment that has no terminals is told two different things
/// by two implementations of one contract, and the driver port's answer is the one the recorded
/// decision argues against in its own words: it invites the client to drop `wait` and retry into a
/// refusal it can never get past. This asserts only that the two agree, which is the part neither
/// port gets to decide alone.
///
/// Portable lane. `HostConfig::minimum` on a temporary directory is a host with no terminals, which
/// is the deployment the ordering decision is about.
#[tokio::test(flavor = "multi_thread")]
async fn the_two_entry_points_name_one_refusal_for_a_pty_start_that_earns_two() {
    let harness = Harness::open().await;
    let workspace = harness.create_workspace("01JPTYWORKSPACECREATE006").await;
    let mut body = pty_start(&workspace, Some(json!({"columns": 80, "rows": 24})));
    body["exec"]["wait"] = json!(true);
    let (status, refusal) = harness
        .call(
            Method::POST,
            "/v1/sessions",
            mutation("01JPTYSESSIONWAIT0000001", body),
        )
        .await;
    assert_ne!(status, StatusCode::ACCEPTED, "{refusal}");
    let over_http = refusal["error"]["code"]
        .as_str()
        .expect("refusal code")
        .to_owned();

    let directory = tempfile::tempdir().expect("temporary host root");
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    assert_eq!(
        driver.machine().facts.sessions_pty,
        None,
        "this case is about a deployment that never proved it can give a terminal"
    );
    std::fs::create_dir_all(driver.root().join("ws_pty_order")).expect("workspace directory");
    let input = PipeSessionStartInput {
        capture: substrate_wire::CaptureMode::Recorded,
        exec: ExecStartInput {
            writable_roots: Vec::new(),
            workspace: "ws_pty_order".to_owned(),
            argv: vec!["/bin/sh".to_owned()],
            env: substrate_wire::ExecEnvironment {
                allow: vec![],
                set: std::collections::BTreeMap::new(),
            },
            sandbox: substrate_wire::ConfinementRequest {
                capability_snapshot: driver.machine().snapshot.clone(),
                network: substrate_wire::NetworkMode::None,
                aperture: None,
                apertures: Vec::new(),
                profile: SandboxProfile::Workspace,
                required: true,
            },
            limits: substrate_wire::ExecLimits {
                timeout_ms: 10_000,
                output_bytes: 1_048_576,
                processes: 8,
                memory_bytes: 67_108_864,
                cpu_millis: 1_000,
            },
            wait: true,
            workspace_access: substrate_wire::WorkspaceAccess::ReadWrite,
            scratch: None,
            measurements: std::collections::BTreeSet::new(),
            read_only_roots: Vec::new(),
            read_only_files: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            lease_ttl_ms: Some(60_000),
            workdir: None,
        },
        input_limit_bytes: 1_048_576,
        frame_limit_bytes: 65_536,
        queued_frames: 16,
        mode: substrate_wire::SessionMode::Pty,
        window: Some(substrate_wire::PtyWindow {
            columns: 80,
            rows: 24,
        }),
    };
    let DispatchOutcome::NotDispatched(error) = driver
        .start_pipe_session("ex_ptywaitorder", "ws_pty_order", &input)
        .await
    else {
        panic!("a terminal must never be served as a pipe session instead");
    };
    assert_eq!(
        error.code, over_http,
        "the driver port and the daemon are two implementations of one refusal order, and a \
         request that earns several refusals must not be told a different one by each"
    );
}

/// Drives one client frame at a fresh **raw-pipe** attachment and returns the code it gets.
///
/// The mirror of `pty_protocol_error_code` next door, for the channel whose published vocabulary is
/// `schemas/pipe-channel-frame.json`.
async fn pipe_protocol_error_code(frame: &Value) -> String {
    let harness = Harness::open().await;
    let (session_id, _exec) = harness.start_pipe().await;
    let mut socket = harness
        .attach(&format!("/v1/sessions/{session_id}/attach"))
        .await;
    socket
        .send_text(serde_json::to_vec(frame).expect("client frame").as_slice())
        .await;
    let answer = socket.next_json().await;
    assert_eq!(
        answer["kind"], "protocol-error",
        "{frame} must be refused, not served: {answer}"
    );
    answer["code"]
        .as_str()
        .expect("protocol-error code")
        .to_owned()
}

/// The mirror of `a_pty_attachment_that_closes_input_is_told_in_the_code_minted_for_it`, one channel
/// over — and the half of that class round 3 did not close.
///
/// `schemas/pipe-channel-frame.json` has no `resize` branch in any released bundle (`0.4.0` through
/// `0.10.0`: `stdin`, `close-input`, `signal`, `output`, `truncated`, `exit`, `protocol-error`), so a
/// `resize` reaching a raw-pipe attachment is a frame outside the closed vocabulary of the mode that
/// attachment serves. `substrate_wire::SESSION_FRAME_INVALID` is defined as exactly that
/// (`crates/substrate-wire/src/lib.rs:117`), and `substrate_wire::SESSION_NOT_PTY` as "The exec this
/// operation names is not a pty session at all" (`:104`) — either is a true statement here.
///
/// The attachment loop says neither. It collapses the mode gate and the cell bound into one
/// condition — `if mode != SessionMode::Pty || !window.within_bounds()`
/// (`crates/substrate-daemon/src/app/sessions.rs:1017-1027`) — and answers
/// `session.resize-invalid`, whose published meaning is "A resize named a window outside 1..=1000
/// cells on an axis" (`crates/substrate-wire/src/lib.rs:101`) and whose message says "A resize names
/// 1 to 1000 cells on each axis of a pty session". The window below is 80x24: inside those bounds on
/// both axes. The client is told its window is out of range when the window is fine and the channel
/// is wrong, which is the one thing it cannot act on.
///
/// `session.not-pty` is published in this very vocabulary's `x-b10x-codes` and no path in this tree
/// emits it — the daemon's mode gate is the only caller of `resize_pty_session` and it never lets a
/// non-pty exec reach the driver's own `SESSION_NOT_PTY` branch (`process.rs:815-822`). This
/// condition is the emitter it was minted for.
///
/// Portable lane. No confinement backend, no cgroup delegation.
#[tokio::test(flavor = "multi_thread")]
async fn a_resize_on_a_raw_pipe_attachment_is_not_answered_as_an_out_of_bounds_window() {
    let code = pipe_protocol_error_code(
        &json!({"kind": "resize", "sequence": 1, "window": {"columns": 80, "rows": 24}}),
    )
    .await;
    assert!(
        code == substrate_wire::SESSION_FRAME_INVALID || code == substrate_wire::SESSION_NOT_PTY,
        "a raw-pipe attachment has no resize frame in any released vocabulary, and this window is \
         80x24 — inside 1..={}x1..={} on both axes. The answer must be a true statement: \
         {} (the frame is outside this mode's vocabulary) or {} (this exec is not a pty session). \
         It was {code}, whose published meaning is that the window is out of range.",
        substrate_wire::MAX_PTY_WINDOW_COLUMNS,
        substrate_wire::MAX_PTY_WINDOW_ROWS,
        substrate_wire::SESSION_FRAME_INVALID,
        substrate_wire::SESSION_NOT_PTY,
    );
}

/// `0.10.0`'s pty resize branch carries `x-b10x-out-of-bounds: "session.resize-invalid"`, and it
/// holds only for out-of-bounds windows that happen to fit in a `u16`.
///
/// The branch declares `columns` and `rows` as `{"type": "integer", "minimum": 1, "maximum": 1000}`
/// (`contracts/substrate-wire/0.10.0/schemas/pty-channel-frame.json`), and the annotation next to
/// them names the one code a window outside that range earns. 1001 earns it. 65536 does not:
/// `PipeClientFrame::Resize` carries a `PtyWindow` whose axes are `u16`
/// (`crates/substrate-wire/src/lib.rs:1456-1459`), so the frame fails `serde_json::from_slice`
/// before the loop's own bound is reached and the client is answered `session.frame-invalid`
/// (`crates/substrate-daemon/src/app/sessions.rs:907-916`).
///
/// So the boundary between "your window is out of bounds" and "your frame is not a frame" is at
/// 65535, a number no document in any released bundle names, and it sits inside a range the
/// published schema describes with one rule and one refusal code.
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn every_window_the_pty_resize_branch_declares_out_of_bounds_earns_the_code_it_publishes() {
    let mut wrong = Vec::new();
    for columns in [
        u64::from(substrate_wire::MAX_PTY_WINDOW_COLUMNS) + 1,
        u64::from(u16::MAX),
        u64::from(u16::MAX) + 1,
        100_000,
    ] {
        let frame = json!({
            "kind": "resize",
            "sequence": 1,
            "window": {"columns": columns, "rows": 24}
        });
        let code = pty_protocol_error_code(&frame).await;
        if code != substrate_wire::SESSION_RESIZE_INVALID {
            wrong.push(format!("columns {columns} is refused {code}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "the pty resize branch declares columns 1..=1000 and publishes \
         x-b10x-out-of-bounds: {}, so every window outside that range earns that code; these do \
         not:\n{}",
        substrate_wire::SESSION_RESIZE_INVALID,
        wrong.join("\n")
    );
}

/// The one member of the pty client vocabulary whose refusal is not in the pty server vocabulary.
///
/// Design 13 rates the new `resize` frame on the control window that already existed
/// (`docs/design/13-pty-sessions.md:97-99`), and `schemas/pty-channel-frame.json` says so on the
/// branch itself: `x-b10x-rated-on: "the-control-window"`. Everything else that branch's siblings
/// can get wrong — a window out of bounds, a bad sequence, a grace over the bound, an input frame
/// the driver refuses — is answered with a `protocol-error` frame carrying a code from that same
/// document's `x-b10x-codes`. Crossing the rate is answered with a WebSocket close, code 1008 and a
/// reason string (`crates/substrate-daemon/src/app/sessions.rs:1010-1016`): not a branch of the
/// published `oneOf`, not a code in `x-b10x-codes`, and not a word a client can look up.
///
/// The bound is unpublished too. `GET /v1/sessions` publishes every other bound a client has
/// to obey — `max_input_bytes`, `max_frame_bytes`, `max_queued_frames`, `max_window_columns`,
/// `max_window_rows` — and not `max_controls_per_window` / `control_window`
/// (`crates/substrate-daemon/src/app/sessions.rs:50-51`, `:65-66`). So the one bound a terminal
/// client is most likely to cross is the one it cannot read, and `Ping` shares the same budget:
/// a 500 ms keepalive spends all 120 of them in a minute and leaves a single resize to be closed on.
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn a_resize_over_the_control_rate_is_answered_in_the_published_vocabulary() {
    let harness = Harness::with_terminals().await;
    let (session_id, _exec) = harness.start_pty().await;
    let mut socket = harness
        .attach(&format!("/v1/sessions/{session_id}/attach"))
        .await;
    // One more than `PipeSessionPolicy::production().max_controls_per_window`, inside one window.
    for sequence in 1..=121_u64 {
        socket
            .send_text(
                serde_json::to_vec(&json!({
                    "kind": "resize",
                    "sequence": sequence,
                    "window": {"columns": 80, "rows": 24}
                }))
                .expect("resize frame")
                .as_slice(),
            )
            .await;
    }
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next_frame())
        .await
        .expect("a bounded server answer")
        .expect("a server answer");
    let described = if frame.opcode == 0x8 {
        let code = frame
            .payload
            .get(0..2)
            .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]));
        format!(
            "a websocket close, code {code:?}, reason {:?}",
            String::from_utf8_lossy(frame.payload.get(2..).unwrap_or_default())
        )
    } else {
        format!(
            "opcode {:#x}, payload {:?}",
            frame.opcode,
            String::from_utf8_lossy(&frame.payload)
        )
    };
    assert_eq!(
        frame.opcode, 0x1,
        "0.10.0/schemas/pty-channel-frame.json publishes a closed server vocabulary of output, exit \
         and protocol-error, and every other way a client frame can be refused answers with a \
         protocol-error carrying a published code. Crossing the rate the resize branch says it is \
         rated on answers with {described} — a bound no document publishes, refused in a word no \
         document names."
    );
    let answer: Value = serde_json::from_slice(&frame.payload).expect("server JSON frame");
    let published = published_pty_protocol_error_codes();
    let code = answer["code"].as_str().unwrap_or_default().to_owned();
    assert!(
        answer["kind"] == "protocol-error" && published.contains(&code),
        "the refusal must be a protocol-error in the published code set {published:?}: {answer}"
    );
}

// ---------------------------------------------------------------------------------------------
// Fifth adversarial pass. Cases added below this line; nothing above it was changed.
// ---------------------------------------------------------------------------------------------

/// The released result schema for the contract version a capability document names.
fn published_capability_schema(contract: &str) -> Value {
    let version = contract.strip_prefix("substrate-wire/").unwrap_or_else(|| {
        panic!("a capability document names a substrate-wire contract: {contract}")
    });
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("contracts/substrate-wire")
        .join(version)
        .join("schemas/results/pipe-session-capabilities.json");
    serde_json::from_slice(&std::fs::read(&path).unwrap_or_else(|error| {
        panic!("{contract} names a bundle with no capability result schema: {error}")
    }))
    .expect("JSON")
}

/// `GET /v1/sessions` answers with a body the contract the body itself names admits.
///
/// The operation registry binds `session.capabilities` to
/// `schemas/results/pipe-session-capabilities.json`, and the document answers with
/// `"contract": "substrate-wire/0.4.0"` — pinned by
/// `durable_pipe_start_single_attachment_and_terminal_output_are_scoped`
/// (`crates/substrate-daemon/tests/pipe_session.rs:786`) and written at
/// `crates/substrate-daemon/src/app/sessions.rs:190`. So the schema a client validates this body
/// against is `0.4.0`'s, and `0.4.0`'s is `additionalProperties: false` over nine properties.
///
/// Before this unit the body had exactly those nine and validated. This unit added five —
/// `modes`, `max_window_columns`, `max_window_rows`, `max_controls_per_window`, `control_window_ms`
/// — and left `contract` at `0.4.0`. The body now validates against no released bundle: not
/// `0.4.0`'s, which forbids the five, and not `0.10.0`'s, whose `contract` is
/// `{"const": "substrate-wire/0.10.0"}`.
///
/// Nothing catches it because `session.capabilities` has no executable vector: the only vector
/// naming it is `vectors/driver/pipe-session-capabilities.json`, which is absent from
/// `/conformance/executable_vectors`, and which expects `"contract": "substrate-wire/0.10.0"`.
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn the_capability_document_is_admitted_by_the_contract_it_names() {
    let harness = Harness::with_terminals().await;
    let (status, capabilities) = harness
        .call(Method::GET, "/v1/sessions", Body::empty())
        .await;
    assert_eq!(status, StatusCode::OK, "{capabilities}");
    let result = capabilities["result"]
        .as_object()
        .expect("a capability result object");
    let contract = result["contract"].as_str().expect("a named contract");
    let schema = published_capability_schema(contract);
    assert_eq!(
        schema["additionalProperties"],
        json!(false),
        "{contract}'s capability result schema is meant to be closed"
    );
    assert_eq!(
        schema
            .pointer("/properties/contract/const")
            .and_then(Value::as_str),
        Some(contract),
        "the body names {contract} and that bundle's schema names a different contract"
    );
    let declared: Vec<&String> = schema["properties"]
        .as_object()
        .expect("schema properties")
        .keys()
        .collect();
    let undeclared: Vec<&String> = result
        .keys()
        .filter(|field| !declared.iter().any(|known| known == field))
        .collect();
    assert!(
        undeclared.is_empty(),
        "GET /v1/sessions answers {contract}, whose result schema is closed over \
         {declared:?}. The body carries {undeclared:?} as well, so it is admitted by no released \
         bundle: {contract}'s schema forbids these properties, and the bundle that declares them \
         (0.10.0) requires \"contract\": \"substrate-wire/0.10.0\"."
    );
}

/// `x-b10x-out-of-bounds` holds for every integer the resize branch declares out of bounds, not
/// only for the ones that happen to be non-negative.
///
/// The sibling `every_window_the_pty_resize_branch_declares_out_of_bounds_earns_the_code_it_publishes`
/// closed the top of the range by widening the axes to `u64`, and
/// `PtyWindow::cells`'s own doc states the rule the widening was for: "every non-negative integer a
/// client can write decodes, and the only rule is the published one"
/// (`crates/substrate-wire/src/lib.rs:1512`). The published rule does not say non-negative. The
/// branch says `{"type": "integer", "minimum": 1, "maximum": 1000}` and annotates the whole window
/// `x-b10x-out-of-bounds: "session.resize-invalid"`, and JSON Schema's `integer` admits negatives —
/// so `-1` and `1001` are the same kind of thing to a client reading the contract: a well-typed
/// integer outside the declared range.
///
/// They are not the same thing to the daemon. `0` decodes and earns `session.resize-invalid`; `-1`
/// fails `serde_json::from_slice::<PipeClientFrame>` on a `u64` axis and earns
/// `session.frame-invalid` (`crates/substrate-daemon/src/app/sessions.rs:905-919`). The boundary
/// between "your window is out of range" and "your frame is not a frame" moved from 65535 to -1;
/// it is still a number no released document names, still inside a range the published schema
/// describes with one rule and one refusal code.
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn every_integer_window_the_pty_resize_branch_declares_out_of_bounds_earns_that_code() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("contracts/substrate-wire/0.10.0/schemas/pty-channel-frame.json");
    let document: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("pty frame schema")).expect("JSON");
    let branch = document["oneOf"]
        .as_array()
        .expect("a frame vocabulary")
        .iter()
        .find(|branch| {
            branch
                .pointer("/properties/kind/const")
                .and_then(Value::as_str)
                == Some("resize")
        })
        .expect("a resize branch");
    let published = branch
        .get("x-b10x-out-of-bounds")
        .and_then(Value::as_str)
        .expect("the resize branch names the code an out-of-bounds window earns");
    let minimum = branch
        .pointer("/properties/window/properties/columns/minimum")
        .and_then(Value::as_i64)
        .expect("a declared column minimum");
    assert_eq!(
        branch
            .pointer("/properties/window/properties/columns/type")
            .and_then(Value::as_str),
        Some("integer"),
        "the axis is declared as a JSON Schema integer, which admits negatives"
    );

    let mut wrong = Vec::new();
    for columns in [minimum - 1, minimum - 2, -1_000] {
        let frame = json!({
            "kind": "resize",
            "sequence": 1,
            "window": {"columns": columns, "rows": 24}
        });
        let code = pty_protocol_error_code(&frame).await;
        if code != published {
            wrong.push(format!("columns {columns} is refused {code}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "the pty resize branch declares columns as an integer in {minimum}..=1000 and publishes \
         x-b10x-out-of-bounds: {published}, so every integer outside that range earns that code. \
         columns {} does. These do not:\n{}",
        minimum - 1,
        wrong.join("\n")
    );
}

/// One attach handshake that is refused before the upgrade, with its refusal body.
///
/// `Handshake::open` next door keeps only the status line; a refusal's code lives in the body.
async fn refused_attach(address: SocketAddr, path: &str) -> (u16, Value) {
    let mut stream = TcpStream::connect(address)
        .await
        .expect("connect attach client");
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write attach handshake");
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 16 * 1_024, "bounded response head");
        let mut byte = [0_u8; 1];
        stream
            .read_exact(&mut byte)
            .await
            .expect("read response head");
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).expect("ASCII response head");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .expect("HTTP status");
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .expect("a refusal carries a content-length");
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .await
        .expect("read refusal body");
    (status, serde_json::from_slice(&body).expect("JSON refusal"))
}

/// Every refusal a pty attach can raise has a row in the register that says it lists them all.
///
/// `refusals.json`'s own title is "Every refusal a session can raise, and what a client does with
/// it", and the `0.10.0` CHANGELOG entry says it "gives every session refusal its class, where it
/// arrives, whether it is worth retrying, the sentence substrate sends".
///
/// `check_pty_refusal_class` (`xtask/src/bundle.rs:494`) enforces that claim only over
/// `substrate_wire::SESSION_PTY_REFUSAL_CODES` and `SESSION_PROTOCOL_ERROR_CODES` -- the codes
/// bound to a wire constant. The four attach-preflight refusals are written as literals in the
/// daemon -- `session.not-attachable` (`crates/substrate-daemon/src/app/sessions.rs:790`, `:863`),
/// `session.already-attached` (`:808`, `:847`) and `session.attachment-capacity` (`:824`) -- so
/// neither direction of that check sees them, and none of them has a row.
///
/// This is the refusal that enforces the `single_attachment: true` the capability document
/// publishes: the first attach flips the stored attachment state off `Available`, and the second is
/// refused at the gate above the capacity check. A terminal client whose websocket drops and
/// reconnects gets it, reads the code off the wire, and has nothing to look up -- not its class,
/// not whether retrying is worth anything, not whether waiting would help.
///
/// This same commit rewrote all four sentences to be mode-aware -- "The pty session is not running
/// under an active lease." is new here -- which is the change asserting a terminal client reaches
/// them.
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn every_refusal_a_pty_attach_can_raise_has_a_row_in_the_register() {
    let harness = Harness::with_terminals().await;
    let (session_id, _exec) = harness.start_pty().await;
    let path = format!("/v1/sessions/{session_id}/attach");
    let _held = harness.attach(&path).await;
    let (status, refusal) = refused_attach(harness.server.address, &path).await;
    assert_eq!(status, 409, "a second attach is refused: {refusal}");
    let code = refusal["error"]["code"]
        .as_str()
        .expect("a refusal code")
        .to_owned();

    let register: Value = serde_json::from_slice(
        &std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("contracts/substrate-wire/0.10.0/refusals.json"),
        )
        .expect("the refusal register"),
    )
    .expect("JSON");
    let rows: Vec<&str> = register["refusals"]
        .as_array()
        .expect("register rows")
        .iter()
        .filter_map(|row| row["code"].as_str())
        .collect();
    assert!(
        rows.contains(&code.as_str()),
        "a second attachment is refused with {code} and the message {:?}. \
         0.10.0/refusals.json says it lists every refusal a session can raise, and it lists \
         {rows:?} — {code} is not among them, so a client that receives it has nothing to look up.",
        refusal["error"]["message"]
    );
}

/// The same rule, on the request the pty workflow starts with.
///
/// `schemas/inputs/pipe-session-start.json` declares `window.columns` and `window.rows` as
/// `{"type": "integer", "minimum": 1, "maximum": 1000}`, and `refusals.json` gives the one refusal a
/// window outside that range earns: `session.window-invalid`, class `refused`, status 422, arriving
/// as an HTTP response. `a_window_is_required_for_pty_refused_for_pipes_and_never_defaulted` pins
/// `0` and `1001` on exactly that.
///
/// `-1` is the same kind of thing to the schema and a different thing to the daemon:
/// `PipeSessionStartInput::window` is an `Option<PtyWindow>` whose axes are `u64`
/// (`crates/substrate-wire/src/lib.rs:1490-1494`), so the body never reaches
/// `validate_session_window` and the client is answered for the shape of its request instead of the
/// value of its window. This is the start-path half of
/// `every_integer_window_the_pty_resize_branch_declares_out_of_bounds_earns_that_code`: one root
/// cause, two endpoints, and this is the one a terminal client meets first.
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn a_pty_start_window_outside_the_declared_range_is_refused_in_the_published_code() {
    let harness = Harness::with_terminals().await;
    let workspace = harness.create_workspace("01JPTYWORKSPACECREATE009").await;
    let mut wrong = Vec::new();
    for (operation, columns) in [
        ("01JPTYSTARTWINDOWZERO001", 0_i64),
        ("01JPTYSTARTWINDOWNEG0001", -1_i64),
        ("01JPTYSTARTWINDOWNEG0002", -1_000_i64),
    ] {
        let mut input = pty_start(&workspace, Some(json!({"columns": columns, "rows": 24})));
        input["mode"] = json!("pty");
        let (status, refusal) = harness
            .call(Method::POST, "/v1/sessions", mutation(operation, input))
            .await;
        let code = refusal["error"]["code"].as_str().unwrap_or_default();
        if code != "session.window-invalid" {
            wrong.push(format!(
                "columns {columns} is refused {code} ({status}), address {}",
                refusal["error"]["address"]
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "0.10.0/schemas/inputs/pipe-session-start.json declares the window axes as integers in \
         1..=1000 and 0.10.0/refusals.json gives session.window-invalid as the 422 a bad window \
         earns. columns 0 earns it. These do not:\n{}",
        wrong.join("\n")
    );
}

/// One masked, empty websocket ping from the client.
async fn send_ping(client: &mut WebSocketClient) {
    let mask = [0x11, 0x22, 0x33, 0x44];
    let mut encoded = vec![0x89, 0x80];
    encoded.extend_from_slice(&mask);
    client
        .stream
        .write_all(&encoded)
        .await
        .expect("write websocket ping frame");
}

/// The budget the capability document publishes is one budget; crossing it must have one answer.
///
/// `a_resize_over_the_control_rate_is_answered_in_the_published_vocabulary` closed the resize half:
/// crossing the rate with a `resize` now earns `session.control-rate-exceeded`, a code
/// `refusals.json` gives a row and both frame vocabularies list in `x-b10x-codes`. The 1008 close
/// was kept for websocket control frames on the stated ground that they "are not part of the
/// published `oneOf` and have no frame to answer in"
/// (`crates/substrate-daemon/src/app/sessions.rs:1051-1055`).
///
/// The loop contradicts that ground two arms above. A `Binary` client message is also outside the
/// published client `oneOf`, and it is answered with a `protocol-error` text frame carrying
/// `session.frame-invalid` (`:965-980`). The server's vocabulary is what the *server* sends; it does
/// not need the client's message to have been a member.
///
/// And ping is the half a terminal client actually crosses. `PipeSessionCapabilities`'s own doc
/// says so — "`Ping` shares the budget, so a client choosing a keepalive interval needs both
/// numbers" (`crates/substrate-wire/src/lib.rs:1603-1610`) — and the `0.10.0` CHANGELOG repeats it.
/// A client that reads `max_controls_per_window: 120` and `control_window_ms: 60000` off the
/// document, picks a keepalive, and gets it slightly wrong crosses the rate with a ping. It is then
/// answered with a bare 1008 close carrying "pty control-frame rate exceeded": not a branch of the
/// published `oneOf`, not a code in `x-b10x-codes`, and not a word `refusals.json` has a row for —
/// which is the exact shape of the defect the resize half was fixed for.
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn crossing_the_control_rate_with_a_ping_is_answered_in_the_published_vocabulary() {
    let harness = Harness::with_terminals().await;
    let (session_id, _exec) = harness.start_pty().await;
    let mut socket = harness
        .attach(&format!("/v1/sessions/{session_id}/attach"))
        .await;
    // One more than the `max_controls_per_window` the capability document publishes, inside one
    // published `control_window_ms`.
    for _ping in 0..=substrate_wire::MAX_SESSION_CONTROLS_PER_WINDOW {
        send_ping(&mut socket).await;
    }
    let frame = loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next_frame())
            .await
            .expect("a bounded server answer")
            .expect("a server answer");
        // The transport's own pong for each ping is not an answer to the rate.
        if frame.opcode != 0xa {
            break frame;
        }
    };
    let described = if frame.opcode == 0x8 {
        let code = frame
            .payload
            .get(0..2)
            .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]));
        format!(
            "a websocket close, code {code:?}, reason {:?}",
            String::from_utf8_lossy(frame.payload.get(2..).unwrap_or_default())
        )
    } else {
        format!(
            "opcode {:#x}, payload {:?}",
            frame.opcode,
            String::from_utf8_lossy(&frame.payload)
        )
    };
    assert_eq!(
        frame.opcode,
        0x1,
        "GET /v1/sessions publishes max_controls_per_window {} over control_window_ms {}, \
         ping shares that budget, and 0.10.0/refusals.json gives crossing it exactly one row: \
         {} arriving as a protocol-error-frame. Crossing it with a ping answered with {described}.",
        substrate_wire::MAX_SESSION_CONTROLS_PER_WINDOW,
        substrate_wire::SESSION_CONTROL_WINDOW_MS,
        substrate_wire::SESSION_CONTROL_RATE_EXCEEDED,
    );
    let answer: Value = serde_json::from_slice(&frame.payload).expect("server JSON frame");
    let published = published_pty_protocol_error_codes();
    let code = answer["code"].as_str().unwrap_or_default().to_owned();
    assert!(
        answer["kind"] == "protocol-error" && published.contains(&code),
        "the refusal must be a protocol-error in the published code set {published:?}: {answer}"
    );
}

/// A frame the published pty vocabulary admits is not "outside the closed pty vocabulary".
///
/// JSON Schema 2020-12 defines `"type": "integer"` as "any number with a zero fractional part", so
/// `80.0` is an integer to every conforming validator, and
/// `0.10.0/schemas/pty-channel-frame.json`'s resize branch declares the axes
/// `{"type": "integer", "minimum": 1, "maximum": 1000}`. `{"columns": 80.0, "rows": 24.0}` is
/// therefore a valid member of the published client vocabulary, in bounds on both axes, and the
/// only published answer to it is that the window is applied.
///
/// `PtyWindow`'s axes are `u64` (`crates/substrate-wire/src/lib.rs:1492-1493`), and `serde_json`
/// will not read `80.0` into one, so the frame fails `from_slice::<PipeClientFrame>` and the client
/// is told `session.frame-invalid` — whose published meaning is "The client frame is outside the
/// closed vocabulary of the mode this attachment serves" (`0.10.0/refusals.json`). The frame is
/// inside it. Any client whose window came out of a division rather than a literal writes this:
/// `json.dumps({"columns": width / 2})` in Python is `80.0`.
///
/// Same root cause as the two cases above — the refusal is decided by a Rust integer type rather
/// than by the published rule — and a different consequence: not a true refusal in the wrong code,
/// but a false statement about an admitted frame.
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn a_resize_whose_axes_have_a_zero_fractional_part_is_inside_the_published_vocabulary() {
    let harness = Harness::with_terminals().await;
    let (session_id, exec_id) = harness.start_pty().await;
    let mut socket = harness
        .attach(&format!("/v1/sessions/{session_id}/attach"))
        .await;
    let frame = "{\"kind\":\"resize\",\"sequence\":1,\"window\":{\"columns\":132.0,\"rows\":43.0}}";
    socket.send_text(frame.as_bytes()).await;
    // Scaffolding only, adapted in round 5: an *accepted* resize has no answer in the published
    // server vocabulary — `output`, `exit`, `protocol-error` and nothing else — so waiting for a
    // frame and unwrapping it made the case unreachable once the frame stopped being refused. The
    // deadline is now one more thing `described` can report; every assertion below is unchanged.
    let described = match tokio::time::timeout(Duration::from_secs(5), socket.next_frame()).await {
        Err(_deadline) => "no frame at all, and the attachment stayed open".to_owned(),
        Ok(None) => "the connection closed with no frame at all".to_owned(),
        Ok(Some(frame)) if frame.opcode == 0x1 => {
            format!("{}", String::from_utf8_lossy(&frame.payload))
        }
        Ok(Some(frame)) => format!("opcode {:#x}", frame.opcode),
    };
    assert_eq!(
        harness.resizes(&exec_id),
        vec![substrate_wire::PtyWindow {
            columns: 132,
            rows: 43
        }],
        "0.10.0/schemas/pty-channel-frame.json declares the resize axes as JSON Schema integers in \
         1..=1000, and 132.0 and 43.0 are integers with a zero fractional part to every conforming \
         validator. {frame} is an admitted member of the published client vocabulary with a window \
         in bounds on both axes, so the only published outcome is that the window reaches the \
         terminal. Substrate answered: {described}"
    );
}

// ---------------------------------------------------------------------------------------------
// Sixth adversarial pass. Cases added below this line; nothing above it was changed.
// ---------------------------------------------------------------------------------------------

/// The rule round 5 adopted for the window axes, on the field beside them.
///
/// `a_resize_whose_axes_have_a_zero_fractional_part_is_inside_the_published_vocabulary` above
/// established the rule and round 5 implemented it: the published axis is
/// `{"type": "integer", "minimum": 1, "maximum": 1000}`, JSON Schema 2020-12 defines `integer` as
/// any number with a zero fractional part, so `132.0` is an admitted member of the closed client
/// vocabulary and refusing it as "outside the closed pty vocabulary" is a false statement about an
/// admitted frame. `PtyWindow` grew a hand-written `Deserialize` to say so
/// (`crates/substrate-wire/src/lib.rs:1588-1636`).
///
/// `sequence` sits in the same branch of the same document, declared
/// `{"type": "integer", "minimum": 1, "maximum": 18446744073709551615}` — the same keyword, the same
/// reading. It is still a plain `u64` on `PipeClientFrame`, so `serde_json` refuses `1.0` and the
/// whole frame fails `from_slice::<PipeClientFrame>` at
/// `crates/substrate-daemon/src/app/sessions.rs:936-951`, before any window is looked at. The
/// client is told `session.frame-invalid` about a frame the published schema admits, which is the
/// exact statement the round-5 fix was made to stop making.
///
/// Wider than the window rule, not narrower: `sequence` is on **every** client frame of **both**
/// vocabularies — `stdin`, `close-input`, `signal`, `resize` — so this reaches the released
/// `pipes` workflow and not only the new `pty` one.
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_sequence_with_a_zero_fractional_part_is_inside_the_published_vocabulary() {
    let harness = Harness::with_terminals().await;
    let (session_id, exec_id) = harness.start_pty().await;
    let mut socket = harness
        .attach(&format!("/v1/sessions/{session_id}/attach"))
        .await;
    let frame = "{\"kind\":\"resize\",\"sequence\":1.0,\"window\":{\"columns\":132,\"rows\":43}}";
    socket.send_text(frame.as_bytes()).await;
    let described = match tokio::time::timeout(Duration::from_secs(5), socket.next_frame()).await {
        Err(_deadline) => "no frame at all, and the attachment stayed open".to_owned(),
        Ok(None) => "the connection closed with no frame at all".to_owned(),
        Ok(Some(frame)) if frame.opcode == 0x1 => {
            String::from_utf8_lossy(&frame.payload).into_owned()
        }
        Ok(Some(frame)) => format!("opcode {:#x}", frame.opcode),
    };
    assert_eq!(
        harness.resizes(&exec_id),
        vec![substrate_wire::PtyWindow {
            columns: 132,
            rows: 43
        }],
        "0.10.0/schemas/pty-channel-frame.json declares every client frame's sequence as a JSON \
         Schema integer in 1..=18446744073709551615, and 1.0 is an integer with a zero fractional \
         part to every conforming validator — the same reading round 5 adopted for the window axes \
         one property over. {frame} is an admitted member of the published client vocabulary with \
         a first sequence and an in-bounds window, so the only published outcome is that the \
         window reaches the terminal. Substrate answered: {described}"
    );
}

/// Reads the bounded JSON failure returned when a WebSocket upgrade is refused.
async fn refused_upgrade(address: SocketAddr, path: &str) -> (u16, Value) {
    let mut stream = TcpStream::connect(address)
        .await
        .expect("connect refused-upgrade client");
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write refused-upgrade handshake");
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 16 * 1_024, "bounded handshake response");
        let mut byte = [0_u8; 1];
        stream
            .read_exact(&mut byte)
            .await
            .expect("read refused-upgrade head");
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
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .expect("a refusal carries a bounded JSON body");
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .await
        .expect("read refused-upgrade body");
    (
        status,
        serde_json::from_slice(&body).expect("JSON refusal body"),
    )
}

impl Harness {
    /// Starts one pty session under a caller-chosen operation id, avoiding replay in capacity tests.
    async fn start_pty_as(&self, workspace: &str, operation: &str) -> String {
        self.start_pty_for(&identity(), workspace, operation).await
    }

    /// [`Harness::start_pty_as`] in the scope of `identity`'s subject.
    async fn start_pty_for(&self, identity: &Identity, workspace: &str, operation: &str) -> String {
        let (status, session) = self
            .call_as(
                identity,
                Method::POST,
                "/v1/sessions",
                mutation(
                    operation,
                    pty_start(workspace, Some(json!({"columns": 80, "rows": 24}))),
                ),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{session}");
        session["result"]["id"]
            .as_str()
            .expect("pty session id")
            .to_owned()
    }

    /// One subject's view of this daemon: a listener that admits every connection as that
    /// subject, as a listener admits one peer, and a workspace in its scope.
    async fn subject_view(&self, subject: &str, workspace_operation: &str) -> SubjectView {
        let identity = identity_of(subject);
        let workspace = self
            .create_workspace_for(&identity, workspace_operation)
            .await;
        let server = TestServer::spawn_as(Arc::clone(&self.app), identity.clone()).await;
        SubjectView {
            identity,
            server,
            workspace,
        }
    }

    /// Starts a pty session in `view`'s workspace and returns its attach path.
    async fn attach_path_for(&self, view: &SubjectView, operation: &str) -> String {
        let session = self
            .start_pty_for(&view.identity, &view.workspace, operation)
            .await;
        format!("/v1/sessions/{session}/attach")
    }
}

struct SubjectView {
    identity: Identity,
    server: TestServer,
    workspace: String,
}

/// Which of two subject views holds the attachment numbered `index` when the deployment's 32
/// are filled at the per-subject cap: the first sixteen are the first subject's.
fn holder(index: u32) -> usize {
    usize::from(index >= ATTACHMENTS_PER_SUBJECT)
}

fn published_refusal_row(code: &str) -> Value {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("contracts/substrate-wire/0.10.0/refusals.json");
    let register: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("the refusal register")).expect("JSON");
    register["refusals"]
        .as_array()
        .expect("register rows")
        .iter()
        .find(|row| row["code"].as_str() == Some(code))
        .cloned()
        .unwrap_or_else(|| panic!("{code} has no row in 0.10.0/refusals.json"))
}

/// The attach path uses the same per-code retry fact that the published refusal register uses.
///
/// The deployment's 32 attachments are held by two subjects at their per-subject cap of 16 (ADR
/// 0042 decision 4), so the thirty-third, from a third subject that holds none, meets the global
/// cap and nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn the_attachment_capacity_refusal_carries_the_retriable_the_register_publishes() {
    let harness = Harness::with_terminals().await;
    let views = [
        harness
            .subject_view(SUBJECT, "01JADV7WORKSPACE00000001")
            .await,
        harness
            .subject_view(OTHER_SUBJECT, "01JADV7WORKSPACE00000002")
            .await,
    ];
    let mut held = Vec::new();
    for index in 0..32_u32 {
        let view = &views[holder(index)];
        let path = harness
            .attach_path_for(view, &format!("01JADV7ATTACHCAPACITY{index:04}"))
            .await;
        held.push(Handshake::open(view.server.address, &path).await.upgraded());
    }
    let third = harness
        .subject_view(THIRD_SUBJECT, "01JADV7WORKSPACE00000003")
        .await;
    let overflow = harness
        .attach_path_for(&third, "01JADV7ATTACHCAPACITYXX1")
        .await;
    let (status, body) = refused_upgrade(third.server.address, &overflow).await;
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(substrate_wire::SESSION_ATTACHMENT_CAPACITY),
        "the thirty-third attachment must be the capacity refusal: {body}"
    );
    let row = published_refusal_row(substrate_wire::SESSION_ATTACHMENT_CAPACITY);
    assert_eq!(body["error"]["class"], row["class"]);
    assert_eq!(json!(status), row["status"]);
    assert_eq!(body["error"]["retriable"], row["retriable"]);
}

/// ADR 0042 decision 4, the per-subject attachment cap, on the served route.
///
/// Once an upgraded connection hands its transport slot to its attachment, no transport bound
/// stands between one subject and the deployment's 32 attachments, which it could then hold for
/// the hour and deny every other subject `session.attachment-capacity`. One subject attaches
/// sixteen sessions; its seventeenth is refused `429 session.attachment-capacity` while the global
/// cap still has sixteen free, another subject still attaches, and the first subject's refused
/// session attaches once one of its own attachments has ended.
#[tokio::test(flavor = "multi_thread")]
async fn one_subject_is_refused_at_its_attachment_cap_while_another_subject_still_attaches() {
    let harness = Harness::with_terminals().await;
    let first = harness
        .subject_view(SUBJECT, "01JCAPSUBJWORKSPACE00001")
        .await;
    let second = harness
        .subject_view(OTHER_SUBJECT, "01JCAPSUBJWORKSPACE00002")
        .await;
    let mut held = Vec::new();
    for index in 0..ATTACHMENTS_PER_SUBJECT {
        let path = harness
            .attach_path_for(&first, &format!("01JCAPSUBJATTACHED{index:06}"))
            .await;
        held.push(
            Handshake::open(first.server.address, &path)
                .await
                .upgraded(),
        );
    }
    let over = harness
        .attach_path_for(&first, "01JCAPSUBJOVERTHECAP0001")
        .await;
    let (status, body) = refused_upgrade(first.server.address, &over).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (429, Some(substrate_wire::SESSION_ATTACHMENT_CAPACITY)),
        "one subject's seventeenth attachment is over its per-subject cap: {body}"
    );
    assert_eq!(
        body["error"]["retriable"],
        json!(substrate_wire::session_refusal_is_retriable(
            substrate_wire::SESSION_ATTACHMENT_CAPACITY
        )),
        "the per-subject refusal carries the retriable the register publishes: {body}"
    );

    let other = harness
        .attach_path_for(&second, "01JCAPSUBJOTHERSUBJECT01")
        .await;
    let handshake = Handshake::open(second.server.address, &other).await;
    assert_eq!(
        handshake.status, 101,
        "another subject attaches while the first is at its cap"
    );
    let _other = handshake.upgraded();

    drop(held.pop());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let handshake = Handshake::open(first.server.address, &over).await;
        if handshake.status == 101 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the first subject's refused session is still answered {} ten seconds after one of \
             its attachments ended",
            handshake.status
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The acceptance sentence of `story:unattached-claimed-session-is-contained`, on the server shape
/// production actually runs.
///
/// The story asks that "a client that drops after the attachment claim and before the upgrade
/// leaves no running process within one maintenance tick, and the session reads as terminal with a
/// named refusal". The case this unit shipped builds that state from the *server* side, with a
/// `serve_connection` that never calls `with_upgrades()`; all three production listeners
/// (`crates/substrate-daemon/src/runtime.rs:595`, `:918`, `:1083`) do call it, so hyper reaches
/// `pending.manual()` there and never in the daemon. This is the same sentence with the client
/// doing the dropping and the server built the way production builds it.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_a_client_that_drops_after_the_switch_leaves_no_running_process() {
    let harness = Harness::open().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    let handshake = Handshake::open(
        harness.server.address,
        &format!("/v1/sessions/{session_id}/attach"),
    )
    .await;
    assert_eq!(
        handshake.status, 101,
        "the attach handshake is answered before the client can drop"
    );
    drop(handshake);

    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (status, observed) = harness
                .call(Method::GET, &format!("/v1/execs/{exec_id}"), Body::empty())
                .await;
            assert_eq!(status, StatusCode::OK);
            if observed["result"]["state"] == "cancelled" {
                return observed;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect(
        "a client that drops straight after the 101 on a with_upgrades() listener must leave no \
         running process",
    );
    assert_eq!(observed["result"]["exit"]["signal"], "KILL");

    let (status, session) = harness
        .call(
            Method::GET,
            &format!("/v1/sessions/{session_id}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(session["result"]["state"], "cancelled", "{session}");

    let (status, refusal) = refused_upgrade(
        harness.server.address,
        &format!("/v1/sessions/{session_id}/attach"),
    )
    .await;
    assert_eq!(status, 409, "{refusal}");
    assert_eq!(
        refusal["error"]["code"].as_str(),
        Some(substrate_wire::SESSION_NOT_ATTACHABLE),
        "{refusal}"
    );
    let row = published_refusal_row(substrate_wire::SESSION_NOT_ATTACHABLE);
    assert_eq!(refusal["error"]["class"], row["class"], "{refusal}");
    assert_eq!(json!(status), row["status"], "{refusal}");
    assert_eq!(refusal["error"]["retriable"], row["retriable"], "{refusal}");
}

/// One stranded attach whose kill could not be proven must not cost the daemon an attachment slot
/// for the rest of its life -- the **failed-upgrade** half of that guarantee.
///
/// A driver that cannot prove its kill is not a fixture curiosity: `terminate_pipe_session`
/// reports `false` whenever `driver.signal` errors, exceeds `MAINTENANCE_DRIVER_TIMEOUT`, or its
/// observation cannot be written. What must not happen then is a permit that fails to return its
/// slot, because `PipeAttachmentPermit::drop` is the only thing that returns one and the fixed
/// global bound is 32. Thirty-two such events without it and the daemon serves no attachment at
/// all, answering `session.attachment-capacity` -- published as *exhausted* and *retriable* -- for
/// capacity that is destroyed and that no retry recovers.
///
/// This case is the reason `PipeAttachmentPermit` no longer has a `remove_key_on_drop` flag, a
/// `retain_attachment_tombstone` method or a `global.forget()` arm: it went red against the first
/// shape of the `on_failed_upgrade` containment, which retained a tombstone the durable claim had
/// already made unreachable.
/// `adversary_a_served_attachment_with_an_unproven_kill_keeps_the_bounded_capacity` below is the
/// same property on the served-attachment path, which carried the same defect on `617bbed`.
///
/// `the_attachment_capacity_refusal_carries_the_retriable_the_register_publishes` above pins that a
/// healthy daemon serves all 32 at once to two subjects of sixteen each, so 32 is the number this
/// case is entitled to.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_a_stranded_attach_with_an_unproven_kill_keeps_the_bounded_capacity() {
    let harness = Harness::with_terminals().await;
    let workspace = harness.create_workspace("01JADVU3WORKSPACE0000001").await;
    let stranded = harness
        .start_pty_as(&workspace, "01JADVU3STRANDEDSESSION1")
        .await;
    harness
        .driver
        .refuse_signal
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let stranding = TestServer::spawn_without_upgrades(Arc::clone(&harness.app)).await;
    let handshake = Handshake::open(
        stranding.address,
        &format!("/v1/sessions/{stranded}/attach"),
    )
    .await;
    assert_eq!(
        handshake.status, 101,
        "the attach handshake is answered before the upgrade can fail"
    );
    drop(handshake);

    // The containment task runs, cannot prove the kill, and drops the permit. One second is far
    // longer than a refused fixture signal takes.
    tokio::time::sleep(Duration::from_secs(1)).await;
    harness
        .driver
        .refuse_signal
        .store(false, std::sync::atomic::Ordering::Relaxed);

    // The 32 slots are taken at the per-subject cap (ADR 0042 decision 4): the stranding subject
    // takes sixteen, so a slot its stranded permit kept would fail its own cap as well as the
    // global one, and a second subject takes the other sixteen.
    let views = [
        harness
            .subject_view(SUBJECT, "01JADVU3WORKSPACE0000002")
            .await,
        harness
            .subject_view(OTHER_SUBJECT, "01JADVU3WORKSPACE0000003")
            .await,
    ];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut held = Vec::new();
    for index in 0..32_u32 {
        let view = &views[holder(index)];
        let path = harness
            .attach_path_for(view, &format!("01JADVU3CAPACITYSLOT{index:04}"))
            .await;
        loop {
            let handshake = Handshake::open(view.server.address, &path).await;
            if handshake.status == 101 {
                held.push(handshake.upgraded());
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "attachment {index} of the bounded 32 is still answered {} twenty seconds after \
                 one stranded attach whose kill could not be proven: a permit failed to return \
                 its slot on drop, so one of this daemon's 32 attachment slots is gone until \
                 restart. session.attachment-capacity is published as exhausted and retriable; \
                 destroyed capacity is neither.",
                handshake.status
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    assert_eq!(
        held.len(),
        32,
        "all 32 bounded attachment slots are servable"
    );
    drop(stranding);
}

/// An unproven containment kill is retried, so a stranded exec ends inside the maintenance window,
/// with its lease nowhere near expiry.
///
/// **This case is the inversion of the one that pinned the defect**,
/// `a_stranded_attach_whose_kill_is_unproven_is_only_ended_by_lease_expiry`. That case asserted
/// that the failed-upgrade containment was one `terminate_pipe_session` call, that nothing retried
/// a refused kill, and that only `cleanup_expired` acting on the expired lease ended the process —
/// the very "until its lease or timeout ended it" `story:unattached-claimed-session-is-contained`
/// calls the defect. `story:session-containment-retries-an-unproven-kill` landed the retry, and the
/// case went red as its own documentation said it would.
///
/// The setup and the window are unchanged: the same transient 500 ms refusal, the same 20
/// maintenance ticks 25 ms apart, the same sixty-second lease, and no renewal. What is inverted is
/// the outcome: the exec must now be `cancelled` by a `KILL` inside that window — not `expired`,
/// which is what lease cleanup records.
#[tokio::test(flavor = "multi_thread")]
async fn a_stranded_attach_whose_kill_is_unproven_is_contained_before_lease_expiry() {
    let harness = Harness::open().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    harness
        .driver
        .refuse_signal
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let stranding = harness.strand(&session_id).await;

    // The refusal is transient: the driver is healthy again long before the window below closes.
    tokio::time::sleep(Duration::from_millis(500)).await;
    harness
        .driver
        .refuse_signal
        .store(false, std::sync::atomic::Ordering::Relaxed);

    // The same maintenance ticks under a lease that is nowhere near expiry. The exec must leave
    // `running` inside them, because the containment step retried the refused kill.
    let mut contained = None;
    for _tick in 0..20_u32 {
        harness.app.sweep_expired().await;
        let observed = harness.exec(&exec_id).await;
        if observed["result"]["state"] != "running" {
            contained = Some(observed);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let observed = contained.expect(
        "twenty maintenance ticks after the driver recovered, a stranded exec whose first \
         containment kill was refused is still running: nothing retried the unproven kill, so \
         only lease expiry would end it",
    );
    assert_eq!(
        observed["result"]["state"], "cancelled",
        "the containment kill, not lease cleanup, must end the exec: {observed}"
    );
    assert_eq!(observed["result"]["exit"]["signal"], "KILL", "{observed}");
    assert_eq!(
        harness.app.pending_containments(),
        0,
        "a proven kill leaves no pending containment"
    );
    drop(stranding);
}

/// The failed-upgrade hand-off: a first kill the driver refuses is retried by the next containment
/// round once the driver recovers, and that one round leaves no running process.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_upgrade_whose_first_kill_is_refused_is_contained_by_the_next_round() {
    let harness = Harness::driven().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    harness.driver.refuse_signals.store(1, Ordering::SeqCst);

    let stranding = harness.strand(&session_id).await;
    harness.await_pending_containments(1).await;
    harness.spend_refused_kills().await;

    let observed = harness.exec(&exec_id).await;
    assert_eq!(
        observed["result"]["state"], "running",
        "the refused kill proved nothing: {observed}"
    );
    assert_eq!(
        harness.app.pending_containments(),
        1,
        "an unproven containment stays pending rather than being forgotten"
    );

    harness.app.contain_pending().await;
    let observed = harness.exec(&exec_id).await;
    assert_eq!(
        observed["result"]["state"], "cancelled",
        "one containment round after the driver recovered must leave no running process: \
         {observed}"
    );
    assert_eq!(observed["result"]["exit"]["signal"], "KILL", "{observed}");
    assert_eq!(harness.app.pending_containments(), 0);
    drop(stranding);
}

/// A failed upgrade's containment is performed by the containment step, `App::contain_pending`,
/// and by nothing the attach handler spawns.
///
/// No round runs in this harness unless the case calls one, so the exec is still running, and its
/// driver has seen no kill, until the one round the case drives — and that round contains it.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_upgrade_is_contained_by_one_driven_containment_round() {
    let harness = Harness::driven().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    let stranding = harness.strand(&session_id).await;
    harness.await_pending_containments(1).await;
    assert_eq!(
        harness.driver.signal_calls.load(Ordering::SeqCst),
        0,
        "no kill is attempted outside the containment step"
    );
    assert_eq!(harness.exec(&exec_id).await["result"]["state"], "running");

    harness.app.contain_pending().await;
    let observed = harness.exec(&exec_id).await;
    assert_eq!(
        observed["result"]["state"], "cancelled",
        "one containment round contains a failed upgrade: {observed}"
    );
    assert_eq!(observed["result"]["exit"]["signal"], "KILL", "{observed}");
    assert_eq!(harness.app.pending_containments(), 0);
    drop(stranding);
}

/// The served-attachment hand-off: an attachment that ends without a terminal observation, whose
/// kill the driver refuses once, is contained by the next containment round.
#[tokio::test(flavor = "multi_thread")]
async fn a_served_attachment_whose_first_kill_is_refused_is_contained_by_the_next_round() {
    let harness = Harness::driven().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    let client = harness
        .attach(&format!("/v1/sessions/{session_id}/attach"))
        .await;
    harness.driver.refuse_signals.store(1, Ordering::SeqCst);
    // The client goes away: `run_pipe_attachment` reads EOF and reports a non-terminal attachment,
    // and the `on_upgrade` hand-off's own kill is refused.
    drop(client);
    harness.await_pending_containments(1).await;
    harness.spend_refused_kills().await;

    let observed = harness.exec(&exec_id).await;
    assert_eq!(
        observed["result"]["state"], "running",
        "the refused kill proved nothing: {observed}"
    );

    harness.app.contain_pending().await;
    let observed = harness.exec(&exec_id).await;
    assert_eq!(
        observed["result"]["state"], "cancelled",
        "one containment round after the driver recovered must leave no running process: \
         {observed}"
    );
    assert_eq!(observed["result"]["exit"]["signal"], "KILL", "{observed}");
    assert_eq!(harness.app.pending_containments(), 0);
}

/// A kill the driver never answers holds its containment attempt, and nothing else.
///
/// `App::sweep_expired` returns while that attempt is still pending in the driver, a second round
/// does not attempt the same exec twice, and an attempt cancelled with the task that ran it leaves
/// the exec pending for the next round rather than marked in flight forever.
#[tokio::test(flavor = "multi_thread")]
async fn sweep_expired_returns_while_a_containment_kill_is_unanswered() {
    let harness = Harness::driven().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    harness.driver.refuse_signals.store(1, Ordering::SeqCst);
    let stranding = harness.strand(&session_id).await;
    harness.await_pending_containments(1).await;
    harness.spend_refused_kills().await;

    harness.driver.hang_signal.store(true, Ordering::SeqCst);
    let calls = harness.driver.signal_calls.load(Ordering::SeqCst);
    let round_app = Arc::clone(&harness.app);
    let round = tokio::spawn(async move { round_app.contain_pending().await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while harness.driver.signal_calls.load(Ordering::SeqCst) == calls {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the containment round reaches the driver's kill");

    tokio::time::timeout(Duration::from_secs(5), harness.app.sweep_expired())
        .await
        .expect("sweep_expired must not wait behind a containment kill the driver never answers");
    assert!(
        !round.is_finished(),
        "the containment attempt is still pending in the driver"
    );
    assert_eq!(harness.app.pending_containments(), 1);

    tokio::time::timeout(Duration::from_secs(5), harness.app.contain_pending())
        .await
        .expect("a round with every pending entry already in flight returns at once");
    assert_eq!(
        harness.driver.signal_calls.load(Ordering::SeqCst),
        calls + 1,
        "an entry with an attempt in flight is not attempted twice"
    );
    assert_eq!(harness.exec(&exec_id).await["result"]["state"], "running");

    round.abort();
    assert!(
        round
            .await
            .expect_err("the hung round is cancelled")
            .is_cancelled(),
        "the hung round ends by cancellation"
    );
    harness.driver.hang_signal.store(false, Ordering::SeqCst);
    harness.app.contain_pending().await;
    let observed = harness.exec(&exec_id).await;
    assert_eq!(
        observed["result"]["state"], "cancelled",
        "a cancelled attempt leaves its exec pending for the next round: {observed}"
    );
    assert_eq!(harness.app.pending_containments(), 0);
    drop(stranding);
}

/// Every maintenance round asks the driver to kill again the exec trees it ended without proving
/// their cgroups empty, so such a tree is killed within one round of its lease expiring, not at
/// the next daemon start (`story:an-unknown-exec-is-killed-again-before-restart`).
#[tokio::test(flavor = "multi_thread")]
async fn every_maintenance_round_retries_unproven_exec_trees() {
    let harness = Harness::driven().await;
    let before = harness.driver.unproven_retries.load(Ordering::SeqCst);
    harness.app.sweep_expired().await;
    assert_eq!(
        harness.driver.unproven_retries.load(Ordering::SeqCst),
        before + 1,
        "one maintenance round, one retry"
    );
    harness.app.sweep_expired().await;
    assert_eq!(
        harness.driver.unproven_retries.load(Ordering::SeqCst),
        before + 2
    );
}

/// A pending containment whose exec the store already records terminal leaves the set without a
/// kill: there is nothing left to contain.
#[tokio::test(flavor = "multi_thread")]
async fn a_pending_containment_whose_exec_is_terminal_is_dropped_without_a_kill() {
    let harness = Harness::driven().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    harness.driver.refuse_signals.store(1, Ordering::SeqCst);
    let stranding = harness.strand(&session_id).await;
    harness.await_pending_containments(1).await;
    harness.spend_refused_kills().await;

    harness.driver.terminal(&exec_id, ExecState::Exited, None);
    harness.app.sweep_expired().await;
    assert_eq!(harness.exec(&exec_id).await["result"]["state"], "exited");

    let calls = harness.driver.signal_calls.load(Ordering::SeqCst);
    harness.app.contain_pending().await;
    assert_eq!(harness.app.pending_containments(), 0);
    assert_eq!(
        harness.driver.signal_calls.load(Ordering::SeqCst),
        calls,
        "a terminal exec is not signalled again"
    );
    drop(stranding);
}

/// A pending containment whose lease has expired leaves the set without a kill, and lease cleanup
/// (`cleanup_expired`) is what ends the exec from then on.
#[tokio::test(flavor = "multi_thread")]
async fn a_pending_containment_whose_lease_expired_is_left_to_lease_cleanup() {
    let harness = Harness::driven().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    harness
        .driver
        .refuse_signal
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let stranding = harness.strand(&session_id).await;
    harness.await_pending_containments(1).await;

    let (status, renewed) = harness
        .call(
            Method::POST,
            &format!("/v1/sessions/{session_id}/lease/renew"),
            mutation(
                "01JU1CONTAINLEASERENEW01",
                json!({"ttl_ms": substrate_wire::MIN_LEASE_TTL_MS}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    let lease = harness.exec(&exec_id).await["result"]["lease"].clone();
    let renew_by: chrono::DateTime<Utc> = lease["renew_by"]
        .as_str()
        .unwrap_or_else(|| panic!("the renewed exec names its lease deadline: {lease}"))
        .parse()
        .expect("an RFC 3339 lease deadline");
    let remaining = (renew_by - Utc::now()).to_std().unwrap_or_default();
    tokio::time::sleep(remaining + Duration::from_millis(100)).await;

    let calls = harness.driver.signal_calls.load(Ordering::SeqCst);
    harness.app.contain_pending().await;
    assert_eq!(
        harness.app.pending_containments(),
        0,
        "an expired lease hands the exec to lease cleanup"
    );
    assert_eq!(
        harness.driver.signal_calls.load(Ordering::SeqCst),
        calls,
        "the containment step makes no kill for an exec lease cleanup owns"
    );

    harness
        .driver
        .refuse_signal
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let observed = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            harness.app.sweep_expired().await;
            let observed = harness.exec(&exec_id).await;
            if observed["result"]["state"] != "running" {
                return observed;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("lease cleanup ends an exec the containment step handed over");
    assert_eq!(observed["result"]["state"], "expired", "{observed}");
    drop(stranding);
}

/// The story's own sentence, reached the way production reaches it: a real client aborting a real
/// connection on a listener that calls `with_upgrades()`.
///
/// `an_attachment_whose_upgrade_never_completes_is_terminated_and_reads_terminal` above builds the
/// failed upgrade from the server side, through hyper's `Pending::manual()` — the branch taken only
/// when `with_upgrades()` was *not* called, which no daemon listener does
/// (`crates/substrate-daemon/src/runtime.rs:595`, `:918`, `:1083`). This case never touches that
/// branch. It sends the handshake, lets the handler consume the durable claim, then sends a **RST**
/// (`set_linger(0)`, not a FIN) while the upgrade hand-off is still in flight. Hyper's connection
/// future then errors with the pending upgrade still unfulfilled, and `axum::Error` reads
/// `operation was canceled`.
///
/// Two hand-offs can catch that client, and which one catches a given round is a race this case
/// deliberately does not pin: an abort late enough that the upgrade already completed is caught by
/// `run_pipe_attachment` seeing EOF, and an abort inside the hand-off window is caught by
/// `on_failed_upgrade`. The assertion is therefore on the outcome the story states, for every
/// round whose claim the handler really did consume, and it holds whichever hand-off ran.
///
/// It is the containment that makes it hold. On `617bbed`, with this file unchanged and only
/// `app/sessions.rs` reverted, 18 of these 24 rounds were left `attached` and running and the case
/// failed on the first of them; the other 6 aborted late enough for the EOF path to catch them.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_aborts_the_connection_around_the_switch_leaves_no_running_process() {
    let harness = Harness::with_terminals().await;
    let workspace = harness.create_workspace("01JU3ABORTWORKSPACE00001").await;
    let address = harness.server.address;
    let mut claimed = Vec::new();
    for round in 0..24_u32 {
        let session = harness
            .start_pty_as(&workspace, &format!("01JU3ABORTSESSION{round:07}"))
            .await;
        let mut stream = TcpStream::connect(address)
            .await
            .expect("connect aborting attach client");
        // An **abortive** close. A plain FIN does not reach the branch this case is about: with
        // one, all 24 rounds are caught by `run_pipe_attachment` seeing EOF, on base and here
        // alike (measured). `set_linger(0)` is deprecated in tokio because `SO_LINGER` can block
        // the closing thread — with a zero timeout it cannot, which is the whole point: the
        // kernel emits RST and returns immediately.
        #[allow(deprecated)]
        stream
            .set_linger(Some(Duration::ZERO))
            .expect("an abortive close, so the peer sees RST rather than FIN");
        let path = format!("/v1/sessions/{session}/attach");
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write attach handshake");
        // Walk the abort across the hand-off window rather than guessing one instant in it.
        tokio::time::sleep(Duration::from_micros(u64::from(round + 1) * 200)).await;
        drop(stream);
        let (status, document) = harness
            .call(
                Method::GET,
                &format!("/v1/sessions/{session}"),
                Body::empty(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{document}");
        if matches!(
            document["result"]["attachment"].as_str(),
            Some("attached" | "consumed" | "uncertain")
        ) {
            claimed.push((
                session,
                document["result"]["exec"]
                    .as_str()
                    .expect("session exec id")
                    .to_owned(),
            ));
        }
    }
    assert!(
        !claimed.is_empty(),
        "every round aborted before the handler consumed its attachment claim, so this case \
         proved nothing about containment; the abort must land after the claim"
    );

    for (session, exec) in &claimed {
        let observed = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let (status, observed) = harness
                    .call(Method::GET, &format!("/v1/execs/{exec}"), Body::empty())
                    .await;
                assert_eq!(status, StatusCode::OK);
                if observed["result"]["state"] == "cancelled" {
                    return observed;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "session {session} spent its attachment claim to an aborted connection and is \
                 still running: neither hand-off out of the attach handler contained it, so the \
                 process runs unattached until its lease or timeout ends it"
            )
        });
        assert_eq!(observed["result"]["exit"]["signal"], "KILL", "{observed}");
        let (status, document) = harness
            .call(
                Method::GET,
                &format!("/v1/sessions/{session}"),
                Body::empty(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{document}");
        assert_eq!(document["result"]["state"], "cancelled", "{document}");
    }
}

/// A **served** attachment whose containment kill cannot be proven permanently destroys one of
/// this daemon's 32 global attachment slots.
///
/// This was true on `617bbed` and stayed true through this unit's first two commits: the
/// `on_upgrade` path called `retain_attachment_tombstone()` whenever `terminate_pipe_session`
/// reported `false`, and `PipeAttachmentPermit::drop` answered the retained flag with
/// `global.forget()`, removing a permit from the fixed `Semaphore` for the life of the process
/// rather than until the next attach.
///
/// The tombstone barred nothing the durable claim had not already barred. The claim is consumed
/// before the upgrade and `attachment` never returns to `available` afterwards:
/// `claim_pipe_session_attachment` answers `AlreadyClaimed` for `attached`, `consumed` and
/// `uncertain` alike (`crates/substrate-store/src/sessions.rs:734-739`), the attach handler's own
/// gate refuses anything but `available` before `acquire` is reached (`sessions.rs`), and only
/// `finish_pipe_session_start` sets `available` (`crates/substrate-daemon/src/app/operations.rs`).
/// So the flag was unreachable state and the forgotten permit was pure loss. Both are now gone,
/// and `PipeAttachmentPermit::drop` returns the slot unconditionally.
///
/// One client disconnect plus one driver that cannot prove its kill, and the daemon serves 31
/// attachments from then on. Nothing names the loss: the 32nd attach is answered with
/// `session.attachment-capacity`, whose published row calls the capacity "exhausted" and
/// retriable, when it is destroyed and no retry can ever recover it.
///
/// `the_attachment_capacity_refusal_carries_the_retriable_the_register_publishes` pins that a
/// healthy daemon serves all 32 at once to two subjects of sixteen each, so 32 is the number this
/// case is entitled to, and
/// `adversary_a_stranded_attach_with_an_unproven_kill_keeps_the_bounded_capacity` pins the same
/// property for the sibling `on_failed_upgrade` path this unit corrected. This case is that case
/// with the upgrade allowed to succeed.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_a_served_attachment_with_an_unproven_kill_keeps_the_bounded_capacity() {
    let harness = Harness::with_terminals().await;
    let workspace = harness.create_workspace("01JADVP2WORKSPACE0000001").await;
    let stranded = harness
        .start_pty_as(&workspace, "01JADVP2STRANDEDSESSION1")
        .await;
    // A *served* attachment: the upgrade completes, so `on_upgrade` runs and owns the permit.
    let client = harness
        .attach(&format!("/v1/sessions/{stranded}/attach"))
        .await;
    harness
        .driver
        .refuse_signal
        .store(true, std::sync::atomic::Ordering::Relaxed);
    // The client goes away. `run_pipe_attachment` reads EOF and reports a non-terminal
    // attachment, and the single containment kill is refused. Whatever the handler does about
    // that, the permit it holds must still return its slot when it drops.
    drop(client);
    tokio::time::sleep(Duration::from_secs(1)).await;
    harness
        .driver
        .refuse_signal
        .store(false, std::sync::atomic::Ordering::Relaxed);

    // The 32 slots are taken at the per-subject cap (ADR 0042 decision 4): the subject whose
    // attachment was stranded takes sixteen, so a slot its permit kept would fail its own cap as
    // well as the global one, and a second subject takes the other sixteen.
    let views = [
        harness
            .subject_view(SUBJECT, "01JADVP2WORKSPACE0000002")
            .await,
        harness
            .subject_view(OTHER_SUBJECT, "01JADVP2WORKSPACE0000003")
            .await,
    ];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut held = Vec::new();
    for index in 0..32_u32 {
        let view = &views[holder(index)];
        let path = harness
            .attach_path_for(view, &format!("01JADVP2CAPACITYSLOT{index:04}"))
            .await;
        loop {
            let handshake = Handshake::open(view.server.address, &path).await;
            if handshake.status == 101 {
                held.push(handshake.upgraded());
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "attachment {index} of the bounded 32 is still answered {} ten seconds after one \
                 served attachment whose containment kill could not be proven: a permit failed to \
                 return its slot on drop, so one of this daemon's 32 attachment slots is gone \
                 until restart. session.attachment-capacity is published as exhausted and \
                 retriable; destroyed capacity is neither.",
                handshake.status
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    assert_eq!(
        held.len(),
        32,
        "all 32 bounded attachment slots are servable"
    );
}

/// Integrity conformance check of the containment round's batch bound: an entry beyond the first
/// batch is attempted even while every entry of that batch stays unproven.
///
/// `App::contain_pending` takes at most one batch (64) of pending containments per round, in the
/// set's own iteration order. When the entries a round takes stay unproven — a driver that cannot
/// prove those particular kills — the next round must still reach an entry the previous one left
/// out; otherwise that exec's process runs unattached until its lease expires, which is the defect
/// `story:session-containment-retries-an-unproven-kill` exists to remove (invariant 3).
///
/// Sixty-five failed upgrades are queued. One round with every kill refused names the 64 execs it
/// reached and the one it left out. From then on the driver refuses only those 64 and is healthy for
/// the one left out, and eight more rounds run: the left-out exec must have been signalled and
/// contained.
#[tokio::test(flavor = "multi_thread")]
async fn a_pending_containment_beyond_one_batch_is_reached_while_the_batch_stays_unproven() {
    const STRANDED: u32 = 65;
    let harness = Harness::driven().await;
    let mut execs = Vec::new();
    let mut strandings = Vec::new();
    for n in 0..STRANDED {
        let (session_id, exec_id) = harness.start_pipe_numbered(n).await;
        strandings.push(harness.strand(&session_id).await);
        execs.push(exec_id);
    }
    harness
        .await_pending_containments(usize::try_from(STRANDED).expect("small count"))
        .await;

    harness.driver.refuse_signal.store(true, Ordering::SeqCst);
    harness.app.contain_pending().await;
    let reached: std::collections::HashSet<String> = harness
        .driver
        .signalled
        .lock()
        .expect("signalled fixture lock")
        .iter()
        .cloned()
        .collect();
    assert_eq!(reached.len(), 64, "one round attempts exactly one batch");
    let left_out: Vec<String> = execs
        .iter()
        .filter(|exec| !reached.contains(*exec))
        .cloned()
        .collect();
    assert_eq!(left_out.len(), 1, "one exec is beyond the first batch");
    let left_out = left_out.into_iter().next().expect("one left-out exec");

    *harness
        .driver
        .refuse_signal_for
        .lock()
        .expect("refuse_signal_for fixture lock") = reached;
    harness.driver.refuse_signal.store(false, Ordering::SeqCst);
    for _round in 0..8_u32 {
        harness.app.contain_pending().await;
    }

    let attempts = harness
        .driver
        .signalled
        .lock()
        .expect("signalled fixture lock")
        .iter()
        .filter(|exec| **exec == left_out)
        .count();
    let observed = harness.exec(&left_out).await;
    assert_eq!(
        observed["result"]["state"], "cancelled",
        "eight containment rounds after the first never contained the exec the first batch left \
         out ({attempts} kill attempts reached the driver for it): while the 64 entries ahead of \
         it stay unproven, every round takes the same batch, so this exec runs unattached until \
         its lease expires. {observed}"
    );
    assert_eq!(
        harness.app.pending_containments(),
        64,
        "only the 64 unproven entries remain pending"
    );
    drop(strandings);
}

/// Everything the daemon logs on the calling thread while it is held, as plain text lines.
///
/// A thread-local default subscriber: a containment round awaited directly by the case is polled
/// on the case's own thread, so its lines land here and no other case's do.
struct CapturedLog {
    lines: Arc<Mutex<Vec<u8>>>,
    _default: tracing::subscriber::DefaultGuard,
}

#[derive(Clone)]
struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CapturedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("captured log lock")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl CapturedLog {
    fn start() -> Self {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let writer = CapturedWriter(Arc::clone(&lines));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || writer.clone())
            .finish();
        Self {
            lines,
            _default: tracing::subscriber::set_default(subscriber),
        }
    }

    /// The captured lines at `level` that name `exec` and contain `message`.
    fn count(&self, level: &str, exec: &str, message: &str) -> usize {
        let text = String::from_utf8(self.lines.lock().expect("captured log lock").clone())
            .expect("utf-8 log");
        text.lines()
            .filter(|line| line.contains(level) && line.contains(exec) && line.contains(message))
            .count()
    }
}

/// A pending containment whose exec the store records `unknown` leaves the set, and says so at
/// `warn`, naming the exec, because the whole-tree kill was never proven. The containment step
/// does not retry it itself: the driver kills that tree again once per maintenance round until
/// its cgroup is proven gone (`every_maintenance_round_retries_unproven_exec_trees`).
#[tokio::test(flavor = "multi_thread")]
async fn a_pending_containment_whose_exec_ended_unknown_is_dropped_with_a_warning() {
    let harness = Harness::driven().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    harness.driver.refuse_signals.store(1, Ordering::SeqCst);
    let stranding = harness.strand(&session_id).await;
    harness.await_pending_containments(1).await;
    harness.spend_refused_kills().await;

    harness.driver.terminal(&exec_id, ExecState::Unknown, None);
    harness.app.sweep_expired().await;
    assert_eq!(harness.exec(&exec_id).await["result"]["state"], "unknown");

    let log = CapturedLog::start();
    harness.app.contain_pending().await;
    assert_eq!(harness.app.pending_containments(), 0);
    assert_eq!(
        log.count("WARN", &exec_id, "unknown"),
        1,
        "an exec dropped from containment as `unknown` is reported at warn, naming the exec"
    );
    drop(log);
    drop(stranding);
}

/// An unproven containment is reported at `warn` on its first failed attempt and then only on
/// attempts that double the count — 1, 2, 4, 8 — not once per 250 ms round.
#[tokio::test(flavor = "multi_thread")]
async fn an_unproven_containment_is_reported_on_its_first_failure_then_at_doubling_attempts() {
    let harness = Harness::driven().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    harness
        .driver
        .refuse_signal
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let stranding = harness.strand(&session_id).await;
    harness.await_pending_containments(1).await;

    let log = CapturedLog::start();
    for _round in 0..9_u32 {
        harness.app.contain_pending().await;
    }
    assert_eq!(harness.app.pending_containments(), 1);
    assert_eq!(
        log.count("WARN", &exec_id, "containment will retry"),
        4,
        "nine failed attempts are reported on attempts 1, 2, 4 and 8 only"
    );
    drop(log);
    drop(stranding);
}

/// A containment kill the driver answers with `unknown` ended the exec without proving the whole
/// tree went with it. The containment step drops it and says so at `warn`, naming the exec, rather
/// than reporting it terminated; the driver's per-round retry of unproven trees, not this step,
/// kills that tree again (`every_maintenance_round_retries_unproven_exec_trees`).
#[tokio::test(flavor = "multi_thread")]
async fn a_containment_kill_answered_unknown_is_reported_at_warn() {
    let harness = Harness::driven().await;
    let (session_id, exec_id) = harness.start_pipe().await;
    let stranding = harness.strand(&session_id).await;
    harness.await_pending_containments(1).await;
    harness
        .driver
        .kill_ends_unknown
        .store(true, Ordering::SeqCst);

    let log = CapturedLog::start();
    harness.app.contain_pending().await;
    assert_eq!(harness.app.pending_containments(), 0);
    assert_eq!(harness.exec(&exec_id).await["result"]["state"], "unknown");
    assert_eq!(
        log.count("WARN", &exec_id, "unknown"),
        1,
        "a kill answered `unknown` is reported at warn, naming the exec"
    );
    assert_eq!(
        log.count("INFO", &exec_id, "terminated"),
        0,
        "a kill answered `unknown` is not reported as a termination"
    );
    drop(log);
    drop(stranding);
}

/// Adversary cases for `story:an-unknown-exec-is-killed-again-before-restart`, wave 2026-10-23.
mod adversary_unknown_exec_retry {
    use super::*;

    /// The acceptance's second clause, through the path its first clause names: an exec the
    /// driver recorded `unknown` whose lease then expires is handed to lease cleanup, whose KILL
    /// the driver again answers `unknown` (the tree still cannot be proven empty). What the daemon
    /// reports afterwards must stay `unknown`, never a terminated state such as `expired`.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_unknown_exec_whose_lease_expires_is_still_reported_unknown() {
        let harness = Harness::driven().await;
        let (session_id, exec_id) = harness.start_pipe().await;
        let (status, renewed) = harness
            .call(
                Method::POST,
                &format!("/v1/sessions/{session_id}/lease/renew"),
                mutation(
                    "01JU2UNKNOWNLEASERENEW01",
                    json!({"ttl_ms": substrate_wire::MIN_LEASE_TTL_MS}),
                ),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{renewed}");
        let lease = harness.exec(&exec_id).await["result"]["lease"].clone();
        let renew_by: chrono::DateTime<Utc> = lease["renew_by"]
            .as_str()
            .unwrap_or_else(|| panic!("the renewed exec names its lease deadline: {lease}"))
            .parse()
            .expect("an RFC 3339 lease deadline");

        harness
            .driver
            .kill_ends_unknown
            .store(true, Ordering::SeqCst);
        harness.driver.terminal(&exec_id, ExecState::Unknown, None);
        harness.app.sweep_expired().await;
        assert_eq!(harness.exec(&exec_id).await["result"]["state"], "unknown");

        let remaining = (renew_by - Utc::now()).to_std().unwrap_or_default();
        tokio::time::sleep(remaining + Duration::from_millis(100)).await;
        let observed = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                harness.app.sweep_expired().await;
                let observed = harness.exec(&exec_id).await;
                if observed["result"]["lease"]["state"] == "expired"
                    || observed["result"]["state"] != "unknown"
                {
                    return observed;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("lease cleanup reaches the expired exec lease");
        assert_eq!(
            observed["result"]["state"], "unknown",
            "a tree that still cannot be proven empty is never reported terminated: {observed}"
        );
    }
}

/// Correction round 2 (F2) of `story:an-unknown-exec-is-killed-again-before-restart`: when the
/// driver answers lease cleanup's KILL for an `unknown` exec with `exec.cgroup-not-empty`, as the
/// host driver now does for a tree it has not proven gone, workspace-lease cleanup keeps the
/// workspace and tries again on a later round. It ends the workspace only on `NotFound`, and
/// `NotFound` would end it with the tree's processes still in it.
#[tokio::test(flavor = "multi_thread")]
async fn workspace_lease_cleanup_keeps_a_workspace_whose_unknown_exec_tree_is_not_proven_gone() {
    let harness = Harness::driven().await;
    let (_session_id, exec_id) = harness
        .start_pipe_numbered_leased(0, Some(substrate_wire::MIN_LEASE_TTL_MS))
        .await;
    let observed = harness.exec(&exec_id).await;
    let workspace = observed["result"]["workspace"]
        .as_str()
        .unwrap_or_else(|| panic!("the exec names its workspace: {observed}"))
        .to_owned();
    let (status, leased) = harness
        .call(
            Method::GET,
            &format!("/v1/workspaces/{workspace}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{leased}");
    let renew_by: chrono::DateTime<Utc> = leased["result"]["lease"]["renew_by"]
        .as_str()
        .unwrap_or_else(|| panic!("the workspace names its lease deadline: {leased}"))
        .parse()
        .expect("an RFC 3339 lease deadline");

    harness.driver.terminal(&exec_id, ExecState::Unknown, None);
    harness
        .driver
        .unproven_tree_for
        .lock()
        .expect("unproven_tree_for fixture lock")
        .insert(exec_id.clone());
    harness.app.sweep_expired().await;
    assert_eq!(harness.exec(&exec_id).await["result"]["state"], "unknown");

    let remaining = (renew_by - Utc::now()).to_std().unwrap_or_default();
    tokio::time::sleep(remaining + Duration::from_millis(100)).await;
    // Long enough for the first cleanup attempt and at least one retry after its backoff.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let attempts = loop {
        harness.app.sweep_expired().await;
        let attempts = harness
            .driver
            .signalled
            .lock()
            .expect("signalled fixture lock")
            .iter()
            .filter(|signalled| **signalled == exec_id)
            .count();
        if attempts >= 2 || tokio::time::Instant::now() >= deadline {
            break attempts;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        attempts >= 2,
        "workspace-lease cleanup retries the KILL on a later round ({attempts} reached the driver)"
    );

    let (status, kept) = harness
        .call(
            Method::GET,
            &format!("/v1/workspaces/{workspace}"),
            Body::empty(),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a workspace whose exec tree is not proven gone is never ended: {kept}"
    );
    assert_ne!(
        kept["result"]["state"], "expired",
        "a workspace whose exec tree is not proven gone is never ended: {kept}"
    );
    assert_eq!(harness.exec(&exec_id).await["result"]["state"], "unknown");
}
