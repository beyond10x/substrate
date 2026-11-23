//! Review pass 2 of `story:namespace-driver-start-holds-its-proof-until-pod-create` (design 34
//! § 6.3): cases a reviewer added beside the story's own, each on the fake API server and each run
//! once. They hold the driver's memory of stated snapshots to the rules § 6.3 writes for it: a
//! start is held to the Lease of its admission, the memory keeps the newest
//! [`MAX_STATED_SNAPSHOTS`], reading a snapshot again adds nothing to it, and a snapshot another
//! process stated is not in it.

use super::*;
use crate::exec::MAX_STATED_SNAPSHOTS;

const RULE: &str = "design 34 § 6.3";
/// When a later probe runs, on the case's clock: its proof is valid until another instant than
/// the first one's, so its snapshot has another digest. The Lease stays held.
const LATER: Duration = Duration::from_secs(1);
/// How long another process's Lease must read unchanged before it is taken (§ 6.1 step 3).
const TAKEOVER: Duration = Duration::from_secs(30);

/// A start as the daemon admits it now: it names the snapshot the driver states.
fn admitted(rig: &Rig) -> ExecStartInput {
    let mut start = input(&["/bin/true"]);
    start.sandbox.capability_snapshot = rig.driver.machine().snapshot;
    start
}

/// The start was refused `exec.capability-stale`, which the daemon answers 422 and records as
/// refused, and nothing was created under its row.
fn assert_stale_before_any_pod(rig: &Rig, id: &str, outcome: DispatchOutcome<ExecObservation>) {
    let refused = match outcome {
        DispatchOutcome::NotDispatched(refused) => refused,
        DispatchOutcome::Observed(observation) => panic!(
            "{RULE}: the start was dispatched, {:?}",
            observation.resource.state
        ),
        DispatchOutcome::ContainedAbsent(error) | DispatchOutcome::OutcomeUnknown(error) => {
            panic!("{RULE}: {}: {}", error.code, error.message)
        }
    };
    assert_eq!(refused.code, CAPABILITY_STALE, "{}", refused.message);
    assert_eq!(refused.class, DriverErrorClass::Refused);
    assert_eq!(refused.address.as_deref(), Some("capability_snapshot"));
    assert_eq!(rig.pod_creates(), 0, "no Pod create was sent");
    let pod = names::exec_pod(DEPLOYMENT, id);
    assert_eq!(rig.ledger.row(&pod).uid, None, "no uid is bound to the row");
    assert_eq!(rig.pod(&pod), None);
}

/// The start was served: one Pod was created for it.
fn assert_served(rig: &Rig, id: &str, outcome: DispatchOutcome<ExecObservation>) {
    match outcome {
        DispatchOutcome::Observed(observation) => {
            assert_eq!(observation.resource.state, ExecState::Running);
        }
        DispatchOutcome::NotDispatched(error)
        | DispatchOutcome::ContainedAbsent(error)
        | DispatchOutcome::OutcomeUnknown(error) => panic!(
            "{RULE}: the start was not served: {}: {}",
            error.code, error.message
        ),
    }
    assert_eq!(rig.pod_creates(), 1);
    assert!(rig.pod(&names::exec_pod(DEPLOYMENT, id)).is_some());
}

/// § 6.3: "a Lease lost or taken again since the admission is `exec.capability-stale` although
/// the same holder has it". The Lease epoch is not in the digest, and the digest is all the
/// start carries. Here the probe under the Lease taken again reads the same wall-clock instant as
/// the probe before it, so both snapshots have one digest, and the daemon reads the snapshot
/// again before the start reaches the driver. The start was admitted under the first Lease and
/// must not be served under the second.
#[tokio::test]
async fn one_digest_stated_under_two_leases_holds_the_start_to_the_lease_of_its_admission() {
    let rig = rig().await;
    let start = admitted(&rig);
    rig.driver.forget_probe();
    rig.driver.prove_on(NODE);
    let again = rig.driver.machine();
    assert_eq!(
        again.snapshot, start.sandbox.capability_snapshot,
        "the control: the two Leases' snapshots have one digest"
    );
    assert_eq!(
        again.facts.confinement_no_egress,
        Some(true),
        "the control: the new Lease's probe proved the fact"
    );
    assert!(rig.driver.lease_held(), "the control: the Lease is held");
    let id = "exec_one_digest_two_leases";
    let outcome = rig.start(id, &start).await;
    assert_stale_before_any_pod(&rig, id, outcome);
}

/// The same Lease loss with the later probe at a later instant, as two probes of a running
/// daemon are: the renewed proof has another digest, and the start admitted under the first Lease
/// is stale although the daemon read the new snapshot before the start reached the driver.
#[tokio::test]
async fn a_lease_taken_again_before_the_start_reaches_the_driver_is_stale() {
    let rig = rig().await;
    let start = admitted(&rig);
    rig.driver.forget_probe();
    rig.clock.set(LATER);
    rig.driver.prove_on(NODE);
    let again = rig.driver.machine();
    assert_ne!(
        again.snapshot, start.sandbox.capability_snapshot,
        "the control: the renewed proof has another digest"
    );
    assert_eq!(
        again.facts.confinement_no_egress,
        Some(true),
        "the control: the new Lease's probe proved the fact"
    );
    assert!(rig.driver.lease_held(), "the control: the Lease is held");
    let id = "exec_lease_retaken_before";
    let outcome = rig.start(id, &start).await;
    assert_stale_before_any_pod(&rig, id, outcome);
}

/// § 6.3: "It keeps the newest 2,048". A start naming the oldest of exactly that many stated
/// snapshots is still remembered and is served under the renewed proof; the story's own case
/// holds the other side, one snapshot more.
#[tokio::test]
async fn the_oldest_of_the_remembered_snapshots_still_serves_its_start() {
    assert_eq!(MAX_STATED_SNAPSHOTS, 2_048, "{RULE} states the bound");
    let rig = rig().await;
    let start = admitted(&rig);
    for later in 1..MAX_STATED_SNAPSHOTS {
        rig.clock.set(Duration::from_millis(
            u64::try_from(later).expect("a small count"),
        ));
        rig.driver.prove_on(NODE);
        assert_ne!(
            rig.driver.machine().snapshot,
            start.sandbox.capability_snapshot,
            "the control: every later proof has another digest"
        );
    }
    assert!(rig.driver.lease_held(), "the control: the Lease is held");
    let id = "exec_oldest_remembered";
    let outcome = rig.start(id, &start).await;
    assert_served(&rig, id, outcome);
}

/// Reading a snapshot again adds nothing to the memory, and the digest does not move while the
/// proof stands, whatever the clocks read: a caller that reads the machine route more often than
/// the memory is long does not push a waiting start's admission out of it.
#[tokio::test]
async fn reading_one_snapshot_again_and_again_pushes_no_admission_out_of_the_memory() {
    let rig = rig().await;
    let start = admitted(&rig);
    rig.clock.set(LATER);
    rig.driver.prove_on(NODE);
    let renewed = rig.driver.machine();
    assert_ne!(
        renewed.snapshot, start.sandbox.capability_snapshot,
        "the control: the renewed proof has another digest"
    );
    for read in 1..=MAX_STATED_SNAPSHOTS + 16 {
        rig.clock
            .set(LATER + Duration::from_millis(u64::try_from(read).expect("a small count")));
        let read_again = rig.driver.machine();
        assert_eq!(
            read_again.snapshot, renewed.snapshot,
            "{RULE}: the digest moved without a probe"
        );
        assert_eq!(read_again.valid_until, renewed.valid_until);
    }
    assert!(rig.driver.lease_held(), "the control: the Lease is held");
    let id = "exec_reads_do_not_evict";
    let outcome = rig.start(id, &start).await;
    assert_served(&rig, id, outcome);
}

/// A clock whose two readings a case moves separately: a host's wall clock is set, or runs on
/// through a suspend, while its monotonic clock does not move.
struct SplitClock {
    base: Instant,
    wall_base: chrono::DateTime<Utc>,
    readings: Mutex<(Duration, Duration)>,
}

impl SplitClock {
    /// The monotonic clock reads `elapsed` after its start, the wall clock `wall` after its own.
    fn set(&self, elapsed: Duration, wall: Duration) {
        *self.readings.lock().unwrap_or_else(PoisonError::into_inner) = (elapsed, wall);
    }

    fn readings(&self) -> (Duration, Duration) {
        *self.readings.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Clock for SplitClock {
    fn now(&self) -> Instant {
        self.base + self.readings().0
    }

    fn wall(&self) -> chrono::DateTime<Utc> {
        self.wall_base
            + chrono::Duration::from_std(self.readings().1).expect("a case's offset is small")
    }
}

/// The machine route's contract as the shipped conformance runner holds it (design 31's
/// inventory, group `machine`: "`valid_until` not passed";
/// `crates/b10x-substrate-conformance/src/cases.rs`, `MACHINE_VALID_UNTIL`): a snapshot never
/// states a `valid_until` that has passed. § 6.3 lets the wall clock state `valid_until` and
/// decide nothing, so after the wall clock is set forward past the proof's span, or runs on
/// through a suspend, with the monotonic clock unmoved, the snapshot must not state a passed
/// `valid_until` beside its facts.
#[tokio::test]
async fn a_snapshot_never_states_a_valid_until_that_has_passed() {
    let mut rig = rig().await;
    let clock = Arc::new(SplitClock {
        base: Instant::now(),
        wall_base: Utc::now(),
        readings: Mutex::new((Duration::ZERO, Duration::ZERO)),
    });
    let driver = NamespaceDriver::parse_with_clock(
        &kubeconfig_with(&rig.fake, Some(extension())),
        DEPLOYMENT,
        Arc::clone(&rig.ledger) as Arc<dyn BackendLedger>,
        Arc::clone(&clock) as Arc<dyn Clock>,
    )
    .expect("the configuration loads");
    let held = driver
        .open()
        .await
        .expect_err("the rig's own driver holds the Lease");
    assert_eq!(held.code, crate::LEASE_HELD, "{}", held.message);
    clock.set(TAKEOVER, TAKEOVER);
    driver.open().await.expect("the Lease is taken over");
    driver.prove_on(NODE);
    drop(std::mem::replace(&mut rig.driver, driver));

    let proved = rig.driver.machine();
    let until = proved.valid_until.expect("a proved snapshot is bounded");
    assert!(until > clock.wall(), "the control: the proof is fresh");
    let window = (until - proved.probed_at)
        .to_std()
        .expect("the proof is valid after the probe");

    clock.set(TAKEOVER, TAKEOVER + window + Duration::from_secs(1));
    assert!(rig.driver.lease_held(), "the control: the Lease is held");
    let read = rig.driver.machine();
    assert!(
        read.valid_until.is_none_or(|until| until > clock.wall()),
        "design 31, machine group: the snapshot states valid_until {:?}, which passed at the \
         wall clock's {:?}, with confinement.no-egress {:?}",
        read.valid_until,
        clock.wall(),
        read.facts.confinement_no_egress
    );
}

/// § 6.3: "A start that names a snapshot the driver does not remember, one never stated by this
/// process included, is `exec.capability-stale`." The process that takes the namespace over
/// proved every fact on the keeper's node; a start admitted by the process before it names that
/// process's snapshot and creates no Pod.
#[tokio::test]
async fn a_start_admitted_by_the_process_before_a_restart_is_stale() {
    let mut rig = rig().await;
    let start = admitted(&rig);
    let reopened = driver_over(&rig.fake, &rig.ledger, &rig.clock);
    let held = reopened
        .open()
        .await
        .expect_err("the earlier process's Lease is not yet expired");
    assert_eq!(held.code, crate::LEASE_HELD, "{}", held.message);
    rig.clock.set(TAKEOVER);
    reopened.open().await.expect("the Lease is taken over");
    reopened.prove_on(NODE);
    let current = reopened.machine();
    assert_eq!(
        current.facts.confinement_no_egress,
        Some(true),
        "the control: the new process proved the fact"
    );
    assert_ne!(
        current.snapshot, start.sandbox.capability_snapshot,
        "the control: the new process states another snapshot"
    );
    drop(std::mem::replace(&mut rig.driver, reopened));
    let id = "exec_admitted_before_restart";
    let outcome = rig.start(id, &start).await;
    assert_stale_before_any_pod(&rig, id, outcome);
}
