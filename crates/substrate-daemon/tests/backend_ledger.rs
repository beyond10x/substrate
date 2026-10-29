#![forbid(unsafe_code)]
//! Design 34 slice U3: the backend ledger, through the daemon.
//!
//! The host driver names no backend object, so it writes no ledger row and every answer it gives
//! is the one it gave before the ledger existed. A driver that names objects has them reserved in
//! the transaction that records `accepted`, before it is dispatched (ADR 0005). After a restart, an
//! exec whose `pod` row never had a uid bound and whose driver answers `NotFound` is dispatch
//! absence and fails; a bound uid, or no row at all, keeps today's physical-absence reading
//! (§ 5.1).

use std::sync::{Arc, Mutex};

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use substrate_daemon::{App, Identity, StoreBackendLedger, operation_digest, router};
use substrate_host::{
    BackendKind, BackendLedger as _, BackendName, BackendRole, DispatchOutcome, Driver,
    DriverError, ExecObservation, HostConfig, HostDriver, UidBinding, WorkspaceDestroyProgress,
};
use substrate_store::{BackendObjectKind, BackendObjectRole, Scope, Store};
use substrate_wire::{
    CapabilitySnapshot, ExecOutputQuery, ExecSignalInput, ExecStartInput, FileAbsence,
    FileObservation, FileReadQuery, FileReadResult, LeaseObservation, OperationOutcome,
    OperationState, OutputSlice, Workspace, WorkspaceAbsence, WorkspaceCreateInput,
};
use tempfile::TempDir;
use tower::ServiceExt as _;

const SUBJECT: &str = "local:1000";
const DEPLOYMENT: &str = "dep_backend_ledger";

fn scope() -> Scope {
    Scope {
        deployment: DEPLOYMENT.to_owned(),
        subject: SUBJECT.to_owned(),
    }
}

fn pod_name(id: &str) -> String {
    format!("sbx-ex-{}", id.to_ascii_lowercase())
}

/// The host driver with the four Linux exec facts published, so an exec start reaches dispatch on
/// a host without delegation. With `names` false it names exactly what the host driver names;
/// with it true it names a PVC and keeper per workspace and a Pod per exec, as the namespace
/// driver will. Every start answers `OutcomeUnknown` and every exec reads `NotFound`: the crash
/// window of § 5.1.
struct Fixture {
    host: Arc<HostDriver>,
    store: Arc<Store>,
    names: bool,
    /// For each start, how many backend rows the exec already had when the driver was called.
    rows_at_dispatch: Mutex<Vec<usize>>,
}

#[async_trait::async_trait]
impl Driver for Fixture {
    fn machine(&self) -> CapabilitySnapshot {
        let mut capability = self.host.machine();
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

    async fn shutdown(&self) -> Result<(), DriverError> {
        self.host.shutdown().await
    }

    fn workspace_root_identity(&self, id: &str) -> Result<String, DriverError> {
        self.host.workspace_root_identity(id)
    }

    fn backend_names(&self, kind: BackendKind, id: &str) -> Vec<BackendName> {
        if !self.names {
            return self.host.backend_names(kind, id);
        }
        let id = id.to_ascii_lowercase();
        match kind {
            BackendKind::Workspace => vec![
                BackendName {
                    role: BackendRole::Pvc,
                    name: format!("sbx-ws-{id}"),
                },
                BackendName {
                    role: BackendRole::Keeper,
                    name: format!("sbx-ws-{id}-keeper"),
                },
            ],
            BackendKind::Exec => vec![BackendName {
                role: BackendRole::Pod,
                name: format!("sbx-ex-{id}"),
            }],
            BackendKind::Probe => Vec::new(),
        }
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

    async fn forget_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<WorkspaceAbsence, DriverError> {
        self.host.forget_workspace(workspace_id, root_name).await
    }

    async fn start_exec(
        &self,
        id: &str,
        _workspace_root_name: &str,
        _input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        let rows = self
            .store
            .backend_objects(DEPLOYMENT, BackendObjectKind::Exec, id)
            .expect("ledger read at dispatch");
        self.rows_at_dispatch
            .lock()
            .expect("fixture lock")
            .push(rows.len());
        DispatchOutcome::OutcomeUnknown(DriverError::failed(
            "exec.containment-unproven",
            "The fixture cannot prove whether the exec runs.",
        ))
    }

    async fn observe_exec(&self, _id: &str) -> Result<ExecObservation, DriverError> {
        Err(DriverError::not_found())
    }

    async fn output(&self, id: &str, query: &ExecOutputQuery) -> Result<OutputSlice, DriverError> {
        self.host.output(id, query).await
    }

    async fn signal(
        &self,
        _id: &str,
        _input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        Err(DriverError::not_found())
    }

    fn completed_execs(&self) -> Vec<ExecObservation> {
        Vec::new()
    }

    fn set_exec_lease(&self, _id: &str, _lease: Option<LeaseObservation>) {}

    fn acknowledge_exec(&self, _persisted: &ExecObservation) {}

    fn discard_superseded_exec(&self, _id: &str) {}
}

struct Harness {
    directory: TempDir,
    store: Arc<Store>,
    app: Arc<App>,
}

impl Harness {
    fn host() -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver");
        let app = App::new(Arc::clone(&store), driver, DEPLOYMENT);
        Self {
            directory,
            store,
            app,
        }
    }

    fn fixture(names: bool) -> (Self, Arc<Fixture>) {
        let directory = tempfile::tempdir().expect("temporary harness");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = Arc::new(Fixture {
            host: HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
                .expect("host driver"),
            store: Arc::clone(&store),
            names,
            rows_at_dispatch: Mutex::new(Vec::new()),
        });
        let app = App::new(Arc::clone(&store), driver.clone(), DEPLOYMENT);
        (
            Self {
                directory,
                store,
                app,
            },
            driver,
        )
    }

    /// The same state directory and store under a new daemon run over `driver`.
    fn restart(&self, driver: Arc<dyn Driver>) -> Arc<App> {
        assert!(self.directory.path().join("state.db").is_file());
        App::new(Arc::clone(&self.store), driver, DEPLOYMENT)
    }

    async fn call(&self, method: Method, uri: &str, body: Body) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-request-id", "req_backend_ledger")
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: SUBJECT.to_owned(),
            actor: "backend-ledger-test".to_owned(),
            principal: None,
        };
        let response = router(Arc::clone(&self.app))
            .layer(Extension(identity))
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2_097_152)
            .await
            .expect("response body");
        (
            status,
            serde_json::from_slice(&bytes).expect("JSON response"),
        )
    }

    async fn create_workspace(&self, operation: &str) -> (StatusCode, Value) {
        self.call(
            Method::POST,
            "/v1/workspaces",
            mutation(
                operation,
                json!({ "source": "empty", "labels": { "case": "backend-ledger" } }),
            ),
        )
        .await
    }

    async fn start_exec(&self, operation: &str, workspace: &str) -> (StatusCode, Value) {
        let snapshot = self.app.driver.machine().snapshot;
        self.call(
            Method::POST,
            "/v1/execs",
            mutation(
                operation,
                json!({
                    "workspace": workspace,
                    "argv": ["/bin/true"],
                    "env": { "allow": [], "set": {} },
                    "sandbox": {
                        "capability_snapshot": snapshot,
                        "network": "none",
                        "profile": "workspace",
                        "require": true
                    },
                    "limits": {
                        "timeout_ms": 5000,
                        "output_bytes": 65536,
                        "processes": 16,
                        "memory_bytes": 67_108_864,
                        "cpu_millis": 1000
                    },
                    "wait": false
                }),
            ),
        )
        .await
    }

    fn ledger_rows(&self) -> i64 {
        let connection = rusqlite::Connection::open(self.directory.path().join("state.db"))
            .expect("second connection");
        connection
            .query_row("SELECT COUNT(*) FROM backend_objects", [], |row| row.get(0))
            .expect("ledger row count")
    }

    fn physically_absent(&self, exec: &str) -> Option<bool> {
        let connection = rusqlite::Connection::open(self.directory.path().join("state.db"))
            .expect("second connection");
        connection
            .query_row(
                "SELECT physically_absent FROM execs WHERE deployment = ?1 AND id = ?2",
                rusqlite::params![DEPLOYMENT, exec],
                |row| row.get::<_, i64>(0),
            )
            .ok()
            .map(|value| value == 1)
    }

    fn operation(&self, operation: &str) -> substrate_wire::OperationRecord {
        self.store
            .operation(&scope(), operation)
            .expect("operation read")
            .expect("operation recorded")
    }
}

#[allow(clippy::needless_pass_by_value)]
fn mutation(operation: &str, input: Value) -> Body {
    Body::from(serde_json::to_vec(&json!({ "op": operation, "input": input })).expect("JSON"))
}

fn workspace_id(created: &Value) -> String {
    created["result"]["id"]
        .as_str()
        .expect("workspace id")
        .to_owned()
}

fn exec_id(harness: &Harness, operation: &str) -> String {
    harness
        .operation(operation)
        .resource
        .expect("exec resource")
}

/// The host driver itself: a workspace is created, replayed and read exactly as before, an exec
/// start answers what this host answers, and the ledger holds no row.
#[tokio::test(flavor = "multi_thread")]
async fn the_host_driver_writes_no_backend_row_and_answers_as_before() {
    let harness = Harness::host();
    let (status, created) = harness.create_workspace("01JBACKENDHOSTCREATE0001").await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = workspace_id(&created);
    assert!(id.starts_with("ws_"), "{created}");
    assert_eq!(created["result"]["state"], "ready", "{created}");
    assert_eq!(created["result"]["origin"], "created", "{created}");
    let (status, replayed) = harness.create_workspace("01JBACKENDHOSTCREATE0001").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed["result"], created["result"]);
    let (status, read) = harness
        .call(Method::GET, &format!("/v1/workspaces/{id}"), Body::empty())
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["result"]["id"], created["result"]["id"]);

    let (status, started) = harness.start_exec("01JBACKENDHOSTEXEC000001", &id).await;
    let facts = &harness.app.driver.machine().facts;
    if facts.exec_namespaces.is_none() {
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{started}");
        assert_eq!(started["error"]["code"], "exec.sandbox-unavailable");
    } else {
        assert!(status.is_success(), "{status} {started}");
    }
    assert_eq!(harness.ledger_rows(), 0);
}

/// The default name set read through a dispatched start: nothing is reserved, and after a
/// restart the exec's `NotFound` is physical absence, exactly as before: the operation stays
/// `unknown` and the exec row stays, marked absent.
#[tokio::test(flavor = "multi_thread")]
async fn without_backend_names_restart_absence_stays_unknown_as_before() {
    let (harness, driver) = Harness::fixture(false);
    let (status, created) = harness.create_workspace("01JBACKENDNONECREATE0001").await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let workspace = workspace_id(&created);
    let (status, started) = harness
        .start_exec("01JBACKENDNONEEXEC000001", &workspace)
        .await;
    assert!(!status.is_success(), "{status} {started}");
    assert_eq!(started["error"]["code"], "operation.outcome-unknown");
    assert_eq!(*driver.rows_at_dispatch.lock().expect("fixture lock"), [0]);
    let exec = exec_id(&harness, "01JBACKENDNONEEXEC000001");
    assert_eq!(harness.ledger_rows(), 0);

    let restarted = harness.restart(driver);
    restarted.sweep_expired().await;
    let record = harness.operation("01JBACKENDNONEEXEC000001");
    assert_eq!(record.state, OperationState::Unknown);
    assert_eq!(harness.physically_absent(&exec), Some(true));
}

/// A named Pod is durable before dispatch; after a restart its `NotFound`, with no uid ever bound,
/// is dispatch absence: the start fails and the exec row is gone.
#[tokio::test(flavor = "multi_thread")]
async fn an_unbound_pod_row_reads_restart_absence_as_dispatch_absence() {
    let (harness, driver) = Harness::fixture(true);
    let (status, created) = harness.create_workspace("01JBACKENDNAMECREATE0001").await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let workspace = workspace_id(&created);
    let workspace_rows = harness
        .store
        .backend_objects(DEPLOYMENT, BackendObjectKind::Workspace, &workspace)
        .expect("workspace rows");
    assert_eq!(
        workspace_rows
            .iter()
            .map(|row| (row.role, row.operation_digest.clone(), row.uid.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                BackendObjectRole::Keeper,
                operation_digest(SUBJECT, "01JBACKENDNAMECREATE0001"),
                None
            ),
            (
                BackendObjectRole::Pvc,
                operation_digest(SUBJECT, "01JBACKENDNAMECREATE0001"),
                None
            ),
        ]
    );

    let (status, started) = harness
        .start_exec("01JBACKENDNAMEEXEC000001", &workspace)
        .await;
    assert!(!status.is_success(), "{status} {started}");
    assert_eq!(*driver.rows_at_dispatch.lock().expect("fixture lock"), [1]);
    let exec = exec_id(&harness, "01JBACKENDNAMEEXEC000001");
    assert_eq!(harness.ledger_rows(), 3);

    let restarted = harness.restart(driver);
    restarted.sweep_expired().await;
    let record = harness.operation("01JBACKENDNAMEEXEC000001");
    assert_eq!(record.state, OperationState::Terminal);
    assert!(
        matches!(&record.outcome, Some(OperationOutcome::Error { error }) if !error.retriable),
        "{:?}",
        record.outcome
    );
    assert!(
        harness
            .store
            .exec(&scope(), &exec)
            .expect("exec read")
            .is_none()
    );
}

/// A Pod row whose uid was bound is the object that was stored: its `NotFound` after a restart is
/// physical absence, as today, and a second uid for the same name is refused as a collision.
#[tokio::test(flavor = "multi_thread")]
async fn a_bound_pod_row_reads_restart_absence_as_physical_absence() {
    let (harness, driver) = Harness::fixture(true);
    let (status, created) = harness.create_workspace("01JBACKENDBINDCREATE0001").await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let workspace = workspace_id(&created);
    let (status, started) = harness
        .start_exec("01JBACKENDBINDEXEC000001", &workspace)
        .await;
    assert!(!status.is_success(), "{status} {started}");
    let exec = exec_id(&harness, "01JBACKENDBINDEXEC000001");

    let ledger = StoreBackendLedger::new(Arc::clone(&harness.store), DEPLOYMENT);
    let recorded = ledger
        .recorded(BackendKind::Exec, &exec)
        .expect("recorded rows");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].role, BackendRole::Pod);
    assert_eq!(recorded[0].name, pod_name(&exec));
    assert_eq!(
        ledger.bind_uid(&pod_name(&exec), "uid-1").expect("bind"),
        UidBinding::Bound
    );
    assert_eq!(
        ledger
            .bind_uid(&pod_name(&exec), "uid-2")
            .expect("second uid"),
        UidBinding::Collision {
            bound: "uid-1".to_owned()
        }
    );

    let restarted = harness.restart(driver);
    restarted.sweep_expired().await;
    let record = harness.operation("01JBACKENDBINDEXEC000001");
    assert_eq!(record.state, OperationState::Unknown);
    assert_eq!(harness.physically_absent(&exec), Some(true));
}
