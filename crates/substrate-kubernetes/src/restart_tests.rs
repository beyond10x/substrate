//! Restart reconciliation of execs on the fake API server (design 34 § 5, the exec start row's
//! restart column, § 5.1 and § 12 U11).
//!
//! The restart is played at the driver: the driver that started the exec is dropped without a
//! word to the cluster, and a new one is opened over the same ledger rows and the same cluster
//! state, as a new daemon process opens its driver over the same state directory. The daemon
//! cannot reach the fake (its feature is fenced from every package with a binary target), so the
//! daemon's half, that a restart asks the driver with the exec as it recorded it and keeps what
//! the driver answers, is `crates/substrate-daemon/tests/backend_ledger.rs`. Every case runs once,
//! in seconds.

use std::time::Duration;

use serde_json::json;
use substrate_executor::frame::{Frame, Stream};
use substrate_host::{Driver as _, DriverErrorClass, ExecObservation};
use substrate_kubernetes_client::Method;
use substrate_kubernetes_client::fake::Fault;
use substrate_wire::ExecState;

use super::{
    DEPLOYMENT, Rig, SNAPSHOT, applied_line, driver_over, exit_line, frame, input, rig, uid,
};
use crate::names::{self, DEPLOYMENT_LABEL, MANAGED_BY, MANAGED_BY_LABEL, OPERATION_ANNOTATION};
use crate::pod::KILL_GRACE_SECONDS;
use crate::{API_FAILED, LEASE_HELD, NamespaceDriver};

/// How long the earlier process's Lease must read unchanged before a new process takes it (§ 6.1
/// step 3, D6).
const TAKEOVER: Duration = Duration::from_secs(30);

/// The exec as the daemon records it once its restart pass ran: `unknown`, with no exit.
fn recorded_unknown(started: &ExecObservation) -> ExecObservation {
    let mut recorded = started.clone();
    recorded.resource.state = ExecState::Unknown;
    recorded.resource.exit = None;
    recorded.output_complete = true;
    recorded
}

/// A new process's driver over the same ledger and cluster takes the Lease the dropped one left:
/// refused while it may still be renewed, taken once it read unchanged for [`TAKEOVER`].
async fn take_over(rig: &mut Rig, reopened: NamespaceDriver) {
    let held = reopened
        .open()
        .await
        .expect_err("the dropped process's Lease is not yet expired");
    assert_eq!(held.code, LEASE_HELD, "{}", held.message);
    rig.clock.set(TAKEOVER);
    reopened.open().await.expect("the Lease is taken over");
    let generation = reopened.binding().expect("bound").config_generation();
    reopened.publish_exec_snapshot(SNAPSHOT, generation);
    drop(std::mem::replace(&mut rig.driver, reopened));
}

async fn restart(rig: &mut Rig) {
    let reopened = driver_over(&rig.fake, &rig.ledger, &rig.clock);
    take_over(rig, reopened).await;
}

/// N8 A: a Pod still running across a restart is deleted with grace 1 s under its uid and the
/// exec stays `unknown`, with no exit, and with the identity and request it was recorded with. A
/// process that does not yet hold the Lease touches nothing. Once the Pod reads absent under its
/// bound uid, the exec is physically absent and its row is settled.
#[tokio::test]
async fn a_running_exec_across_a_restart_is_unknown_and_its_pod_is_gone() {
    let mut rig = rig().await;
    let id = "exec_restart_running";
    let started = rig.started(id, &input(&["/bin/sleep", "600"])).await;
    let name = names::exec_pod(DEPLOYMENT, id);
    rig.running(&name).await;
    let pod_uid = uid(&rig.pod(&name).expect("the Pod runs"));
    let previous = recorded_unknown(&started);

    let reopened = driver_over(&rig.fake, &rig.ledger, &rig.clock);
    let waiting = reopened
        .recover_exec(&previous)
        .await
        .expect_err("a process without the Lease does not reconcile");
    assert_eq!(waiting.code, API_FAILED, "{}", waiting.message);
    assert!(waiting.retriable);
    assert!(
        rig.pod(&name).is_some(),
        "nothing is deleted before the Lease"
    );
    take_over(&mut rig, reopened).await;

    let recovered = rig
        .driver
        .recover_exec(&previous)
        .await
        .expect("reconciled");
    assert_eq!(recovered.resource.state, ExecState::Unknown);
    assert_eq!(recovered.resource.exit, None);
    assert_eq!(recovered.resource.id, id);
    assert_eq!(recovered.resource.workspace, started.resource.workspace);
    assert_eq!(recovered.resource.requested, started.resource.requested);
    assert_eq!(recovered.resource.applied, started.resource.applied);
    assert!(rig.pod(&name).is_none(), "the running Pod is gone");
    assert_eq!(rig.deleted_grace(&name).await, Some(KILL_GRACE_SECONDS));
    assert_eq!(rig.driver.exec_completed(), vec![recovered.clone()]);

    rig.driver.exec_acknowledge(&recovered);
    let absent = rig
        .driver
        .recover_exec(&previous)
        .await
        .expect_err("the Pod reads absent");
    assert_eq!(absent.class, DriverErrorClass::NotFound);
    let row = rig.ledger.row(&name);
    assert_eq!(row.uid.as_deref(), Some(pod_uid.as_str()));
    assert!(row.settled_at.is_some(), "the bound row is settled");
}

/// A Pod whose container shows `terminated` because a deletion is tearing it down is not an end
/// this process may report: the exec stays `unknown`, with no exit, and the deletion is sent
/// again under its uid.
#[tokio::test]
async fn a_pod_already_being_deleted_across_a_restart_stays_unknown() {
    let mut rig = rig().await;
    let id = "exec_restart_deleting";
    let started = rig.started(id, &input(&["/bin/sleep", "600"])).await;
    let name = names::exec_pod(DEPLOYMENT, id);
    let log = [applied_line(67_108_864, 500), exit_line(137)].concat();
    rig.terminated(&name, 137, log.as_bytes()).await;
    let mut pod = rig.pod(&name).expect("the Pod is present");
    pod["metadata"]["deletionTimestamp"] = json!("2026-10-29T10:00:00Z");
    rig.admin
        .update(&format!("{}/{name}", rig.admin.pods()), &pod)
        .await
        .expect("the kubelet is tearing the Pod down");
    restart(&mut rig).await;

    let recovered = rig
        .driver
        .recover_exec(&recorded_unknown(&started))
        .await
        .expect("reconciled");
    assert_eq!(recovered.resource.state, ExecState::Unknown);
    assert_eq!(recovered.resource.exit, None);
    assert!(recovered.stdout.is_empty());
    assert!(rig.pod(&name).is_none(), "the deletion was sent again");
}

/// D339 U3: a reconciliation of a running exec decides `unknown` once its deletion is answered.
/// While that delete is in flight, an observation answers the exec as the daemon recorded it,
/// `unknown`, and the daemon acknowledges that answer, so the driver forgets the exec. The
/// reconciliation's late end is answered, never a panic, and the forgotten exec is not tracked
/// again.
#[tokio::test]
async fn a_recovery_whose_exec_was_ended_and_acknowledged_meanwhile_does_not_panic() {
    let mut rig = rig().await;
    let id = "exec_restart_raced";
    let started = rig.started(id, &input(&["/bin/sleep", "600"])).await;
    let name = names::exec_pod(DEPLOYMENT, id);
    rig.running(&name).await;
    restart(&mut rig).await;
    rig.fake.fault(Fault::Delay {
        method: Method::DELETE,
        path: format!("{}/{name}", rig.admin.pods()),
        by: Duration::from_millis(400),
    });

    let previous = recorded_unknown(&started);
    let (late, first) = tokio::join!(rig.driver.recover_exec(&previous), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let first = rig.driver.exec_observe(id).await.expect("the first end");
        rig.driver.exec_acknowledge(&first);
        first
    });
    assert_eq!(first.resource.state, ExecState::Unknown);
    let late = late.expect("the late reconciliation is answered");
    assert_eq!(late.resource.state, ExecState::Unknown);
    assert_eq!(late.resource.exit, None);
    assert!(rig.pod(&name).is_none(), "the running Pod is gone");
    assert!(
        rig.driver.exec_completed().is_empty(),
        "a late end tracks the acknowledged exec again"
    );
}

/// A Pod whose container is shown `terminated` across a restart keeps its end: the log is read
/// under the bound uid, the exit is the API's, the output is the log's, and the Pod is deleted.
#[tokio::test]
async fn a_terminated_exec_across_a_restart_keeps_its_exit_and_output() {
    let mut rig = rig().await;
    let id = "exec_restart_ended";
    let started = rig.started(id, &input(&["/bin/true"])).await;
    let name = names::exec_pod(DEPLOYMENT, id);
    let log = [
        applied_line(67_108_864, 500),
        frame(&Frame::Output {
            stream: Stream::Stdout,
            bytes: b"before the restart".to_vec(),
        }),
        frame(&Frame::Output {
            stream: Stream::Stderr,
            bytes: b"to err".to_vec(),
        }),
        exit_line(4),
    ]
    .concat();
    rig.terminated(&name, 4, log.as_bytes()).await;
    restart(&mut rig).await;

    let recovered = rig
        .driver
        .recover_exec(&recorded_unknown(&started))
        .await
        .expect("reconciled");
    assert_eq!(recovered.resource.state, ExecState::Exited);
    let exit = recovered.resource.exit.clone().expect("an exit");
    assert_eq!((exit.code, exit.signal), (Some(4), None));
    assert_eq!(recovered.stdout, b"before the restart");
    assert_eq!(recovered.stderr, b"to err");
    assert!(recovered.output_complete);
    assert_eq!(recovered.resource.workspace, started.resource.workspace);
    assert_eq!(
        rig.output(id, substrate_wire::OutputStream::Stdout)
            .expect("stdout"),
        b"before the restart"
    );
    assert!(
        rig.pod(&name).is_none(),
        "the Pod is deleted once its end is read"
    );
    assert_eq!(rig.driver.exec_completed(), vec![recovered]);
}

/// § 5.1: a `pod` row reserved before a create whose answer never came. Absent, it stays
/// undecided (a retriable failure, the row open) until the N9 bound has passed on the driver's
/// wall clock; then it is `NotFound` with no uid ever bound, the row closed, which the daemon
/// records as the operation `failed` with no resource. A create that did land is adopted by its
/// operation annotation and, running, is deleted like any running Pod across a restart.
#[tokio::test]
async fn an_unbound_create_is_failed_past_the_bound_and_a_landed_one_is_adopted() {
    let mut rig = rig().await;
    let never = "exec_restart_never";
    let landed = "exec_restart_landed";
    let template = rig
        .started("exec_restart_template", &input(&["/bin/true"]))
        .await;
    let never_pod = rig.reserve(never);
    let landed_pod = rig.reserve(landed);
    let row = rig.ledger.row(&landed_pod);
    rig.fake
        .seed(
            "pods",
            json!({
                "metadata": {
                    "name": landed_pod,
                    "labels": {
                        MANAGED_BY_LABEL: MANAGED_BY,
                        DEPLOYMENT_LABEL: names::deployment_label(DEPLOYMENT),
                        "b10x-substrate/role": "pod",
                    },
                    "annotations": {
                        OPERATION_ANNOTATION: names::operation_annotation(&row.operation_digest),
                    },
                },
                "spec": {"restartPolicy": "Never", "nodeName": "node-a"},
                "status": {"phase": "Running"},
            }),
        )
        .expect("the create landed before its answer was lost");
    restart(&mut rig).await;
    let recorded = |id: &str| {
        let mut previous = recorded_unknown(&template);
        previous.resource.id = id.to_owned();
        previous
    };

    let undecided = rig
        .driver
        .recover_exec(&recorded(never))
        .await
        .expect_err("inside the bound the create may still land");
    assert_eq!(undecided.code, API_FAILED, "{}", undecided.message);
    assert!(undecided.retriable);
    assert!(rig.ledger.row(&never_pod).settled_at.is_none());

    let adopted = rig
        .driver
        .recover_exec(&recorded(landed))
        .await
        .expect("the landed Pod is adopted");
    assert_eq!(adopted.resource.state, ExecState::Unknown);
    assert_eq!(adopted.resource.exit, None);
    assert!(
        rig.ledger.row(&landed_pod).uid.is_some(),
        "its uid is bound"
    );
    assert!(rig.pod(&landed_pod).is_none(), "the running Pod is gone");

    let past = TAKEOVER + rig.driver.dispatch_bound() + Duration::from_secs(1);
    rig.clock.set(past);
    rig.driver.renew().await;
    let failed = rig
        .driver
        .recover_exec(&recorded(never))
        .await
        .expect_err("past the bound nothing was stored");
    assert_eq!(failed.class, DriverErrorClass::NotFound);
    let row = rig.ledger.row(&never_pod);
    assert_eq!(row.uid, None, "no uid was ever bound");
    assert!(row.settled_at.is_some(), "the row is closed");
    assert!(rig.pod(&never_pod).is_none());
}

/// Security review, design 34 U11, invariant 3: the exec the daemon's restart pass recorded is
/// `unknown` with `output_complete` set, and a restarted driver seeds its answer from it. A
/// terminated Pod whose log is a protocol failure (§ 7.3) still has no output proven, so the
/// recovered exec must not say its (empty) output is whole, exactly as
/// `a_non_frame_line_or_an_overlong_log_is_a_protocol_failure` holds without a restart. The
/// daemon stores what this answers and serves the stored bytes with `eof` from that flag.
#[tokio::test]
async fn a_protocol_failure_across_a_restart_is_not_whole_output() {
    let mut rig = rig().await;
    let id = "exec_restart_interloper";
    let started = rig.started(id, &input(&["/bin/true"])).await;
    let name = names::exec_pod(DEPLOYMENT, id);
    let log = [
        applied_line(67_108_864, 500),
        "hello from a sidecar\n".to_owned(),
        exit_line(0),
    ]
    .concat();
    rig.terminated(&name, 0, log.as_bytes()).await;
    restart(&mut rig).await;

    let recovered = rig
        .driver
        .recover_exec(&recorded_unknown(&started))
        .await
        .expect("reconciled");
    assert_eq!(recovered.resource.exit.and_then(|exit| exit.code), Some(0));
    assert!(recovered.stdout.is_empty());
    assert!(
        !recovered.output_complete,
        "a recovered exec whose log is a protocol failure reports its empty output as whole"
    );
}

/// The same class as the protocol failure: a running exec killed across a restart reads no log,
/// so the recorded `output_complete` must not survive into its answer.
#[tokio::test]
async fn a_killed_exec_across_a_restart_is_not_whole_output() {
    let mut rig = rig().await;
    let id = "exec_restart_killed_output";
    let started = rig.started(id, &input(&["/bin/sleep", "600"])).await;
    let name = names::exec_pod(DEPLOYMENT, id);
    rig.running(&name).await;
    restart(&mut rig).await;

    let recovered = rig
        .driver
        .recover_exec(&recorded_unknown(&started))
        .await
        .expect("reconciled");
    assert_eq!(recovered.resource.state, ExecState::Unknown);
    assert!(recovered.stdout.is_empty());
    assert!(
        !recovered.output_complete,
        "a killed exec whose log was never read reports its empty output as whole"
    );
}
