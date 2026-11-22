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
//!
//! The second half holds the amendments of 2026-11-22 to ADR 0010 and ADR 0033: a read-only root
//! whose host path equals, lies under or contains a path the daemon keeps private, and a
//! read-only file whose host path equals or lies under one, is refused at daemon admission. The
//! harness builds the application as the composition root does, with the read set of a daemon
//! configuration over its own directory (`PrivateHostPaths::of`), and the fixture also publishes
//! `exec.read-only-files` and counts session starts.
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
use substrate_daemon::{
    App, DaemonConfig, DelegatedContextPolicy, Identity, PrivateHostPaths, SystemAuthority, router,
};
use substrate_host::{
    DispatchOutcome, Driver, DriverError, ExecObservation, GitRootBinding, HostConfig, HostDriver,
    WorkspaceDestroyProgress,
};
use substrate_store::Store;
use substrate_wire::{
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, CapabilitySnapshot, Exec,
    ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState, FileAbsence, FileObservation,
    FileReadQuery, FileReadResult, LeaseObservation, OutputSlice, PipeSessionStartInput,
    ReadOnlyFileFacts, ReadOnlyRootFacts, SandboxProfile, Workspace, WorkspaceAbsence,
    WorkspaceCreateInput,
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
    /// Every session start the daemon dispatched to the driver.
    session_starts: AtomicUsize,
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
        // Published in every case: none here turns on its absence. With the two facts below it
        // is what a session start needs to reach the checks of its embedded exec input.
        capability.facts.exec_read_only_files = Some(ReadOnlyFileFacts {
            max_files: substrate_wire::MAX_READ_ONLY_FILES,
        });
        capability.facts.leases_explicit = Some(true);
        capability.facts.exec_output_limit_bytes = Some(1_048_576);
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
                    requires: None,
                    writable_roots: Vec::new(),
                    workspace_access: substrate_wire::WorkspaceAccess::ReadWrite,
                    read_only_roots: Vec::new(),
                    read_only_files: Vec::new(),
                    secret_slots: Vec::new(),
                    workdir: None,
                    capability_snapshot: SNAPSHOT.to_owned(),
                    cgroup: Some(format!("fixture-{id}")),
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

    async fn start_pipe_session(
        &self,
        _id: &str,
        _workspace_root_name: &str,
        _input: &PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.session_starts.fetch_add(1, Ordering::SeqCst);
        DispatchOutcome::NotDispatched(DriverError::unserved(
            substrate_wire::SESSION_UNSERVED,
            "The fixture counts a session start and runs none.",
            "session",
        ))
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
    directory: TempDir,
    driver: Arc<Fixture>,
    app: Arc<App>,
}

impl Harness {
    /// A daemon's own layout in one directory: the state directory `state`, the workspace root
    /// `workspaces` and the socket directory `run`, with `toolchain` beside them, which the
    /// daemon does not keep. The application holds the read set of that configuration.
    fn open(publishes_fact: bool) -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        for child in ["state", "workspaces", "run", "toolchain"] {
            std::fs::create_dir(directory.path().join(child)).expect("harness directory");
        }
        let config = DaemonConfig::minimum(
            directory.path().join("run/api.sock"),
            directory.path().join("state/state.db"),
            directory.path().join("workspaces"),
            "dep_rorootfact",
            vec![1000],
        );
        let store = Arc::new(Store::open(&config.state).expect("state store"));
        let driver = Arc::new(Fixture {
            host: HostDriver::open(HostConfig::minimum(&config.workspaces)).expect("host driver"),
            publishes_fact: AtomicBool::new(publishes_fact),
            starts: AtomicUsize::new(0),
            session_starts: AtomicUsize::new(0),
            execs: Mutex::new(HashMap::new()),
        });
        // The constructor the composition root uses: the read set is its argument, so these cases
        // fail when that constructor stops holding an application to the set it was given.
        let app = App::with_delegated_context(
            Arc::clone(&store),
            Arc::clone(&driver) as Arc<dyn Driver>,
            "dep_rorootfact",
            Arc::new(SystemAuthority),
            DelegatedContextPolicy::none(),
            PrivateHostPaths::of(&config).expect("the read set"),
        );
        Self {
            directory,
            driver,
            app,
        }
    }

    /// A host path in the harness directory, as a start names it.
    fn host_path(&self, relative: &str) -> String {
        let path = if relative.is_empty() {
            self.directory.path().to_path_buf()
        } else {
            self.directory.path().join(relative)
        };
        path.to_str().expect("a UTF-8 harness path").to_owned()
    }

    async fn operation_state(&self, operation: &str) -> Value {
        let (status, body) = self
            .call(Method::GET, &format!("/v1/ops/{operation}"), Body::empty())
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["result"]["state"].clone()
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
        self.start_with(operation, workspace, roots, json!([]))
            .await
    }

    async fn start_with(
        &self,
        operation: &str,
        workspace: &str,
        roots: Value,
        files: Value,
    ) -> (StatusCode, Value) {
        let body = exec_input(workspace, roots, files);
        self.call(Method::POST, "/v1/execs", mutation(operation, body))
            .await
    }

    async fn start_session(
        &self,
        operation: &str,
        workspace: &str,
        roots: Value,
    ) -> (StatusCode, Value) {
        let mut exec = exec_input(workspace, roots, json!([]));
        exec["lease_ttl_ms"] = json!(60_000);
        let body = json!({
            "exec": exec,
            "input_limit_bytes": 65536,
            "frame_limit_bytes": 4096,
            "queued_frames": 4
        });
        self.call(Method::POST, "/v1/sessions", mutation(operation, body))
            .await
    }
}

#[allow(clippy::needless_pass_by_value)]
fn exec_input(workspace: &str, roots: Value, files: Value) -> Value {
    let mut input = json!({
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
    // Absent means none, as every start before ADR 0033 wrote it.
    if files != json!([]) {
        input["read_only_files"] = files;
    }
    input
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

/// The message of `exec.read-only-root-absent` for a root that meets the read set (ADR 0010,
/// amendment of 2026-11-22). It names no entry of the set.
const PRIVATE_ROOT_MESSAGE: &str = "A declared read-only root is not admitted: its host path \
                                    meets a path this host keeps private.";
/// The same for `exec.read-only-file-absent` (ADR 0033, amendment of 2026-11-22).
const PRIVATE_FILE_MESSAGE: &str = "A declared read-only file is not admitted: its host path \
                                    meets a path this host keeps private.";

fn root_at(host_path: &str) -> Value {
    json!([{ "host_path": host_path, "mount": "/opt/toolchain" }])
}

fn file_at(host_path: &str) -> Value {
    json!([{ "host_path": host_path, "mount": "/opt/tools/bin/tool" }])
}

/// A refusal of the amendment: 422 `refused` under the published code of its list, at the list,
/// with the message that names no path.
fn assert_private_source_refusal(
    status: StatusCode,
    refused: &Value,
    code: &str,
    message: &str,
    address: &str,
) {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let error = &refused["error"];
    assert_eq!(error["class"], "refused", "{refused}");
    assert_eq!(error["code"], code, "{refused}");
    assert_eq!(error["message"], message, "{refused}");
    assert_eq!(error["address"], address, "{refused}");
    assert_eq!(error["retriable"], false, "{refused}");
}

/// A root whose host path equals or lies under the state directory or the workspace root is
/// refused at admission: the driver receives no start and the ledger records the operation as
/// refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_at_or_under_a_private_directory_is_refused_before_dispatch() {
    let harness = Harness::open(true);
    let workspace = harness.workspace("01JROPRIVATEROOT00000001").await;
    for (relative, operation) in [
        ("state", "01JROPRIVATEROOT00000002"),
        ("state/state.processes", "01JROPRIVATEROOT00000003"),
        ("workspaces", "01JROPRIVATEROOT00000004"),
        ("workspaces/other", "01JROPRIVATEROOT00000005"),
    ] {
        let (status, refused) = harness
            .start(operation, &workspace, root_at(&harness.host_path(relative)))
            .await;
        assert_eq!(
            harness.driver.starts.load(Ordering::SeqCst),
            0,
            "a start naming {relative} as a read-only root reached the driver ({status} {refused})"
        );
        assert_private_source_refusal(
            status,
            &refused,
            "exec.read-only-root-absent",
            PRIVATE_ROOT_MESSAGE,
            "read_only_roots",
        );
        assert_eq!(harness.operation_state(operation).await, "refused");
    }
}

/// A file whose host path equals or lies under an entry of the read set is refused the same way,
/// under the published code of its own list.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_at_or_under_a_private_path_is_refused_before_dispatch() {
    let harness = Harness::open(true);
    let workspace = harness.workspace("01JROPRIVATEFILE00000001").await;
    for (relative, operation) in [
        ("state/state.db", "01JROPRIVATEFILE00000002"),
        ("workspaces/other/file", "01JROPRIVATEFILE00000003"),
        ("run/api.sock", "01JROPRIVATEFILE00000004"),
    ] {
        let (status, refused) = harness
            .start_with(
                operation,
                &workspace,
                json!([]),
                file_at(&harness.host_path(relative)),
            )
            .await;
        assert_eq!(
            harness.driver.starts.load(Ordering::SeqCst),
            0,
            "a start naming {relative} as a read-only file reached the driver ({status} {refused})"
        );
        assert_private_source_refusal(
            status,
            &refused,
            "exec.read-only-file-absent",
            PRIVATE_FILE_MESSAGE,
            "read_only_files",
        );
        assert_eq!(harness.operation_state(operation).await, "refused");
    }
}

/// A root that contains an entry of the read set shows it as well as one that names it: the
/// harness directory holds the state directory and the workspace root, and `run` the API socket.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_that_contains_a_private_path_is_refused_before_dispatch() {
    let harness = Harness::open(true);
    let workspace = harness.workspace("01JROPRIVATEHOLD00000001").await;
    for (relative, operation) in [
        ("", "01JROPRIVATEHOLD00000002"),
        ("run", "01JROPRIVATEHOLD00000003"),
    ] {
        let (status, refused) = harness
            .start(operation, &workspace, root_at(&harness.host_path(relative)))
            .await;
        assert_eq!(
            harness.driver.starts.load(Ordering::SeqCst),
            0,
            "a start whose read-only root holds a private path reached the driver \
             ({status} {refused})"
        );
        assert_private_source_refusal(
            status,
            &refused,
            "exec.read-only-root-absent",
            PRIVATE_ROOT_MESSAGE,
            "read_only_roots",
        );
        assert_eq!(harness.operation_state(operation).await, "refused");
    }
}

/// The control: a toolchain directory beside the daemon's own, a file in it, and a directory
/// that only shares a name prefix with the state directory are outside the read set, and each
/// start reaches the driver.
#[tokio::test(flavor = "multi_thread")]
async fn a_toolchain_directory_outside_the_read_set_reaches_the_driver() {
    let harness = Harness::open(true);
    let workspace = harness.workspace("01JROPRIVATECLEAR0000001").await;
    let (status, started) = harness
        .start_with(
            "01JROPRIVATECLEAR0000002",
            &workspace,
            root_at(&harness.host_path("toolchain")),
            file_at(&harness.host_path("toolchain/bin/tool")),
        )
        .await;
    assert!(status.is_success(), "{status} {started}");
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 1);
    let (status, started) = harness
        .start(
            "01JROPRIVATECLEAR0000003",
            &workspace,
            root_at(&harness.host_path("state-tools")),
        )
        .await;
    assert!(status.is_success(), "{status} {started}");
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 2);
}

/// The fact stays first: a host that does not publish `exec.read-only-roots` says nothing about
/// a path, so the same root is answered as the unserved capability.
#[tokio::test(flavor = "multi_thread")]
async fn a_private_root_is_answered_by_the_absent_fact_first() {
    let harness = Harness::open(false);
    let workspace = harness.workspace("01JROPRIVATEFACT00000001").await;
    let (status, refused) = harness
        .start(
            "01JROPRIVATEFACT00000002",
            &workspace,
            root_at(&harness.host_path("state")),
        )
        .await;
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 0);
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{refused}");
    assert_eq!(
        refused["error"]["code"], "exec.sandbox-unavailable",
        "{refused}"
    );
}

/// `session.start` carries the same exec input and is held to the same rule. The control first:
/// a session naming the toolchain directory reaches the driver, so the refusal below is not the
/// fixture refusing every session.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_start_naming_a_private_root_is_refused_before_dispatch() {
    let harness = Harness::open(true);
    let workspace = harness.workspace("01JROPRIVATESESSION00001").await;
    let (status, answered) = harness
        .start_session(
            "01JROPRIVATESESSION00002",
            &workspace,
            root_at(&harness.host_path("toolchain")),
        )
        .await;
    assert_eq!(
        harness.driver.session_starts.load(Ordering::SeqCst),
        1,
        "the control session start did not reach the driver ({status} {answered})"
    );
    let operation = "01JROPRIVATESESSION00003";
    let (status, refused) = harness
        .start_session(operation, &workspace, root_at(&harness.host_path("state")))
        .await;
    assert_eq!(
        harness.driver.session_starts.load(Ordering::SeqCst),
        1,
        "a session start naming the state directory as a read-only root reached the driver \
         ({status} {refused})"
    );
    assert_private_source_refusal(
        status,
        &refused,
        "exec.read-only-root-absent",
        PRIVATE_ROOT_MESSAGE,
        "read_only_roots",
    );
    assert_eq!(harness.operation_state(operation).await, "refused");
}
