//! Adversary cases for `workspace.adopt` (ADR 0035), driven through the router.
//!
//! Each case asserts the ADR directly: adopt is no cross-subject existence oracle (decision 5),
//! every refusal and driver failure is a ledger row a retry replays (decision 6), another subject
//! cannot reach an adopted workspace, and no adopt is admitted without the host observation of a
//! directory the decided refusal order puts before the store write.
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use substrate_daemon::{App, Authority, Identity, SystemAuthority, router};
use substrate_host::{
    DispatchOutcome, Driver, DriverError, ExecObservation, HostConfig, HostDriver,
    WorkspaceDestroyProgress,
};
use substrate_store::{LeaseClock, NewOperation, Scope, Store, WorkspaceDestroyReservation};
use substrate_wire::{
    CapabilitySnapshot, ExecOutputQuery, ExecSignalInput, ExecStartInput, FileAbsence,
    FileObservation, FileReadQuery, FileReadResult, LeaseObservation, OutputSlice, Workspace,
    WorkspaceAbsence, WorkspaceCreateInput, WorkspaceKind,
};
use tempfile::TempDir;
use tower::ServiceExt as _;

const SUBJECT: &str = "local:1000";
const OTHER: &str = "local:1001";
const DEPLOYMENT: &str = "dep_adopt_adversary";
const SNAPSHOT: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

/// The real host driver for every workspace call, an exec start that only records that it was
/// reached, and an observation fault the test can switch on for one root name.
struct ProbeDriver {
    host: Arc<HostDriver>,
    started_in: Mutex<Vec<String>>,
    fail_observe_for: Mutex<Option<String>>,
}

#[async_trait]
impl Driver for ProbeDriver {
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
        if self.fail_observe_for.lock().expect("probe lock").as_deref() == Some(root_name) {
            return Err(DriverError::failed(
                "workspace.io-failed",
                "probe: the host could not observe the directory",
            ));
        }
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
        _id: &str,
        workspace_root_name: &str,
        _input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.started_in
            .lock()
            .expect("probe lock")
            .push(workspace_root_name.to_owned());
        DispatchOutcome::NotDispatched(DriverError::failed(
            "exec.start-failed",
            "probe: exec start is only recorded",
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

type RaceAction = Box<dyn FnOnce() + Send>;

/// The system authority, plus one armed action: after the next `workspace_id()` (the adopt
/// handler mints its id), the action runs inside the `fire_at`-th `now()` call that follows. It
/// stands in for another request committing between two steps of one adopt.
struct RaceAuthority {
    system: SystemAuthority,
    counting: AtomicBool,
    seen: AtomicUsize,
    fire_at: AtomicUsize,
    action: Mutex<Option<RaceAction>>,
    fired: AtomicBool,
}

impl RaceAuthority {
    fn new() -> Self {
        Self {
            system: SystemAuthority,
            counting: AtomicBool::new(false),
            seen: AtomicUsize::new(0),
            fire_at: AtomicUsize::new(0),
            action: Mutex::new(None),
            fired: AtomicBool::new(false),
        }
    }

    fn arm(&self, fire_at: usize, action: RaceAction) {
        self.fire_at.store(fire_at, Ordering::SeqCst);
        self.fired.store(false, Ordering::SeqCst);
        *self.action.lock().expect("race lock") = Some(action);
    }
}

impl Authority for RaceAuthority {
    fn now(&self) -> DateTime<Utc> {
        if self.counting.load(Ordering::SeqCst) {
            let seen = self.seen.fetch_add(1, Ordering::SeqCst) + 1;
            if seen == self.fire_at.load(Ordering::SeqCst) {
                self.counting.store(false, Ordering::SeqCst);
                let action = self.action.lock().expect("race lock").take();
                if let Some(action) = action {
                    action();
                    self.fired.store(true, Ordering::SeqCst);
                }
            }
        }
        self.system.now()
    }

    fn request_id(&self) -> String {
        self.system.request_id()
    }

    fn workspace_id(&self) -> String {
        if self.action.lock().expect("race lock").is_some() {
            self.seen.store(0, Ordering::SeqCst);
            self.counting.store(true, Ordering::SeqCst);
        }
        self.system.workspace_id()
    }

    fn exec_id(&self) -> String {
        self.system.exec_id()
    }

    fn session_id(&self) -> String {
        self.system.session_id()
    }

    fn snapshot_id(&self) -> String {
        self.system.snapshot_id()
    }

    fn lease_clock(&self) -> Result<LeaseClock, String> {
        self.system.lease_clock()
    }
}

struct Harness {
    directory: TempDir,
    app: Arc<App>,
    driver: Arc<ProbeDriver>,
    authority: Arc<RaceAuthority>,
}

impl Harness {
    fn open() -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        std::fs::create_dir(directory.path().join("workspaces")).expect("workspace root");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = Arc::new(ProbeDriver {
            host: HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
                .expect("host driver"),
            started_in: Mutex::new(Vec::new()),
            fail_observe_for: Mutex::new(None),
        });
        let authority = Arc::new(RaceAuthority::new());
        let app = App::with_authority(
            store,
            Arc::clone(&driver) as Arc<dyn Driver>,
            DEPLOYMENT,
            Arc::clone(&authority) as Arc<dyn Authority>,
        );
        Self {
            directory,
            app,
            driver,
            authority,
        }
    }

    fn root(&self) -> std::path::PathBuf {
        self.directory.path().join("workspaces")
    }

    fn state(&self) -> std::path::PathBuf {
        self.directory.path().join("state.db")
    }

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
            .header("x-request-id", "req_adopt_adversary")
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: subject.to_owned(),
            actor: "adopt-adversary".to_owned(),
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
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("JSON response")
        };
        (status, value)
    }

    async fn adopt_value(
        &self,
        subject: &str,
        operation: &str,
        root_name: Value,
    ) -> (StatusCode, Value) {
        self.call(
            subject,
            Method::POST,
            "/v1/workspaces/adopt",
            mutation(operation, json!({ "root_name": root_name, "labels": {} })),
        )
        .await
    }

    async fn adopt(&self, subject: &str, operation: &str, root_name: &str) -> (StatusCode, Value) {
        self.adopt_value(subject, operation, json!(root_name)).await
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

/// An operation record with every member that legitimately differs between two requests removed:
/// the ids, the request hash and the times.
fn comparable_record(record: &Value) -> Value {
    let mut result = record["result"].clone();
    let object = result.as_object_mut().expect("operation record");
    for member in ["operation", "request_hash", "accepted_at", "terminal_at"] {
        object.remove(member);
    }
    if let Some(error) = object
        .get_mut("outcome")
        .and_then(|outcome| outcome.get_mut("error"))
        .and_then(Value::as_object_mut)
    {
        error.remove("operation");
    }
    result
}

fn exec_body(workspace: &str) -> Value {
    json!({
        "workspace": workspace,
        "argv": ["/bin/true"],
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
async fn adopt_ledger_record_is_no_cross_subject_existence_oracle() {
    // ADR 0035 decision 5: "Another subject's row answers exactly as an absent directory does, so
    // adopt is no cross-subject existence oracle." The error body is one channel; the caller's own
    // operation record, read back through `GET /v1/ops/{id}`, is another. Both refusals must leave
    // the same record apart from ids and times.
    let harness = Harness::open();
    harness.place("held-elsewhere", &[("secret.txt", "s")]);
    let (status, adopted) = harness
        .adopt(SUBJECT, "01JADVORACLE000000001", "held-elsewhere")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{adopted}");

    let (held_status, held) = harness
        .adopt(OTHER, "01JADVORACLE000000002", "held-elsewhere")
        .await;
    let (absent_status, absent) = harness
        .adopt(OTHER, "01JADVORACLE000000003", "never-placed")
        .await;
    assert_eq!(held_status, StatusCode::NOT_FOUND, "{held}");
    assert_eq!(absent_status, StatusCode::NOT_FOUND, "{absent}");

    let held_record = harness.operation(OTHER, "01JADVORACLE000000002").await;
    let absent_record = harness.operation(OTHER, "01JADVORACLE000000003").await;
    assert_eq!(
        held_record["result"]["resource"], absent_record["result"]["resource"],
        "the held-name refusal names a resource the absent-directory refusal does not: \
         held={held_record} absent={absent_record}"
    );
    assert_eq!(
        comparable_record(&held_record),
        comparable_record(&absent_record),
        "held={held_record} absent={absent_record}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One scenario, asserted end to end.
async fn adopt_never_admits_a_name_freed_after_its_holder_was_read_without_observing_it() {
    // ADR 0035 decision 5 puts the host observation (absent: 404; not a directory: 422) before the
    // store write for every adopt that is admitted. The handler skips the observation when the
    // name is held and passes its unobserved provisional resource to the reservation, which writes
    // a Ready row whenever the in-transaction holder check finds the name free. A destroy (a
    // forget) committed between the two checks, with the operator removing the directory, must
    // still answer `resource.not-found`, never 201 for a directory nothing observed.
    //
    // The race is driven deterministically: the action runs inside the n-th clock read after the
    // adopt mints its id, for every n up to six, so the case does not depend on which read sits
    // between the holder read and the reservation.
    for fire_at in 1..=6 {
        let harness = Harness::open();
        let path = harness.place("contested", &[("keep.txt", "keep")]);
        let (status, adopted) = harness
            .adopt(
                SUBJECT,
                &format!("01JADVRACE{fire_at:02}000000001"),
                "contested",
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{adopted}");
        let workspace = text(&adopted["result"]["id"]).to_owned();

        let second = Store::open(harness.state()).expect("second store handle");
        let destroy_operation = format!("01JADVRACE{fire_at:02}000000002");
        let action_workspace = workspace.clone();
        let action_path = path.clone();
        harness.authority.arm(
            fire_at,
            Box::new(move || {
                let scope = Scope {
                    deployment: DEPLOYMENT.to_owned(),
                    subject: SUBJECT.to_owned(),
                };
                let destroy = NewOperation {
                    scope: scope.clone(),
                    operation: destroy_operation.clone(),
                    operation_kind: "workspace.destroy".to_owned(),
                    request_hash: "e".repeat(64),
                    accepted_at: Utc::now().to_rfc3339(),
                    capability_snapshot: None,
                    actor: "adopt-adversary".to_owned(),
                    principal: None,
                    grant_ref: None,
                    platform_principal: None,
                    resource: Some(action_workspace.clone()),
                };
                assert!(matches!(
                    second
                        .reserve_workspace_destroy(&destroy, None)
                        .expect("reserve destroy"),
                    WorkspaceDestroyReservation::Admitted { .. }
                ));
                let absence = WorkspaceAbsence {
                    kind: WorkspaceKind::Workspace,
                    id: action_workspace.clone(),
                    absent: true,
                    observed_at: Utc::now(),
                };
                second
                    .complete_workspace_absence(
                        &scope,
                        &destroy_operation,
                        &Utc::now().to_rfc3339(),
                        200,
                        &action_workspace,
                        &absence,
                    )
                    .expect("forget committed");
                std::fs::remove_dir_all(&action_path).expect("operator removes the directory");
            }),
        );

        let (status, raced) = harness
            .adopt(
                OTHER,
                &format!("01JADVRACE{fire_at:02}000000003"),
                "contested",
            )
            .await;
        let fired = harness.authority.fired.load(Ordering::SeqCst);
        if fire_at <= 3 {
            assert!(
                fired,
                "fire_at={fire_at}: the adopt read the clock fewer times than expected"
            );
        }
        assert!(
            !path.exists() || !fired,
            "fire_at={fire_at}: the action did not remove the directory"
        );
        assert_ne!(
            status,
            StatusCode::CREATED,
            "fire_at={fire_at}: an adopt was admitted for a directory that no longer exists and \
             was never observed: {raced}"
        );
        assert_eq!(status, StatusCode::NOT_FOUND, "fire_at={fire_at}: {raced}");
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One scenario, asserted end to end.
async fn another_subject_cannot_reach_an_adopted_workspace_by_any_route() {
    // Rows are subject-scoped (ADR 0035, Consequences): another allowed uid's get, file read,
    // write, delete, exec start and destroy of an adopted workspace all answer not-found, the
    // operator's files are untouched, and the owner still reaches it afterwards.
    let harness = Harness::open();
    let path = harness.place("owned-by-subject", &[("keep.txt", "keep me")]);
    let (status, adopted) = harness
        .adopt(SUBJECT, "01JADVCROSS0000000001", "owned-by-subject")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{adopted}");
    let workspace = text(&adopted["result"]["id"]).to_owned();

    let (status, body) = harness
        .call(
            OTHER,
            Method::GET,
            &format!("/v1/workspaces/{workspace}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "get {body}");

    let (status, body) = harness
        .call(
            OTHER,
            Method::GET,
            &format!("/v1/workspaces/{workspace}/files/keep.txt?mode=file&offset=0&limit_bytes=16"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "read {body}");

    let (status, body) = harness
        .call(
            OTHER,
            Method::PUT,
            &format!("/v1/workspaces/{workspace}/files/keep.txt"),
            mutation(
                "01JADVCROSS0000000002",
                json!({ "content": { "encoding": "base64", "data": "b3duZWQ=" } }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "write {body}");

    let (status, body) = harness
        .call(
            OTHER,
            Method::DELETE,
            &format!("/v1/workspaces/{workspace}/files/keep.txt"),
            mutation("01JADVCROSS0000000003", json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "delete file {body}");

    let (status, body) = harness
        .call(
            OTHER,
            Method::POST,
            "/v1/execs",
            mutation("01JADVCROSS0000000004", exec_body(&workspace)),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "exec {body}");
    assert!(
        harness
            .driver
            .started_in
            .lock()
            .expect("probe lock")
            .is_empty(),
        "another subject's exec reached the driver"
    );

    let (status, body) = harness
        .call(
            OTHER,
            Method::DELETE,
            &format!("/v1/workspaces/{workspace}"),
            mutation("01JADVCROSS0000000005", json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "destroy {body}");

    assert_eq!(
        std::fs::read_to_string(path.join("keep.txt")).expect("kept"),
        "keep me"
    );
    let (status, body) = harness
        .call(
            SUBJECT,
            Method::GET,
            &format!("/v1/workspaces/{workspace}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "owner get {body}");
    assert_eq!(body["result"]["origin"], "adopted");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_driver_failure_during_adopt_is_recorded_replayed_and_leaves_no_row() {
    // ADR 0035 decision 6: every adopt answer before the row is written is a ledger row a retry
    // with the same operation id replays. A host failure observing the directory is one: it is
    // recorded, the same operation id replays it after the fault clears, it holds no root name,
    // and a fresh operation id then adopts the directory.
    let harness = Harness::open();
    let path = harness.place("flaky", &[("keep.txt", "keep")]);
    *harness.driver.fail_observe_for.lock().expect("probe lock") = Some("flaky".to_owned());

    let (status, failed) = harness
        .adopt(SUBJECT, "01JADVFAULT0000000001", "flaky")
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{failed}");
    assert_eq!(failed["error"]["code"], "workspace.io-failed");
    let record = harness.operation(SUBJECT, "01JADVFAULT0000000001").await;
    assert_eq!(record["result"]["kind"], "workspace.adopt", "{record}");
    assert_eq!(
        record["result"]["outcome"]["error"]["code"], "workspace.io-failed",
        "{record}"
    );

    *harness.driver.fail_observe_for.lock().expect("probe lock") = None;
    let (replay_status, replayed) = harness
        .adopt(SUBJECT, "01JADVFAULT0000000001", "flaky")
        .await;
    assert_eq!(replay_status, status, "{replayed}");
    assert_eq!(replayed["error"], failed["error"]);

    let (status, adopted) = harness
        .adopt(SUBJECT, "01JADVFAULT0000000002", "flaky")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{adopted}");
    assert_eq!(adopted["result"]["root_name"], "flaky");
    assert_eq!(
        std::fs::read_to_string(path.join("keep.txt")).expect("kept"),
        "keep"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn adopt_name_boundaries_are_refused_or_admitted_exactly_at_the_set_edge() {
    // ADR 0035 decision 1: 1 to 255 bytes of ASCII alphanumerics, `_` and `-`, no leading `-`.
    // Each out-of-set name is a recorded `request.schema-invalid` at `root_name`; the longest
    // member reaches the host (404 absent, then 201 once placed).
    let harness = Harness::open();
    for (index, name) in [
        ".",
        "..",
        "/",
        "a\u{0}b",
        " ",
        "a.b",
        "ws_x/..",
        "\u{e9}t\u{e9}",
    ]
    .into_iter()
    .enumerate()
    {
        let operation = format!("01JADVBOUND{index:04}000001");
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
        let record = harness.operation(SUBJECT, &operation).await;
        assert_eq!(record["result"]["state"], "refused", "{name:?} {record}");
    }

    let longest = "a".repeat(substrate_wire::MAX_ROOT_NAME_BYTES);
    let (status, absent) = harness
        .adopt(SUBJECT, "01JADVBOUNDLONG0000001", &longest)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{absent}");
    assert_eq!(absent["error"]["code"], "resource.not-found");
    std::fs::create_dir(harness.root().join(&longest)).expect("NAME_MAX directory");
    let (status, adopted) = harness
        .adopt(SUBJECT, "01JADVBOUNDLONG0000002", &longest)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{adopted}");
    assert_eq!(adopted["result"]["root_name"], longest.as_str());
}

#[tokio::test(flavor = "multi_thread")]
async fn adopting_the_callers_own_created_workspace_root_is_refused_and_keeps_its_origin() {
    // ADR 0035 decision 5: the caller's own row holding the root name is
    // `workspace.already-exists`, whatever origin that row has. Adoption cannot re-label a
    // created workspace as adopted, and the created workspace keeps being destroyed by removal.
    let harness = Harness::open();
    let (status, created) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/workspaces",
            mutation(
                "01JADVOWNROOT00000001",
                json!({ "source": "empty", "labels": {} }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let minted = text(&created["result"]["id"]).to_owned();
    let root_name = text(&created["result"]["root_name"]).to_owned();

    let (status, refused) = harness
        .adopt(SUBJECT, "01JADVOWNROOT00000002", &root_name)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"]["code"], "workspace.already-exists");

    let (status, observed) = harness
        .call(
            SUBJECT,
            Method::GET,
            &format!("/v1/workspaces/{minted}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{observed}");
    assert_eq!(observed["result"]["origin"], "created");
    assert!(harness.root().join(&root_name).is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_adopts_of_one_name_admit_exactly_one_row() {
    // The reservation re-checks the holder inside its transaction: two adopts of one free name,
    // by two subjects at once, admit one row and answer the other not-found (or, for a same-subject
    // pair, already-exists). Never two rows for one root name.
    for round in 0..8 {
        let harness = Harness::open();
        harness.place("contested", &[]);
        let first_operation = format!("01JADVCONC{round:02}0000000001");
        let second_operation = format!("01JADVCONC{round:02}0000000002");
        let (first, second) = tokio::join!(
            harness.adopt(SUBJECT, &first_operation, "contested"),
            harness.adopt(OTHER, &second_operation, "contested"),
        );
        let statuses = [first.0, second.0];
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CREATED)
                .count(),
            1,
            "round {round}: {first:?} {second:?}"
        );
        assert!(
            statuses.contains(&StatusCode::NOT_FOUND),
            "round {round}: {first:?} {second:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn destroy_of_an_adopted_workspace_removes_only_its_own_baseline() {
    // ADR 0035 decision 7: forgetting removes what the daemon made for that workspace outside the
    // directory, such as `.substrate-git-baselines/<root_name>`, and nothing else: not the
    // operator's directory, not a sibling directory, and not another root name's baseline.
    let harness = Harness::open();
    let path = harness.place("forgotten", &[("keep.txt", "keep")]);
    let sibling = harness.place("sibling", &[("keep.txt", "sibling")]);
    let baselines = harness.root().join(".substrate-git-baselines");
    std::fs::create_dir_all(&baselines).expect("baseline root");
    std::fs::write(baselines.join("forgotten"), "0".repeat(40)).expect("own baseline");
    std::fs::write(baselines.join("sibling"), "1".repeat(40)).expect("sibling baseline");

    let (status, adopted) = harness
        .adopt(SUBJECT, "01JADVBASELINE0000001", "forgotten")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{adopted}");
    let workspace = text(&adopted["result"]["id"]).to_owned();
    let (status, absent) = harness
        .call(
            SUBJECT,
            Method::DELETE,
            &format!("/v1/workspaces/{workspace}"),
            mutation("01JADVBASELINE0000002", json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{absent}");
    assert_eq!(
        std::fs::read_to_string(path.join("keep.txt")).expect("kept"),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(sibling.join("keep.txt")).expect("sibling kept"),
        "sibling"
    );
    assert!(!baselines.join("forgotten").exists());
    assert_eq!(
        std::fs::read_to_string(baselines.join("sibling")).expect("sibling baseline kept"),
        "1".repeat(40)
    );
}
