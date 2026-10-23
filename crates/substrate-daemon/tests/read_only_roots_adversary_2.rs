//! Adversary pass 2 for the daemon's read-only-root start checks (ADR 0010, bundle `0.23.0`).
//!
//! Correction round 1 added `check_read_only_roots` to `validate_exec_input` and published three
//! lexical refusal vectors plus a compatibility text saying a broken root rule is "checked before
//! any capability". Nothing runs those vectors against the daemon (`executable-vectors.json` names
//! none of them), so these cases do: the fixture below is the one from
//! `read_only_roots_fact_security.rs`, publishing the confinement floor and toggling only
//! `exec.read-only-roots`, and counting every start that reaches it.
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
            "dep_rorootadv2",
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
            .header("x-request-id", "req_roroot_adv2")
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: SUBJECT.to_owned(),
            actor: "read-only-roots-adversary-2".to_owned(),
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

    async fn start(&self, operation: &str, input: Value) -> (StatusCode, Value) {
        self.call(Method::POST, "/v1/execs", mutation(operation, input))
            .await
    }
}

#[allow(clippy::needless_pass_by_value)]
fn mutation(operation: &str, input: Value) -> Body {
    Body::from(serde_json::to_vec(&json!({ "op": operation, "input": input })).expect("JSON"))
}

fn base_input(workspace: &str) -> Value {
    json!({
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
        "wait": false
    })
}

fn five_roots() -> Value {
    Value::Array(
        (1..=5)
            .map(|i| json!({ "host_path": format!("/srv/r{i}"), "mount": format!("/opt/r{i}") }))
            .collect(),
    )
}

fn bundle_vector(name: &str) -> Value {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/substrate-wire/0.23.0/vectors/http")
        .join(name);
    serde_json::from_slice(&std::fs::read(&path).expect("vector bytes")).expect("vector JSON")
}

/// Control: with the fact published and no other member, five roots are the lexical refusal.
#[tokio::test(flavor = "multi_thread")]
async fn control_five_roots_alone_are_read_only_root_invalid() {
    let harness = Harness::open(true);
    let workspace = harness.workspace("01JROROOTADVTWOCONTROL01").await;
    let mut input = base_input(&workspace);
    input["read_only_roots"] = five_roots();
    let (status, refused) = harness.start("01JROROOTADVTWOCONTROL02", input).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(
        refused["error"]["code"], "exec.read-only-root-invalid",
        "{refused}"
    );
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 0);
}

/// `compatibility.json` (0.23.0) states the order the daemon follows: the start's other confinement
/// members first, each its rule and then its capability, then the root list's lexical rule, then
/// the roots capability. The fixture publishes the roots fact and the floor, and not
/// `exec.read-only-files`. A start naming five roots and one admissible read-only file breaks a
/// root's lexical rule and needs an unpublished capability; by that order it is 501
/// `exec.read-only-files-unserved`.
///
/// Corrected in u3 correction round 2: this case first pinned 422 `exec.read-only-root-invalid`,
/// from the round-1 text "checked before any capability". The coordinator chose to keep the
/// daemon's order and corrected the published text instead; this assertion now pins that order.
#[tokio::test(flavor = "multi_thread")]
async fn an_unserved_sibling_capability_is_answered_before_a_broken_root_rule() {
    let harness = Harness::open(true);
    let workspace = harness.workspace("01JROROOTADVTWOORDER0001").await;
    let mut input = base_input(&workspace);
    input["read_only_roots"] = five_roots();
    input["read_only_files"] =
        json!([{ "host_path": "/etc/gitconfig", "mount": "/etc/gitconfig" }]);
    let (status, refused) = harness.start("01JROROOTADVTWOORDER0002", input).await;
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 0);
    assert_eq!(
        (status, refused["error"]["code"].clone()),
        (
            StatusCode::NOT_IMPLEMENTED,
            json!("exec.read-only-files-unserved")
        ),
        "the bundle says the other members' checks come before the root list's; the daemon \
         answered {refused}"
    );
}

/// The three lexical vectors of 0.23.0, replayed against the daemon with the fact published: the
/// status and the whole error body, apart from the per-call operation and request id, must be the
/// daemon's own answer.
#[tokio::test(flavor = "multi_thread")]
async fn each_lexical_refusal_vector_is_the_daemons_answer() {
    let harness = Harness::open(true);
    let workspace = harness.workspace("01JROROOTADVTWOVECTOR001").await;
    let mut mismatched = Vec::new();
    for (index, name) in [
        "read-only-roots-five-is-refused.json",
        "read-only-root-path-escape-is-refused.json",
        "read-only-root-mount-collision-is-refused.json",
    ]
    .into_iter()
    .enumerate()
    {
        let vector = bundle_vector(name);
        let mut input = base_input(&workspace);
        input["read_only_roots"] =
            vector["action"]["request"]["body"]["input"]["read_only_roots"].clone();
        let (status, answer) = harness
            .start(&format!("01JROROOTADVTWOVECTOR10{index}"), input)
            .await;
        let expected = &vector["expected"]["response"];
        let mut published = expected["body"]["error"].clone();
        let mut answered = answer["error"].clone();
        for body in [&mut published, &mut answered] {
            if let Some(object) = body.as_object_mut() {
                object.remove("operation");
            }
        }
        if expected["status"] != json!(status.as_u16()) || published != answered {
            mismatched.push(format!(
                "{name}: published {} {published}, daemon {status} {answered}",
                expected["status"]
            ));
        }
    }
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 0);
    assert!(mismatched.is_empty(), "{}", mismatched.join("\n"));
}
