//! Writable Git roots (ADR 0038 kind 2), driven through the router: the daemon resolves each Git
//! root a start names in the caller's scope, hands the driver the root name the store holds, and
//! refuses every root it cannot bind by name, before dispatch, as a durable ledger row a retry
//! replays.
//!
//! [`GitRootFixtureDriver`] delegates the workspace half to the real `HostDriver`, publishes
//! `exec.writable-roots.git`, answers "materialized from Git" for the root names a case marks, and
//! records the bindings each start was handed instead of running anything. It proves the daemon's
//! admission; the bind itself and a child's commit through it are the host driver's and the
//! delegated lane's to prove (`runtime_vectors.rs`), not this file's.
#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
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
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, AppliedWritableRoot, CapabilitySnapshot,
    Exec, ExecExit, ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState, FileAbsence,
    FileObservation, FileReadQuery, FileReadResult, LeaseObservation, OutputSlice, SandboxProfile,
    Workspace, WorkspaceAbsence, WorkspaceCreateInput, WritableRoot,
};
use tempfile::TempDir;
use tower::ServiceExt as _;

const SUBJECT: &str = "local:1000";
const OTHER: &str = "local:1001";
const SNAPSHOT: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
/// An argv that leaves the fixture's exec `running`, so it holds its roots.
const KEEP_RUNNING: &str = "/fixture/keep-running";

struct GitRootFixtureDriver {
    host: Arc<HostDriver>,
    /// Root names the fixture reports as materialized from a Git source.
    git_origins: Mutex<HashSet<String>>,
    /// Root names whose destroy stays pending, leaving the workspace `destroying`.
    pending_destroy: Mutex<HashSet<String>>,
    /// The Git bindings every dispatched start was handed, in order.
    dispatched: Mutex<Vec<Vec<GitRootBinding>>>,
    execs: Mutex<HashMap<String, ExecObservation>>,
}

impl GitRootFixtureDriver {
    fn observe(&self, id: &str, git_roots: &[GitRootBinding], input: &ExecStartInput) -> Exec {
        let running = input.argv[0] == KEEP_RUNNING;
        let writable_roots = input
            .writable_roots
            .iter()
            .filter_map(|root| match root {
                WritableRoot::Git(root) => {
                    let binding = git_roots
                        .iter()
                        .find(|binding| binding.workspace == root.workspace)
                        .expect("every Git root the daemon admitted is bound");
                    Some(AppliedWritableRoot::Git {
                        workspace: root.workspace.clone(),
                        host_path: self
                            .host
                            .workspace_path(&binding.root_name)
                            .expect("bound root name")
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
            state: if running {
                ExecState::Running
            } else {
                ExecState::Exited
            },
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
            exit: (!running).then_some(ExecExit {
                code: Some(0),
                signal: None,
            }),
            usage: None,
            lease: None,
            refusal: None,
        }
    }

    fn dispatch(
        &self,
        id: &str,
        git_roots: &[GitRootBinding],
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.dispatched
            .lock()
            .expect("fixture lock")
            .push(git_roots.to_vec());
        let resource = self.observe(id, git_roots, input);
        let complete = resource.state == ExecState::Exited;
        let observation = ExecObservation {
            resource,
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            stdout_kept: None,
            stderr_kept: None,
            output_complete: complete,
            cgroup: None,
            leader_pid: None,
        };
        self.execs
            .lock()
            .expect("fixture lock")
            .insert(id.to_owned(), observation.clone());
        DispatchOutcome::Observed(observation)
    }
}

#[async_trait]
impl Driver for GitRootFixtureDriver {
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
        if self
            .pending_destroy
            .lock()
            .expect("fixture lock")
            .contains(root_name)
        {
            return Ok(WorkspaceDestroyProgress::Pending { removed_items: 0 });
        }
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
        id: &str,
        _workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.dispatch(id, &[], input)
    }

    async fn start_exec_with_git_roots(
        &self,
        id: &str,
        _workspace_root_name: &str,
        git_roots: &[GitRootBinding],
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.dispatch(id, git_roots, input)
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
    driver: Arc<GitRootFixtureDriver>,
    app: Arc<App>,
}

impl Harness {
    fn open() -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        std::fs::create_dir(directory.path().join("workspaces")).expect("workspace root");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = Arc::new(GitRootFixtureDriver {
            host: HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
                .expect("host driver"),
            git_origins: Mutex::new(HashSet::new()),
            pending_destroy: Mutex::new(HashSet::new()),
            dispatched: Mutex::new(Vec::new()),
            execs: Mutex::new(HashMap::new()),
        });
        let app = App::new(
            store,
            Arc::clone(&driver) as Arc<dyn Driver>,
            "dep_gitroots",
        );
        Self {
            directory,
            driver,
            app,
        }
    }

    async fn call(
        &self,
        subject: &str,
        method: Method,
        uri: &str,
        body: Body,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-request-id", "req_git_roots_test")
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: subject.to_owned(),
            actor: "git-roots-test".to_owned(),
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

    /// An empty workspace; with `git`, the fixture reports it materialized from a Git source.
    async fn workspace(&self, subject: &str, operation: &str, git: bool) -> String {
        let (status, created) = self
            .call(
                subject,
                Method::POST,
                "/v1/workspaces",
                mutation(operation, json!({ "source": "empty", "labels": {} })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = text(&created["result"]["id"]).to_owned();
        if git {
            let root_name = text(&created["result"]["root_name"]).to_owned();
            self.driver
                .git_origins
                .lock()
                .expect("fixture lock")
                .insert(root_name);
        }
        id
    }

    async fn start(
        &self,
        subject: &str,
        operation: &str,
        workspace: &str,
        argv: &str,
        roots: Value,
    ) -> (StatusCode, Value) {
        let mut body = exec_body(workspace, argv);
        body["writable_roots"] = roots;
        self.call(
            subject,
            Method::POST,
            "/v1/execs",
            mutation(operation, body),
        )
        .await
    }

    async fn operation(&self, subject: &str, operation: &str) -> Value {
        let (status, body) = self
            .call(
                subject,
                Method::GET,
                &format!("/v1/ops/{operation}"),
                Body::empty(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    fn dispatched(&self) -> Vec<Vec<GitRootBinding>> {
        self.driver.dispatched.lock().expect("fixture lock").clone()
    }
}

#[allow(clippy::needless_pass_by_value)] // Test call sites construct one-shot JSON values.
fn mutation(operation: &str, input: Value) -> Body {
    Body::from(
        serde_json::to_vec(&json!({ "op": operation, "input": input })).expect("mutation JSON"),
    )
}

fn text(value: &Value) -> &str {
    value.as_str().expect("JSON string")
}

fn exec_body(workspace: &str, argv: &str) -> Value {
    json!({
        "workspace": workspace,
        "argv": [argv],
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
        "wait": true
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_git_root_is_resolved_in_scope_and_handed_to_the_driver_by_root_name() {
    let harness = Harness::open();
    let own = harness
        .workspace(SUBJECT, "01JGITROOTRESOLVE000001", false)
        .await;
    let checkout = harness
        .workspace(SUBJECT, "01JGITROOTRESOLVE000002", true)
        .await;
    let (status, started) = harness
        .start(
            SUBJECT,
            "01JGITROOTRESOLVE000003",
            &own,
            "/usr/bin/true",
            json!([{ "workspace": checkout, "mount": "/srv/checkout" }]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(
        harness.dispatched(),
        [vec![GitRootBinding {
            workspace: checkout.clone(),
            root_name: checkout.clone(),
        }]]
    );
    let host_path = harness.directory.path().join("workspaces").join(&checkout);
    assert_eq!(
        started["result"]["applied"]["writable_roots"],
        json!([{
            "kind": "git",
            "workspace": checkout,
            "host_path": host_path.display().to_string(),
            "mount": "/srv/checkout",
        }]),
        "{started}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One table of refusals, each replayed and read back from the ledger.
async fn every_git_root_refusal_is_named_before_dispatch_and_recorded() {
    let harness = Harness::open();
    let own = harness
        .workspace(SUBJECT, "01JGITROOTREFUSE0000001", true)
        .await;
    let checkout = harness
        .workspace(SUBJECT, "01JGITROOTREFUSE0000002", true)
        .await;
    let empty = harness
        .workspace(SUBJECT, "01JGITROOTREFUSE0000003", false)
        .await;
    let foreign = harness
        .workspace(OTHER, "01JGITROOTREFUSE0000004", true)
        .await;
    let destroying = harness
        .workspace(SUBJECT, "01JGITROOTREFUSE0000005", true)
        .await;

    std::fs::create_dir(
        harness
            .directory
            .path()
            .join("workspaces")
            .join("operator-checkout"),
    )
    .expect("operator directory");
    let (status, adopted) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/workspaces/adopt",
            mutation(
                "01JGITROOTREFUSE0000006",
                json!({ "root_name": "operator-checkout", "labels": {} }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{adopted}");
    let adopted = text(&adopted["result"]["id"]).to_owned();
    // Even when the driver would call it Git, an adopted directory is the operator's, not a
    // materialization: the daemon hands the driver its recorded origin, which refuses it.
    harness
        .driver
        .git_origins
        .lock()
        .expect("fixture lock")
        .insert("operator-checkout".to_owned());

    harness
        .driver
        .pending_destroy
        .lock()
        .expect("fixture lock")
        .insert(destroying.clone());
    let (status, destroyed) = harness
        .call(
            SUBJECT,
            Method::DELETE,
            &format!("/v1/workspaces/{destroying}"),
            mutation("01JGITROOTREFUSE0000007", json!({})),
        )
        .await;
    // A destroy the driver has not finished answers outcome-unknown and leaves the workspace
    // `destroying` for maintenance: no longer Ready.
    assert_eq!(
        destroyed["error"]["code"], "operation.outcome-unknown",
        "{status} {destroyed}"
    );
    let (_, observed) = harness
        .call(
            SUBJECT,
            Method::GET,
            &format!("/v1/workspaces/{destroying}"),
            Body::empty(),
        )
        .await;
    assert_ne!(observed["result"]["state"], "ready", "{observed}");

    let root = |workspace: &str, mount: &str| json!({ "workspace": workspace, "mount": mount });
    let cases = [
        (
            "01JGITROOTREFUSE0000101",
            json!([root(&empty, "/srv/a")]),
            StatusCode::UNPROCESSABLE_ENTITY,
            "exec.writable-root-not-git",
            Some(empty.clone()),
        ),
        (
            "01JGITROOTREFUSE0000102",
            json!([root(&adopted, "/srv/a")]),
            StatusCode::UNPROCESSABLE_ENTITY,
            "exec.writable-root-not-git",
            Some(adopted.clone()),
        ),
        (
            "01JGITROOTREFUSE0000103",
            json!([root(&checkout, "/srv/a"), root(&own, "/srv/b")]),
            StatusCode::UNPROCESSABLE_ENTITY,
            "exec.writable-root-own-workspace",
            Some(own.clone()),
        ),
        (
            "01JGITROOTREFUSE0000104",
            json!([root(&checkout, "/srv/a"), root(&checkout, "/srv/b")]),
            StatusCode::UNPROCESSABLE_ENTITY,
            "exec.writable-root-workspace-duplicate",
            Some(checkout.clone()),
        ),
        (
            "01JGITROOTREFUSE0000105",
            json!([root("ws_neverexisted", "/srv/a")]),
            StatusCode::NOT_FOUND,
            "resource.not-found",
            None,
        ),
        (
            "01JGITROOTREFUSE0000106",
            json!([root(&foreign, "/srv/a")]),
            StatusCode::NOT_FOUND,
            "resource.not-found",
            None,
        ),
        (
            "01JGITROOTREFUSE0000107",
            json!([root(&checkout, "/srv/a"), root(&destroying, "/srv/b")]),
            StatusCode::CONFLICT,
            "workspace.not-ready",
            None,
        ),
    ];
    let mut not_found_bodies = Vec::new();
    for (operation, roots, expected, code, named) in cases {
        let (status, refused) = harness
            .start(SUBJECT, operation, &own, "/usr/bin/true", roots.clone())
            .await;
        assert_eq!(status, expected, "{operation} {refused}");
        assert_eq!(refused["error"]["code"], code, "{operation} {refused}");
        assert_eq!(
            refused["error"]["address"], "writable_roots",
            "{operation} {refused}"
        );
        if let Some(named) = named {
            assert!(
                text(&refused["error"]["message"]).contains(&named),
                "{operation} {refused}"
            );
        }
        let (replay_status, replayed) = harness
            .start(SUBJECT, operation, &own, "/usr/bin/true", roots)
            .await;
        assert_eq!(replay_status, status, "{operation}");
        assert_eq!(replayed["error"], refused["error"], "{operation}");
        let record = harness.operation(SUBJECT, operation).await;
        assert_eq!(record["result"]["state"], "refused", "{operation} {record}");
        assert_eq!(record["result"]["operation_kind"], "exec.start");
        if code == "resource.not-found" {
            let mut error = refused["error"].clone();
            error.as_object_mut().expect("error").remove("operation");
            not_found_bodies.push(error);
        }
    }
    // Another subject's workspace and one that never existed answer alike, apart from the
    // operation id: a Git root is no existence oracle across subjects.
    assert_eq!(not_found_bodies[0], not_found_bodies[1]);
    assert!(
        harness.dispatched().is_empty(),
        "no refused start reached the driver: {:?}",
        harness.dispatched()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_a_running_exec_binds_as_a_git_root_is_not_destroyed_under_it() {
    let harness = Harness::open();
    let own = harness
        .workspace(SUBJECT, "01JGITROOTINUSE00000001", false)
        .await;
    let checkout = harness
        .workspace(SUBJECT, "01JGITROOTINUSE00000002", true)
        .await;
    let (status, started) = harness
        .start(
            SUBJECT,
            "01JGITROOTINUSE00000003",
            &own,
            KEEP_RUNNING,
            json!([{ "workspace": checkout, "mount": "/srv/checkout" }]),
        )
        .await;
    assert!(status.is_success(), "{status} {started}");
    assert_eq!(started["result"]["state"], "running", "{started}");
    let (status, refused) = harness
        .call(
            SUBJECT,
            Method::DELETE,
            &format!("/v1/workspaces/{checkout}"),
            mutation("01JGITROOTINUSE00000004", json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(
        refused["error"]["code"], "workspace.execs-active",
        "{refused}"
    );
    assert!(
        harness
            .directory
            .path()
            .join("workspaces")
            .join(&checkout)
            .is_dir(),
        "the bound checkout is still on the host"
    );
}
