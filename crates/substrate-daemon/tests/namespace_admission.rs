//! Exec admission over both fact sets (design 34 § 6.4, ADR 0047 N6, § 12 U10).
//!
//! An exec is admitted on the four Linux facts, or on design 30's neutral set:
//! `confinement.family.process-run`, `.wait`, `.output-read`, `.limits`,
//! `confinement.whole-tree-kill` and `confinement.no-egress`. Otherwise it is
//! `exec.sandbox-unavailable` (501), as before. The inline-capsule check comes after it: a capsule
//! on a driver that does not publish `exec.inline-capsule` is `exec.inline-capsule-unserved` (501,
//! address `capsule`), and the host, which publishes the capsule fact whenever it publishes the
//! four, answers exactly as before.
//!
//! The current bundle carries `confinement.no-egress` and none of the neutral families, so no
//! snapshot can state the neutral set yet: `confinement.no-egress` alone is never admission. A
//! fixture driver publishes each fact set and counts every start it receives, so a refusal is
//! shown to be made before dispatch.
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use substrate_daemon::{App, Identity, router};
use substrate_host::{
    DispatchOutcome, Driver, DriverError, ExecObservation, HostConfig, HostDriver,
    WorkspaceDestroyProgress,
};
use substrate_store::Store;
use substrate_wire::{
    CapabilitySnapshot, ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecutionCapsuleFacts,
    FileAbsence, FileObservation, FileReadQuery, FileReadResult, HostDriverKind, LeaseObservation,
    OutputSlice, Workspace, WorkspaceAbsence, WorkspaceCreateInput,
};
use tempfile::TempDir;
use tower::ServiceExt as _;

const SUBJECT: &str = "local:1000";
const SNAPSHOT: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

/// The facts a fixture publishes beside the host's own workspace facts.
#[derive(Clone, Copy)]
enum Facts {
    /// The four Linux facts, with or without `exec.inline-capsule`.
    Linux { capsule: bool },
    /// What a namespace driver can state in this bundle: `confinement.no-egress` and argv-only,
    /// no Linux fact and no capsule fact.
    NoEgressOnly,
    /// Neither set.
    Neither,
}

struct Fixture {
    host: Arc<HostDriver>,
    facts: Facts,
    starts: AtomicUsize,
}

#[async_trait]
impl Driver for Fixture {
    async fn shutdown(&self) -> Result<(), DriverError> {
        self.host.shutdown().await
    }

    fn machine(&self) -> CapabilitySnapshot {
        let mut capability = self.host.machine();
        SNAPSHOT.clone_into(&mut capability.snapshot);
        let facts = &mut capability.facts;
        facts.exec_argv_only = Some(true);
        let (linux, capsule) = match self.facts {
            Facts::Linux { capsule } => (true, capsule),
            Facts::NoEgressOnly | Facts::Neither => (false, false),
        };
        facts.exec_namespaces = linux.then_some(substrate_wire::NamespaceFacts {
            user: true,
            mount: true,
            pid: true,
            ipc: true,
            uts: true,
            network: true,
        });
        facts.exec_cgroup_limits = linux.then_some(substrate_wire::CgroupLimitFacts {
            processes: true,
            memory: true,
            cpu: true,
        });
        facts.exec_cgroup_kill = linux.then_some(true);
        facts.exec_no_egress = linux.then_some(true);
        facts.confinement_no_egress = matches!(self.facts, Facts::NoEgressOnly).then_some(true);
        facts.exec_inline_capsule = capsule.then_some(ExecutionCapsuleFacts {
            mount: substrate_wire::EXECUTION_CAPSULE_MOUNT.to_owned(),
            max_files: substrate_wire::MAX_EXECUTION_CAPSULE_FILES,
            max_file_bytes: substrate_wire::MAX_EXECUTION_CAPSULE_FILE_BYTES,
            max_total_bytes: substrate_wire::MAX_EXECUTION_CAPSULE_BYTES,
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

    async fn start_exec(
        &self,
        _id: &str,
        _workspace_root_name: &str,
        _input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        // The fixture runs nothing: a start that reaches it is answered as a refusal the driver
        // made, which the daemon records. Only whether it was reached matters here.
        DispatchOutcome::NotDispatched(DriverError::unserved(
            "exec.fixture-reached",
            "The fixture driver was reached.",
            "exec",
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
    _directory: TempDir,
    driver: Arc<Fixture>,
    app: Arc<App>,
}

impl Harness {
    fn open(facts: Facts) -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        std::fs::create_dir(directory.path().join("workspaces")).expect("workspace root");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = Arc::new(Fixture {
            host: HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
                .expect("host driver"),
            facts,
            starts: AtomicUsize::new(0),
        });
        let app = App::new(
            Arc::clone(&store),
            Arc::clone(&driver) as Arc<dyn Driver>,
            "dep_nsadmission",
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
            .header("x-request-id", "req_ns_admission")
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: SUBJECT.to_owned(),
            actor: "namespace-admission".to_owned(),
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

    async fn start(&self, operation: &str, capsule: bool) -> (StatusCode, Value) {
        let workspace = self.workspace(&format!("{operation}W")).await;
        let mut body = json!({
            "workspace": workspace,
            "argv": ["/fixture/run"],
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
            "wait": false
        });
        if capsule {
            body["capsule"] = json!({
                "manifest_sha256": "a".repeat(64),
                "entrypoint": "run",
                "files": [{
                    "path": "run",
                    "role": "runtime",
                    "executable": true,
                    "sha256": "b".repeat(64),
                    "content": { "encoding": "base64", "data": "eA==" }
                }]
            });
        }
        self.call(Method::POST, "/v1/execs", mutation(operation, body))
            .await
    }

    fn starts(&self) -> usize {
        self.driver.starts.load(Ordering::SeqCst)
    }
}

#[allow(clippy::needless_pass_by_value)]
fn mutation(operation: &str, input: Value) -> Body {
    Body::from(serde_json::to_vec(&json!({ "op": operation, "input": input })).expect("JSON"))
}

fn assert_refused(response: &(StatusCode, Value), code: &str, address: &str) {
    let (status, body) = response;
    assert_eq!(*status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["error"]["class"], "unserved", "{body}");
    assert_eq!(body["error"]["code"], code, "{body}");
    assert_eq!(body["error"]["address"], address, "{body}");
}

/// The control: the four Linux facts admit a start, which reaches the driver. Without it every
/// refusal below could be a fixture that admits nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_four_linux_facts_admit_a_start_and_it_reaches_the_driver() {
    let harness = Harness::open(Facts::Linux { capsule: false });
    let response = harness.start("01JNSADMISSIONLINUX00001", false).await;
    assert_eq!(harness.starts(), 1, "{response:?}");
}

/// `confinement.no-egress` alone is not the neutral set: without the families and whole-tree
/// kill, which this bundle cannot state, the start is refused as before and never dispatched.
#[tokio::test(flavor = "multi_thread")]
async fn no_egress_alone_is_sandbox_unavailable_and_never_dispatched() {
    let harness = Harness::open(Facts::NoEgressOnly);
    let response = harness.start("01JNSADMISSIONEGRESS0001", false).await;
    assert_refused(&response, "exec.sandbox-unavailable", "exec.namespaces");
    assert_eq!(harness.starts(), 0);
}

/// § 6.4: the capsule check comes after the confinement check. A capsule on a driver that admits
/// the start but publishes no `exec.inline-capsule` is refused by name before dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn a_capsule_without_the_capsule_fact_is_inline_capsule_unserved_before_dispatch() {
    let harness = Harness::open(Facts::Linux { capsule: false });
    let response = harness.start("01JNSADMISSIONCAPSULE001", true).await;
    assert_refused(
        &response,
        substrate_wire::EXEC_INLINE_CAPSULE_UNSERVED,
        "capsule",
    );
    let unserved = substrate_wire::DRIVER_UNSERVED_REFUSALS
        .iter()
        .find(|refusal| refusal.code == substrate_wire::EXEC_INLINE_CAPSULE_UNSERVED)
        .expect("the wire crate states the refusal");
    assert_eq!(response.1["error"]["message"], unserved.message);
    assert_eq!(harness.starts(), 0);
}

/// With neither set, a capsule still hears `exec.sandbox-unavailable` first: the order of
/// § 6.4, which keeps the portable host's answer to a capsule unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn without_either_set_a_capsule_hears_sandbox_unavailable_first() {
    for (facts, operation) in [
        (Facts::Neither, "01JNSADMISSIONNEITHER001"),
        (Facts::NoEgressOnly, "01JNSADMISSIONNEITHER002"),
    ] {
        let harness = Harness::open(facts);
        let response = harness.start(operation, true).await;
        assert_refused(&response, "exec.sandbox-unavailable", "exec.namespaces");
        assert_eq!(harness.starts(), 0);
    }
}

/// The host lane: whatever this host proves, its snapshot names the host driver and never the
/// neutral no-egress fact, and it publishes `exec.inline-capsule` exactly when it publishes the
/// four Linux facts, so the capsule check never answers a host start the old check admitted.
#[test]
fn the_host_snapshot_is_unchanged_by_the_namespace_facts() {
    let directory = tempfile::tempdir().expect("temporary host");
    std::fs::create_dir(directory.path().join("workspaces")).expect("workspace root");
    let host = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    let snapshot = host.machine();
    assert_eq!(snapshot.driver, HostDriverKind::Host);
    assert_eq!(snapshot.facts.confinement_no_egress, None);
    let linux = snapshot.facts.exec_namespaces.is_some()
        && snapshot.facts.exec_cgroup_limits.is_some()
        && snapshot.facts.exec_cgroup_kill == Some(true)
        && snapshot.facts.exec_no_egress == Some(true);
    assert_eq!(snapshot.facts.exec_inline_capsule.is_some(), linux);
}
