//! The probe and the snapshot on the fake API server (design 34 § 6.2, § 6.3, § 12 U10). The
//! fake has no kubelet: a case plays it, binding the probe keeper to a node and writing the probe
//! Pod's log as the executor frames it, with the `probe-child` report of the argv the Pod carries
//! (or, in the negative case, of an argv something expanded). The keeper's `fs` calls run the real
//! executor against a temporary directory. Every case runs once, in seconds.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use substrate_executor::Handshake;
use substrate_executor::frame::{Applied, Control, ExitRecord, Frame, Refusal, Stream};
use substrate_executor::message::{Request, Response, read_request};
use substrate_executor::probe::{
    EgressAttempt, EgressOutcome, EgressTarget, HEARTBEAT_FILE, Observation, ProbeTargets,
    TARGETS_ENV,
};
use substrate_executor::serve::{open_workspace, serve};
use substrate_executor::spec::RunSpec;
use substrate_host::{BackendKind, BackendRole, Driver, DriverErrorClass};
use substrate_kubernetes_client::fake::{ExecAnswer, ExecRequest, FakeCluster, Fault};
use substrate_kubernetes_client::{Api, ListForm, Method};
use substrate_wire::{
    CapabilityFacts, CapabilitySnapshot, ConfinementFamily, ConfinementProperty, HostDriverKind,
    Signal,
};

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

/// How the played kubelet runs the probe Pod: its child's argv line, the observation line that
/// follows it, what becomes of the heartbeat file in the probe's volume, and how the run ends.
#[derive(Clone)]
struct Play {
    child: Child,
    observed: Option<String>,
    heartbeat: Heartbeat,
    end: End,
}

impl Play {
    /// The default run: the argv read back, no observation line, no heartbeat, exit 0.
    const fn of(child: Child) -> Self {
        Self {
            child,
            observed: None,
            heartbeat: Heartbeat::Absent,
            end: End::Exit,
        }
    }

    fn observing(observation: &Observation, heartbeat: Heartbeat) -> Self {
        Self {
            observed: Some(observation.line()),
            heartbeat,
            ..Self::of(Child::Exact)
        }
    }
}

/// The heartbeat file the probe's targets name, as the played kubelet leaves it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Heartbeat {
    /// Never written: the keeper's `stat` finds nothing.
    Absent,
    /// Written before the container shows `terminated`, never again.
    Stopped,
    /// Written every 100 ms, before and after the container shows `terminated`.
    Moving,
}

/// How the played probe Pod ends.
#[derive(Clone, Copy, Debug)]
enum End {
    /// The executor's frames, its exit record agreeing with the API's exit code 0.
    Exit,
    /// The executor read other bounds back and refused the run, exit 125.
    LimitsUnapplied,
    /// A line that is not a frame follows the applied record.
    NotAFrame,
    /// The exit record states 0 while the API shows exit code 1.
    CodeDisagrees,
    /// The Pod is deleted before its container shows an end.
    Gone,
}

struct Rig {
    fake: Arc<FakeCluster>,
    clock: Arc<ManualClock>,
    driver: NamespaceDriver,
    ledger: Arc<MemoryLedger>,
    /// Every probe Pod the kubelet ran, as the driver created it, in order.
    ran: Arc<Mutex<Vec<Value>>>,
    root: tempfile::TempDir,
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
    rig_with(Play::of(child), handshake, None).await
}

/// [`rig`] whose keeper answers `check` with `check` instead of running the executor's proofs,
/// as an executor that proves less, or answers in an older shape, would.
async fn rig_answering_check(child: Child, check: Value) -> Rig {
    rig_with(Play::of(child), None, Some(check)).await
}

/// The keeper's whole check, stating the Kubernetes Service host its environment holds.
fn check_with_host(host: &str) -> Value {
    json!({
        "openat2_beneath": true,
        "symlink_escape_refused": true,
        "atomic_replace": true,
        "kubernetes_service_host": host,
    })
}

async fn rig_with(play: Play, handshake: Option<String>, check: Option<Value>) -> Rig {
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
                if let Some(value) = check.clone()
                    && matches!(
                        read_request(&mut request.stdin.as_slice()),
                        Ok((Request::Check {}, _))
                    )
                {
                    Response::Ok {
                        value,
                        omitted_entries: 0,
                    }
                    .write_to(&mut stdout)
                    .expect("the stated check answer is written");
                    return ExecAnswer {
                        stdout,
                        ..ExecAnswer::default()
                    };
                }
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
        play,
        Arc::clone(&ran),
        root.path().to_path_buf(),
    ));
    Rig {
        fake,
        clock,
        driver,
        ledger,
        ran,
        root,
        kubelet,
    }
}

/// Binds every probe keeper to `NODE` and ends every probe Pod as `play` states, writing its
/// heartbeat into `volume`, the directory the keeper's `fs` calls serve.
async fn play_kubelet(
    admin: Api,
    fake: Arc<FakeCluster>,
    play: Play,
    ran: Arc<Mutex<Vec<Value>>>,
    volume: PathBuf,
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
                    lock(&ran).push(pod.clone());
                    if play.heartbeat != Heartbeat::Absent {
                        beat(&spec, &volume, play.heartbeat);
                    }
                    if let End::Gone = play.end {
                        let _ = admin.delete(&path, 0).await;
                        continue;
                    }
                    fake.set_log(&name, &probe_log(&spec, &play));
                    let code = match play.end {
                        End::LimitsUnapplied => 125,
                        End::CodeDisagrees => 1,
                        End::Exit | End::NotAFrame | End::Gone => 0,
                    };
                    pod["status"] = json!({
                        "phase": "Succeeded",
                        "containerStatuses": [{"name": "run", "state": {"terminated": {"exitCode": code, "reason": "Completed"}}}],
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

/// The targets the driver stated in the probe Pod's run spec.
fn targets_of(spec: &RunSpec) -> ProbeTargets {
    ProbeTargets::parse(
        spec.env
            .get(TARGETS_ENV)
            .expect("the spec states the targets"),
    )
    .expect("the targets parse")
}

/// Writes the heartbeat the probe's targets name into `volume`, once for [`Heartbeat::Stopped`]
/// and every 100 ms from now on for [`Heartbeat::Moving`], as the probe child's grandchild would.
fn beat(spec: &RunSpec, volume: &std::path::Path, heartbeat: Heartbeat) {
    let named = targets_of(spec).heartbeat;
    let file = volume.join(
        named
            .strip_prefix("/workspace/")
            .expect("the heartbeat is in the probe's volume"),
    );
    std::fs::write(&file, "7 0\n").expect("the heartbeat is written");
    if heartbeat == Heartbeat::Moving {
        tokio::spawn(async move {
            for beat in 1_u64.. {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if std::fs::write(&file, format!("7 {beat}\n")).is_err() {
                    return;
                }
            }
        });
    }
}

/// The log the executor writes for `probe-child` under `spec` as `play` states: its bounds read
/// back as applied, the child's argv line and its observation line, and exit 0.
fn probe_log(spec: &RunSpec, play: &Play) -> Vec<u8> {
    let mut argv = spec.argv.clone();
    if let Child::Expanded = play.child {
        for part in &mut argv {
            *part = part.replace("$(HOME)", "/var/empty");
        }
    }
    let mut stdout = substrate_executor::probe::report(&argv).into_bytes();
    if let Some(observed) = &play.observed {
        stdout.extend_from_slice(observed.as_bytes());
    }
    let unapplied = matches!(play.end, End::LimitsUnapplied);
    let applied = Frame::Control(Control::Applied(Applied {
        memory_bytes: Some(spec.memory_bytes + u64::from(unapplied)),
        cpu_millicores: Some(spec.cpu_millicores),
        pids_max: Some("max".to_owned()),
    }));
    let mut log = Vec::new();
    let mut push = |line: &str| {
        log.extend_from_slice(line.as_bytes());
        log.push(b'\n');
    };
    push(&applied.encode().expect("a frame encodes"));
    if unapplied {
        let refused = Frame::Control(Control::Refused(Refusal::LimitsUnapplied));
        push(&refused.encode().expect("a frame encodes"));
        return log;
    }
    if let End::NotAFrame = play.end {
        push("the child wrote this line itself");
    }
    for frame in [
        Frame::Output {
            stream: Stream::Stdout,
            bytes: stdout,
        },
        Frame::Control(Control::Exit(ExitRecord::Code(0))),
    ] {
        push(&frame.encode().expect("a frame encodes"));
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

/// What this driver never publishes, whatever its probe observed: the four Linux facts, and every
/// family and property § 6.2 has no row for.
fn assert_never_published(snapshot: &CapabilitySnapshot) {
    let facts = &snapshot.facts;
    assert_eq!(snapshot.driver, HostDriverKind::Kubernetes);
    for family in ConfinementFamily::ALL {
        if !SEVEN_FAMILIES.contains(&family) {
            assert!(
                !facts.serves_family(family),
                "{family:?} is never published"
            );
        }
    }
    for property in ConfinementProperty::ALL {
        if property != ConfinementProperty::WholeTreeKill {
            assert!(!facts.proves(property), "{property:?} is never published");
        }
    }
    assert_eq!(facts.confinement_family_filesystem_view, None);
    assert_eq!(facts.confinement_family_network_egress, None);
    assert_eq!(facts.confinement_family_network_ingress, None);
    assert_eq!(facts.exec_namespaces, None);
    assert_eq!(facts.exec_cgroup_limits, None);
    assert_eq!(facts.exec_cgroup_kill, None);
    assert_eq!(facts.exec_no_egress, None);
    assert_eq!(facts.exec_inline_capsule, None);
    assert_eq!(facts.exec_workspace_scoped_write, None);
    assert_eq!(facts.workspace_storage_quota, None);
    assert_eq!(facts.process_host, None);
}

/// The seven families § 6.2 row 3 publishes, in [`ConfinementFamily::ALL`]'s order.
const SEVEN_FAMILIES: [ConfinementFamily; 7] = [
    ConfinementFamily::ProcessRun,
    ConfinementFamily::ProcessWatch,
    ConfinementFamily::OutputRead,
    ConfinementFamily::Wait,
    ConfinementFamily::Workspace,
    ConfinementFamily::Limits,
    ConfinementFamily::Audit,
];

/// The seven families as the snapshot states them: all `Some(true)` or all absent.
fn families(facts: &CapabilityFacts) -> [Option<bool>; 7] {
    [
        facts.confinement_family_process_run,
        facts.confinement_family_process_watch,
        facts.confinement_family_output_read,
        facts.confinement_family_wait,
        facts.confinement_family_workspace,
        facts.confinement_family_limits,
        facts.confinement_family_audit,
    ]
}

/// Neither observed property: a run whose child observed nothing proves neither.
fn assert_no_observed_property(snapshot: &CapabilitySnapshot) {
    let facts = &snapshot.facts;
    assert_eq!(facts.confinement_no_egress, None, "no egress was observed");
    assert_eq!(
        facts.confinement_whole_tree_kill, None,
        "no heartbeat was observed"
    );
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
    assert_eq!(families(facts), [None; 7]);
    assert_no_observed_property(snapshot);
    assert_eq!(snapshot.valid_until, None);
}

#[tokio::test]
async fn a_probe_publishes_what_it_observed_and_never_a_linux_or_egress_fact() {
    let rig = rig(Child::Exact, None).await;
    let before = rig.machine();
    assert_never_published(&before);
    assert_no_observed_property(&before);
    assert_daemon_facts(&before);
    assert_eq!(before.facts.exec_argv_only, None);
    assert_eq!(families(&before.facts), [None; 7]);

    rig.driver.probe().await.expect("the probe runs");

    let snapshot = rig.machine();
    let facts = &snapshot.facts;
    assert_never_published(&snapshot);
    // The played child writes its argv line and no observation, as on a cluster nothing confines.
    assert_no_observed_property(&snapshot);
    assert_daemon_facts(&snapshot);
    assert_eq!(
        families(facts),
        [Some(true); 7],
        "the Pod ended, its frames decoded and its applied record is its spec's"
    );
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
    let ran: Vec<Vec<String>> = lock(&rig.ran)
        .iter()
        .map(|pod| run_spec(pod).argv)
        .collect();
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
    // Its spec states what the child observes, and the volume its heartbeat goes to is writable.
    let pod = lock(&rig.ran)[0].clone();
    let targets = targets_of(&run_spec(&pod));
    assert_eq!(targets.heartbeat, format!("/workspace/{HEARTBEAT_FILE}"));
    assert_eq!(targets.resolv_conf, "/etc/resolv.conf");
    assert_eq!(
        targets.egress_probe_address,
        "192.0.2.10:443".parse::<SocketAddr>().expect("an address")
    );
    assert_eq!(run_spec(&pod).env.len(), 1, "the targets and nothing else");
    assert_eq!(
        pod["spec"]["containers"][0]["volumeMounts"][0],
        json!({"name": "workspace", "mountPath": "/workspace", "readOnly": false})
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

/// § 6.1: retaking a lost Lease runs steps 4 to 7 again, and a proof holds only under the Lease
/// it was taken under. A probe that was in flight when the Lease lapsed and finishes after the
/// Lease was retaken was not taken under the retaken Lease, so it may not stand for step 7: the
/// probe is due again and its facts stay absent until a probe started under the new Lease.
#[tokio::test]
async fn a_probe_in_flight_across_a_lost_and_retaken_lease_does_not_stand_for_step_seven() {
    let mut rig = rig(Child::Exact, None).await;
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    rig.kubelet.abort();
    rig.kubelet = {
        let admin = admin(&rig.fake);
        let fake = Arc::clone(&rig.fake);
        let ran = Arc::clone(&rig.ran);
        let release = Arc::clone(&release);
        let volume = rig.root.path().to_path_buf();
        tokio::spawn(async move {
            // Schedules the probe keeper, and holds the probe Pod unended until released.
            while !release.load(std::sync::atomic::Ordering::SeqCst) {
                let listed = admin
                    .list(&admin.pods(), "PodList", ListForm::Whole, 100, 4)
                    .await
                    .map(|listed| listed.items)
                    .unwrap_or_default();
                for mut pod in listed {
                    if pod["metadata"]["labels"]["b10x-substrate/role"] == "keeper"
                        && pod["spec"]["nodeName"].is_null()
                    {
                        let name = pod["metadata"]["name"].as_str().unwrap_or_default();
                        let path = format!("{}/{name}", admin.pods());
                        pod["spec"]["nodeName"] = json!(NODE);
                        let _ = admin.update(&path, &pod).await;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            play_kubelet(admin, fake, Play::of(Child::Exact), ran, volume).await;
        })
    };
    let lapse = async {
        while !rig
            .ledger
            .rows_of(BackendKind::Probe)
            .iter()
            .any(|row| row.role == BackendRole::Pod && row.uid.is_some())
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        rig.clock.set(HELD_FOR + Duration::from_secs(1));
        assert!(!rig.driver.lease_held(), "the Lease lapsed mid-probe");
        rig.driver.lease_round().await;
        assert!(rig.driver.lease_held(), "the Lease is retaken");
        release.store(true, std::sync::atomic::Ordering::SeqCst);
    };
    let (probed, ()) = tokio::join!(rig.driver.probe(), lapse);
    probed.expect("the in-flight probe completes");
    assert!(
        rig.driver.probe_due(),
        "a retaken Lease runs step 7 again; the probe begun under the lost Lease stood for it"
    );
    assert_eq!(
        rig.machine().facts.exec_argv_only,
        None,
        "a proof begun under the lost Lease is published under the retaken one"
    );
}

/// § 6.1 and ADR 0045 D5 A: the first open sequence settles when an open is refused for a reason
/// no retry clears, or when the first probe after a successful open has run, whatever it proved.
/// An open alone does not settle it, because the facts readiness reads arrive with the probe, and
/// neither does a refusal the open's own retry can clear: a Lease still held by a predecessor, or
/// an API that did not answer.
#[tokio::test]
async fn the_first_open_sequence_settles_on_a_lasting_refusal_or_after_the_first_probe() {
    let fake = passing_fake();
    let clock = ManualClock::new();
    let (refused, _ledger) = driver(&fake, &clock);
    assert!(!refused.opening_settled(), "nothing has run yet");
    fake.set_rules(vec![
        json!({"verbs": ["get"], "apiGroups": [""], "resources": ["secrets"]}),
    ]);
    refused.lease_round().await;
    assert!(!refused.opened());
    assert!(refused.opening_settled(), "rbac-excess settles it at once");
    tokio::time::timeout(Duration::from_secs(5), refused.opening_settles())
        .await
        .expect("a settled open sequence is awaited at once");

    let unrestricted = passing_fake();
    unrestricted.enforce_restricted(false);
    let (admitted, _ledger) = driver(&unrestricted, &clock);
    admitted.lease_round().await;
    assert!(!admitted.opened());
    assert!(
        admitted.opening_settled(),
        "pod-security-unenforced settles it at once"
    );

    let silent = passing_fake();
    silent.fault(Fault::DropAnswer {
        method: Method::POST,
        path: "/apis/authorization.k8s.io/v1/selfsubjectrulesreviews".to_owned(),
    });
    let (unanswered, _ledger) = driver(&silent, &clock);
    unanswered.lease_round().await;
    assert!(!unanswered.opened());
    assert!(
        !unanswered.opening_settled(),
        "an API that did not answer is retried, not settled"
    );

    // A Recreate rollout: the predecessor's Lease has not expired when the successor opens.
    let predecessor = rig(Child::Exact, None).await;
    let successor_clock = ManualClock::new();
    let successor = driver_over(
        &predecessor.fake,
        &successor_clock,
        &predecessor.ledger,
        DEPLOYMENT,
    );
    successor.lease_round().await;
    assert!(!successor.opened(), "the predecessor still holds the Lease");
    assert!(
        !successor.opening_settled(),
        "lease-held is retried, not settled"
    );
    successor_clock.set(crate::lease::TAKEOVER_AFTER);
    successor.lease_round().await;
    assert!(successor.opened(), "the unrenewed Lease is taken over");
    assert!(!successor.opening_settled(), "its probe has not run yet");
    successor.probe().await.expect("the successor probes");
    assert!(successor.opening_settled());
    assert_eq!(Driver::machine(&successor).facts.exec_argv_only, Some(true));
    drop(predecessor);

    let opened = rig(Child::Exact, None).await;
    assert!(opened.driver.opened());
    assert!(
        !opened.driver.opening_settled(),
        "an open whose first probe has not run has not settled"
    );
    opened.driver.probe().await.expect("the probe runs");
    assert!(
        opened.driver.opening_settled(),
        "the first probe settles it"
    );

    let other = r#"{"protocol":"substrate-executor/0","version":"0.0.0","source":"x"}"#;
    let failed = rig(Child::Exact, Some(other.to_owned())).await;
    assert!(!failed.driver.opening_settled());
    let handshake = failed
        .driver
        .probe()
        .await
        .expect_err("the handshake fails");
    assert_eq!(handshake.code, EXECUTOR_HANDSHAKE_FAILED);
    assert!(
        failed.driver.opening_settled(),
        "a failed first probe settles it too, with its facts absent"
    );
}

/// Every reason code an open can refuse with, split by whether the open's own retry can clear it.
#[test]
fn every_open_refusal_is_either_retried_or_settles_the_opening() {
    use crate::{
        CONFIG_INVALID, LEASE_FOREIGN, LEASE_HELD, POD_SECURITY_UNENFORCED,
        POLICY_NOT_DEFAULT_DENY, RBAC_EXCESS,
    };
    for lasting in [
        CONFIG_INVALID,
        RBAC_EXCESS,
        POLICY_NOT_DEFAULT_DENY,
        POD_SECURITY_UNENFORCED,
    ] {
        assert!(super::settles_opening(lasting), "{lasting}");
    }
    for retried in [LEASE_HELD, LEASE_FOREIGN, crate::API_FAILED] {
        assert!(!super::settles_opening(retried), "{retried}");
    }
}

#[tokio::test]
async fn a_probe_whose_check_proves_only_openat2_publishes_no_guarded_io_or_atomic_replace() {
    // The answer the executor gave before its check made the other two proofs.
    let rig = rig_answering_check(Child::Exact, json!({ "openat2_beneath": true })).await;
    rig.driver.probe().await.expect("the probe runs");
    let facts = rig.machine().facts;
    assert_eq!(
        facts.workspace_guarded_io, None,
        "the keeper's check answers openat2 availability alone: no symlink escape is refused"
    );
    assert_eq!(
        facts.workspace_atomic_replace, None,
        "the keeper's check runs no renameat2 replacement"
    );
}

/// The workspace facts a probe publishes when the keeper's check answers `check`.
async fn workspace_facts_for(check: Value) -> [Option<bool>; 3] {
    let rig = rig_answering_check(Child::Exact, check).await;
    rig.driver.probe().await.expect("the probe runs");
    let facts = rig.machine().facts;
    let guarded = facts.workspace_guarded_io;
    assert_eq!(
        facts.workspace_max_file_bytes.is_some(),
        guarded.is_some(),
        "the file bounds are served exactly when guarded I/O is"
    );
    [
        facts.workspace_openat2_beneath,
        guarded,
        facts.workspace_atomic_replace,
    ]
}

#[tokio::test]
async fn each_workspace_fact_is_published_from_its_own_proof_of_the_check() {
    let all = json!({
        "openat2_beneath": true,
        "symlink_escape_refused": true,
        "atomic_replace": true,
    });
    assert_eq!(
        workspace_facts_for(all).await,
        [Some(true), Some(true), Some(true)]
    );
    let no_openat2 = json!({
        "openat2_beneath": false,
        "symlink_escape_refused": true,
        "atomic_replace": true,
    });
    assert_eq!(
        workspace_facts_for(no_openat2).await,
        [None, None, Some(true)],
        "guarded I/O needs the openat2 proof as well as the escape proof"
    );
    let escape_followed = json!({
        "openat2_beneath": true,
        "symlink_escape_refused": false,
        "atomic_replace": true,
    });
    assert_eq!(
        workspace_facts_for(escape_followed).await,
        [Some(true), None, Some(true)]
    );
    let no_replacement = json!({
        "openat2_beneath": true,
        "symlink_escape_refused": true,
        "atomic_replace": false,
    });
    assert_eq!(
        workspace_facts_for(no_replacement).await,
        [Some(true), Some(true), None]
    );
    let not_booleans = json!({
        "openat2_beneath": "true",
        "symlink_escape_refused": 1,
        "atomic_replace": null,
    });
    assert_eq!(workspace_facts_for(not_booleans).await, [None, None, None]);
}

/// The cluster address the keeper's environment states for the Kubernetes Service.
const SERVICE_HOST: &str = "10.96.0.1";

fn attempt(target: EgressTarget, address: &str, outcome: EgressOutcome) -> EgressAttempt {
    EgressAttempt {
        target,
        address: Some(address.parse().expect("an address")),
        outcome,
        errno: None,
        elapsed_ms: 3_000,
    }
}

/// What a child under a default-deny policy its plugin enforces inside the Pod reports: each
/// attempt was denied before it left (`EPERM`, `EACCES`), at the addresses the driver
/// named and the nameserver its resolver names. A timed-out or no-route attempt proves nothing (D333, D334).
fn blocked() -> Vec<EgressAttempt> {
    vec![
        attempt(
            EgressTarget::KubernetesService,
            "10.96.0.1:443",
            EgressOutcome::Denied,
        ),
        attempt(
            EgressTarget::Nameserver,
            "10.96.0.10:53",
            EgressOutcome::Denied,
        ),
        attempt(
            EgressTarget::EgressProbe,
            "192.0.2.10:443",
            EgressOutcome::Denied,
        ),
    ]
}

/// The facts one probe publishes when the kubelet plays `play` and the keeper's check answers
/// `check` (the real executor's when `None`). None of them is ever a Linux fact.
async fn facts_after(play: Play, check: Option<Value>) -> CapabilityFacts {
    let rig = rig_with(play, None, check).await;
    rig.driver.probe().await.expect("the probe runs");
    let snapshot = rig.machine();
    assert_never_published(&snapshot);
    snapshot.facts
}

/// § 6.2 and § 6.4: a probe whose Pod ended with its frames and bounds, whose heartbeat stood
/// still after the end, and whose three egress attempts all failed publishes design 34 § 6.4's
/// neutral set, beside the seven families; the facts lapse with the proof.
#[tokio::test]
async fn a_probe_that_observed_all_of_it_publishes_the_neutral_set_until_valid_until() {
    let observation = Observation {
        heartbeat: true,
        egress: blocked(),
    };
    let rig = rig_with(
        Play::observing(&observation, Heartbeat::Stopped),
        None,
        Some(check_with_host(SERVICE_HOST)),
    )
    .await;
    rig.driver.probe().await.expect("the probe runs");
    let snapshot = rig.machine();
    let facts = &snapshot.facts;
    assert_never_published(&snapshot);
    assert_eq!(families(facts), [Some(true); 7]);
    assert_eq!(facts.confinement_whole_tree_kill, Some(true));
    assert_eq!(facts.confinement_no_egress, Some(true));
    assert_eq!(
        facts.exec_argv_only,
        Some(true),
        "the argv line keeps its bytes"
    );
    for family in [
        ConfinementFamily::ProcessRun,
        ConfinementFamily::Wait,
        ConfinementFamily::OutputRead,
        ConfinementFamily::Limits,
    ] {
        assert!(facts.serves_family(family), "{family:?}");
    }
    assert!(facts.proves(ConfinementProperty::WholeTreeKill));

    // The child was told to dial the host the keeper's environment states.
    let pod = lock(&rig.ran)[0].clone();
    assert_eq!(
        targets_of(&run_spec(&pod)).kubernetes_service_host,
        Some(SERVICE_HOST.parse().expect("an address"))
    );

    // The proof lapses at `valid_until`, the observed facts with it.
    rig.clock.set(INTERVAL + SLACK);
    let lapsed = rig.machine();
    assert_eq!(families(&lapsed.facts), [None; 7]);
    assert_no_observed_property(&lapsed);
}

/// § 6.2 row 3: the Pod's end, its decoded frames and its applied bounds each hold the seven
/// families; any one failing leaves all seven absent, whatever the argv proof says.
#[tokio::test]
async fn the_seven_families_are_absent_when_the_end_the_frames_or_the_applied_bounds_fail() {
    for end in [
        End::LimitsUnapplied,
        End::NotAFrame,
        End::CodeDisagrees,
        End::Gone,
    ] {
        let play = Play {
            end,
            ..Play::of(Child::Exact)
        };
        let facts = facts_after(play, None).await;
        assert_eq!(families(&facts), [None; 7], "{end:?}");
        assert_eq!(facts.exec_argv_only, None, "{end:?}");
        assert_eq!(
            facts.workspace_guarded_io,
            Some(true),
            "{end:?}: the keeper proved its own"
        );
    }
    let expanded = facts_after(Play::of(Child::Expanded), None).await;
    assert_eq!(
        families(&expanded),
        [Some(true); 7],
        "an argv that did not read back is the argv proof's, not the families'"
    );
    assert_eq!(expanded.exec_argv_only, None);
}

/// § 6.2 row 4: whole-tree kill needs the child to have seen its grandchild's heartbeat move and
/// the keeper to see it stand still for 2 s after the end. A heartbeat that moves on, one the
/// keeper cannot read, and one the child never saw move leave it absent.
#[tokio::test]
async fn whole_tree_kill_is_absent_unless_the_heartbeat_moved_and_then_stood_still() {
    let moving = Observation {
        heartbeat: true,
        egress: blocked(),
    };
    let facts = facts_after(
        Play::observing(&moving, Heartbeat::Moving),
        Some(check_with_host(SERVICE_HOST)),
    )
    .await;
    assert_eq!(
        facts.confinement_whole_tree_kill, None,
        "the heartbeat moves on"
    );
    assert_eq!(
        facts.confinement_no_egress,
        Some(true),
        "each property stands on its own observation"
    );

    let unread = facts_after(Play::observing(&moving, Heartbeat::Absent), None).await;
    assert_eq!(
        unread.confinement_whole_tree_kill, None,
        "no heartbeat to read"
    );

    let never_moved = Observation {
        heartbeat: false,
        egress: blocked(),
    };
    let still = facts_after(Play::observing(&never_moved, Heartbeat::Stopped), None).await;
    assert_eq!(
        still.confinement_whole_tree_kill, None,
        "the child never saw a grandchild beat"
    );

    let unreported = Play {
        heartbeat: Heartbeat::Stopped,
        ..Play::of(Child::Exact)
    };
    let unreported = facts_after(unreported, None).await;
    assert_eq!(
        unreported.confinement_whole_tree_kill, None,
        "no observation line"
    );
    assert_eq!(families(&unreported), [Some(true); 7]);
}

/// § 6.2 row 5: no egress needs every one of the three attempts, in order, to have been denied,
/// at the addresses the driver named. A reached, refused, timed-out or no-route attempt, an
/// attempt that was never made, another address, and an observation that does not read leave it
/// absent.
#[tokio::test]
async fn no_egress_is_absent_unless_all_three_attempts_failed_at_the_named_addresses() {
    let held = Observation {
        heartbeat: false,
        egress: blocked(),
    };
    let check = || Some(check_with_host(SERVICE_HOST));
    let facts = facts_after(Play::observing(&held, Heartbeat::Absent), check()).await;
    assert_eq!(facts.confinement_no_egress, Some(true), "the control");
    assert_eq!(facts.confinement_whole_tree_kill, None);

    let mut cases: Vec<(String, Play, Option<Value>)> = Vec::new();
    let mut with = |why: &str, change: &dyn Fn(&mut Vec<EgressAttempt>)| {
        let mut egress = blocked();
        change(&mut egress);
        let observation = Observation {
            heartbeat: false,
            egress,
        };
        cases.push((
            why.to_owned(),
            Play::observing(&observation, Heartbeat::Absent),
            check(),
        ));
    };
    for index in 0..3 {
        with(&format!("attempt {index} reached"), &|egress| {
            egress[index].outcome = EgressOutcome::Reached;
        });
    }
    with("the egress probe refused", &|egress| {
        egress[2].outcome = EgressOutcome::Refused;
    });
    with("the nameserver attempt failed otherwise", &|egress| {
        egress[1].outcome = EgressOutcome::Failed;
    });
    with("the nameserver was not read", &|egress| {
        egress[1].address = None;
        egress[1].outcome = EgressOutcome::Unread;
    });
    with("another egress address", &|egress| {
        egress[2].address = Some("192.0.2.11:443".parse().expect("an address"));
    });
    with("another Kubernetes Service address", &|egress| {
        egress[0].address = Some("10.96.0.2:443".parse().expect("an address"));
    });
    with("the Kubernetes Service on another port", &|egress| {
        egress[0].address = Some("10.96.0.1:6443".parse().expect("an address"));
    });
    with("an attempt missing", &|egress| {
        egress.remove(1);
    });
    with("the attempts out of order", &|egress| egress.swap(0, 2));
    cases.push((
        "the keeper states no Kubernetes Service host".to_owned(),
        Play::observing(&held, Heartbeat::Absent),
        Some(json!({
            "openat2_beneath": true,
            "symlink_escape_refused": true,
            "atomic_replace": true,
        })),
    ));
    for (why, line) in [
        ("an observation that is not one", "{\"heartbeat\":false}\n"),
        (
            "two observation lines",
            &format!("{}{}", held.line(), held.line()),
        ),
        ("an observation without its newline", held.line().trim_end()),
    ] {
        cases.push((
            why.to_owned(),
            Play {
                observed: Some(line.to_owned()),
                ..Play::of(Child::Exact)
            },
            check(),
        ));
    }
    for (why, play, check) in cases {
        let facts = facts_after(play, check).await;
        assert_eq!(facts.confinement_no_egress, None, "{why}");
        assert_eq!(families(&facts), [Some(true); 7], "{why}");
    }
}

/// Security review, wave 2026-10-31: one probe after the namespace's policy set gained `policy`
/// beside the default-deny one, the driver reopened over it (§ 14 Q1 B binds the whole set), and
/// the probe child reporting what a child the policy does not reach reports: all three attempts
/// were denied. Answers the snapshot and the probe Pod the kubelet ran.
async fn probe_beside(policy: Value) -> (CapabilitySnapshot, Value) {
    let observation = Observation {
        heartbeat: false,
        egress: blocked(),
    };
    let rig = rig_with(
        Play::observing(&observation, Heartbeat::Absent),
        None,
        Some(check_with_host(SERVICE_HOST)),
    )
    .await;
    let before = rig.driver.binding().expect("bound at open").digest();
    rig.fake
        .seed("networkpolicies", policy)
        .expect("the additional policy is stored");
    rig.driver
        .open()
        .await
        .expect("the driver reopens over a set whose default-deny policy still denies");
    assert_ne!(
        rig.driver.binding().expect("bound again").digest(),
        before,
        "the added policy is in the bound set"
    );
    rig.driver.probe().await.expect("the probe runs");
    let snapshot = rig.machine();
    assert_eq!(
        families(&snapshot.facts),
        [Some(true); 7],
        "the control: the probe Pod ran and ended as its spec says"
    );
    let pod = lock(&rig.ran)[0].clone();
    (snapshot, pod)
}

/// Security review, wave 2026-10-31 (invariant 3, design 34 § 6.2 row 5 and § 14 Q1 B): an
/// operator lets the exec Pods reach the network with a policy that selects them by the label
/// every exec Pod carries, `b10x-substrate/exec`. The probe Pod carries `b10x-substrate/probe`
/// instead, so the policy never selects it and its three attempts time out as under the
/// default-deny policy alone. The probe's observation then stands for no Pod an exec runs in, and
/// `confinement.no-egress` must not be published while the bound set lets exec Pods egress.
#[tokio::test]
async fn a_policy_selecting_exec_pods_but_not_the_probe_pod_withholds_no_egress() {
    let exec_labels = names::labels(
        DEPLOYMENT,
        BackendKind::Exec,
        &names::resource_hash(DEPLOYMENT, "ex_0001"),
        BackendRole::Pod,
    );
    assert!(
        exec_labels.contains_key("b10x-substrate/exec"),
        "every exec Pod carries the key the policy selects on"
    );
    let (snapshot, probe_pod) = probe_beside(json!({
        "metadata": {"name": "exec-pods-reach-the-mirror"},
        "spec": {
            "podSelector": {"matchExpressions": [
                {"key": "b10x-substrate/exec", "operator": "Exists"},
            ]},
            "policyTypes": ["Egress"],
            "egress": [{}],
        },
    }))
    .await;
    assert!(
        probe_pod["metadata"]["labels"]
            .get("b10x-substrate/exec")
            .is_none(),
        "the probe Pod is not selected by the policy: {}",
        probe_pod["metadata"]["labels"]
    );
    assert_eq!(
        snapshot.facts.confinement_no_egress, None,
        "confinement.no-egress was published from a Pod the egress-allowing policy does not \
         select, while every exec Pod it admits is selected by it"
    );
}

/// Security review, wave 2026-10-31 (invariant 3, design 34 § 6.2 row 5 and § 14 Q1 B): a bound
/// policy selecting every Pod allows egress to one range. The probe dials three fixed addresses
/// outside it, so all three time out, while every exec Pod may reach the range. The driver holds
/// the policy that states egress is allowed; `confinement.no-egress` must not be published from
/// three sampled attempts against it.
#[tokio::test]
async fn a_bound_policy_allowing_egress_to_a_range_the_probe_does_not_dial_withholds_no_egress() {
    let (snapshot, _) = probe_beside(json!({
        "metadata": {"name": "allow-one-range"},
        "spec": {
            "podSelector": {},
            "policyTypes": ["Egress"],
            "egress": [{"to": [{"ipBlock": {"cidr": "203.0.113.0/24"}}]}],
        },
    }))
    .await;
    assert_eq!(
        snapshot.facts.confinement_no_egress, None,
        "confinement.no-egress was published while the bound policy set allows every Pod egress \
         to 203.0.113.0/24"
    );
}

/// Adversary, wave 2026-10-31 (claim 5; design 34 § 6.2, amendment of 2026-10-31): "the heartbeat
/// and the egress observation count only from a run whose argv read back byte for byte". The
/// suite holds an expanded argv only with no observation line, so dropping the argv gate in
/// `read_run` stays green. Here the child's argv was expanded on its way to `execve` and its
/// observation and heartbeat would each prove their property on their own: both stay absent,
/// while the seven families, which rest on the end and not on the argv, are published.
#[tokio::test]
async fn adversary_w1031_an_expanded_argv_voids_the_heartbeat_and_the_egress_observation() {
    let observation = Observation {
        heartbeat: true,
        egress: blocked(),
    };
    let control = facts_after(
        Play::observing(&observation, Heartbeat::Stopped),
        Some(check_with_host(SERVICE_HOST)),
    )
    .await;
    assert_eq!(
        (
            control.confinement_whole_tree_kill,
            control.confinement_no_egress
        ),
        (Some(true), Some(true)),
        "the control: the same observation from an exact argv proves both"
    );
    let expanded = Play {
        child: Child::Expanded,
        ..Play::observing(&observation, Heartbeat::Stopped)
    };
    let facts = facts_after(expanded, Some(check_with_host(SERVICE_HOST))).await;
    assert_eq!(facts.exec_argv_only, None);
    assert_eq!(families(&facts), [Some(true); 7]);
    assert_eq!(
        (
            facts.confinement_whole_tree_kill,
            facts.confinement_no_egress
        ),
        (None, None),
        "an observation from a run whose argv did not read back proves nothing"
    );
}

/// Linux errno values as the traced attempts recorded them.
const EPERM: i32 = 1;
const EAGAIN: i32 = 11;
const EACCES: i32 = 13;
const ENETUNREACH: i32 = 101;
const ECONNREFUSED: i32 = 111;
const EHOSTUNREACH: i32 = 113;

/// The three attempts of one traced probe, with the errno and elapsed time the trace recorded.
fn traced(outcomes: [(EgressOutcome, Option<i32>, u64); 3]) -> Vec<EgressAttempt> {
    let [kubernetes, nameserver, egress] = outcomes;
    [
        (EgressTarget::KubernetesService, "10.96.0.1:443", kubernetes),
        (EgressTarget::Nameserver, "10.96.0.10:53", nameserver),
        (EgressTarget::EgressProbe, "192.0.2.10:443", egress),
    ]
    .into_iter()
    .map(
        |(target, address, (outcome, errno, elapsed_ms))| EgressAttempt {
            errno,
            elapsed_ms,
            ..attempt(target, address, outcome)
        },
    )
    .collect()
}

/// D332 trace 1 (`approval-record:d332-no-egress-fix-wave`), kind at `3b3998c8`: the Kubernetes
/// Service and the nameserver answered and the egress probe address refused. Egress is open, and
/// `confinement.no-egress` stays absent.
#[tokio::test]
async fn d332_trace_one_reached_reached_refused_withholds_no_egress() {
    let observation = Observation {
        heartbeat: true,
        egress: traced([
            (EgressOutcome::Reached, None, 0),
            (EgressOutcome::Reached, None, 21),
            (EgressOutcome::Refused, Some(ECONNREFUSED), 0),
        ]),
    };
    let facts = facts_after(
        Play::observing(&observation, Heartbeat::Stopped),
        Some(check_with_host(SERVICE_HOST)),
    )
    .await;
    assert_eq!(
        families(&facts),
        [Some(true); 7],
        "the control: the run proved its end"
    );
    assert_eq!(facts.confinement_no_egress, None);
}

/// D332 trace 2 (`approval-record:d332-no-egress-fix-wave`), kind at `3b3998c8`: all three
/// attempts timed out while the lane's own Pod under the same default-deny policy reached all three
/// targets. Silence is not a denial: a timeout never proves the attempt was stopped (D333, option
/// A), so `confinement.no-egress` stays absent.
#[tokio::test]
async fn d332_trace_two_three_timeouts_withhold_no_egress() {
    let observation = Observation {
        heartbeat: true,
        egress: traced([
            (EgressOutcome::TimedOut, None, 3_001),
            (EgressOutcome::TimedOut, Some(EAGAIN), 3_058),
            (EgressOutcome::TimedOut, None, 3_001),
        ]),
    };
    let facts = facts_after(
        Play::observing(&observation, Heartbeat::Stopped),
        Some(check_with_host(SERVICE_HOST)),
    )
    .await;
    assert_eq!(
        families(&facts),
        [Some(true); 7],
        "the control: the run proved its end"
    );
    assert_eq!(
        facts.confinement_whole_tree_kill,
        Some(true),
        "the control: the heartbeat proof is untouched"
    );
    assert_eq!(
        facts.confinement_no_egress, None,
        "three timed-out attempts published confinement.no-egress"
    );
}

/// D333: one timeout among two local denials is still silence, and withholds the fact.
#[tokio::test]
async fn one_timeout_among_local_denials_withholds_no_egress() {
    for position in 0..3 {
        let mut outcomes = [
            (EgressOutcome::Denied, Some(EPERM), 0),
            (EgressOutcome::Denied, Some(EACCES), 0),
            (EgressOutcome::Denied, Some(EPERM), 0),
        ];
        outcomes[position] = (EgressOutcome::TimedOut, None, 3_000);
        let observation = Observation {
            heartbeat: true,
            egress: traced(outcomes),
        };
        let facts = facts_after(
            Play::observing(&observation, Heartbeat::Stopped),
            Some(check_with_host(SERVICE_HOST)),
        )
        .await;
        assert_eq!(facts.confinement_no_egress, None, "timeout at {position}");
    }
}

/// D334: one attempt without a route among two local denials withholds the fact. `ENETUNREACH`
/// and `EHOSTUNREACH` can come from a remote router's ICMP or an address of the wrong family, so
/// they do not prove the attempt was stopped inside the Pod.
#[tokio::test]
async fn one_no_route_attempt_among_local_denials_withholds_no_egress() {
    for errno in [ENETUNREACH, EHOSTUNREACH] {
        for (position, elapsed_ms) in [(0, 0), (1, 0), (2, 0), (2, 3_000)] {
            let mut outcomes = [
                (EgressOutcome::Denied, Some(EPERM), 0),
                (EgressOutcome::Denied, Some(EACCES), 0),
                (EgressOutcome::Denied, Some(EPERM), 0),
            ];
            outcomes[position] = (EgressOutcome::Unreachable, Some(errno), elapsed_ms);
            let observation = Observation {
                heartbeat: true,
                egress: traced(outcomes),
            };
            let facts = facts_after(
                Play::observing(&observation, Heartbeat::Stopped),
                Some(check_with_host(SERVICE_HOST)),
            )
            .await;
            assert_eq!(
                facts.confinement_no_egress, None,
                "errno {errno} at {position} after {elapsed_ms} ms"
            );
        }
    }
}

/// D334: three attempts each denied before they left the Pod (`EPERM`, `EACCES`) publish the
/// fact.
#[tokio::test]
async fn three_local_denials_publish_no_egress() {
    let observation = Observation {
        heartbeat: true,
        egress: traced([
            (EgressOutcome::Denied, Some(EPERM), 0),
            (EgressOutcome::Denied, Some(EACCES), 0),
            (EgressOutcome::Denied, Some(EPERM), 0),
        ]),
    };
    let facts = facts_after(
        Play::observing(&observation, Heartbeat::Stopped),
        Some(check_with_host(SERVICE_HOST)),
    )
    .await;
    assert_eq!(facts.confinement_no_egress, Some(true));
}

thread_local! {
    /// The buffer [`CapturedLog`] holds on this thread, if any.
    static CAPTURE: std::cell::RefCell<Option<Arc<Mutex<Vec<u8>>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Writes into this thread's [`CAPTURE`], and nowhere when it holds none.
struct ThreadWriter;

impl std::io::Write for ThreadWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        CAPTURE.with(|capture| {
            if let Some(lines) = capture.borrow().as_ref() {
                lock(lines).extend_from_slice(buf);
            }
        });
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Every line the driver logs on this thread while it is held, as `tracing`'s plain formatter
/// writes them. The subscriber is the process's global one, installed once: a scoped default
/// races the other tests' first use of a callsite, which can cache its interest as never.
struct CapturedLog {
    lines: Arc<Mutex<Vec<u8>>>,
}

impl CapturedLog {
    fn start() -> Self {
        static INSTALLED: std::sync::Once = std::sync::Once::new();
        INSTALLED.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .with_max_level(tracing::Level::DEBUG)
                .with_writer(|| ThreadWriter)
                .finish();
            tracing::subscriber::set_global_default(subscriber).expect("one global subscriber");
        });
        let lines = Arc::new(Mutex::new(Vec::new()));
        CAPTURE.with(|capture| *capture.borrow_mut() = Some(Arc::clone(&lines)));
        Self { lines }
    }

    /// The captured `WARN` lines.
    fn warnings(&self) -> Vec<String> {
        String::from_utf8(lock(&self.lines).clone())
            .expect("utf-8 log")
            .lines()
            .filter(|line| line.contains("WARN"))
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for CapturedLog {
    fn drop(&mut self) {
        CAPTURE.with(|capture| *capture.borrow_mut() = None);
    }
}

/// D337 S6a: a Role without `pods/log` `get` leaves the probe Pod's run unread. The run's facts
/// stay absent, and the log says why at `warn`: the reason code `kubernetes.api-failed`, and each
/// fact the run would have proved as "<fact> was not proved", so an operator (and the namespace
/// lane) can tell a refused read from a run that proved less.
#[tokio::test]
async fn a_refused_pod_log_names_its_reason_and_each_unproved_fact() {
    let observation = Observation {
        heartbeat: true,
        egress: blocked(),
    };
    let rig = rig_with(
        Play::observing(&observation, Heartbeat::Stopped),
        None,
        Some(check_with_host(SERVICE_HOST)),
    )
    .await;
    rig.fake.refuse_logs(
        403,
        "pods \"probe\" is forbidden: User \"system:serviceaccount:substrate:substrate-driver\" \
         cannot get resource \"pods/log\" in API group \"\" in the namespace \"substrate\"",
    );
    let log = CapturedLog::start();
    rig.driver.probe().await.expect("the probe runs");
    let snapshot = rig.machine();
    let facts = &snapshot.facts;
    assert_never_published(&snapshot);
    assert_eq!(families(facts), [None; 7]);
    assert_eq!(facts.exec_argv_only, None);
    assert_eq!(facts.confinement_whole_tree_kill, None);
    assert_eq!(facts.confinement_no_egress, None);

    let warnings = log.warnings();
    assert!(
        warnings.iter().any(
            |line| line.contains("code=\"kubernetes.api-failed\"") && line.contains("pods/log")
        ),
        "no warning names kubernetes.api-failed with the API's answer: {warnings:#?}"
    );
    let run_facts = SEVEN_FAMILIES.iter().map(|family| family.fact()).chain([
        "exec.argv-only",
        substrate_wire::CONFINEMENT_WHOLE_TREE_KILL_FACT,
        substrate_wire::CONFINEMENT_NO_EGRESS_FACT,
    ]);
    for fact in run_facts {
        let stated = format!(" {fact} was not proved");
        assert!(
            warnings.iter().any(|line| line.contains(&stated)),
            "no warning says {fact} was not proved: {warnings:#?}"
        );
    }
}

/// Adversary cases for D337 S6a: the reason code the probe Pod's failed run names, on the other
/// paths into `RunFailed::api`.
mod adversary_d337 {
    use std::sync::Weak;

    use substrate_host::{BackendLedger, BackendName, BackendRecord, DriverError, UidBinding};

    use super::*;
    use crate::driver_tests::driver_over_any;

    /// The daemon's store failing the one write that binds the probe Pod's uid, as
    /// `StoreBackendLedger::bind_uid` answers any store error (`state.store-failed`); every other
    /// call is `inner`'s.
    struct StoreFailsProbePodBind {
        inner: Arc<MemoryLedger>,
    }

    impl BackendLedger for StoreFailsProbePodBind {
        fn recorded(&self, kind: BackendKind, id: &str) -> Result<Vec<BackendRecord>, DriverError> {
            self.inner.recorded(kind, id)
        }

        fn bind_uid(&self, name: &str, uid: &str) -> Result<UidBinding, DriverError> {
            let probe_pod = self.inner.unsettled(None)?.into_iter().any(|row| {
                row.name == name && row.kind == BackendKind::Probe && row.role == BackendRole::Pod
            });
            if probe_pod {
                return Err(DriverError::failed(
                    "state.store-failed",
                    "Durable state operation failed.",
                ));
            }
            self.inner.bind_uid(name, uid)
        }

        fn settle(&self, name: &str) -> Result<bool, DriverError> {
            self.inner.settle(name)
        }

        fn reserve_probe(&self, nonce: &str, names: &[BackendName]) -> Result<(), DriverError> {
            self.inner.reserve_probe(nonce, names)
        }

        fn unsettled(&self, after: Option<&str>) -> Result<Vec<BackendRecord>, DriverError> {
            self.inner.unsettled(after)
        }
    }

    /// The keeper's handshake and its whole check, as [`rig_with`] answers them; `on_check` runs
    /// once the check is answered, before the probe Pod is created.
    fn answer_keeper(fake: &FakeCluster, on_check: impl Fn() + Send + Sync + 'static) {
        fake.set_exec(move |request: ExecRequest| {
            let command: Vec<&str> = request.command.iter().map(String::as_str).collect();
            match command.as_slice() {
                ["/substrate-executor", "version"] => ExecAnswer {
                    stdout: format!("{}\n", Handshake::current().line()).into_bytes(),
                    ..ExecAnswer::default()
                },
                ["/substrate-executor", "fs"]
                    if matches!(
                        read_request(&mut request.stdin.as_slice()),
                        Ok((Request::Check {}, _))
                    ) =>
                {
                    let mut stdout = Vec::new();
                    Response::Ok {
                        value: check_with_host(SERVICE_HOST),
                        omitted_entries: 0,
                    }
                    .write_to(&mut stdout)
                    .expect("the check answer is written");
                    on_check();
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
    }

    /// The line that says the probe Pod's run proved nothing, and why.
    fn run_line(warnings: &[String]) -> String {
        warnings
            .iter()
            .find(|line| {
                line.contains("the probe Pod's run proved nothing") && line.contains("why=")
            })
            .cloned()
            .unwrap_or_else(|| panic!("no warning says why the run proved nothing: {warnings:#?}"))
    }

    /// A store write that failed after the API stored the probe Pod is not the API's failure:
    /// the run's warning names the code the create was answered with, `state.store-failed`,
    /// which its own `why` already states, and not `kubernetes.api-failed`.
    #[tokio::test]
    async fn a_store_failure_binding_the_probe_pod_is_not_named_kubernetes_api_failed() {
        let fake = Arc::new(passing_fake());
        let clock = ManualClock::new();
        let inner = Arc::new(MemoryLedger::default());
        let driver = driver_over_any(
            &fake,
            &clock,
            Arc::new(StoreFailsProbePodBind {
                inner: Arc::clone(&inner),
            }),
        );
        driver.open().await.expect("the driver opens");
        answer_keeper(&fake, || {});
        let root = tempfile::tempdir().expect("a temporary PVC");
        let kubelet = tokio::spawn(play_kubelet(
            admin(&fake),
            Arc::clone(&fake),
            Play::of(Child::Exact),
            Arc::new(Mutex::new(Vec::new())),
            root.path().to_path_buf(),
        ));
        let log = CapturedLog::start();
        driver.probe().await.expect("the probe runs");
        kubelet.abort();
        assert_eq!(Driver::machine(&driver).facts.exec_argv_only, None);
        let run = run_line(&log.warnings());
        assert!(
            run.contains("state.store-failed"),
            "the run failed on the store write: {run}"
        );
        assert!(
            run.contains("code=\"state.store-failed\"")
                && !run.contains("code=\"kubernetes.api-failed\""),
            "a failed store write is named an API failure: {run}"
        );
    }

    /// The probe Pod's create refused by RBAC (the other path the fix names): the run's warning
    /// names `kubernetes.api-failed` with the API's answer, and each run fact as not proved.
    #[tokio::test]
    async fn a_refused_probe_pod_create_names_its_reason_and_each_unproved_fact() {
        let fake = Arc::new(passing_fake());
        let clock = ManualClock::new();
        let (driver, _ledger) = driver(&fake, &clock);
        driver.open().await.expect("the driver opens");
        let weak: Weak<FakeCluster> = Arc::downgrade(&fake);
        answer_keeper(&fake, move || {
            if let Some(fake) = weak.upgrade() {
                fake.fault(Fault::Answer {
                    method: Method::POST,
                    path: "/api/v1/namespaces/substrate/pods".to_owned(),
                    status: 403,
                    body: json!({"kind": "Status", "apiVersion": "v1", "status": "Failure", "reason": "Forbidden", "code": 403,
                        "message": "pods is forbidden: User \"system:serviceaccount:substrate:substrate-driver\" cannot create resource \"pods\" in API group \"\" in the namespace \"substrate\""}),
                });
            }
        });
        let root = tempfile::tempdir().expect("a temporary PVC");
        let ran = Arc::new(Mutex::new(Vec::new()));
        let kubelet = tokio::spawn(play_kubelet(
            admin(&fake),
            Arc::clone(&fake),
            Play::of(Child::Exact),
            Arc::clone(&ran),
            root.path().to_path_buf(),
        ));
        let log = CapturedLog::start();
        driver.probe().await.expect("the probe runs");
        kubelet.abort();
        assert!(lock(&ran).is_empty(), "the probe Pod was never created");
        let snapshot = Driver::machine(&driver);
        assert_eq!(families(&snapshot.facts), [None; 7]);
        assert_eq!(snapshot.facts.exec_argv_only, None);
        let warnings = log.warnings();
        let run = run_line(&warnings);
        assert!(
            run.contains("code=\"kubernetes.api-failed\"") && run.contains("cannot create"),
            "{run}"
        );
        for fact in SEVEN_FAMILIES.iter().map(|family| family.fact()).chain([
            "exec.argv-only",
            substrate_wire::CONFINEMENT_WHOLE_TREE_KILL_FACT,
            substrate_wire::CONFINEMENT_NO_EGRESS_FACT,
        ]) {
            let stated = format!(" {fact} was not proved");
            assert!(
                warnings.iter().any(|line| line.contains(&stated)),
                "no warning says {fact} was not proved: {warnings:#?}"
            );
        }
    }
}
