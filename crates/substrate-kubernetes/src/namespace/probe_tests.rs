//! The probe and the snapshot on the fake API server (design 34 § 6.2, § 6.3, § 12 U10). The
//! fake has no kubelet: a case plays it, binding the probe keeper to a node and writing the probe
//! Pod's log as the executor frames it, with the `probe-child` report of the argv the Pod carries
//! (or, in the negative case, of an argv something expanded). The keeper's `fs` calls run the real
//! executor against a temporary directory. Every case runs once, in seconds.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use substrate_executor::Handshake;
use substrate_executor::frame::{Applied, Control, ExitRecord, Frame, Stream};
use substrate_executor::serve::{open_workspace, serve};
use substrate_executor::spec::RunSpec;
use substrate_host::{BackendKind, BackendRole, Driver, DriverErrorClass};
use substrate_kubernetes_client::fake::{ExecAnswer, ExecRequest, FakeCluster, Fault};
use substrate_kubernetes_client::{Api, ListForm, Method};
use substrate_wire::{CapabilitySnapshot, HostDriverKind, Signal};

use super::NamespaceDriver;
use crate::driver_tests::{
    DEPLOYMENT, ManualClock, MemoryLedger, admin, driver, driver_over, passing_fake,
};
use crate::lease::HELD_FOR;
use crate::names::{self, DEPLOYMENT_LABEL, MANAGED_BY, MANAGED_BY_LABEL, OPERATION_ANNOTATION};
use crate::{EXECUTOR_HANDSHAKE_FAILED, pod};

const NODE: &str = "node-a";
const PROBE_LABEL: &str = "b10x-substrate/probe";
const INTERVAL: Duration = Duration::from_mins(10);
const SLACK: Duration = Duration::from_mins(1);

/// What the played kubelet does with the probe Pod's argv before `probe-child` reports it.
#[derive(Clone, Copy)]
enum Child {
    /// Reports exactly the argv the Pod's spec carries.
    Exact,
    /// Reports the argv as a shell would have left it: `$(HOME)` expanded.
    Expanded,
}

struct Rig {
    fake: Arc<FakeCluster>,
    clock: Arc<ManualClock>,
    driver: NamespaceDriver,
    ledger: Arc<MemoryLedger>,
    /// The argv of every probe Pod the kubelet ran, in order.
    ran: Arc<Mutex<Vec<Vec<String>>>>,
    _root: tempfile::TempDir,
    kubelet: tokio::task::JoinHandle<()>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.kubelet.abort();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An open driver over a passing fake whose keeper `pods/exec` runs the real executor's `fs` role
/// and handshake (or `handshake`, when a case states another line), and whose kubelet plays
/// `child`.
async fn rig(child: Child, handshake: Option<String>) -> Rig {
    let fake = Arc::new(passing_fake());
    let clock = ManualClock::new();
    let (driver, ledger) = driver(&fake, &clock);
    driver.open().await.expect("the driver opens");
    let root = tempfile::tempdir().expect("a temporary PVC");
    let directory = root.path().to_path_buf();
    fake.set_exec(move |request: ExecRequest| {
        let command: Vec<&str> = request.command.iter().map(String::as_str).collect();
        match command.as_slice() {
            ["/substrate-executor", "version"] => {
                let line = handshake
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
                    open_workspace(&directory)
                })
                .expect("the executor writes its answer");
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
    });
    let ran = Arc::new(Mutex::new(Vec::new()));
    let kubelet = tokio::spawn(play_kubelet(
        admin(&fake),
        Arc::clone(&fake),
        child,
        Arc::clone(&ran),
    ));
    Rig {
        fake,
        clock,
        driver,
        ledger,
        ran,
        _root: root,
        kubelet,
    }
}

/// Binds every probe keeper to `NODE` and ends every probe Pod with the log `child` makes.
async fn play_kubelet(
    admin: Api,
    fake: Arc<FakeCluster>,
    child: Child,
    ran: Arc<Mutex<Vec<Vec<String>>>>,
) {
    loop {
        let listed = admin
            .list(&admin.pods(), "PodList", ListForm::Whole, 100, 4)
            .await
            .map(|listed| listed.items)
            .unwrap_or_default();
        for mut pod in listed {
            if pod["metadata"]["labels"][PROBE_LABEL].is_null()
                || !pod["status"]["containerStatuses"].is_null()
            {
                continue;
            }
            let name = pod["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let path = format!("{}/{name}", admin.pods());
            match pod["metadata"]["labels"]["b10x-substrate/role"].as_str() {
                Some("keeper") if pod["spec"]["nodeName"].is_null() => {
                    pod["spec"]["nodeName"] = json!(NODE);
                    let _ = admin.update(&path, &pod).await;
                }
                Some("pod") => {
                    let spec = run_spec(&pod);
                    lock(&ran).push(spec.argv.clone());
                    fake.set_log(&name, &probe_log(&spec, child));
                    pod["status"] = json!({
                        "phase": "Succeeded",
                        "containerStatuses": [{"name": "run", "state": {"terminated": {"exitCode": 0, "reason": "Completed"}}}],
                    });
                    let _ = admin.update(&path, &pod).await;
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn run_spec(pod: &Value) -> RunSpec {
    let command: Vec<String> = pod["spec"]["containers"][0]["command"]
        .as_array()
        .expect("a command")
        .iter()
        .map(|part| part.as_str().expect("a string").to_owned())
        .collect();
    assert_eq!(command[..2], ["/.substrate/substrate-executor", "run"]);
    RunSpec::decode_parts(&command[2..]).expect("the spec decodes")
}

/// The log the executor writes for `probe-child` under `spec`: its bounds read back as applied,
/// the child's report, and exit 0.
fn probe_log(spec: &RunSpec, child: Child) -> Vec<u8> {
    let mut argv = spec.argv.clone();
    if let Child::Expanded = child {
        for part in &mut argv {
            *part = part.replace("$(HOME)", "/var/empty");
        }
    }
    let frames = [
        Frame::Control(Control::Applied(Applied {
            memory_bytes: Some(spec.memory_bytes),
            cpu_millicores: Some(spec.cpu_millicores),
            pids_max: Some("max".to_owned()),
        })),
        Frame::Output {
            stream: Stream::Stdout,
            bytes: substrate_executor::probe::report(&argv).into_bytes(),
        },
        Frame::Control(Control::Exit(ExitRecord::Code(0))),
    ];
    let mut log = Vec::new();
    for frame in &frames {
        log.extend_from_slice(frame.encode().expect("a frame encodes").as_bytes());
        log.push(b'\n');
    }
    log
}

impl Rig {
    fn machine(&self) -> CapabilitySnapshot {
        Driver::machine(&self.driver)
    }

    fn probe_names(&self) -> Vec<String> {
        self.ledger
            .rows_of(BackendKind::Probe)
            .into_iter()
            .map(|row| row.name)
            .collect()
    }

    fn present(&self, name: &str) -> bool {
        self.fake.object("pods", name).is_some()
            || self.fake.object("persistentvolumeclaims", name).is_some()
    }
}

fn assert_never_published(snapshot: &CapabilitySnapshot) {
    let facts = &snapshot.facts;
    assert_eq!(snapshot.driver, HostDriverKind::Kubernetes);
    assert_eq!(facts.confinement_no_egress, None, "O3 did not hold");
    assert_eq!(facts.exec_namespaces, None);
    assert_eq!(facts.exec_cgroup_limits, None);
    assert_eq!(facts.exec_cgroup_kill, None);
    assert_eq!(facts.exec_no_egress, None);
    assert_eq!(facts.exec_inline_capsule, None);
    assert_eq!(facts.exec_workspace_scoped_write, None);
    assert_eq!(facts.workspace_storage_quota, None);
    assert_eq!(facts.process_host, None);
}

fn assert_daemon_facts(snapshot: &CapabilitySnapshot) {
    let facts = &snapshot.facts;
    assert_eq!(facts.events_pull, Some(true));
    assert_eq!(facts.events_stream, Some(true));
    assert_eq!(facts.events_retention_events, Some(10_000));
    assert_eq!(
        facts.snapshot_provenance_events,
        Some(substrate_wire::MAX_SNAPSHOT_PROVENANCE_EVENTS)
    );
    assert_eq!(
        facts.events_stream_bounds,
        Some(substrate_wire::StreamBoundsFacts::events())
    );
}

fn assert_no_driver_facts(snapshot: &CapabilitySnapshot) {
    let facts = &snapshot.facts;
    assert_eq!(facts.workspace_guarded_io, None);
    assert_eq!(facts.workspace_openat2_beneath, None);
    assert_eq!(facts.workspace_atomic_replace, None);
    assert_eq!(facts.workspace_max_file_bytes, None);
    assert_eq!(facts.exec_argv_only, None);
    assert_eq!(facts.workspace_max_current, None);
    assert_eq!(facts.exec_max_current, None);
    assert_eq!(facts.exec_signals, None);
    assert_eq!(facts.exec_output_limit_bytes, None);
    assert_eq!(snapshot.valid_until, None);
}

#[tokio::test]
async fn a_probe_publishes_what_it_observed_and_never_a_linux_or_egress_fact() {
    let rig = rig(Child::Exact, None).await;
    let before = rig.machine();
    assert_never_published(&before);
    assert_daemon_facts(&before);
    assert_eq!(before.facts.exec_argv_only, None);

    rig.driver.probe().await.expect("the probe runs");

    let snapshot = rig.machine();
    let facts = &snapshot.facts;
    assert_never_published(&snapshot);
    assert_daemon_facts(&snapshot);
    assert_eq!(facts.workspace_guarded_io, Some(true));
    assert_eq!(facts.workspace_openat2_beneath, Some(true));
    assert_eq!(facts.workspace_atomic_replace, Some(true));
    assert_eq!(facts.exec_argv_only, Some(true));
    assert_eq!(
        facts.workspace_max_file_bytes,
        Some(substrate_wire::MAX_FILE_BYTES)
    );
    assert_eq!(
        facts.workspace_read_limit_bytes,
        Some(substrate_wire::MAX_IO_BYTES)
    );
    assert_eq!(
        facts.workspace_list_limit_items,
        Some(substrate_wire::MAX_LIST_ITEMS)
    );
    assert_eq!(facts.workspace_max_current, Some(64));
    assert_eq!(facts.exec_max_current, Some(32));
    assert_eq!(facts.exec_signals, Some(vec![Signal::Term, Signal::Kill]));
    assert_eq!(
        facts.exec_output_limit_bytes,
        Some(pod::output_limit_bytes(10_485_760))
    );
    assert_eq!(
        snapshot.valid_until,
        Some(snapshot.probed_at + INTERVAL + SLACK),
        "valid until the next run plus 60 s"
    );
    assert_ne!(snapshot.snapshot, before.snapshot);
    assert_eq!(
        rig.machine().snapshot,
        snapshot.snapshot,
        "the digest is stable"
    );

    // The Pod ran the probe child with the argv no shell or expansion may change.
    let ran = lock(&rig.ran).clone();
    assert_eq!(
        ran,
        [vec![
            "/.substrate/substrate-executor",
            "probe-child",
            "$(HOME)",
            "$$",
            "a b",
            "\u{e9}",
        ]]
    );
    // Its three objects are gone and every row is settled.
    let rows = rig.ledger.rows_of(BackendKind::Probe);
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert!(row.settled_at.is_some(), "{} is settled", row.name);
        assert!(row.uid.is_some(), "{} was created and bound", row.name);
        assert!(!rig.present(&row.name), "{} is gone", row.name);
    }
}

#[tokio::test]
async fn an_argv_that_does_not_read_back_byte_for_byte_leaves_argv_only_absent() {
    let rig = rig(Child::Expanded, None).await;
    rig.driver.probe().await.expect("the probe runs");
    let snapshot = rig.machine();
    assert_eq!(snapshot.facts.exec_argv_only, None);
    assert_eq!(snapshot.facts.workspace_guarded_io, Some(true));
    assert_never_published(&snapshot);
    for name in rig.probe_names() {
        assert!(!rig.present(&name), "{name} is gone");
    }
}

#[tokio::test]
async fn an_executor_of_another_release_fails_the_probe_and_proves_nothing() {
    let other = r#"{"protocol":"substrate-executor/0","version":"0.0.0","source":"x"}"#;
    let rig = rig(Child::Exact, Some(other.to_owned())).await;
    let refused = rig.driver.probe().await.expect_err("the handshake fails");
    assert_eq!(refused.code, EXECUTOR_HANDSHAKE_FAILED);
    let snapshot = rig.machine();
    assert_no_driver_facts(&snapshot);
    assert_daemon_facts(&snapshot);
    assert!(lock(&rig.ran).is_empty(), "no probe Pod was created");
    for row in rig.ledger.rows_of(BackendKind::Probe) {
        assert!(row.settled_at.is_some(), "{} is settled", row.name);
        assert!(!rig.present(&row.name), "{} is gone", row.name);
    }
}

#[tokio::test]
async fn a_driver_that_is_not_open_publishes_only_the_daemon_side_facts() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, _ledger) = driver(&fake, &clock);
    let snapshot = Driver::machine(&driver);
    assert_eq!(snapshot.config_generation, 0);
    assert_no_driver_facts(&snapshot);
    assert_never_published(&snapshot);
    assert_daemon_facts(&snapshot);
    assert!(
        !driver.probe_due(),
        "nothing is probed before the driver opens"
    );
    assert!(driver.probe().await.is_err());
}

#[tokio::test]
async fn proofs_lapse_with_the_lease_and_after_valid_until_and_a_retaken_lease_probes_again() {
    let rig = rig(Child::Exact, None).await;
    assert!(rig.driver.probe_due());
    rig.driver.probe().await.expect("the probe runs");
    assert!(!rig.driver.probe_due(), "the repeat waits for the interval");
    assert_eq!(rig.machine().facts.exec_argv_only, Some(true));

    // The Lease lapses: every driver fact is absent, the daemon's are not.
    rig.clock.set(HELD_FOR + Duration::from_secs(1));
    let lapsed = rig.machine();
    assert_no_driver_facts(&lapsed);
    assert_daemon_facts(&lapsed);
    assert!(
        !rig.driver.probe_due(),
        "nothing is probed without the Lease"
    );

    // Retaking it runs the open sequence again and forgets the proof (§ 6.1).
    rig.driver.lease_round().await;
    assert!(rig.driver.lease_held());
    assert_eq!(rig.machine().facts.exec_argv_only, None);
    assert!(rig.driver.probe_due());

    // A new proof lapses at `valid_until` even with the Lease held.
    rig.driver.probe().await.expect("the probe runs again");
    assert_eq!(lock(&rig.ran).len(), 2);
    assert_eq!(rig.machine().facts.exec_argv_only, Some(true));
    rig.clock
        .set(HELD_FOR + Duration::from_secs(1) + INTERVAL + SLACK);
    rig.driver.renew().await;
    assert!(rig.driver.lease_held());
    let late = rig.machine();
    assert_eq!(late.facts.exec_argv_only, None);
    assert_eq!(late.facts.workspace_guarded_io, None);
    assert_eq!(
        late.facts.exec_max_current,
        Some(32),
        "declared bounds stay"
    );
    assert!(rig.driver.probe_due(), "the repeat is due");
}

#[tokio::test]
async fn objects_an_earlier_probe_left_are_removed_before_the_next() {
    let rig = rig(Child::Exact, None).await;
    let nonce = "leftoverleftover";
    let digest = crate::policy::hex(&<sha2::Sha256 as sha2::Digest>::digest(nonce.as_bytes()));
    let mut left = Vec::new();
    for name in names::probe_names(nonce) {
        rig.ledger
            .reserve(BackendKind::Probe, nonce, name.role, &name.name, &digest);
        let (resource, object) = if name.role == BackendRole::Pvc {
            ("persistentvolumeclaims", json!({"spec": {}}))
        } else {
            ("pods", json!({"spec": {"restartPolicy": "Never"}}))
        };
        let mut object = object;
        object["metadata"] = json!({
            "name": name.name,
            "labels": {
                MANAGED_BY_LABEL: MANAGED_BY,
                DEPLOYMENT_LABEL: names::deployment_label(DEPLOYMENT),
                "b10x-substrate/role": names::role_name(name.role),
            },
            "annotations": {OPERATION_ANNOTATION: names::operation_annotation(&digest)},
        });
        rig.fake
            .seed(resource, object)
            .expect("the leftover is stored");
        left.push(name.name);
    }
    rig.driver.probe().await.expect("the probe runs");
    for name in &left {
        assert!(!rig.present(name), "{name} is gone");
        assert!(
            rig.ledger.row(name).settled_at.is_some(),
            "{name}'s row is settled"
        );
    }
    assert_eq!(lock(&rig.ran).len(), 1, "the leftover Pod was never run");
}

/// § 5's probe row across a restart (§ 12 U11): a process whose probe could not delete its
/// objects stops, its rows unsettled and bound. The process that takes the namespace over, over
/// the same ledger, removes them under the uid rule before its own probe creates anything.
#[tokio::test]
async fn probe_objects_a_stopped_process_left_are_removed_by_the_next_process() {
    let mut rig = rig(Child::Exact, None).await;
    let failing = |path: &str| Fault::Answer {
        method: Method::DELETE,
        path: format!("/api/v1/namespaces/substrate/{path}/sbx-pr-"),
        status: 500,
        body: json!({"kind": "Status", "apiVersion": "v1", "status": "Failure", "code": 500, "message": "etcdserver: request timed out"}),
    };
    for path in ["pods", "pods", "persistentvolumeclaims"] {
        rig.fake.fault(failing(path));
    }
    rig.driver.probe().await.expect("the first process probes");
    let left: Vec<_> = rig
        .ledger
        .rows_of(BackendKind::Probe)
        .into_iter()
        .filter(|row| row.settled_at.is_none())
        .collect();
    assert_eq!(left.len(), 3, "{left:?}");
    for row in &left {
        assert!(row.uid.is_some(), "{} is bound", row.name);
        assert!(rig.present(&row.name), "{} was left", row.name);
    }

    let reopened = driver_over(&rig.fake, &rig.clock, &rig.ledger, DEPLOYMENT);
    let held = reopened
        .open()
        .await
        .expect_err("the stopped process's Lease is not yet expired");
    assert_eq!(held.code, crate::LEASE_HELD, "{}", held.message);
    rig.clock.set(Duration::from_secs(30));
    reopened.open().await.expect("the Lease is taken over");
    drop(std::mem::replace(&mut rig.driver, reopened));

    rig.driver.probe().await.expect("the next process probes");
    for row in &left {
        assert!(!rig.present(&row.name), "{} is gone", row.name);
        assert!(
            rig.ledger.row(&row.name).settled_at.is_some(),
            "{}'s row is settled",
            row.name
        );
    }
    assert!(
        rig.ledger
            .rows_of(BackendKind::Probe)
            .iter()
            .all(|row| row.settled_at.is_some()),
        "the next process's own probe left nothing"
    );
    assert_eq!(lock(&rig.ran).len(), 2);
}

#[tokio::test]
async fn adopt_is_unserved_before_any_name_is_looked_at() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (driver, _ledger) = driver(&fake, &clock);
    let adopted: substrate_wire::Workspace = serde_json::from_value(json!({
        "id": "ws_adopt", "kind": "workspace", "labels": {},
        "observed_at": "2026-10-29T10:00:00Z", "state": "ready", "origin": "adopted",
        "root_name": "operator-dir",
    }))
    .expect("an adopted provisional workspace");
    let refused = Driver::observe_workspace(&driver, "ws_adopt", "operator-dir", &adopted)
        .await
        .expect_err("adopt is unserved");
    assert_eq!(refused.code, substrate_wire::WORKSPACE_ADOPT_UNSERVED);
    assert_eq!(refused.class, DriverErrorClass::Unserved);
    assert_eq!(refused.address, None);
    assert!(!refused.retriable);
    assert!(
        fake.requests().is_empty(),
        "nothing was asked of the cluster"
    );
}
