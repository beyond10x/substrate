//! Adversary pass 1 on `story:pty-session-refuses-first-attach-under-load`.
//!
//! A sweep that records a session's exec as exited after the driver answered the start and before
//! `complete_pipe_session_start` commits is overwritten by the start's stale `running` observation,
//! so the session reads `ready` and `available` over a child the driver has already reported dead.
//! The driver here forgets a terminal exec once the daemon acknowledges its exact observation, as
//! `HostDriver` does (`substrate-host/src/process.rs`, `ProcessRuntime::acknowledge`), so nothing
//! reports the exit again. The sweep runs inside `set_exec_lease`, the driver call the start makes
//! between dispatch and the store write, so the interleaving is fixed and the case runs once.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{OnceLock, Weak};

use substrate_host::Listed;

use super::{
    App, Arc, Body, CapabilitySnapshot, DEPLOYMENT, DispatchOutcome, Driver, DriverError, Duration,
    ExecObservation, ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState, FileAbsence,
    FileObservation, FileReadQuery, FileReadResult, Harness, LeaseObservation, Method, Mutex,
    Ordering, OutputSlice, PipeFixtureDriver, PipeFrame, PipeSessionStartInput, StatusCode,
    TestServer, Workspace, WorkspaceCreateInput, WorkspaceDestroyProgress, async_trait, json,
    mutation, pty_start,
};

/// The fixture driver, plus a host driver's forgetting of an acknowledged terminal exec and one
/// armed sweep between dispatch and the start's store write.
struct SweepingDriver {
    inner: Arc<PipeFixtureDriver>,
    app: OnceLock<Weak<App>>,
    armed: AtomicBool,
    retired: Mutex<HashSet<String>>,
}

#[async_trait]
impl Driver for SweepingDriver {
    async fn shutdown(&self) -> Result<(), DriverError> {
        self.inner.shutdown().await
    }
    fn machine(&self) -> CapabilitySnapshot {
        self.inner.machine()
    }
    fn workspace_root_identity(&self, id: &str) -> Result<String, DriverError> {
        self.inner.workspace_root_identity(id)
    }
    async fn create_workspace(
        &self,
        id: &str,
        root_name: &str,
        input: &WorkspaceCreateInput,
    ) -> DispatchOutcome<Workspace> {
        self.inner.create_workspace(id, root_name, input).await
    }
    async fn observe_workspace(
        &self,
        id: &str,
        root_name: &str,
        previous: &Workspace,
    ) -> Result<Workspace, DriverError> {
        self.inner.observe_workspace(id, root_name, previous).await
    }
    async fn read_workspace_path(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<Listed<FileReadResult>, DriverError> {
        self.inner
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
        self.inner
            .write_workspace_file(workspace_id, root_name, path, content)
            .await
    }
    async fn delete_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
    ) -> Result<FileAbsence, DriverError> {
        self.inner
            .delete_workspace_file(workspace_id, root_name, path)
            .await
    }
    async fn destroy_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<WorkspaceDestroyProgress, DriverError> {
        self.inner.destroy_workspace(workspace_id, root_name).await
    }
    async fn start_exec(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.inner.start_exec(id, workspace_root_name, input).await
    }
    async fn start_pipe_session(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.inner
            .start_pipe_session(id, workspace_root_name, input)
            .await
    }
    async fn write_pipe_session(&self, id: &str, bytes: &[u8]) -> Result<(), DriverError> {
        self.inner.write_pipe_session(id, bytes).await
    }
    async fn read_pipe_session(
        &self,
        id: &str,
        timeout: Duration,
    ) -> Result<Option<PipeFrame>, DriverError> {
        self.inner.read_pipe_session(id, timeout).await
    }
    async fn close_pipe_session_input(&self, id: &str) -> Result<(), DriverError> {
        self.inner.close_pipe_session_input(id).await
    }
    async fn resize_pty_session(
        &self,
        id: &str,
        window: substrate_wire::PtyWindow,
    ) -> Result<(), DriverError> {
        self.inner.resize_pty_session(id, window).await
    }
    async fn observe_exec(&self, id: &str) -> Result<ExecObservation, DriverError> {
        // As `ProcessRuntime::observe`: a retired exec is no longer tracked, so it is not found.
        if self.retired.lock().expect("retired lock").contains(id) {
            return Err(DriverError::not_found());
        }
        self.inner.observe_exec(id).await
    }
    async fn output(&self, id: &str, query: &ExecOutputQuery) -> Result<OutputSlice, DriverError> {
        self.inner.output(id, query).await
    }
    async fn signal(
        &self,
        id: &str,
        input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        self.inner.signal(id, input).await
    }
    fn completed_execs(&self) -> Vec<ExecObservation> {
        let retired = self.retired.lock().expect("retired lock").clone();
        self.inner
            .completed_execs()
            .into_iter()
            .filter(|item| !retired.contains(&item.resource.id))
            .collect()
    }
    fn set_exec_lease(&self, id: &str, lease: Option<LeaseObservation>) {
        self.inner.set_exec_lease(id, lease);
        if self.armed.swap(false, Ordering::SeqCst) {
            // The child exits now, and one sweep round persists it before the start's write.
            self.inner.terminal(id, ExecState::Exited, None);
            let app = self
                .app
                .get()
                .and_then(Weak::upgrade)
                .expect("the daemon this driver serves");
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(app.sweep_expired());
            });
        }
    }
    fn acknowledge_exec(&self, persisted: &ExecObservation) {
        if !matches!(
            persisted.resource.state,
            ExecState::Exited | ExecState::Cancelled | ExecState::Unknown | ExecState::Expired
        ) {
            return;
        }
        let exact = self
            .inner
            .pipes
            .lock()
            .expect("pipe fixture lock")
            .get(&persisted.resource.id)
            .is_some_and(|pipe| pipe.observation == *persisted);
        if exact {
            self.retired
                .lock()
                .expect("retired lock")
                .insert(persisted.resource.id.clone());
        }
    }
    fn discard_superseded_exec(&self, id: &str) {
        self.inner.discard_superseded_exec(id);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adversary_a_child_that_exits_before_the_start_commits_is_not_left_ready() {
    let directory = tempfile::tempdir().expect("temporary pipe harness");
    let store = Arc::new(
        substrate_store::Store::open(directory.path().join("state.db")).expect("state store"),
    );
    let inner = PipeFixtureDriver::with_terminals(&directory.path().join("workspaces"), true);
    let driver = Arc::new(SweepingDriver {
        inner: Arc::clone(&inner),
        app: OnceLock::new(),
        armed: AtomicBool::new(false),
        retired: Mutex::new(HashSet::new()),
    });
    let app = App::new(store, Arc::clone(&driver) as Arc<dyn Driver>, DEPLOYMENT);
    driver
        .app
        .set(Arc::downgrade(&app))
        .unwrap_or_else(|_| panic!("set once"));
    let server = TestServer::spawn(Arc::clone(&app)).await;
    let harness = Harness {
        driver: inner,
        _directory: directory,
        app,
        server,
        _containment: None,
    };

    let workspace = harness.create_workspace("01JADV1RACEWORKSPACE0001").await;
    driver.armed.store(true, Ordering::SeqCst);
    let (status, started) = harness
        .call(
            Method::POST,
            "/v1/sessions",
            mutation(
                "01JADV1RACESESSIONSTART1",
                pty_start(&workspace, Some(json!({"columns": 80, "rows": 24}))),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert!(
        !driver.armed.load(Ordering::SeqCst),
        "the sweep ran between dispatch and the start's store write"
    );
    let session_id = started["result"]["id"].as_str().expect("session id");
    let exec_id = started["result"]["exec"].as_str().expect("exec id");
    assert!(
        driver
            .retired
            .lock()
            .expect("retired lock")
            .contains(exec_id),
        "precondition: the sweep persisted the exited observation and the daemon acknowledged it"
    );

    let (_, exec) = harness
        .call(Method::GET, &format!("/v1/execs/{exec_id}"), Body::empty())
        .await;
    let (_, session) = harness
        .call(
            Method::GET,
            &format!("/v1/sessions/{session_id}"),
            Body::empty(),
        )
        .await;
    assert_eq!(
        (
            &session["result"]["state"],
            &session["result"]["attachment"],
            &exec["result"]["state"]
        ),
        (&json!("exited"), &json!("consumed"), &json!("exited")),
        "the driver reported this exec exited and the daemon acknowledged that observation; the \
         session must not read ready and attachable, nor the exec running. session {session}"
    );
}
