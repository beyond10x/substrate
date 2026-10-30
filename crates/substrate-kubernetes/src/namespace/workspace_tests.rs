//! The workspace path on the fake API server (design 34 § 12 U8): each § 5 row for workspaces and
//! files, a crash between the PVC and the keeper, and the N9 bound. `pods/exec` runs the real
//! executor's `fs` role against a temporary directory as the PVC. Every case runs once; time moves
//! only when a case advances the clock, except the two bounded waits it names.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use substrate_executor::Handshake;
use substrate_executor::message::{PROTOCOL_FAILURE, Request};
use substrate_executor::serve::{open_workspace, serve};
use substrate_host::{
    BackendKind, BackendLedger, BackendRole, DispatchOutcome, Driver, DriverErrorClass,
    WorkspaceDestroyProgress,
};
use substrate_kubernetes_client::Method;
use substrate_kubernetes_client::fake::{ExecAnswer, ExecRequest, FakeCluster, Fault};
use substrate_wire::{
    FileEditInput, FilePatchInput, FileReadQuery, FileReadResult, FileReplaceInput, Workspace,
    WorkspaceCreateInput, WorkspaceTreeQuery,
};

use super::NamespaceDriver;
use crate::driver_tests::{DEPLOYMENT, ManualClock, MemoryLedger, admin, driver, passing_fake};
use crate::names::{self, DEPLOYMENT_LABEL, MANAGED_BY, MANAGED_BY_LABEL, OPERATION_ANNOTATION};
use crate::policy::hex;
use crate::{API_FAILED, EXECUTOR_UNANSWERED, WORKSPACE_CAPACITY};

const WORKSPACE: &str = "ws_one";
const PVC_DIGEST: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const KEEPER_DIGEST: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const EXEC_DIGEST: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const CLAIMS: &str = "/api/v1/namespaces/substrate/persistentvolumeclaims";
const PODS: &str = "/api/v1/namespaces/substrate/pods";

/// The executor behind the fake's `pods/exec`: the real `fs` role on `root`, and the real
/// handshake line unless a case states another.
struct Executor {
    root: PathBuf,
    calls: Mutex<Vec<ExecRequest>>,
    handshake: Mutex<Option<String>>,
    /// The next `fs` call is served and its answer then held past the client's bound: an answer
    /// lost after the executor acted.
    stall_next: AtomicBool,
    stall: Mutex<Duration>,
}

impl Executor {
    fn answer(&self, request: &ExecRequest) -> ExecAnswer {
        lock(&self.calls).push(request.clone());
        let command: Vec<&str> = request.command.iter().map(String::as_str).collect();
        match command.as_slice() {
            ["/substrate-executor", "version"] => {
                let line = lock(&self.handshake)
                    .clone()
                    .unwrap_or_else(|| Handshake::current().line());
                ExecAnswer {
                    stdout: format!("{line}\n").into_bytes(),
                    ..ExecAnswer::default()
                }
            }
            ["/substrate-executor", "fs"] => {
                let mut stdout = Vec::new();
                serve(&mut request.stdin.as_slice(), &mut stdout, || {
                    open_workspace(&self.root)
                })
                .expect("the executor writes its answer");
                if self.stall_next.swap(false, Ordering::SeqCst) {
                    std::thread::sleep(*lock(&self.stall));
                }
                ExecAnswer {
                    stdout,
                    ..ExecAnswer::default()
                }
            }
            _ => ExecAnswer {
                exit_code: 2,
                ..ExecAnswer::default()
            },
        }
    }

    /// The `fs` calls so far, each as its request header's `op`.
    fn fs_ops(&self) -> Vec<String> {
        lock(&self.calls)
            .iter()
            .filter(|call| call.command.last().is_some_and(|part| part == "fs"))
            .map(|call| {
                let request: Request =
                    substrate_executor::message::read_request(&mut call.stdin.as_slice())
                        .expect("the driver sends one framed request")
                        .0;
                serde_json::to_value(&request).expect("a request serializes")["op"]
                    .as_str()
                    .expect("an op")
                    .to_owned()
            })
            .collect()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Setup {
    fake: FakeCluster,
    clock: Arc<ManualClock>,
    driver: NamespaceDriver,
    ledger: Arc<MemoryLedger>,
    executor: Arc<Executor>,
    root: tempfile::TempDir,
}

fn pvc_name() -> String {
    names::workspace_pvc(DEPLOYMENT, WORKSPACE)
}

fn keeper_name() -> String {
    names::keeper_pod(DEPLOYMENT, WORKSPACE)
}

impl Setup {
    fn file(&self, path: &str) -> PathBuf {
        self.root.path().join(path)
    }

    fn deletes(&self) -> Vec<String> {
        self.fake
            .requests()
            .into_iter()
            .filter(|request| request.starts_with("DELETE "))
            .collect()
    }

    fn count(&self, request: &str) -> usize {
        self.fake
            .requests()
            .iter()
            .filter(|seen| seen.as_str() == request)
            .count()
    }

    async fn create(&self) -> DispatchOutcome<Workspace> {
        Driver::create_workspace(&self.driver, WORKSPACE, &pvc_name(), &empty()).await
    }

    async fn created(&self) -> Workspace {
        match self.create().await {
            DispatchOutcome::Observed(workspace) => workspace,
            other => panic!("the create is not observed: {}", outcome(&other)),
        }
    }

    async fn observe(&self) -> Result<Workspace, substrate_host::DriverError> {
        Driver::observe_workspace(&self.driver, WORKSPACE, &pvc_name(), &provisional()).await
    }
}

/// A passing fake, a driver on an injected clock, the workspace's two rows reserved as the daemon
/// reserves them, and the executor installed behind `pods/exec`.
fn setup() -> Setup {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, ledger) = driver(&fake, &clock);
    for name in names::backend_names(DEPLOYMENT, BackendKind::Workspace, WORKSPACE) {
        let digest = match name.role {
            BackendRole::Pvc => PVC_DIGEST,
            _ => KEEPER_DIGEST,
        };
        ledger.reserve(
            BackendKind::Workspace,
            WORKSPACE,
            name.role,
            &name.name,
            digest,
        );
    }
    let root = tempfile::tempdir().expect("a temporary workspace");
    let executor = Arc::new(Executor {
        root: root.path().to_path_buf(),
        calls: Mutex::new(Vec::new()),
        handshake: Mutex::new(None),
        stall_next: AtomicBool::new(false),
        stall: Mutex::new(Duration::ZERO),
    });
    let handler = Arc::clone(&executor);
    fake.set_exec(move |request| handler.answer(&request));
    Setup {
        fake,
        clock,
        driver,
        ledger,
        executor,
        root,
    }
}

fn empty() -> WorkspaceCreateInput {
    serde_json::from_value(json!({"source": "empty", "labels": {"team": "a"}}))
        .expect("an empty-source create")
}

fn provisional() -> Workspace {
    serde_json::from_value(json!({
        "id": WORKSPACE, "kind": "workspace", "labels": {"team": "a"},
        "observed_at": "2026-10-29T10:00:00Z", "state": "unknown", "origin": "created",
        "root_name": names::workspace_pvc(DEPLOYMENT, WORKSPACE),
    }))
    .expect("a provisional workspace")
}

fn outcome<T: std::fmt::Debug>(outcome: &DispatchOutcome<T>) -> String {
    match outcome {
        DispatchOutcome::Observed(value) => format!("Observed({value:?})"),
        DispatchOutcome::NotDispatched(error) => format!("NotDispatched({error:?})"),
        DispatchOutcome::ContainedAbsent(error) => format!("ContainedAbsent({error:?})"),
        DispatchOutcome::OutcomeUnknown(error) => format!("OutcomeUnknown({error:?})"),
    }
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn file_query() -> FileReadQuery {
    serde_json::from_value(json!({"mode": "file", "offset": 0, "limit_bytes": 65_536}))
        .expect("a file query")
}

/// The `Status` the API server answers for a create over the namespace's `ResourceQuota`, in the
/// shape O7 observed on the lane.
fn quota_refusal(resource: &str, name: &str, requested: &str) -> Value {
    json!({
        "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
        "message": format!(
            "{resource} \"{name}\" is forbidden: exceeded quota: substrate-lane, requested: \
             {requested}=1, used: {requested}=4, limited: {requested}=4"
        ),
        "reason": "Forbidden", "details": {"name": name, "kind": resource}, "code": 403,
    })
}

#[tokio::test]
async fn a_create_binds_the_pvc_then_the_keeper_and_is_ready_after_the_handshake() {
    let s = setup();
    let workspace = s.created().await;
    assert_eq!(workspace.state, substrate_wire::WorkspaceState::Ready);
    assert_eq!(workspace.root_name.as_str(), pvc_name());
    assert_eq!(workspace.labels["team"], "a");

    let pvc = s
        .fake
        .object("persistentvolumeclaims", &pvc_name())
        .expect("the PVC is stored");
    let keeper = s
        .fake
        .object("pods", &keeper_name())
        .expect("the keeper is stored");
    let pvc_uid = pvc["metadata"]["uid"].as_str().expect("a uid");
    assert_eq!(s.ledger.row(&pvc_name()).uid.as_deref(), Some(pvc_uid));
    assert_eq!(
        s.ledger.row(&keeper_name()).uid.as_deref(),
        keeper["metadata"]["uid"].as_str()
    );
    let requests = s.fake.requests();
    let posted = |collection: &str| {
        requests
            .iter()
            .position(|request| *request == format!("POST {collection}"))
            .expect("the create was sent")
    };
    assert!(
        posted(CLAIMS) < posted(PODS),
        "the PVC is created first: {requests:?}"
    );

    assert_eq!(pvc["spec"]["storageClassName"], "standard");
    assert_eq!(
        pvc["spec"]["resources"]["requests"]["storage"],
        "1073741824"
    );
    for (object, role, digest) in [
        (&pvc, "pvc", PVC_DIGEST),
        (&keeper, "keeper", KEEPER_DIGEST),
    ] {
        let labels = &object["metadata"]["labels"];
        assert_eq!(labels[MANAGED_BY_LABEL], MANAGED_BY);
        assert_eq!(
            labels[DEPLOYMENT_LABEL],
            names::deployment_label(DEPLOYMENT)
        );
        assert_eq!(
            labels["b10x-substrate/workspace"],
            names::resource_hash(DEPLOYMENT, WORKSPACE)
        );
        assert_eq!(labels["b10x-substrate/role"], role);
        assert_eq!(
            object["metadata"]["annotations"][OPERATION_ANNOTATION],
            format!("sha256:{digest}")
        );
    }
    assert_eq!(
        keeper["metadata"]["ownerReferences"],
        json!([{"apiVersion": "v1", "kind": "PersistentVolumeClaim", "name": pvc_name(),
                "uid": pvc_uid, "controller": false}])
    );
    let spec = &keeper["spec"];
    assert_eq!(spec["automountServiceAccountToken"], false);
    assert_eq!(
        spec["volumes"][0]["persistentVolumeClaim"]["claimName"],
        pvc_name()
    );
    let container = &spec["containers"][0];
    assert_eq!(container["name"], "keeper");
    assert_eq!(
        container["command"],
        json!(["/substrate-executor", "keeper"])
    );
    assert_eq!(container["volumeMounts"][0]["mountPath"], "/workspace");
    assert_eq!(container["securityContext"]["readOnlyRootFilesystem"], true);
    assert_eq!(
        container["securityContext"]["capabilities"]["drop"],
        json!(["ALL"])
    );
    assert!(
        container.get("env").is_none(),
        "the keeper is given no environment"
    );

    let calls = lock(&s.executor.calls).clone();
    assert_eq!(calls.len(), 1, "one handshake: {calls:?}");
    assert_eq!(calls[0].pod, keeper_name());
    assert_eq!(calls[0].container, "keeper");
    assert_eq!(calls[0].command, ["/substrate-executor", "version"]);
}

#[tokio::test]
async fn a_quota_refusal_is_workspace_capacity_and_any_other_403_is_api_failed() {
    let s = setup();
    s.fake.fault(Fault::Answer {
        method: Method::POST,
        path: CLAIMS.to_owned(),
        status: 403,
        body: quota_refusal("persistentvolumeclaims", &pvc_name(), "requests.storage"),
    });
    let DispatchOutcome::NotDispatched(refused) = s.create().await else {
        panic!("a quota refusal stores nothing");
    };
    assert_eq!(refused.class, DriverErrorClass::Exhausted);
    assert_eq!(refused.code, WORKSPACE_CAPACITY);
    assert_eq!(refused.address.as_deref(), Some("workspace"));
    assert!(
        s.fake
            .object("persistentvolumeclaims", &pvc_name())
            .is_none()
    );
    assert_eq!(s.count(&format!("POST {PODS}")), 0);

    s.fake.fault(Fault::Answer {
        method: Method::POST,
        path: CLAIMS.to_owned(),
        status: 403,
        body: json!({
            "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
            "message": "persistentvolumeclaims is forbidden: User \"system:serviceaccount:\
                        substrate:driver\" cannot create resource \"persistentvolumeclaims\"",
            "reason": "Forbidden", "code": 403,
        }),
    });
    let DispatchOutcome::NotDispatched(failed) = s.create().await else {
        panic!("another 403 stores nothing either");
    };
    assert_eq!(failed.class, DriverErrorClass::Failed);
    assert_eq!(failed.code, API_FAILED);

    // A keeper over the quota: the PVC alone was stored, and it is removed under its uid.
    s.fake.fault(Fault::Answer {
        method: Method::POST,
        path: PODS.to_owned(),
        status: 403,
        body: quota_refusal("pods", &keeper_name(), "pods"),
    });
    let DispatchOutcome::ContainedAbsent(contained) = s.create().await else {
        panic!("a refused keeper contains the create once its PVC reads absent");
    };
    assert_eq!(contained.code, WORKSPACE_CAPACITY);
    assert!(
        s.fake
            .object("persistentvolumeclaims", &pvc_name())
            .is_none()
    );
    assert_eq!(s.deletes(), [format!("DELETE {CLAIMS}/{}", pvc_name())]);
}

/// D184: the quota message alone is not a quota refusal. A 403 whose `Status.reason` is not
/// `Forbidden` is `kubernetes.api-failed`, whatever its message says, as on the exec path.
#[tokio::test]
async fn a_quota_message_under_another_reason_is_not_workspace_capacity() {
    let s = setup();
    for reason in [json!("Invalid"), json!(""), Value::Null] {
        let mut body = quota_refusal("persistentvolumeclaims", &pvc_name(), "requests.storage");
        body["reason"] = reason.clone();
        s.fake.fault(Fault::Answer {
            method: Method::POST,
            path: CLAIMS.to_owned(),
            status: 403,
            body,
        });
        let DispatchOutcome::NotDispatched(failed) = s.create().await else {
            panic!("a refused create stores nothing");
        };
        assert_eq!(failed.code, API_FAILED, "reason {reason}");
        assert_eq!(failed.class, DriverErrorClass::Failed, "reason {reason}");
    }
}

#[tokio::test]
async fn a_create_whose_answer_was_lost_adopts_the_stored_pvc_and_sends_nothing_again() {
    let s = setup();
    s.fake.fault(Fault::DropAnswer {
        method: Method::POST,
        path: CLAIMS.to_owned(),
    });
    let workspace = s.created().await;
    assert_eq!(workspace.state, substrate_wire::WorkspaceState::Ready);
    let stored = s
        .fake
        .object("persistentvolumeclaims", &pvc_name())
        .expect("stored");
    assert_eq!(
        s.ledger.row(&pvc_name()).uid.as_deref(),
        stored["metadata"]["uid"].as_str()
    );
    assert_eq!(
        s.count(&format!("POST {CLAIMS}")),
        1,
        "the PVC is never posted again"
    );
}

#[tokio::test]
async fn a_handshake_of_another_release_or_none_within_the_create_bound_is_outcome_unknown() {
    let s = setup();
    *lock(&s.executor.handshake) = Some(
        json!({"protocol": "substrate-executor/1", "version": "0.0.1", "source": "unknown"})
            .to_string(),
    );
    let DispatchOutcome::OutcomeUnknown(other) = s.create().await else {
        panic!("an executor of another release is not ready");
    };
    assert_eq!(other.code, PROTOCOL_FAILURE);
    assert_eq!(
        lock(&s.executor.calls).len(),
        1,
        "waiting does not change a release"
    );
    assert!(s.fake.object("pods", &keeper_name()).is_some());

    // No executor answers: the create waits on the driver's clock, which passes the bound after
    // the first attempt.
    let s = setup();
    for _ in 0..8 {
        s.fake.fault(Fault::Answer {
            method: Method::GET,
            path: format!("{PODS}/{}/exec", keeper_name()),
            status: 400,
            body: json!({"kind": "Status", "status": "Failure", "reason": "BadRequest",
                         "message": "container keeper is not running", "code": 400}),
        });
    }
    let clock = Arc::clone(&s.clock);
    let advance = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        clock.set(Duration::from_secs(61));
    });
    let DispatchOutcome::OutcomeUnknown(late) = s.create().await else {
        panic!("a keeper that never answers within the bound is unknown");
    };
    advance.await.expect("the clock advances");
    assert_eq!(late.code, API_FAILED);
    assert!(late.message.contains("within 60 s"), "{}", late.message);
    assert!(late.retriable);
}

#[tokio::test]
async fn observation_reads_ready_once_both_objects_are_ours_adopting_a_lost_keeper_answer() {
    let s = setup();
    let api = admin(&s.fake);
    let pvc = api
        .create(
            CLAIMS,
            &s.driver.render_pvc(WORKSPACE, &s.ledger.row(&pvc_name())),
        )
        .await
        .expect("the PVC is stored");
    let pvc_uid = pvc["metadata"]["uid"].as_str().expect("a uid").to_owned();
    s.ledger.bind_uid(&pvc_name(), &pvc_uid).expect("bound");
    let keeper = s.driver.render_keeper(
        WORKSPACE,
        &s.ledger.row(&keeper_name()),
        &pvc_name(),
        &pvc_uid,
    );
    let keeper = api
        .create(PODS, &keeper)
        .await
        .expect("the keeper is stored");

    let observed = s.observe().await.expect("both objects are ours");
    assert_eq!(observed.state, substrate_wire::WorkspaceState::Ready);
    assert_eq!(observed.labels["team"], "a");
    assert_eq!(
        s.ledger.row(&keeper_name()).uid.as_deref(),
        keeper["metadata"]["uid"].as_str(),
        "the keeper's uid is adopted by its annotation"
    );
    assert_eq!(s.count(&format!("POST {PODS}")), 1, "nothing is created");
}

#[tokio::test]
async fn a_crash_between_pvc_and_keeper_stays_unknown_until_the_n9_bound_then_fails() {
    let s = setup();
    // What a crash after the PVC's create and bind leaves: the keeper was never posted.
    let api = admin(&s.fake);
    let pvc = api
        .create(
            CLAIMS,
            &s.driver.render_pvc(WORKSPACE, &s.ledger.row(&pvc_name())),
        )
        .await
        .expect("the PVC is stored");
    s.ledger
        .bind_uid(&pvc_name(), pvc["metadata"]["uid"].as_str().expect("a uid"))
        .expect("bound");

    let undecided = s.observe().await.expect_err("the keeper may still land");
    assert_eq!(undecided.class, DriverErrorClass::Failed);
    assert!(undecided.retriable);
    assert!(
        s.fake
            .object("persistentvolumeclaims", &pvc_name())
            .is_some()
    );
    assert!(s.deletes().is_empty());

    s.clock
        .set(s.driver.dispatch_bound() + Duration::from_secs(1));
    let failed = s
        .observe()
        .await
        .expect_err("past the bound the create failed");
    assert_eq!(failed.class, DriverErrorClass::NotFound);
    assert!(
        s.fake
            .object("persistentvolumeclaims", &pvc_name())
            .is_none()
    );
    assert_eq!(s.deletes(), [format!("DELETE {CLAIMS}/{}", pvc_name())]);
    assert!(s.ledger.row(&pvc_name()).settled_at.is_some());
    assert!(s.ledger.row(&keeper_name()).settled_at.is_some());
    assert_eq!(
        s.count(&format!("POST {PODS}")),
        0,
        "nothing is created again"
    );
}

#[tokio::test]
async fn an_unbound_pvc_read_absent_is_dispatch_absence_only_past_the_n9_bound() {
    let s = setup();
    let undecided = s.observe().await.expect_err("the PVC may still land");
    assert_eq!(undecided.class, DriverErrorClass::Failed);
    assert!(s.ledger.row(&pvc_name()).settled_at.is_none());

    s.clock
        .set(s.driver.dispatch_bound() + Duration::from_secs(1));
    let absent = s.observe().await.expect_err("never stored");
    assert_eq!(absent.class, DriverErrorClass::NotFound);
    assert!(s.ledger.row(&pvc_name()).settled_at.is_some());
    assert!(s.ledger.row(&keeper_name()).settled_at.is_some());
    assert_eq!(s.count(&format!("POST {CLAIMS}")), 0);
}

#[tokio::test]
async fn every_served_file_route_is_one_executor_call_in_the_keeper() {
    let s = setup();
    s.created().await;
    let pvc = pvc_name();
    let d = &s.driver;

    let written = Driver::write_workspace_file(d, WORKSPACE, &pvc, "a.txt", b"one\ntwo\n")
        .await
        .expect("written");
    assert_eq!(written.sha256, sha(b"one\ntwo\n"));
    assert_eq!(
        std::fs::read(s.file("a.txt")).expect("on the volume"),
        b"one\ntwo\n"
    );

    let read = Driver::read_workspace_path(d, WORKSPACE, &pvc, "a.txt", &file_query())
        .await
        .expect("read");
    let FileReadResult::File(slice) = read.value else {
        panic!("a file reads as a slice");
    };
    assert_eq!(slice.content.decode().expect("base64"), b"one\ntwo\n");

    let digested = Driver::read_workspace_file_v2(d, WORKSPACE, &pvc, "a.txt", &file_query())
        .await
        .expect("read v2");
    assert_eq!(digested.sha256, sha(b"one\ntwo\n"));

    let tree_query: WorkspaceTreeQuery = serde_json::from_value(json!({})).expect("a query");
    let tree = Driver::list_workspace_tree_v2(d, WORKSPACE, &pvc, &tree_query)
        .await
        .expect("tree");
    assert_eq!(tree.value.items.len(), 1);

    let replace: FileReplaceInput = serde_json::from_value(json!({
        "content": {"encoding": "base64", "data": "b25lCnR3bwp0aHJlZQo="},
        "expected": {"state": "sha256", "sha256": sha(b"one\ntwo\n")},
    }))
    .expect("a replacement");
    let replaced = Driver::replace_workspace_file_v2(d, WORKSPACE, &pvc, "a.txt", &replace)
        .await
        .expect("replaced");
    assert_eq!(replaced.after_sha256, sha(b"one\ntwo\nthree\n"));

    let edit: FileEditInput = serde_json::from_value(json!({
        "expected_sha256": sha(b"one\ntwo\nthree\n"), "old_text": "two",
        "new_text": "TWO", "match_policy": "exact",
    }))
    .expect("an edit");
    let edited = Driver::edit_workspace_file_v2(d, WORKSPACE, &pvc, "a.txt", &edit)
        .await
        .expect("edited");
    assert_eq!(edited.after_sha256, sha(b"one\nTWO\nthree\n"));

    let patch: FilePatchInput = serde_json::from_value(json!({
        "expected_sha256": sha(b"one\nTWO\nthree\n"),
        "edits": [{"kind": "insert_before", "line": 1, "text": "zero\n"}],
    }))
    .expect("a patch");
    let patched = Driver::patch_workspace_file_v2(d, WORKSPACE, &pvc, "a.txt", &patch)
        .await
        .expect("patched");
    let on_volume = std::fs::read(s.file("a.txt")).expect("on the volume");
    assert_eq!(patched.after_sha256, sha(&on_volume));

    let escape = Driver::write_workspace_file(d, WORKSPACE, &pvc, "../escape", b"x")
        .await
        .expect_err("a path outside the volume is refused by the guarded code");
    assert_eq!(escape.code, "workspace.path-escape");
    assert!(!s.root.path().join("../escape").exists());

    let absence = Driver::delete_workspace_file(d, WORKSPACE, &pvc, "a.txt")
        .await
        .expect("deleted");
    assert!(absence.absent);
    assert!(!s.file("a.txt").exists());

    assert_eq!(
        s.executor.fs_ops(),
        [
            "write",
            "read",
            "read-v2",
            "tree-v2",
            "replace-v2",
            "edit-v2",
            "patch-v2",
            "write",
            "delete"
        ]
    );
    for call in lock(&s.executor.calls).iter() {
        assert_eq!(
            (call.pod.as_str(), call.container.as_str()),
            (keeper_name().as_str(), "keeper")
        );
    }
}

/// Holds the next `fs` answer past a short client bound, after the executor acted.
fn stall_next(s: &mut Setup) {
    s.driver.api.timeout = Duration::from_secs(1);
    *lock(&s.executor.stall) = Duration::from_millis(1600);
    s.executor.stall_next.store(true, Ordering::SeqCst);
}

#[tokio::test]
async fn a_lost_write_answer_is_resolved_by_one_stat_and_never_resent() {
    let mut s = setup();
    s.created().await;
    stall_next(&mut s);
    let observed =
        Driver::write_workspace_file(&s.driver, WORKSPACE, &pvc_name(), "a.txt", b"kept")
            .await
            .expect("the stat shows the written content");
    assert_eq!(observed.sha256, sha(b"kept"));
    assert_eq!(observed.size, 4);
    assert_eq!(s.executor.fs_ops(), ["write", "stat"]);
    assert_eq!(std::fs::read(s.file("a.txt")).expect("written"), b"kept");
}

#[tokio::test]
async fn a_lost_answer_with_another_digest_is_executor_unanswered_and_retriable() {
    let s = setup();
    s.created().await;
    // The upgrade is dropped before the executor runs: the write never happened.
    s.fake.fault(Fault::DropAnswer {
        method: Method::GET,
        path: format!("{PODS}/{}/exec", keeper_name()),
    });
    let failed = Driver::write_workspace_file(&s.driver, WORKSPACE, &pvc_name(), "a.txt", b"lost")
        .await
        .expect_err("the stat shows no such content");
    assert_eq!(failed.class, DriverErrorClass::Failed);
    assert_eq!(failed.code, EXECUTOR_UNANSWERED);
    assert!(failed.retriable);
    assert_eq!(s.executor.fs_ops(), ["stat"], "nothing is sent again");
    assert!(!s.file("a.txt").exists());
}

/// Only an unconditional delete (or write) is resolved by its `stat`; every request with a
/// precondition (replace, edit, patch) is `kubernetes.executor-unanswered` and retriable even when
/// it applied and the `stat` shows exactly the state it would leave.
#[tokio::test]
async fn lost_answers_resolve_only_unconditional_mutations_by_their_stat() {
    let mut s = setup();
    s.created().await;
    std::fs::write(s.file("a.txt"), b"old\n").expect("seeded");
    let pvc = pvc_name();

    stall_next(&mut s);
    let replace: FileReplaceInput = serde_json::from_value(json!({
        "content": {"encoding": "base64", "data": "bmV3Cg=="},
        "expected": {"state": "sha256", "sha256": sha(b"old\n")},
    }))
    .expect("a replacement");
    let replaced = Driver::replace_workspace_file_v2(&s.driver, WORKSPACE, &pvc, "a.txt", &replace)
        .await
        .expect_err("a conditional replace is never resolved by its stat");
    assert_eq!(replaced.class, DriverErrorClass::Failed);
    assert_eq!(replaced.code, EXECUTOR_UNANSWERED);
    assert!(replaced.retriable);
    assert_eq!(std::fs::read(s.file("a.txt")).expect("applied"), b"new\n");

    stall_next(&mut s);
    let edit: FileEditInput = serde_json::from_value(json!({
        "expected_sha256": sha(b"new\n"), "old_text": "new",
        "new_text": "edited", "match_policy": "exact",
    }))
    .expect("an edit");
    let edited = Driver::edit_workspace_file_v2(&s.driver, WORKSPACE, &pvc, "a.txt", &edit)
        .await
        .expect_err("an edit states no resulting digest");
    assert_eq!(edited.code, EXECUTOR_UNANSWERED);
    assert!(edited.retriable);
    assert_eq!(std::fs::read(s.file("a.txt")).expect("kept"), b"edited\n");

    stall_next(&mut s);
    let patch: FilePatchInput = serde_json::from_value(json!({
        "expected_sha256": sha(b"edited\n"),
        "edits": [{"kind": "insert_before", "line": 1, "text": "zero\n"}],
    }))
    .expect("a patch");
    let patched = Driver::patch_workspace_file_v2(&s.driver, WORKSPACE, &pvc, "a.txt", &patch)
        .await
        .expect_err("a conditional patch is never resolved by its stat");
    assert_eq!(patched.code, EXECUTOR_UNANSWERED);
    assert!(patched.retriable);
    assert_eq!(
        std::fs::read(s.file("a.txt")).expect("patched"),
        b"zero\nedited\n"
    );

    stall_next(&mut s);
    let absence = Driver::delete_workspace_file(&s.driver, WORKSPACE, &pvc, "a.txt")
        .await
        .expect("the stat shows the file absent");
    assert!(absence.absent);

    assert_eq!(
        s.executor.fs_ops(),
        [
            "replace-v2",
            "stat",
            "edit-v2",
            "stat",
            "patch-v2",
            "stat",
            "delete",
            "stat"
        ]
    );
}

#[tokio::test]
async fn a_lost_or_foreign_keeper_fails_every_file_route_and_is_never_called() {
    let s = setup();
    s.created().await;
    let api = admin(&s.fake);
    api.delete(&format!("{PODS}/{}", keeper_name()), 0)
        .await
        .expect("the keeper is deleted behind the driver's back");
    let gone = Driver::write_workspace_file(&s.driver, WORKSPACE, &pvc_name(), "a.txt", b"x")
        .await
        .expect_err("a lost keeper is not recreated");
    assert_eq!(gone.code, API_FAILED);
    let observed = s
        .observe()
        .await
        .expect_err("nor does observation recreate it");
    assert_eq!(observed.code, API_FAILED);
    assert_eq!(s.count(&format!("POST {PODS}")), 1);

    s.fake
        .seed(
            "pods",
            json!({"metadata": {"name": keeper_name(), "labels": {
                MANAGED_BY_LABEL: MANAGED_BY,
                DEPLOYMENT_LABEL: names::deployment_label("deploy-b"),
            }}}),
        )
        .expect("a same-named Pod of another deployment");
    let foreign = Driver::write_workspace_file(&s.driver, WORKSPACE, &pvc_name(), "a.txt", b"x")
        .await
        .expect_err("a foreign Pod under the keeper's name is never called");
    assert_eq!(foreign.code, API_FAILED);
    assert_eq!(s.driver.collisions_total(), 1);
    assert!(s.executor.fs_ops().is_empty());
}

/// `story:kubernetes-collisions-metric`: the driver port answers the count the operational
/// listener serves as `substrate_kubernetes_collisions_total` (design 34 § 4). A PVC of another
/// deployment already holds the workspace's PVC name: the create meets it, it is one collision,
/// the port's count moves from 0 to exactly 1, and the object is left as it was.
#[tokio::test]
async fn one_collision_on_the_api_moves_the_ports_collision_count_by_exactly_one() {
    let s = setup();
    assert_eq!(Driver::backend_collisions(&s.driver), Some(0));
    let foreign = s
        .fake
        .seed(
            "persistentvolumeclaims",
            json!({"metadata": {"name": pvc_name(), "labels": {
                MANAGED_BY_LABEL: MANAGED_BY,
                DEPLOYMENT_LABEL: names::deployment_label("deploy-b"),
            }}}),
        )
        .expect("a same-named PVC of another deployment");
    let outcome = s.create().await;
    assert!(
        !matches!(outcome, DispatchOutcome::Observed(_)),
        "a foreign PVC is never adopted"
    );
    assert_eq!(Driver::backend_collisions(&s.driver), Some(1));
    assert_eq!(
        s.fake.object("persistentvolumeclaims", &pvc_name()),
        Some(foreign),
        "a collision is never modified"
    );
    assert!(s.deletes().is_empty(), "{:?}", s.deletes());
    assert_eq!(
        s.ledger.row(&pvc_name()).uid,
        None,
        "a collision binds nothing"
    );
}

#[tokio::test]
async fn destroy_removes_exec_pods_then_the_keeper_then_the_pvc_and_is_pending_until_absent() {
    let s = setup();
    s.created().await;
    let pvc_uid = s.ledger.row(&pvc_name()).uid.expect("bound");
    let exec = names::exec_pod(DEPLOYMENT, "ex_one");
    s.ledger.reserve(
        BackendKind::Exec,
        "ex_one",
        BackendRole::Pod,
        &exec,
        EXEC_DIGEST,
    );
    let stored = admin(&s.fake)
        .create(
            PODS,
            &json!({"metadata": {
                "name": exec,
                "labels": names::labels(DEPLOYMENT, BackendKind::Exec,
                    &names::resource_hash(DEPLOYMENT, "ex_one"), BackendRole::Pod),
                "annotations": {OPERATION_ANNOTATION: format!("sha256:{EXEC_DIGEST}")},
                "ownerReferences": [{"apiVersion": "v1", "kind": "PersistentVolumeClaim",
                    "name": pvc_name(), "uid": pvc_uid, "controller": false}],
            }}),
        )
        .await
        .expect("an exec Pod of the workspace");
    s.ledger
        .bind_uid(&exec, stored["metadata"]["uid"].as_str().expect("a uid"))
        .expect("bound");
    // The keeper's deletion is accepted and the keeper still reads present: it is stopping.
    let keeper = s.fake.object("pods", &keeper_name()).expect("the keeper");
    s.fake.fault(Fault::Answer {
        method: Method::DELETE,
        path: format!("{PODS}/{}", keeper_name()),
        status: 200,
        body: keeper,
    });

    let first = Driver::destroy_workspace(&s.driver, WORKSPACE, &pvc_name())
        .await
        .expect("destroying");
    assert!(
        matches!(
            first,
            WorkspaceDestroyProgress::Pending { removed_items: 1 }
        ),
        "{first:?}"
    );
    assert!(s.fake.object("pods", &exec).is_none());
    assert!(s.fake.object("pods", &keeper_name()).is_some());
    assert!(
        s.fake
            .object("persistentvolumeclaims", &pvc_name())
            .is_some()
    );

    let second = Driver::destroy_workspace(&s.driver, WORKSPACE, &pvc_name())
        .await
        .expect("destroyed");
    let WorkspaceDestroyProgress::Absent(absence) = second else {
        panic!("every object reads absent");
    };
    assert_eq!(absence.id, WORKSPACE);
    assert!(s.fake.object("pods", &keeper_name()).is_none());
    assert!(
        s.fake
            .object("persistentvolumeclaims", &pvc_name())
            .is_none()
    );
    assert_eq!(
        s.deletes(),
        [
            format!("DELETE {PODS}/{exec}"),
            format!("DELETE {PODS}/{}", keeper_name()),
            format!("DELETE {PODS}/{}", keeper_name()),
            format!("DELETE {CLAIMS}/{}", pvc_name()),
        ]
    );
    for name in [exec, keeper_name(), pvc_name()] {
        assert!(s.ledger.row(&name).settled_at.is_some(), "{name}");
    }
}

/// A destroy whose first call was `Pending` on a stopping keeper names the API's refusal when a
/// later call reaches the PVC and the Role does not grant its deletion: `kubernetes.api-failed`,
/// the code the first call would have answered, and the PVC's row stays open (invariant 3).
#[tokio::test]
async fn a_pending_destroy_names_a_forbidden_pvc_deletion_on_the_call_that_reaches_it() {
    let s = setup();
    s.created().await;
    let keeper = s.fake.object("pods", &keeper_name()).expect("the keeper");
    s.fake.fault(Fault::Answer {
        method: Method::DELETE,
        path: format!("{PODS}/{}", keeper_name()),
        status: 200,
        body: keeper,
    });
    let first = Driver::destroy_workspace(&s.driver, WORKSPACE, &pvc_name())
        .await
        .expect("destroying");
    assert!(
        matches!(
            first,
            WorkspaceDestroyProgress::Pending { removed_items: 0 }
        ),
        "{first:?}"
    );
    s.fake.fault(Fault::Answer {
        method: Method::DELETE,
        path: format!("{CLAIMS}/{}", pvc_name()),
        status: 403,
        body: json!({"kind": "Status", "apiVersion": "v1", "status": "Failure",
            "reason": "Forbidden", "code": 403,
            "message": "persistentvolumeclaims is forbidden: cannot delete"}),
    });

    let second = Driver::destroy_workspace(&s.driver, WORKSPACE, &pvc_name()).await;
    let error = second.expect_err("a forbidden PVC deletion is a named refusal, never pending");
    assert_eq!(error.code, API_FAILED);
    assert!(s.fake.object("pods", &keeper_name()).is_none());
    assert!(
        s.fake
            .object("persistentvolumeclaims", &pvc_name())
            .is_some()
    );
    assert!(s.ledger.row(&keeper_name()).settled_at.is_some());
    assert!(s.ledger.row(&pvc_name()).settled_at.is_none());
}

#[tokio::test]
async fn a_requested_storage_quota_is_unserved_before_anything_is_sent() {
    let s = setup();
    let input: WorkspaceCreateInput = serde_json::from_value(json!({
        "source": "empty", "labels": {}, "storage": {"max_bytes": 1_048_576, "max_inodes": 100},
    }))
    .expect("a quota create");
    let DispatchOutcome::NotDispatched(unserved) =
        Driver::create_workspace(&s.driver, WORKSPACE, &pvc_name(), &input).await
    else {
        panic!("a quota is refused before dispatch");
    };
    assert_eq!(unserved.class, DriverErrorClass::Unserved);
    assert_eq!(unserved.code, "workspace.storage-quota-unserved");
    assert!(s.fake.requests().is_empty());
}

// Security review of U8 (integrity conformance): what a lost-answer resolution may report, the
// root-name check, the keeper's security profile and the quota match.

/// A create-if-absent the executor refused, because the file already held the same bytes another
/// writer put there, is not a success: the refusal is the answer the executor gave, and one `stat`
/// cannot tell it from an applied create. Reporting `before_sha256: None` would tell a second
/// writer it created a file it did not.
#[tokio::test]
async fn sec_a_lost_answer_never_turns_a_refused_create_if_absent_into_success() {
    let mut s = setup();
    s.created().await;
    std::fs::write(s.file("lock"), b"held\n").expect("another writer created it");
    stall_next(&mut s);
    let create: FileReplaceInput = serde_json::from_value(json!({
        "content": {"encoding": "base64", "data": "aGVsZAo="},
        "expected": {"state": "absent"},
    }))
    .expect("a create-if-absent");
    let answer =
        Driver::replace_workspace_file_v2(&s.driver, WORKSPACE, &pvc_name(), "lock", &create).await;
    assert_eq!(s.executor.fs_ops(), ["replace-v2", "stat"]);
    let error = answer.expect_err(
        "the executor refused the create (the file existed); a lost answer must not report it \
         applied",
    );
    assert_eq!(error.code, EXECUTOR_UNANSWERED);
}

/// The same for a compare-and-swap from a digest the file never held: the executor refused it, and
/// the answer must not state `before_sha256` as the request's expected digest.
#[tokio::test]
async fn sec_a_lost_answer_never_reports_a_compare_and_swap_from_a_digest_the_file_never_had() {
    let mut s = setup();
    s.created().await;
    std::fs::write(s.file("a.txt"), b"new\n").expect("another writer replaced it first");
    stall_next(&mut s);
    let replace: FileReplaceInput = serde_json::from_value(json!({
        "content": {"encoding": "base64", "data": "bmV3Cg=="},
        "expected": {"state": "sha256", "sha256": sha(b"old\n")},
    }))
    .expect("a replacement");
    let answer =
        Driver::replace_workspace_file_v2(&s.driver, WORKSPACE, &pvc_name(), "a.txt", &replace)
            .await;
    assert_eq!(s.executor.fs_ops(), ["replace-v2", "stat"]);
    let error = answer.expect_err(
        "the executor refused the swap (the file never held the expected digest); a lost answer \
         must not report it applied",
    );
    assert_eq!(error.code, EXECUTOR_UNANSWERED);
}

/// Only the exact PVC name derived for the workspace passes `check_root`, on every route, and a
/// refused root sends nothing to the cluster or the executor.
#[tokio::test]
async fn sec_check_root_admits_only_the_exact_derived_name_and_sends_nothing_otherwise() {
    let s = setup();
    s.created().await;
    let exact = pvc_name();
    let sent = s.fake.requests().len();
    let homoglyph = exact.replacen('s', "\u{0455}", 1);
    let near = [
        String::new(),
        exact.to_uppercase(),
        format!("{exact} "),
        format!("{exact}\0"),
        format!("{exact}/.."),
        format!("../{exact}"),
        "..".to_owned(),
        homoglyph,
        "a".repeat(255),
        names::workspace_pvc(DEPLOYMENT, "ws_two"),
        names::workspace_pvc("deploy-b", WORKSPACE),
        keeper_name(),
    ];
    for root in &near {
        let d = &s.driver;
        let errors = [
            Driver::write_workspace_file(d, WORKSPACE, root, "a.txt", b"x")
                .await
                .err(),
            Driver::read_workspace_path(d, WORKSPACE, root, "a.txt", &file_query())
                .await
                .err(),
            Driver::delete_workspace_file(d, WORKSPACE, root, "a.txt")
                .await
                .err(),
            Driver::observe_workspace(d, WORKSPACE, root, &provisional())
                .await
                .err(),
            Driver::destroy_workspace(d, WORKSPACE, root).await.err(),
        ];
        for error in errors {
            let error = error.unwrap_or_else(|| panic!("{root:?} is admitted"));
            assert_eq!(error.code, API_FAILED, "{root:?}");
        }
        assert!(
            !matches!(
                Driver::create_workspace(d, "ws_three", root, &empty()).await,
                DispatchOutcome::Observed(_)
            ),
            "{root:?}"
        );
    }
    assert_eq!(
        s.fake.requests().len(),
        sent,
        "a refused root sends nothing"
    );
    assert!(s.executor.fs_ops().is_empty());
    assert!(!s.file("a.txt").exists());
}

/// The keeper conforms to Pod Security `restricted` and states requests equal to limits, with no
/// token, no environment, no host namespace and no capability added.
#[test]
fn sec_the_keeper_pod_states_the_restricted_profile_and_equal_bounds() {
    let s = setup();
    let keeper = s.driver.render_keeper(
        WORKSPACE,
        &s.ledger.row(&keeper_name()),
        &pvc_name(),
        "uid-1",
    );
    let spec = &keeper["spec"];
    assert_eq!(spec["automountServiceAccountToken"], false);
    assert_eq!(spec["enableServiceLinks"], false);
    for host in ["hostIPC", "hostNetwork", "hostPID"] {
        assert_eq!(spec[host], false, "{host}");
    }
    assert!(spec.get("initContainers").is_none());
    assert!(spec.get("ephemeralContainers").is_none());
    assert_eq!(spec["securityContext"]["runAsNonRoot"], true);
    assert_ne!(spec["securityContext"]["runAsUser"], 0);
    assert_eq!(
        spec["securityContext"]["seccompProfile"]["type"],
        "RuntimeDefault"
    );
    let volumes = spec["volumes"].as_array().expect("volumes");
    assert_eq!(volumes.len(), 1);
    assert!(volumes[0].get("hostPath").is_none());
    let containers = spec["containers"].as_array().expect("containers");
    assert_eq!(containers.len(), 1);
    let container = &containers[0];
    let context = &container["securityContext"];
    assert_eq!(context["runAsNonRoot"], true);
    assert_ne!(context["runAsUser"], 0);
    assert_eq!(context["allowPrivilegeEscalation"], false);
    assert_eq!(context["privileged"], false);
    assert_eq!(context["readOnlyRootFilesystem"], true);
    assert_eq!(context["capabilities"], json!({"drop": ["ALL"]}));
    assert_eq!(context["seccompProfile"]["type"], "RuntimeDefault");
    assert!(container.get("env").is_none() && container.get("envFrom").is_none());
    assert!(container.get("ports").is_none());
    assert_eq!(
        container["resources"]["requests"],
        container["resources"]["limits"]
    );
    for resource in ["cpu", "memory", "ephemeral-storage"] {
        assert!(
            container["resources"]["limits"][resource].is_string(),
            "{resource}"
        );
    }
    let image = container["image"].as_str().expect("an image");
    assert!(image.contains("@sha256:"), "{image}");
}

/// A 403 is capacity only by the API server's quota phrase after `is forbidden: `: the words
/// `exceeded quota` elsewhere, as an admission webhook may write them, stay `api-failed`.
#[tokio::test]
async fn sec_exceeded_quota_without_the_forbidden_phrase_stays_api_failed() {
    let s = setup();
    s.fake.fault(Fault::Answer {
        method: Method::POST,
        path: CLAIMS.to_owned(),
        status: 403,
        body: json!({
            "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
            "message": "admission webhook \"limits.example\" denied the request: exceeded quota: \
                        team-a",
            "reason": "Forbidden", "code": 403,
        }),
    });
    let DispatchOutcome::NotDispatched(failed) = s.create().await else {
        panic!("a refused create stores nothing");
    };
    assert_eq!(failed.code, API_FAILED);
    assert_eq!(failed.class, DriverErrorClass::Failed);
}
