//! Security conformance for writable Git roots (ADR 0038 kind 2): a workspace bound writable by an
//! exec in another workspace must not be destroyed while that exec may still hold the bind.
//!
//! `writable_git_roots.rs` proves the explicit `DELETE` refusal for a `running` exec whose applied
//! record names the root. These cases cover the two paths that file does not:
//!
//! - the lease-expiry path, which kills only the execs whose *own* workspace expired
//!   (`execs_for_workspace`) before it ends the workspace, and
//! - an exec whose dispatch outcome is unknown, whose stored record carries no applied
//!   confinement and therefore no writable root.
//!
//! The fixture driver delegates workspaces to the real `HostDriver`, publishes
//! `exec.writable-roots.git`, and records every signal it receives instead of running anything.
#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
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
use substrate_store::{NewOperation, Reservation, Scope, Store, StoredExec};
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

/// The lease of a workspace bound as a writable Git root expires while the exec that binds it,
/// in another workspace, still runs. Lease cleanup must not end the workspace under that exec:
/// either it leaves the checkout in place or it ends the binding exec first, as it ends the
/// execs of the expiring workspace itself.
#[tokio::test(flavor = "multi_thread")]
async fn lease_expiry_does_not_destroy_a_git_root_under_the_exec_that_binds_it() {
    let harness = Harness::open();
    let own = harness
        .workspace("01JWRSECLEASE0000000001", false, None)
        .await;
    let checkout = harness
        .workspace("01JWRSECLEASE0000000002", true, Some(1_000))
        .await;
    let (status, started) = harness
        .start("01JWRSECLEASE0000000003", &own, &checkout)
        .await;
    assert!(status.is_success(), "{status} {started}");
    assert_eq!(started["result"]["state"], "running", "{started}");
    let exec_id = started["result"]["id"]
        .as_str()
        .expect("exec id")
        .to_owned();

    tokio::time::sleep(Duration::from_millis(1_500)).await;
    for _ in 0..5 {
        harness.app.sweep_expired().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let signalled = harness
        .driver
        .signalled
        .lock()
        .expect("fixture lock")
        .clone();
    assert!(
        harness.checkout_on_host(&checkout) || signalled.contains(&exec_id),
        "lease cleanup destroyed writable Git root {checkout} while exec {exec_id}, which binds \
         it writable, was running and was never signalled (signalled: {signalled:?})"
    );
}

/// An exec whose dispatch outcome is unknown may hold its writable Git root. Its own workspace
/// cannot be destroyed (`workspace.execs-active`); the Git root it named must not be either.
#[tokio::test(flavor = "multi_thread")]
async fn a_git_root_of_an_exec_with_an_unknown_outcome_is_not_destroyed() {
    let harness = Harness::open();
    let own = harness
        .workspace("01JWRSECUNKNOWN00000001", false, None)
        .await;
    let checkout = harness
        .workspace("01JWRSECUNKNOWN00000002", true, None)
        .await;
    harness.driver.unknown.store(true, Ordering::SeqCst);
    let (status, started) = harness
        .start("01JWRSECUNKNOWN00000003", &own, &checkout)
        .await;
    assert_eq!(
        started["error"]["code"], "operation.outcome-unknown",
        "{status} {started}"
    );

    let (own_status, own_refused) = harness
        .call(
            Method::DELETE,
            &format!("/v1/workspaces/{own}"),
            mutation("01JWRSECUNKNOWN00000004", json!({})),
        )
        .await;
    assert_eq!(own_status, StatusCode::CONFLICT, "control: {own_refused}");
    assert_eq!(own_refused["error"]["code"], "workspace.execs-active");

    let (status, answer) = harness
        .call(
            Method::DELETE,
            &format!("/v1/workspaces/{checkout}"),
            mutation("01JWRSECUNKNOWN00000005", json!({})),
        )
        .await;
    assert_eq!(
        (status, answer["error"]["code"].clone()),
        (StatusCode::CONFLICT, json!("workspace.execs-active")),
        "writable Git root {checkout} of an exec whose outcome is unknown was not held: {answer}"
    );
    assert!(harness.checkout_on_host(&checkout));
}

/// An exec accepted but not yet dispatched, as a daemon that stopped between the two leaves it,
/// has no applied record. What acceptance recorded holds its writable Git root.
#[tokio::test(flavor = "multi_thread")]
async fn a_git_root_of_an_exec_accepted_before_dispatch_is_not_destroyed() {
    let harness = Harness::open();
    let own = harness
        .workspace("01JWRSECACCEPT000000001", false, None)
        .await;
    let checkout = harness
        .workspace("01JWRSECACCEPT000000002", true, None)
        .await;
    let scope = Scope {
        deployment: "dep_wrsecurity".to_owned(),
        subject: SUBJECT.to_owned(),
    };
    let exec_id = "ex_01JWRSECACCEPT00000000003";
    let operation = NewOperation {
        scope: scope.clone(),
        operation: "01JWRSECACCEPT000000003".to_owned(),
        operation_kind: "exec.start".to_owned(),
        request_hash: "a".repeat(64),
        accepted_at: Utc::now().to_rfc3339(),
        capability_snapshot: Some(SNAPSHOT.to_owned()),
        actor: "writable-roots-security".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(exec_id.to_owned()),
    };
    let accepted = StoredExec {
        resource: Exec {
            unrecorded_output: None,
            id: exec_id.to_owned(),
            kind: substrate_wire::ExecKind::Exec,
            workspace: own.clone(),
            state: ExecState::Accepted,
            observed_at: Utc::now(),
            requested: substrate_wire::ConfinementRequest {
                capability_snapshot: SNAPSHOT.to_owned(),
                network: substrate_wire::NetworkMode::None,
                aperture: None,
                apertures: Vec::new(),
                profile: SandboxProfile::Workspace,
                required: true,
            },
            applied: None,
            exit: None,
            usage: None,
            lease: None,
            refusal: None,
        },
        stdout: Vec::new(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        output_complete: false,
        cgroup: None,
        leader_pid: None,
    };
    assert_eq!(
        harness
            .store
            .reserve_exec_start_binding(
                &operation,
                &accepted,
                None,
                None,
                std::slice::from_ref(&checkout)
            )
            .expect("accept the exec"),
        Reservation::Accepted
    );

    let (own_status, own_refused) = harness
        .call(
            Method::DELETE,
            &format!("/v1/workspaces/{own}"),
            mutation("01JWRSECACCEPT000000004", json!({})),
        )
        .await;
    assert_eq!(own_status, StatusCode::CONFLICT, "control: {own_refused}");
    assert_eq!(own_refused["error"]["code"], "workspace.execs-active");

    let (status, answer) = harness
        .call(
            Method::DELETE,
            &format!("/v1/workspaces/{checkout}"),
            mutation("01JWRSECACCEPT000000005", json!({})),
        )
        .await;
    assert_eq!(
        (status, answer["error"]["code"].clone()),
        (StatusCode::CONFLICT, json!("workspace.execs-active")),
        "writable Git root {checkout} of an accepted exec was not held: {answer}"
    );
    assert!(harness.checkout_on_host(&checkout));
}
