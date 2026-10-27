//! Security conformance for the `exec.read-only-roots` fact (ADR 0010, bundle `0.23.0`).
//!
//! `0.23.0` publishes the `read_only_roots` start list with `x-b10x-requires-capability:
//! exec.read-only-roots` (`schemas/common.json`) and the fact with `x-b10x-absent-means`: "a start
//! naming a root is refused ... and no root is run unmounted" (`schemas/capability.json`). The
//! wire crate documents the same on `CapabilityFacts::exec_read_only_roots`: "no root is ever
//! mounted by a host that does not publish this".
//!
//! Invariant 4 says drivers implement one contract and expose verified facts, and clients do not
//! branch on driver internals. These cases hold the daemon to the published capability rather
//! than to one driver's internals: the fixture driver publishes the full confinement floor and
//! omits only `exec.read-only-roots`, and it counts every start it receives. A start that names a
//! root must be answered by name without reaching the driver, as every sibling start member that
//! requires a capability is (`check_read_only_files`, `check_workdir`, `check_writable_roots`,
//! `check_egress_aperture`).
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use serde_json::{Value, json};
use substrate_daemon::{App, Identity, router};
use substrate_host::{
    DispatchOutcome, Driver, DriverError, ExecObservation, GitRootBinding, HostConfig, HostDriver,
    WorkspaceDestroyProgress,
};
use substrate_store::Store;
use substrate_wire::{
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, CapabilitySnapshot, Exec,
    ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState, FileAbsence, FileObservation,
    FileReadQuery, FileReadResult, LeaseObservation, OutputSlice, ReadOnlyRootFacts,
    SandboxProfile, Workspace, WorkspaceAbsence, WorkspaceCreateInput,
};
use tempfile::TempDir;
use tower::ServiceExt as _;

const SUBJECT: &str = "local:1000";
const SNAPSHOT: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

struct Fixture {
    host: Arc<HostDriver>,
    /// Whether the fixture publishes `exec.read-only-roots`.
    publishes_fact: AtomicBool,
    /// Every start the daemon dispatched to the driver.
    starts: AtomicUsize,
    execs: Mutex<HashMap<String, ExecObservation>>,
}

#[async_trait]
impl Driver for Fixture {
    async fn shutdown(&self) -> Result<(), DriverError> {
        self.host.shutdown().await
    }

    fn machine(&self) -> CapabilitySnapshot {
        let mut capability = self.host.machine();
        SNAPSHOT.clone_into(&mut capability.snapshot);
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
        capability.facts.exec_argv_only = Some(true);
        capability.facts.exec_read_only_roots = self
            .publishes_fact
            .load(Ordering::SeqCst)
            .then_some(ReadOnlyRootFacts {
                max_roots: substrate_wire::MAX_READ_ONLY_ROOTS,
            });
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

    async fn forget_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<WorkspaceAbsence, DriverError> {
        self.host.forget_workspace(workspace_id, root_name).await
    }

    fn admit_git_root(
        &self,
        workspace: &str,
        _root_name: &str,
        _adopted: bool,
    ) -> Result<(), DriverError> {
        Err(substrate_host::writable_root_not_git(workspace))
    }

    async fn start_exec(
        &self,
        id: &str,
        _workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        // The fixture binds nothing. It answers as a driver that ignored the roots would, which is
        // exactly the state the published fact exists to keep a caller out of.
        let observation = ExecObservation {
            resource: Exec {
                unrecorded_output: None,
                id: id.to_owned(),
                kind: substrate_wire::ExecKind::Exec,
                workspace: input.workspace.clone(),
                state: ExecState::Running,
                observed_at: Utc::now(),
                requested: input.sandbox.clone(),
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
            cgroup: None,
            leader_pid: None,
        };
        self.execs
            .lock()
            .expect("fixture lock")
            .insert(id.to_owned(), observation.clone());
        DispatchOutcome::Observed(observation)
    }

    async fn start_exec_with_git_roots(
        &self,
        id: &str,
        workspace_root_name: &str,
        _git_roots: &[GitRootBinding],
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.start_exec(id, workspace_root_name, input).await
    }

    async fn observe_exec(&self, id: &str) -> Result<ExecObservation, DriverError> {
        self.execs
            .lock()
            .expect("fixture lock")
            .get(id)
            .cloned()
            .ok_or_else(DriverError::not_found)
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
    _directory: TempDir,
    driver: Arc<Fixture>,
    app: Arc<App>,
}

impl Harness {
    fn open(publishes_fact: bool) -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        std::fs::create_dir(directory.path().join("workspaces")).expect("workspace root");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = Arc::new(Fixture {
            host: HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
                .expect("host driver"),
            publishes_fact: AtomicBool::new(publishes_fact),
            starts: AtomicUsize::new(0),
            execs: Mutex::new(HashMap::new()),
        });
        let app = App::new(
            Arc::clone(&store),
            Arc::clone(&driver) as Arc<dyn Driver>,
            "dep_rorootfact",
        );
        Self {
            _directory: directory,
            driver,
            app,
        }
    }

    async fn call(&self, method: Method, uri: &str, body: Body) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-request-id", "req_roroot_fact")
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: SUBJECT.to_owned(),
            actor: "read-only-roots-fact-security".to_owned(),
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
        (status, serde_json::from_slice(&bytes).expect("JSON body"))
    }

    async fn workspace(&self, operation: &str) -> String {
        let (status, created) = self
            .call(
                Method::POST,
                "/v1/workspaces",
                mutation(operation, json!({ "source": "empty", "labels": {} })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        created["result"]["id"].as_str().expect("id").to_owned()
    }

    async fn start(&self, operation: &str, workspace: &str, roots: Value) -> (StatusCode, Value) {
        let body = json!({
            "workspace": workspace,
            "argv": ["/fixture/keep-running"],
            "env": { "allow": [], "set": {} },
            "sandbox": {
                "capability_snapshot": SNAPSHOT,
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
            "wait": false,
            "read_only_roots": roots
        });
        self.call(Method::POST, "/v1/execs", mutation(operation, body))
            .await
    }
}

#[allow(clippy::needless_pass_by_value)]
fn mutation(operation: &str, input: Value) -> Body {
    Body::from(serde_json::to_vec(&json!({ "op": operation, "input": input })).expect("JSON"))
}

fn one_root() -> Value {
    json!([{ "host_path": "/srv/toolchain", "mount": "/opt/toolchain" }])
}

/// The control: with the fact published, the same start reaches the driver. Without this the
/// refusal case below could pass because the fixture never admits any start.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_start_reaches_the_driver_when_the_fact_is_published() {
    let harness = Harness::open(true);
    let workspace = harness.workspace("01JROROOTFACTCONTROL0001").await;
    let (status, started) = harness
        .start("01JROROOTFACTCONTROL0002", &workspace, one_root())
        .await;
    assert!(status.is_success(), "{status} {started}");
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 1);
}

/// `x-b10x-requires-capability: exec.read-only-roots` on the published list, read as the
/// specification it claims to be: a host whose snapshot omits the fact does not dispatch a start
/// that names a root, and answers it as an unserved capability by name (invariants 3 and 4).
#[tokio::test(flavor = "multi_thread")]
async fn a_root_start_is_refused_by_name_when_the_fact_is_absent() {
    let harness = Harness::open(false);
    let workspace = harness.workspace("01JROROOTFACTABSENT00001").await;
    let (status, refused) = harness
        .start("01JROROOTFACTABSENT00002", &workspace, one_root())
        .await;
    let dispatched = harness.driver.starts.load(Ordering::SeqCst);
    assert_eq!(
        dispatched, 0,
        "a start naming a read-only root reached a driver that does not publish \
         exec.read-only-roots ({status} {refused})"
    );
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{refused}");
    assert_eq!(refused["error"]["class"], "unserved", "{refused}");
}
