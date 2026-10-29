//! The exec path on the fake API server (design 34 § 12 U9): each § 5 exec row, signals, a force
//! delete, a 410 in the middle of a run, the frame a child cannot forge and the non-frame line
//! that is refused. The fake has no kubelet; a case plays it, writing the Pod's status and its
//! log, and two cases run the real executor in-process to make that log. Every case runs once, in
//! seconds.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use substrate_executor::frame::{Applied, Control, ExitRecord, Frame, Refusal};
use substrate_executor::spec::RunSpec;
use substrate_host::{
    BackendKind, BackendLedger, BackendName, BackendRecord, BackendRole, DispatchOutcome,
    DriverError, DriverErrorClass, ExecObservation, UidBinding,
};
use substrate_kubernetes_client::fake::{FakeCluster, Fault};
use substrate_kubernetes_client::{Api, Method, kubeconfig};
use substrate_wire::{
    BaselineEnvironment, ConfinementRequest, ExecEnvironment, ExecLimits, ExecMeasurement,
    ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState, ExecutionCapsuleInput,
    NetworkMode, OutputStream, ReadOnlyFile, ReadOnlyRoot, SandboxProfile, SecretSlotRequest,
    Signal, StorageLimit, WorkspaceAccess, WritableGitRoot, WritableHostRoot, WritableRoot,
};

use crate::lease::Clock;
use crate::logs::EXECUTOR_PROTOCOL;
use crate::names::{self, DEPLOYMENT_LABEL, MANAGED_BY, MANAGED_BY_LABEL, OPERATION_ANNOTATION};
use crate::rules::{DEFAULT_RESOURCE_RULES, ROLE_RULES};
use crate::{API_FAILED, CAPABILITY_STALE, EXTENSION, NamespaceDriver};

const DEPLOYMENT: &str = "deploy-a";
const TOKEN: &str = "fake-token-never-quoted";
const POLICY: &str = "default-deny";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const WORKSPACE: &str = "ws_0001";
const NODE: &str = "node-a";
const SNAPSHOT: &str = "sha256:snapshot-under-test";
const POD_CREATE: &str = "POST /api/v1/namespaces/substrate/pods";

struct ManualClock {
    base: Instant,
    wall_base: DateTime<Utc>,
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.base
    }

    fn wall(&self) -> DateTime<Utc> {
        self.wall_base
    }
}

/// The daemon's ledger, in memory: one row per reserved name, paged as the store pages it.
#[derive(Default)]
struct MemoryLedger {
    rows: Mutex<BTreeMap<String, BackendRecord>>,
}

impl MemoryLedger {
    fn reserve(&self, kind: BackendKind, id: &str, role: BackendRole, name: &str) {
        self.lock().insert(
            name.to_owned(),
            BackendRecord {
                kind,
                resource_id: id.to_owned(),
                role,
                name: name.to_owned(),
                operation_digest: "ab".repeat(32),
                uid: None,
                created_at: Utc::now(),
                settled_at: None,
            },
        );
    }

    fn row(&self, name: &str) -> BackendRecord {
        self.lock().get(name).cloned().expect("the row is reserved")
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, BackendRecord>> {
        self.rows.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl BackendLedger for MemoryLedger {
    fn recorded(&self, kind: BackendKind, id: &str) -> Result<Vec<BackendRecord>, DriverError> {
        Ok(self
            .lock()
            .values()
            .filter(|row| row.kind == kind && row.resource_id == id)
            .cloned()
            .collect())
    }

    fn bind_uid(&self, name: &str, uid: &str) -> Result<UidBinding, DriverError> {
        let mut rows = self.lock();
        let Some(row) = rows.get_mut(name) else {
            return Ok(UidBinding::Unrecorded);
        };
        if row.settled_at.is_some() {
            return Ok(UidBinding::Settled);
        }
        match &row.uid {
            Some(bound) if bound != uid => Ok(UidBinding::Collision {
                bound: bound.clone(),
            }),
            _ => {
                row.uid = Some(uid.to_owned());
                Ok(UidBinding::Bound)
            }
        }
    }

    fn settle(&self, name: &str) -> Result<bool, DriverError> {
        Ok(self.lock().get_mut(name).is_some_and(|row| {
            row.settled_at = Some(Utc::now());
            true
        }))
    }

    fn reserve_probe(&self, _nonce: &str, _names: &[BackendName]) -> Result<(), DriverError> {
        Ok(())
    }

    fn unsettled(&self, after: Option<&str>) -> Result<Vec<BackendRecord>, DriverError> {
        Ok(self
            .lock()
            .values()
            .filter(|row| row.settled_at.is_none())
            .filter(|row| after.is_none_or(|after| row.name.as_str() > after))
            .take(2)
            .cloned()
            .collect())
    }
}

fn extension() -> Value {
    json!({
        "storage_class": "standard",
        "workspace_size_bytes": 1_073_741_824_u64,
        "executor_image": format!("registry.invalid/substrate/executor@sha256:{DIGEST}"),
        "exec_image": format!("registry.invalid:5000/busybox@sha256:{DIGEST}"),
        "default_deny_policy": POLICY,
        "egress_probe_address": "192.0.2.10:443",
        "pod_pids_limit": 1024,
        "container_log_max_bytes": 10_485_760,
    })
}

fn kubeconfig_with(fake: &FakeCluster, extension: Option<Value>) -> Vec<u8> {
    let mut config = fake.kubeconfig();
    if let Some(extension) = extension {
        config["contexts"][0]["context"]["extensions"] =
            json!([{"name": EXTENSION, "extension": extension}]);
    }
    serde_json::to_vec(&config).expect("the kubeconfig serializes")
}

/// A client of the fake with no driver settings: the kubelet, an operator or another party.
fn admin(fake: &FakeCluster) -> Api {
    let bytes = kubeconfig_with(fake, None);
    kubeconfig::parse(&bytes, |_, _| Ok(()))
        .expect("the fake's kubeconfig loads")
        .0
}

fn exact_rules() -> Vec<Value> {
    ROLE_RULES
        .iter()
        .chain(DEFAULT_RESOURCE_RULES.iter())
        .map(|(group, resource, verbs)| {
            json!({"verbs": verbs, "apiGroups": [group], "resources": [resource]})
        })
        .collect()
}

/// An open driver over a fake whose workspace `ws_0001` has its PVC and keeper bound, with
/// `SNAPSHOT` published at the open binding's generation.
struct Rig {
    fake: FakeCluster,
    driver: NamespaceDriver,
    ledger: Arc<MemoryLedger>,
    admin: Api,
    pvc: String,
    pvc_uid: String,
}

async fn rig() -> Rig {
    let fake = FakeCluster::start("substrate", TOKEN).expect("the fake starts");
    fake.set_rules(exact_rules());
    fake.seed(
        "networkpolicies",
        json!({
            "metadata": {"name": POLICY},
            "spec": {"podSelector": {}, "policyTypes": ["Ingress", "Egress"]},
        }),
    )
    .expect("the policy is stored");
    fake.enforce_restricted(true);
    let ledger = Arc::new(MemoryLedger::default());
    let driver = NamespaceDriver::parse_with_clock(
        &kubeconfig_with(&fake, Some(extension())),
        DEPLOYMENT,
        Arc::clone(&ledger) as Arc<dyn BackendLedger>,
        Arc::new(ManualClock {
            base: Instant::now(),
            wall_base: Utc::now(),
        }),
    )
    .expect("the configuration loads");
    driver.open().await.expect("the driver opens");
    let generation = driver.binding().expect("bound").config_generation();
    driver.publish_exec_snapshot(SNAPSHOT, generation);

    let labels = |role: &str| {
        json!({
            MANAGED_BY_LABEL: MANAGED_BY,
            DEPLOYMENT_LABEL: names::deployment_label(DEPLOYMENT),
            "b10x-substrate/role": role,
        })
    };
    let pvc = names::workspace_pvc(DEPLOYMENT, WORKSPACE);
    let keeper = names::keeper_pod(DEPLOYMENT, WORKSPACE);
    let claim = fake
        .seed(
            "persistentvolumeclaims",
            json!({"metadata": {"name": pvc, "labels": labels("pvc")}, "spec": {}}),
        )
        .expect("the PVC is stored");
    let held = fake
        .seed(
            "pods",
            json!({
                "metadata": {"name": keeper, "labels": labels("keeper")},
                "spec": {"nodeName": NODE, "restartPolicy": "Always"},
            }),
        )
        .expect("the keeper is stored");
    ledger.reserve(BackendKind::Workspace, WORKSPACE, BackendRole::Pvc, &pvc);
    ledger.reserve(
        BackendKind::Workspace,
        WORKSPACE,
        BackendRole::Keeper,
        &keeper,
    );
    let pvc_uid = uid(&claim);
    ledger.bind_uid(&pvc, &pvc_uid).expect("binds");
    ledger.bind_uid(&keeper, &uid(&held)).expect("binds");
    let admin = admin(&fake);
    Rig {
        fake,
        driver,
        ledger,
        admin,
        pvc,
        pvc_uid,
    }
}

fn uid(object: &Value) -> String {
    object["metadata"]["uid"]
        .as_str()
        .expect("a stored object has a uid")
        .to_owned()
}

fn input(argv: &[&str]) -> ExecStartInput {
    ExecStartInput {
        workspace: WORKSPACE.to_owned(),
        argv: argv.iter().map(|part| (*part).to_owned()).collect(),
        env: ExecEnvironment {
            allow: vec![BaselineEnvironment::Path, BaselineEnvironment::Lang],
            set: BTreeMap::from([("GREETING".to_owned(), "$(HOME) $$".to_owned())]),
        },
        sandbox: ConfinementRequest {
            capability_snapshot: SNAPSHOT.to_owned(),
            network: NetworkMode::None,
            aperture: None,
            apertures: Vec::new(),
            profile: SandboxProfile::Workspace,
            required: true,
        },
        limits: ExecLimits {
            timeout_ms: 10_000,
            output_bytes: 65_536,
            processes: 1_024,
            memory_bytes: 67_108_864,
            cpu_millis: 5_000,
        },
        wait: false,
        workspace_access: WorkspaceAccess::ReadWrite,
        scratch: None,
        measurements: BTreeSet::new(),
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        capsule: None,
        lease_ttl_ms: None,
        workdir: None,
        writable_roots: Vec::new(),
    }
}

impl Rig {
    /// Reserves the exec's `pod` row, as the daemon does with the operation's `accepted` state.
    fn reserve(&self, id: &str) -> String {
        let pod = names::exec_pod(DEPLOYMENT, id);
        self.ledger
            .reserve(BackendKind::Exec, id, BackendRole::Pod, &pod);
        pod
    }

    async fn start(&self, id: &str, input: &ExecStartInput) -> DispatchOutcome<ExecObservation> {
        self.reserve(id);
        self.driver.exec_start(id, &self.pvc, input).await
    }

    async fn started(&self, id: &str, input: &ExecStartInput) -> ExecObservation {
        match self.start(id, input).await {
            DispatchOutcome::Observed(observation) => observation,
            DispatchOutcome::NotDispatched(error)
            | DispatchOutcome::ContainedAbsent(error)
            | DispatchOutcome::OutcomeUnknown(error) => {
                panic!(
                    "the start was not observed: {} {}",
                    error.code, error.message
                )
            }
        }
    }

    fn pod(&self, name: &str) -> Option<Value> {
        self.fake.object("pods", name)
    }

    fn pod_creates(&self) -> usize {
        self.fake
            .requests()
            .iter()
            .filter(|request| request.starts_with(POD_CREATE) && !request.contains("dryRun="))
            .count()
    }

    /// The kubelet writes `status` (and binds the Pod to `NODE`) through the API.
    async fn kubelet_status(&self, name: &str, status: Value) {
        let mut pod = self.pod(name).expect("the Pod is present");
        pod["spec"]["nodeName"] = json!(NODE);
        pod["status"] = status;
        self.admin
            .update(&format!("{}/{name}", self.admin.pods()), &pod)
            .await
            .expect("the kubelet's status is stored");
    }

    async fn running(&self, name: &str) {
        self.kubelet_status(
            name,
            json!({"phase": "Running", "containerStatuses": [{"name": "run", "state": {"running": {}}}]}),
        )
        .await;
    }

    /// The run container ended with `code`, `log` is its log, and the Pod's phase says so.
    async fn terminated(&self, name: &str, code: i32, log: &[u8]) {
        self.fake.set_log(name, log);
        let phase = if code == 0 { "Succeeded" } else { "Failed" };
        self.kubelet_status(
            name,
            json!({
                "phase": phase,
                "containerStatuses": [{"name": "run", "state": {"terminated": {"exitCode": code, "reason": if code == 0 { "Completed" } else { "Error" }}}}],
            }),
        )
        .await;
    }

    /// Plays the kubelet with the real executor: decodes the Pod's run spec from its command,
    /// runs it under a test cgroup root that applies exactly its bounds, and stores the log and
    /// the terminated status. Returns the spec the Pod carried.
    async fn run_executor(&self, name: &str) -> RunSpec {
        let pod = self.pod(name).expect("the Pod is present");
        let command: Vec<String> = pod["spec"]["containers"][0]["command"]
            .as_array()
            .expect("a command")
            .iter()
            .map(|part| part.as_str().expect("a string").to_owned())
            .collect();
        assert_eq!(command[..2], ["/.substrate/substrate-executor", "run"]);
        let spec = RunSpec::decode_parts(&command[2..]).expect("the spec decodes");
        let cgroup = tempfile::tempdir().expect("a test cgroup root");
        std::fs::write(
            cgroup.path().join("memory.max"),
            spec.memory_bytes.to_string(),
        )
        .expect("memory.max");
        std::fs::write(
            cgroup.path().join("cpu.max"),
            format!("{} 100000", spec.cpu_millicores * 100),
        )
        .expect("cpu.max");
        std::fs::write(cgroup.path().join("pids.max"), "max").expect("pids.max");
        let root: PathBuf = cgroup.path().to_owned();
        let run = spec.clone();
        let (code, log) = tokio::task::spawn_blocking(move || {
            let mut log = Vec::new();
            let code = substrate_executor::run::run(&run, &root, &mut log).expect("the run");
            (code, log)
        })
        .await
        .expect("the executor thread");
        self.terminated(name, code, &log).await;
        spec
    }

    async fn deleted_grace(&self, name: &str) -> Option<u64> {
        let path = format!(
            "{}?watch=1&fieldSelector=metadata.name%3D{name}&resourceVersion=1&timeoutSeconds=1",
            self.admin.pods()
        );
        let mut watch = self.admin.watch(&path).await.ok()?;
        while let Ok(Some(event)) = watch.next_event().await {
            if event.kind == "DELETED" {
                return event.object["metadata"]["deletionGracePeriodSeconds"].as_u64();
            }
        }
        None
    }

    fn output(&self, id: &str, stream: OutputStream) -> Result<Vec<u8>, DriverError> {
        use base64::Engine as _;
        let slice = self.driver.exec_output(
            id,
            &ExecOutputQuery {
                stream,
                offset: 0,
                limit_bytes: 65_536,
            },
        )?;
        assert!(slice.eof, "a terminal exec's output is whole");
        Ok(base64::engine::general_purpose::STANDARD
            .decode(slice.content.data)
            .expect("base64"))
    }
}

fn frame(frame: &Frame) -> String {
    frame.encode().expect("encodes") + "\n"
}

fn applied_line(memory_bytes: u64, cpu_millicores: u64) -> String {
    frame(&Frame::Control(Control::Applied(Applied {
        memory_bytes: Some(memory_bytes),
        cpu_millicores: Some(cpu_millicores),
        pids_max: Some("max".to_owned()),
    })))
}

fn exit_line(code: i32) -> String {
    frame(&Frame::Control(Control::Exit(ExitRecord::Code(code))))
}

/// § 5 exec start: the Pod is created with the workspace PVC as owner and the keeper's node as
/// its only node, its uid is bound to the row, and the start answers running. The kubelet then
/// runs the real executor: the end comes from the API's exit code, the output byte for byte from
/// the log (`$`, spaces and non-ASCII untouched, the environment exactly the host rule's), and
/// the Pod is deleted under its uid.
#[tokio::test]
async fn a_start_creates_the_pod_binds_its_uid_and_reads_the_end_from_its_log() {
    let rig = rig().await;
    let id = "exec_0001";
    let script = "printf '%s|%s|%s' \"$1\" \"$GREETING\" \"$PATH\"; printf 'to err' >&2; exit 3";
    let start = input(&["/bin/sh", "-c", script, "sh", "$(HOME) $$ \u{fc} x"]);
    let observation = rig.started(id, &start).await;
    assert_eq!(observation.resource.state, ExecState::Running);
    assert_eq!(observation.resource.exit, None);
    let name = names::exec_pod(DEPLOYMENT, id);
    let pod = rig.pod(&name).expect("the Pod is stored");
    assert_eq!(
        rig.ledger.row(&name).uid.as_deref(),
        Some(uid(&pod).as_str())
    );
    assert_eq!(
        pod["metadata"]["ownerReferences"][0]["uid"],
        rig.pvc_uid.as_str()
    );
    assert_eq!(
        pod["spec"]["affinity"]["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]["nodeSelectorTerms"]
            [0]["matchExpressions"][0]["values"],
        json!([NODE])
    );
    assert_eq!(
        pod["metadata"]["annotations"][OPERATION_ANNOTATION],
        format!("sha256:{}", "ab".repeat(32))
    );
    assert!(pod["spec"]["containers"][0].get("env").is_none());

    let spec = rig.run_executor(&name).await;
    assert_eq!(
        spec.env,
        BTreeMap::from([
            ("GREETING".to_owned(), "$(HOME) $$".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ])
    );
    let ended = rig.driver.exec_observe(id).await.expect("observed");
    assert_eq!(ended.resource.state, ExecState::Exited);
    let exit = ended.resource.exit.clone().expect("an exit");
    assert_eq!((exit.code, exit.signal), (Some(3), None));
    assert!(ended.output_complete);
    assert_eq!(
        ended.stdout,
        "$(HOME) $$ \u{fc} x|$(HOME) $$|/usr/bin:/bin".as_bytes()
    );
    assert_eq!(
        rig.output(id, OutputStream::Stderr).expect("stderr"),
        b"to err"
    );
    assert!(
        rig.pod(&name).is_none(),
        "the Pod is deleted once its end is read"
    );
    assert_eq!(rig.driver.exec_completed(), vec![ended.clone()]);
    rig.driver.exec_acknowledge(&ended);
    assert!(rig.driver.exec_completed().is_empty());
}

/// § 7.3: the child's descriptors are pipes the executor frames, so a child that prints a frame
/// line, an exit record among them, only ever produces stdout bytes. The end is the API's.
#[tokio::test]
async fn a_frame_printed_by_the_child_is_output_never_a_record() {
    let rig = rig().await;
    let id = "exec_forge";
    let forged = "b10x1 c {\"exit\":{\"code\":0}}\nb10x1 o aGk=\n";
    let script = format!("printf '{}'; exit 7", forged.replace('\n', "\\n"));
    rig.started(id, &input(&["/bin/sh", "-c", &script])).await;
    rig.run_executor(&names::exec_pod(DEPLOYMENT, id)).await;
    let ended = rig.driver.exec_observe(id).await.expect("observed");
    assert_eq!(ended.resource.exit.and_then(|exit| exit.code), Some(7));
    assert_eq!(ended.stdout, forged.as_bytes());
}

/// § 7.3: a log line that is not a frame, or a log longer than the run's budget, is a protocol
/// failure: no output is reported and reading it is `kubernetes.executor-protocol`, while the end
/// still comes from the API.
#[tokio::test]
async fn a_non_frame_line_or_an_overlong_log_is_a_protocol_failure() {
    let rig = rig().await;
    let start = input(&["/bin/true"]);
    let interloper = [
        applied_line(67_108_864, 500),
        "hello from a sidecar\n".to_owned(),
        exit_line(0),
    ]
    .concat();
    let overlong = [
        applied_line(67_108_864, 500),
        "x".repeat(400_000),
        exit_line(0),
    ]
    .concat();
    for (id, log) in [("exec_line", interloper), ("exec_long", overlong)] {
        rig.started(id, &start).await;
        rig.terminated(&names::exec_pod(DEPLOYMENT, id), 0, log.as_bytes())
            .await;
        let ended = rig.driver.exec_observe(id).await.expect("observed");
        assert_eq!(ended.resource.state, ExecState::Exited, "{id}");
        assert_eq!(ended.resource.exit.and_then(|exit| exit.code), Some(0));
        assert!(!ended.output_complete, "{id}");
        assert!(ended.stdout.is_empty(), "{id}");
        let refused = rig
            .output(id, OutputStream::Stdout)
            .expect_err("no output is served");
        assert_eq!(refused.code, EXECUTOR_PROTOCOL, "{id}: {}", refused.message);
    }
}

/// § 7.3 run step 1: bounds the executor read back that differ leave the exec `unknown`, counted,
/// with no exit fabricated from the 125 the executor exits with.
#[tokio::test]
async fn limits_not_applied_leave_the_exec_unknown_without_an_exit() {
    let rig = rig().await;
    let id = "exec_limits";
    rig.started(id, &input(&["/bin/true"])).await;
    let log = [
        applied_line(1, 1),
        frame(&Frame::Control(Control::Refused(Refusal::LimitsUnapplied))),
    ]
    .concat();
    rig.terminated(&names::exec_pod(DEPLOYMENT, id), 125, log.as_bytes())
        .await;
    let ended = rig.driver.exec_observe(id).await.expect("observed");
    assert_eq!(ended.resource.state, ExecState::Unknown);
    assert_eq!(ended.resource.exit, None);
    assert_eq!(rig.driver.limits_unapplied_total(), 1);
}

/// § 6.3: a start naming another snapshot, or one admitted before the namespace's policy set
/// moved, is `exec.capability-stale` and creates no Pod.
#[tokio::test]
async fn a_foreign_snapshot_or_a_moved_binding_is_stale_before_any_pod() {
    let rig = rig().await;
    let mut foreign = input(&["/bin/true"]);
    foreign.sandbox.capability_snapshot = "sha256:another".to_owned();
    let DispatchOutcome::NotDispatched(stale) = rig.start("exec_foreign", &foreign).await else {
        panic!("a foreign snapshot was dispatched");
    };
    assert_eq!(stale.code, CAPABILITY_STALE);

    rig.fake
        .seed(
            "networkpolicies",
            json!({"metadata": {"name": "allow-all"}, "spec": {"podSelector": {}, "policyTypes": ["Egress"], "egress": [{}]}}),
        )
        .expect("an added policy is stored");
    let DispatchOutcome::NotDispatched(moved) =
        rig.start("exec_moved", &input(&["/bin/true"])).await
    else {
        panic!("a start over a moved binding was dispatched");
    };
    assert_eq!(moved.code, CAPABILITY_STALE, "{}", moved.message);
    assert_eq!(rig.pod_creates(), 0, "no Pod create was sent");
}

/// O7 and D184: a 403 `Forbidden` whose message says the namespace's quota is exceeded is
/// `exec.capacity` (exhausted, retriable); any other 403 is `kubernetes.api-failed`.
#[tokio::test]
async fn a_quota_refusal_is_exec_capacity_and_any_other_403_is_api_failed() {
    let rig = rig().await;
    let quota = |message: &str| Fault::Answer {
        method: Method::POST,
        path: "/api/v1/namespaces/substrate/pods".to_owned(),
        status: 403,
        body: json!({"kind": "Status", "apiVersion": "v1", "status": "Failure", "reason": "Forbidden", "code": 403, "message": message}),
    };
    let name = names::exec_pod(DEPLOYMENT, "exec_quota");
    rig.fake.fault(quota(&format!(
        "pods \"{name}\" is forbidden: exceeded quota: substrate-lane, requested: pods=1, used: pods=4, limited: pods=4"
    )));
    let DispatchOutcome::NotDispatched(capacity) =
        rig.start("exec_quota", &input(&["/bin/true"])).await
    else {
        panic!("a quota refusal was not a refusal");
    };
    assert_eq!(capacity.code, "exec.capacity");
    assert_eq!(capacity.class, DriverErrorClass::Exhausted);
    assert_eq!(capacity.address.as_deref(), Some("exec"));
    assert!(capacity.retriable);

    rig.fake.fault(quota(
        "pods \"sbx-ex-x\" is forbidden: User \"driver\" cannot create resource \"pods\"",
    ));
    let DispatchOutcome::NotDispatched(other) =
        rig.start("exec_denied", &input(&["/bin/true"])).await
    else {
        panic!("a 403 was not a refusal");
    };
    assert_eq!(other.code, API_FAILED);
    assert_eq!(other.class, DriverErrorClass::Failed);
}

/// § 4: a name held by an object that is not ours is a retriable failure; the holder is never
/// modified, and no uid is bound to the row.
#[tokio::test]
async fn a_held_name_is_a_retriable_failure_and_its_holder_is_untouched() {
    let rig = rig().await;
    let name = names::exec_pod(DEPLOYMENT, "exec_held");
    let holder = rig
        .fake
        .seed("pods", json!({"metadata": {"name": name}, "spec": {}}))
        .expect("a foreign Pod holds the name");
    let DispatchOutcome::NotDispatched(held) = rig.start("exec_held", &input(&["/bin/true"])).await
    else {
        panic!("a held name was dispatched");
    };
    assert_eq!(held.class, DriverErrorClass::Failed);
    assert!(held.retriable);
    assert_eq!(rig.pod(&name), Some(holder), "the holder is untouched");
    assert_eq!(rig.ledger.row(&name).uid, None);
}

/// § 4: a create whose answer was lost after the API stored the Pod is adopted by its operation
/// annotation, and the start is observed.
#[tokio::test]
async fn a_lost_create_answer_is_adopted_by_its_annotation() {
    let rig = rig().await;
    rig.fake.fault(Fault::DropAnswer {
        method: Method::POST,
        path: "/api/v1/namespaces/substrate/pods".to_owned(),
    });
    let observation = rig.started("exec_lost", &input(&["/bin/true"])).await;
    assert_eq!(observation.resource.state, ExecState::Running);
    let name = names::exec_pod(DEPLOYMENT, "exec_lost");
    let pod = rig.pod(&name).expect("the Pod was stored");
    assert_eq!(rig.ledger.row(&name).uid, Some(uid(&pod)));
}

/// § 5 signal: `term` deletes with grace 5 s and `kill` with 1 s; `int` is `exec.signal-unserved`.
/// The end is committed once shown: here the Pod was never bound to a node, so it stopped.
#[tokio::test]
async fn term_and_kill_delete_with_their_grace_and_int_is_unserved() {
    let rig = rig().await;
    for (id, signal, grace) in [
        ("exec_term", Signal::Term, 5),
        ("exec_kill", Signal::Kill, 1),
    ] {
        rig.started(id, &input(&["/bin/sleep", "60"])).await;
        rig.driver
            .exec_signal(
                id,
                &ExecSignalInput {
                    signal,
                    grace_ms: 0,
                },
            )
            .await
            .expect("signalled");
        let name = names::exec_pod(DEPLOYMENT, id);
        assert_eq!(rig.deleted_grace(&name).await, Some(grace), "{id}");
        let ended = rig.driver.exec_observe(id).await.expect("observed");
        assert_eq!(ended.resource.state, ExecState::Cancelled, "{id}");
        assert_eq!(ended.resource.exit, None, "{id}: no exit is fabricated");
    }
    rig.started("exec_int", &input(&["/bin/sleep", "60"])).await;
    let refused = rig
        .driver
        .exec_signal(
            "exec_int",
            &ExecSignalInput {
                signal: Signal::Int,
                grace_ms: 0,
            },
        )
        .await
        .expect_err("int is not served");
    assert_eq!(refused.code, substrate_wire::EXEC_SIGNAL_UNSERVED);
    assert_eq!(refused.class, DriverErrorClass::Unserved);
    assert_eq!(refused.address.as_deref(), Some("signal"));
    assert!(rig.pod(&names::exec_pod(DEPLOYMENT, "exec_int")).is_some());
}

/// § 4: a signal deletes only under the bound uid, so a same-named replacement, even one that
/// carries our labels and annotation, is never deleted.
#[tokio::test]
async fn a_signal_never_deletes_a_same_named_replacement() {
    let rig = rig().await;
    let id = "exec_swap";
    rig.started(id, &input(&["/bin/sleep", "60"])).await;
    let name = names::exec_pod(DEPLOYMENT, id);
    let mut replacement = rig.pod(&name).expect("present");
    rig.admin
        .delete(&format!("{}/{name}", rig.admin.pods()), 0)
        .await
        .expect("an operator deletes it");
    for member in ["uid", "resourceVersion", "creationTimestamp"] {
        replacement["metadata"]
            .as_object_mut()
            .expect("metadata")
            .remove(member);
    }
    let replaced = rig.fake.seed("pods", replacement).expect("replaced");
    rig.driver
        .exec_signal(
            id,
            &ExecSignalInput {
                signal: Signal::Kill,
                grace_ms: 0,
            },
        )
        .await
        .expect("the signal is answered");
    assert_eq!(
        rig.pod(&name),
        Some(replaced),
        "the replacement is untouched"
    );
}

/// Design 28's gone-Pod rule: a Pod bound to a node and force-deleted while running shows no
/// terminated container, so the exec is `unknown` with no exit.
#[tokio::test]
async fn a_force_deleted_running_pod_leaves_the_exec_unknown() {
    let rig = rig().await;
    let id = "exec_forced";
    rig.started(id, &input(&["/bin/sleep", "60"])).await;
    let name = names::exec_pod(DEPLOYMENT, id);
    rig.running(&name).await;
    let running = rig.driver.exec_observe(id).await.expect("observed");
    assert_eq!(running.resource.state, ExecState::Running);
    rig.admin
        .delete(&format!("{}/{name}", rig.admin.pods()), 0)
        .await
        .expect("force-deleted");
    let gone = rig.driver.exec_observe(id).await.expect("observed");
    assert_eq!(gone.resource.state, ExecState::Unknown);
    assert_eq!(gone.resource.exit, None);
}

/// § 5.2: a watch round ends an exec from its events; after the history is compacted, the next
/// round's 410 relists and re-reads every non-terminal exec by name, which ends the second.
#[tokio::test]
async fn a_watch_round_ends_execs_and_a_410_relists_and_rereads_by_name() {
    let rig = rig().await;
    let start = input(&["/bin/true"]);
    rig.started("exec_w1", &start).await;
    rig.started("exec_w2", &start).await;
    let log = [applied_line(67_108_864, 500), exit_line(0)].concat();
    let round = Duration::from_secs(1);
    rig.driver
        .exec_watch_round(round)
        .await
        .expect("the first round lists");

    rig.terminated(&names::exec_pod(DEPLOYMENT, "exec_w1"), 0, log.as_bytes())
        .await;
    rig.driver
        .exec_watch_round(round)
        .await
        .expect("a watch round");
    let states: Vec<ExecState> = rig
        .driver
        .exec_completed()
        .iter()
        .map(|observation| observation.resource.state)
        .collect();
    assert_eq!(
        states,
        vec![ExecState::Exited],
        "the event ended the first exec"
    );

    rig.terminated(&names::exec_pod(DEPLOYMENT, "exec_w2"), 0, log.as_bytes())
        .await;
    rig.fake.compact();
    let lists_before = list_requests(&rig);
    rig.driver
        .exec_watch_round(round)
        .await
        .expect("a 410 relists");
    assert!(list_requests(&rig) > lists_before, "the 410 relisted");
    assert_eq!(rig.driver.exec_completed().len(), 2, "re-read by name");
}

fn list_requests(rig: &Rig) -> usize {
    rig.fake
        .requests()
        .iter()
        .filter(|request| {
            request.starts_with("GET /api/v1/namespaces/substrate/pods?")
                && request.contains("labelSelector")
                && !request.contains("watch=1")
        })
        .count()
}

/// § 5 garbage: a Pod whose deletion failed when its end was read is deleted by the next round,
/// and its row is settled once it reads absent; a running exec's Pod and row are left alone.
#[tokio::test]
async fn a_garbage_round_settles_the_rows_of_ended_execs_once_their_pods_are_absent() {
    let rig = rig().await;
    rig.started("exec_g1", &input(&["/bin/true"])).await;
    rig.started("exec_g2", &input(&["/bin/sleep", "60"])).await;
    let ended = names::exec_pod(DEPLOYMENT, "exec_g1");
    let running = names::exec_pod(DEPLOYMENT, "exec_g2");
    let log = [applied_line(67_108_864, 500), exit_line(0)].concat();
    rig.terminated(&ended, 0, log.as_bytes()).await;
    rig.fake.fault(Fault::Answer {
        method: Method::DELETE,
        path: format!("/api/v1/namespaces/substrate/pods/{ended}"),
        status: 500,
        body: json!({"kind": "Status", "status": "Failure", "code": 500, "message": "etcdserver: request timed out"}),
    });
    let observed = rig.driver.exec_observe("exec_g1").await.expect("observed");
    assert_eq!(observed.resource.state, ExecState::Exited);
    assert!(
        rig.pod(&ended).is_some(),
        "the failed deletion left the Pod"
    );

    rig.driver.exec_garbage_round().await.expect("a round");
    assert!(rig.pod(&ended).is_none(), "the round deleted it");
    assert!(
        rig.ledger.row(&ended).settled_at.is_some(),
        "and settled its row"
    );
    assert!(rig.pod(&running).is_some());
    assert!(rig.ledger.row(&running).settled_at.is_none());
}

/// § 8: every start member this driver does not serve is refused by name before any request,
/// as are bounds a Pod cannot hold and an environment outside the host rule.
#[tokio::test]
async fn members_this_driver_does_not_serve_are_refused_before_any_request() {
    type Change = fn(&mut ExecStartInput);
    let rig = rig().await;
    let base = input(&["/bin/true"]);
    let cases: Vec<(&str, Change)> = vec![
        ("exec.egress-apertures-unserved", |i| {
            i.sandbox.network = NetworkMode::Aperture;
            i.sandbox.aperture = Some("registry".to_owned());
        }),
        ("exec.egress-apertures-unserved", |i| {
            i.sandbox.apertures = vec!["a".to_owned(), "b".to_owned()];
        }),
        ("exec.secret-slots-unserved", |i| {
            i.secret_slots = vec![SecretSlotRequest {
                slot: "key".to_owned(),
                fd: 3,
            }];
        }),
        (substrate_wire::READ_ONLY_FILES_UNSERVED, |i| {
            i.read_only_files = vec![ReadOnlyFile {
                host_path: "/etc/hosts".to_owned(),
                mount: "/etc/hosts".to_owned(),
            }];
        }),
        (substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED, |i| {
            i.writable_roots = vec![WritableRoot::Host(WritableHostRoot {
                host: "cache".to_owned(),
                mount: "/cache".to_owned(),
            })];
        }),
        (substrate_wire::EXEC_WRITABLE_ROOTS_GIT_UNSERVED, |i| {
            i.writable_roots = vec![WritableRoot::Git(WritableGitRoot {
                workspace: "ws_0002".to_owned(),
                mount: "/src".to_owned(),
            })];
        }),
        (substrate_wire::EXEC_WORKDIR_UNSERVED, |i| {
            i.workdir = Some("/workspace/sub".to_owned());
        }),
        ("exec.sandbox-unavailable", |i| {
            i.read_only_roots = vec![ReadOnlyRoot {
                host_path: "/opt/tools".to_owned(),
                mount: "/opt/tools".to_owned(),
            }];
        }),
        (substrate_wire::EXEC_METRICS_UNSERVED, |i| {
            i.measurements = BTreeSet::from([ExecMeasurement::ResourceUsage]);
        }),
        (substrate_wire::EXEC_SCRATCH_QUOTA_UNSERVED, |i| {
            i.scratch = Some(StorageLimit {
                max_bytes: 1_048_576,
                max_inodes: 64,
            });
        }),
        (substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED, |i| {
            i.workspace_access = WorkspaceAccess::Scoped {
                writable_subtrees: vec!["out".to_owned()],
            };
        }),
        (substrate_wire::EXEC_INLINE_CAPSULE_UNSERVED, |i| {
            i.capsule = Some(ExecutionCapsuleInput {
                manifest_sha256: "0".repeat(64),
                entrypoint: "main".to_owned(),
                files: Vec::new(),
            });
        }),
        ("exec.limit-unserved", |i| i.limits.processes = 512),
        ("exec.limit-unserved", |i| i.limits.output_bytes = 2_000_000),
        ("exec.limit-unserved", |i| {
            i.limits.cpu_millis = 1;
        }),
        ("exec.environment-refused", |i| {
            i.env.set.insert("API_TOKEN".to_owned(), "x".to_owned());
        }),
    ];
    for (index, (code, change)) in cases.into_iter().enumerate() {
        let mut changed = base.clone();
        change(&mut changed);
        let id = format!("exec_unserved_{index}");
        let DispatchOutcome::NotDispatched(refused) = rig.start(&id, &changed).await else {
            panic!("{code}: dispatched");
        };
        assert_eq!(refused.code, code, "case {index}: {}", refused.message);
    }
    assert_eq!(rig.pod_creates(), 0, "no Pod create was sent");
}

/// A start with `wait` answers once the run has ended, read from the API and the log.
#[tokio::test]
async fn a_waiting_start_answers_the_end() {
    let rig = rig().await;
    let id = "exec_wait";
    let mut start = input(&["/bin/true"]);
    start.wait = true;
    let name = names::exec_pod(DEPLOYMENT, id);
    let log = [applied_line(67_108_864, 500), exit_line(0)].concat();
    let kubelet = async {
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.pod(&name).is_none() {
            if Instant::now() > deadline {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        rig.terminated(&name, 0, log.as_bytes()).await;
    };
    let (outcome, ()) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(Box::pin(rig.start(id, &start)), kubelet)
    })
    .await
    .expect("the waiting start answers within its bound");
    let DispatchOutcome::Observed(ended) = outcome else {
        panic!("the waiting start was not observed");
    };
    assert_eq!(ended.resource.state, ExecState::Exited);
}
