//! Adversarial pass on the writable-roots wave (ADR 0038 kind 2): a Git root's lease, checked at
//! admission, is not checked again where the start's own workspace's lease is.
//!
//! ADR 0038 decision 3: "Admission treats it as it treats the start's workspace: lease freeze,
//! write authority and storage quota apply unchanged." The start's own workspace is admitted by
//! `admit_workspace` and then frozen-if-due again inside the transaction that accepts the exec
//! (`reserve_exec_start_binding`, `freeze_workspace_lease_if_due`), so a lease that falls due
//! between admission and acceptance refuses the start. The Git roots are passed to that
//! transaction only to be recorded (`record_exec_git_roots`); nothing there re-checks their lease.
//!
//! The fixture is `writable_roots_security.rs`'s, with one addition: `admit_git_root` can stall,
//! standing in for the time admission takes between the store read and the accepting transaction.
//! Each case stalls past a 1 000 ms lease (the minimum) on exactly one of the two workspaces.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, AppliedWritableRoot, CapabilitySnapshot,
    Exec, ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState, FileAbsence,
    FileObservation, FileReadQuery, FileReadResult, LeaseObservation, OutputSlice, SandboxProfile,
    Workspace, WorkspaceAbsence, WorkspaceCreateInput, WritableRoot,
};
use tempfile::TempDir;
use tower::ServiceExt as _;

const SUBJECT: &str = "local:1000";
const SNAPSHOT: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

struct Fixture {
    host: Arc<HostDriver>,
    git_origins: Mutex<HashSet<String>>,
    /// When set, a start answers `OutcomeUnknown`: the driver cannot prove whether it runs.
    unknown: AtomicBool,
    execs: Mutex<HashMap<String, ExecObservation>>,
    /// Every exec id the daemon asked to signal.
    signalled: Mutex<Vec<String>>,
    /// Milliseconds `admit_git_root` blocks before it answers.
    stall_ms: AtomicU64,
}

impl Fixture {
    fn running(&self, id: &str, git_roots: &[GitRootBinding], input: &ExecStartInput) -> Exec {
        let writable_roots = input
            .writable_roots
            .iter()
            .filter_map(|root| match root {
                WritableRoot::Git(root) => {
                    let binding = git_roots
                        .iter()
                        .find(|binding| binding.workspace == root.workspace)
                        .expect("admitted Git root");
                    Some(AppliedWritableRoot::Git {
                        workspace: root.workspace.clone(),
                        host_path: self
                            .host
                            .workspace_path(&binding.root_name)
                            .expect("root name")
                            .display()
                            .to_string(),
                        mount: root.mount.clone(),
                    })
                }
                WritableRoot::Host(_) => None,
            })
            .collect();
        Exec {
            unrecorded_output: None,
            id: id.to_owned(),
            kind: substrate_wire::ExecKind::Exec,
            workspace: input.workspace.clone(),
            state: ExecState::Running,
            observed_at: Utc::now(),
            requested: input.sandbox.clone(),
            applied: Some(AppliedConfinement {
                writable_roots,
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
        }
    }
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
        capability.facts.workspace_git = Some(true);
        capability.facts.exec_writable_roots_git = Some(true);
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
        root_name: &str,
        adopted: bool,
    ) -> Result<(), DriverError> {
        let stall = self.stall_ms.load(Ordering::SeqCst);
        if stall > 0 {
            std::thread::sleep(Duration::from_millis(stall));
        }
        if !adopted
            && self
                .git_origins
                .lock()
                .expect("fixture lock")
                .contains(root_name)
        {
            Ok(())
        } else {
            Err(substrate_host::writable_root_not_git(workspace))
        }
    }

    async fn start_exec(
        &self,
        _id: &str,
        _workspace_root_name: &str,
        _input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        panic!("every start in this file names a Git root");
    }

    async fn start_exec_with_git_roots(
        &self,
        id: &str,
        _workspace_root_name: &str,
        git_roots: &[GitRootBinding],
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        if self.unknown.load(Ordering::SeqCst) {
            return DispatchOutcome::OutcomeUnknown(DriverError::failed(
                "exec.containment-unproven",
                "The fixture cannot prove whether the exec runs.",
            ));
        }
        let observation = ExecObservation {
            resource: self.running(id, git_roots, input),
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
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
        id: &str,
        _input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        self.signalled
            .lock()
            .expect("fixture lock")
            .push(id.to_owned());
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
    driver: Arc<Fixture>,
    app: Arc<App>,
}

impl Harness {
    fn open() -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        std::fs::create_dir(directory.path().join("workspaces")).expect("workspace root");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = Arc::new(Fixture {
            host: HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
                .expect("host driver"),
            git_origins: Mutex::new(HashSet::new()),
            unknown: AtomicBool::new(false),
            execs: Mutex::new(HashMap::new()),
            signalled: Mutex::new(Vec::new()),
            stall_ms: AtomicU64::new(0),
        });
        let app = App::new(
            Arc::clone(&store),
            Arc::clone(&driver) as Arc<dyn Driver>,
            "dep_wrsecurity",
        );
        Self {
            directory,
            store,
            driver,
            app,
        }
    }

    async fn call(&self, method: Method, uri: &str, body: Body) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-request-id", "req_wr_security")
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: SUBJECT.to_owned(),
            actor: "writable-roots-security".to_owned(),
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

    async fn workspace(&self, operation: &str, git: bool, lease_ttl_ms: Option<u64>) -> String {
        let mut input = json!({ "source": "empty", "labels": {} });
        if let Some(ttl) = lease_ttl_ms {
            input["lease_ttl_ms"] = json!(ttl);
        }
        let (status, created) = self
            .call(Method::POST, "/v1/workspaces", mutation(operation, input))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = created["result"]["id"].as_str().expect("id").to_owned();
        if git {
            let root_name = created["result"]["root_name"]
                .as_str()
                .expect("root name")
                .to_owned();
            self.driver
                .git_origins
                .lock()
                .expect("fixture lock")
                .insert(root_name);
        }
        id
    }

    async fn start(&self, operation: &str, own: &str, checkout: &str) -> (StatusCode, Value) {
        let body = json!({
            "workspace": own,
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
            "wait": true,
            "writable_roots": [{ "workspace": checkout, "mount": "/srv/checkout" }]
        });
        self.call(Method::POST, "/v1/execs", mutation(operation, body))
            .await
    }

    fn checkout_on_host(&self, checkout: &str) -> bool {
        self.directory
            .path()
            .join("workspaces")
            .join(checkout)
            .is_dir()
    }
}

#[allow(clippy::needless_pass_by_value)]
fn mutation(operation: &str, input: Value) -> Body {
    Body::from(serde_json::to_vec(&json!({ "op": operation, "input": input })).expect("JSON"))
}

/// Longer than the 1 000 ms lease each case grants, so the lease falls due during admission.
const STALL_PAST_LEASE_MS: u64 = 1_300;

/// Control: the start's own workspace's lease falls due while admission runs. The accepting
/// transaction freezes it and the start is not accepted: nothing is dispatched.
#[tokio::test(flavor = "multi_thread")]
async fn control_own_workspace_lease_due_during_admission_is_not_accepted() {
    let harness = Harness::open();
    let own = harness
        .workspace("01JWRADVLEASEOWN0000001", false, Some(1_000))
        .await;
    let checkout = harness
        .workspace("01JWRADVLEASEOWN0000002", true, None)
        .await;
    harness
        .driver
        .stall_ms
        .store(STALL_PAST_LEASE_MS, Ordering::SeqCst);
    let (status, answer) = harness
        .start("01JWRADVLEASEOWN0000003", &own, &checkout)
        .await;
    assert!(!status.is_success(), "{status} {answer}");
    assert!(
        harness
            .driver
            .execs
            .lock()
            .expect("fixture lock")
            .is_empty(),
        "dispatched: {answer}"
    );
}

/// The same race on the Git root instead: its lease falls due while admission runs. ADR 0038
/// decision 3 applies lease freeze to it unchanged, so the start is not accepted, as the
/// control's is not.
#[tokio::test(flavor = "multi_thread")]
async fn git_root_lease_due_during_admission_is_not_accepted_as_the_own_workspace_is_not() {
    let harness = Harness::open();
    let own = harness
        .workspace("01JWRADVLEASEGIT0000001", false, None)
        .await;
    let checkout = harness
        .workspace("01JWRADVLEASEGIT0000002", true, Some(1_000))
        .await;
    harness
        .driver
        .stall_ms
        .store(STALL_PAST_LEASE_MS, Ordering::SeqCst);
    let (status, answer) = harness
        .start("01JWRADVLEASEGIT0000003", &own, &checkout)
        .await;
    assert!(
        !status.is_success(),
        "a start was accepted and dispatched binding writable Git root {checkout}, whose lease \
         fell due before the start was accepted: {status} {answer}"
    );
}

/// Pre-existing, outside this wave's diff: the control's refusal itself. A lease that falls due
/// between admission and acceptance is a frozen workspace, which admission answers
/// `409 workspace.not-ready` (`retriable: false`); the accepting transaction's
/// `StoreError::WorkspaceFrozen` reaches `reservation_response`'s `Err(error)` arm and is answered
/// `500 state.store-failed` with `retriable: true`, so a client is told to retry a start that can
/// never succeed.
#[tokio::test(flavor = "multi_thread")]
async fn own_workspace_lease_due_during_admission_is_refused_not_ready_not_store_failed() {
    let harness = Harness::open();
    let own = harness
        .workspace("01JWRADVLEASE5000000001", false, Some(1_000))
        .await;
    let checkout = harness
        .workspace("01JWRADVLEASE5000000002", true, None)
        .await;
    harness
        .driver
        .stall_ms
        .store(STALL_PAST_LEASE_MS, Ordering::SeqCst);
    let (status, answer) = harness
        .start("01JWRADVLEASE5000000003", &own, &checkout)
        .await;
    assert_eq!(
        (
            status,
            answer["error"]["code"].clone(),
            answer["error"]["retriable"].clone()
        ),
        (
            StatusCode::CONFLICT,
            json!("workspace.not-ready"),
            json!(false)
        ),
        "{answer}"
    );
}
