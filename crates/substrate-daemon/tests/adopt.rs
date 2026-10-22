//! `workspace.adopt` (ADR 0035), driven through the router: an operator's existing directory is
//! served after an explicit adopt, every adopt refusal is a durable ledger row a retry replays, and
//! no removal path deletes the operator's files.
//!
//! The exec case runs through [`AdoptFixtureDriver`], which delegates every workspace call to the
//! real `HostDriver` and runs the exec's argv, unconfined, in the directory the host resolves for
//! the root name the daemon hands it. It proves the daemon admits an adopted workspace and
//! dispatches its exact root; the confinement floor around that dispatch is the host driver's and
//! the delegated lane's to prove, not this file's.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use serde_json::{Value, json};
use substrate_daemon::{App, Identity, router};
use substrate_host::{
    DispatchOutcome, Driver, DriverError, ExecObservation, HostConfig, HostDriver,
    WorkspaceDestroyProgress,
};
use substrate_store::Store;
use substrate_wire::{
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, CapabilitySnapshot, Exec, ExecExit,
    ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState, FileAbsence, FileObservation,
    FileReadQuery, FileReadResult, LeaseObservation, OutputSlice, SandboxProfile, Workspace,
    WorkspaceAbsence, WorkspaceCreateInput,
};
use tempfile::TempDir;
use tower::ServiceExt as _;

const SUBJECT: &str = "local:1000";
const OTHER: &str = "local:1001";
const SNAPSHOT: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// Delegates the workspace half to the real host driver; runs an exec's argv in the resolved root.
struct AdoptFixtureDriver {
    host: Arc<HostDriver>,
    execs: Mutex<HashMap<String, ExecObservation>>,
    /// The root name every `start_exec` was handed, in order.
    started_in: Mutex<Vec<String>>,
}

#[async_trait]
impl Driver for AdoptFixtureDriver {
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
        workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.started_in
            .lock()
            .expect("fixture lock")
            .push(workspace_root_name.to_owned());
        let directory = match self.host.workspace_path(workspace_root_name) {
            Ok(value) => value,
            Err(error) => return DispatchOutcome::NotDispatched(error),
        };
        let Ok(output) = std::process::Command::new(&input.argv[0])
            .args(&input.argv[1..])
            .current_dir(&directory)
            .env_clear()
            .output()
        else {
            return DispatchOutcome::NotDispatched(DriverError::failed(
                "exec.start-failed",
                "fixture could not run the argv",
            ));
        };
        let observation = ExecObservation {
            resource: Exec {
                unrecorded_output: None,
                id: id.to_owned(),
                kind: substrate_wire::ExecKind::Exec,
                workspace: input.workspace.clone(),
                state: ExecState::Exited,
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
                exit: Some(ExecExit {
                    code: output
                        .status
                        .code()
                        .and_then(|code| u8::try_from(code).ok()),
                    signal: None,
                }),
                usage: None,
                lease: None,
                refusal: None,
            },
            stdout: output.stdout,
            stderr: output.stderr,
            stdout_truncated: false,
            stderr_truncated: false,
            output_complete: true,
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
    app: Arc<App>,
    driver: Arc<AdoptFixtureDriver>,
}

impl Harness {
    fn open() -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        std::fs::create_dir(directory.path().join("workspaces")).expect("workspace root");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = Arc::new(AdoptFixtureDriver {
            host: HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
                .expect("host driver"),
            execs: Mutex::new(HashMap::new()),
            started_in: Mutex::new(Vec::new()),
        });
        let app = App::new(store, Arc::clone(&driver) as Arc<dyn Driver>, "dep_adopt");
        Self {
            directory,
            app,
            driver,
        }
    }

    fn root(&self) -> std::path::PathBuf {
        self.directory.path().join("workspaces")
    }

    /// The operator's placement: a directory under the workspace root, outside the daemon.
    fn place(&self, name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let path = self.root().join(name);
        std::fs::create_dir(&path).expect("operator directory");
        for (file, content) in files {
            std::fs::write(path.join(file), content).expect("operator file");
        }
        path
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
            .header("x-request-id", "req_adopt_test")
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: subject.to_owned(),
            actor: "adopt-test".to_owned(),
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

    async fn adopt(&self, subject: &str, operation: &str, root_name: &str) -> (StatusCode, Value) {
        self.call(
            subject,
            Method::POST,
            "/v1/workspaces/adopt",
            mutation(operation, json!({ "root_name": root_name, "labels": {} })),
        )
        .await
    }

    async fn destroy(
        &self,
        subject: &str,
        operation: &str,
        workspace: &str,
    ) -> (StatusCode, Value) {
        self.call(
            subject,
            Method::DELETE,
            &format!("/v1/workspaces/{workspace}"),
            mutation(operation, json!({})),
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

fn exec_body(workspace: &str, argv: &[&str]) -> Value {
    json!({
        "workspace": workspace,
        "argv": argv,
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
async fn adopt_then_exec_runs_in_the_operator_directory() {
    // Prevents the half-done adoption STATUS.md recorded: a directory the operator placed under
    // the workspace root, without the `ws_` prefix, is served to an exec after one adopt.
    let harness = Harness::open();
    harness.place("adopted-repo", &[("marker.txt", "operator-owned\n")]);
    let (status, adopted) = harness
        .adopt(SUBJECT, "01JADOPTEXECJOURNEY0001", "adopted-repo")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{adopted}");
    let workspace = text(&adopted["result"]["id"]).to_owned();
    assert!(workspace.starts_with("ws_"), "{workspace}");
    assert_eq!(adopted["result"]["origin"], "adopted");
    assert_eq!(adopted["result"]["root_name"], "adopted-repo");
    assert_eq!(adopted["result"]["state"], "ready");
    assert!(adopted["result"].get("lease").is_none());
    assert!(adopted["result"].get("storage").is_none());

    let (status, observed) = harness
        .call(
            SUBJECT,
            Method::GET,
            &format!("/v1/workspaces/{workspace}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{observed}");
    assert_eq!(observed["result"]["origin"], "adopted");
    assert_eq!(observed["result"]["root_name"], "adopted-repo");

    let (status, started) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/execs",
            mutation(
                "01JADOPTEXECJOURNEY0002",
                exec_body(&workspace, &["/bin/sh", "-c", "cat marker.txt; exit 3"]),
            ),
        )
        .await;
    assert!(status.is_success(), "{status} {started}");
    let exec = text(&started["result"]["id"]).to_owned();
    assert_eq!(started["result"]["state"], "exited");
    assert_eq!(started["result"]["exit"]["code"], 3);
    assert_eq!(
        harness
            .driver
            .started_in
            .lock()
            .expect("fixture lock")
            .as_slice(),
        ["adopted-repo".to_owned()]
    );

    let (status, output) = harness
        .call(
            SUBJECT,
            Method::GET,
            &format!("/v1/execs/{exec}/output?stream=stdout&offset=0&limit_bytes=4096"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{output}");
    let chunk = output["result"].to_string();
    assert!(
        chunk.contains("operator-owned") || chunk.contains("b3BlcmF0b3Itb3duZWQK"),
        "{output}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn adopt_refuses_a_name_outside_the_set_before_dispatch_and_replays_it() {
    // Prevents an out-of-set name from reaching the host, and from escaping the ledger (ADR 0035
    // decision 6 amends ADR 0034's decision 4): the refusal is recorded and a retry replays it.
    let harness = Harness::open();
    harness.place("adopted-repo", &[]);
    let too_long = "a".repeat(256);
    for (index, name) in [
        "../escape",
        ".hidden",
        ".substrate-apertures",
        "-rf",
        "a/b",
        "",
        too_long.as_str(),
    ]
    .into_iter()
    .enumerate()
    {
        let operation = format!("01JADOPTOUTOFSET{index:04}X");
        let (status, refused) = harness.adopt(SUBJECT, &operation, name).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{name:?} {refused}"
        );
        assert_eq!(
            refused["error"]["code"], "request.schema-invalid",
            "{name:?}"
        );
        assert_eq!(refused["error"]["address"], "root_name", "{name:?}");
        let (replay_status, replayed) = harness.adopt(SUBJECT, &operation, name).await;
        assert_eq!(replay_status, status, "{name:?}");
        assert_eq!(replayed["error"], refused["error"], "{name:?}");
        let record = harness.operation(SUBJECT, &operation).await;
        assert_eq!(record["result"]["state"], "refused", "{name:?} {record}");
        assert_eq!(
            record["result"]["operation_kind"], "workspace.adopt",
            "{name:?}"
        );
    }
    assert!(
        harness
            .driver
            .started_in
            .lock()
            .expect("fixture lock")
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn adopt_refusals_follow_the_decided_order_and_are_recorded() {
    // Prevents adopt from becoming a cross-subject existence oracle, or from taking over a
    // directory another row already serves (ADR 0035 decision 5).
    let harness = Harness::open();
    harness.place("adopted-repo", &[]);
    std::fs::write(harness.root().join("plain-file"), "not a directory").expect("file");
    std::os::unix::fs::symlink(
        harness.root().join("adopted-repo"),
        harness.root().join("link"),
    )
    .expect("symlink");

    let (status, first) = harness
        .adopt(SUBJECT, "01JADOPTORDER000000001", "adopted-repo")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    let cases = [
        (
            SUBJECT,
            "01JADOPTORDER000000002",
            "adopted-repo",
            StatusCode::CONFLICT,
            "workspace.already-exists",
        ),
        (
            OTHER,
            "01JADOPTORDER000000003",
            "adopted-repo",
            StatusCode::NOT_FOUND,
            "resource.not-found",
        ),
        (
            OTHER,
            "01JADOPTORDER000000004",
            "never-placed",
            StatusCode::NOT_FOUND,
            "resource.not-found",
        ),
        (
            SUBJECT,
            "01JADOPTORDER000000005",
            "plain-file",
            StatusCode::UNPROCESSABLE_ENTITY,
            "workspace.path-escape",
        ),
        (
            SUBJECT,
            "01JADOPTORDER000000006",
            "link",
            StatusCode::UNPROCESSABLE_ENTITY,
            "workspace.path-escape",
        ),
    ];
    let mut not_found_bodies = Vec::new();
    for (subject, operation, name, expected_status, code) in cases {
        let (status, refused) = harness.adopt(subject, operation, name).await;
        assert_eq!(status, expected_status, "{name} {refused}");
        assert_eq!(refused["error"]["code"], code, "{name}");
        let (replay_status, replayed) = harness.adopt(subject, operation, name).await;
        assert_eq!(replay_status, status, "{name}");
        assert_eq!(replayed["error"], refused["error"], "{name}");
        let record = harness.operation(subject, operation).await;
        assert_eq!(record["result"]["state"], "refused", "{name} {record}");
        if code == "resource.not-found" {
            let mut error = refused["error"].clone();
            error.as_object_mut().expect("error").remove("operation");
            not_found_bodies.push(error);
        }
    }
    // Held by another subject and absent answer alike, apart from the operation id.
    assert_eq!(not_found_bodies[0], not_found_bodies[1]);

    // Another subject's minted directory cannot be adopted either.
    let (status, created) = harness
        .call(
            OTHER,
            Method::POST,
            "/v1/workspaces",
            mutation(
                "01JADOPTORDER000000007",
                json!({ "source": "empty", "labels": {} }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["result"]["origin"], "created");
    let minted = text(&created["result"]["root_name"]).to_owned();
    assert_eq!(minted, text(&created["result"]["id"]));
    let (status, refused) = harness
        .adopt(SUBJECT, "01JADOPTORDER000000008", &minted)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn destroy_of_an_adopted_workspace_forgets_it_and_leaves_the_files() {
    // Prevents data loss: destroy removes a created workspace's tree, but an adopted directory
    // is the operator's (ADR 0035 decision 7). The same directory can be adopted again after.
    let harness = Harness::open();
    let path = harness.place("adopted-repo", &[("keep.txt", "keep me")]);
    std::fs::create_dir(path.join("nested")).expect("nested");
    std::fs::write(path.join("nested/deep.txt"), "deep").expect("deep");
    let (status, adopted) = harness
        .adopt(SUBJECT, "01JADOPTDESTROY0000001", "adopted-repo")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{adopted}");
    let workspace = text(&adopted["result"]["id"]).to_owned();

    let (status, absent) = harness
        .destroy(SUBJECT, "01JADOPTDESTROY0000002", &workspace)
        .await;
    assert_eq!(status, StatusCode::OK, "{absent}");
    assert_eq!(absent["result"]["absent"], true);
    assert_eq!(
        std::fs::read_to_string(path.join("keep.txt")).expect("kept"),
        "keep me"
    );
    assert_eq!(
        std::fs::read_to_string(path.join("nested/deep.txt")).expect("kept"),
        "deep"
    );

    let (status, _) = harness
        .call(
            SUBJECT,
            Method::GET,
            &format!("/v1/workspaces/{workspace}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, again) = harness
        .adopt(SUBJECT, "01JADOPTDESTROY0000003", "adopted-repo")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{again}");
    assert_ne!(again["result"]["id"], adopted["result"]["id"]);

    // A created workspace's directory is still removed.
    let (status, created) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/workspaces",
            mutation(
                "01JADOPTDESTROY0000004",
                json!({ "source": "empty", "labels": {} }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let minted = text(&created["result"]["id"]).to_owned();
    assert!(harness.root().join(&minted).is_dir());
    let (status, _) = harness
        .destroy(SUBJECT, "01JADOPTDESTROY0000005", &minted)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!harness.root().join(&minted).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_cleanup_of_an_adopted_workspace_forgets_it_and_leaves_the_files() {
    // Prevents the restart path from deleting what the synchronous path would have kept: a
    // destroy accepted before a crash completes, on restart, as a forget.
    let harness = Harness::open();
    let path = harness.place("adopted-repo", &[("keep.txt", "keep me")]);
    let (status, adopted) = harness
        .adopt(SUBJECT, "01JADOPTRESTART0000001", "adopted-repo")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{adopted}");
    let workspace = text(&adopted["result"]["id"]).to_owned();

    // The crash window: the destroy is durable (row Destroying, cleanup entry written), and the
    // driver never ran.
    let store = Store::open(harness.directory.path().join("state.db")).expect("second handle");
    let scope = substrate_store::Scope {
        deployment: "dep_adopt".to_owned(),
        subject: SUBJECT.to_owned(),
    };
    let destroy = substrate_store::NewOperation {
        scope: scope.clone(),
        operation: "01JADOPTRESTART0000002".to_owned(),
        operation_kind: "workspace.destroy".to_owned(),
        request_hash: "e".repeat(64),
        accepted_at: Utc::now().to_rfc3339(),
        capability_snapshot: None,
        actor: "adopt-test".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(workspace.clone()),
    };
    assert!(matches!(
        store
            .reserve_workspace_destroy(&destroy, None)
            .expect("reserve"),
        substrate_store::WorkspaceDestroyReservation::Admitted { .. }
    ));
    drop(store);

    harness.app.reconcile_destroying_workspaces().await;

    assert_eq!(
        std::fs::read_to_string(path.join("keep.txt")).expect("kept"),
        "keep me"
    );
    let record = harness.operation(SUBJECT, "01JADOPTRESTART0000002").await;
    assert_eq!(record["result"]["state"], "terminal", "{record}");
    let (status, _) = harness
        .call(
            SUBJECT,
            Method::GET,
            &format!("/v1/workspaces/{workspace}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
