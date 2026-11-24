//! `story:pod-force-deleted-keeps-its-policy` (security review finding J2): a Pod removed by a
//! forced deletion, `PodGC`'s after its node stopped answering or an operator's, reads 404 while
//! its container may still run on that node. A read that finds a run's Pod gone therefore releases
//! the run's `NetworkPolicy` only when the API shows the container stopped: a read of the Pod
//! showed the container terminated, or a watch of the Pod from the last `resourceVersion` the
//! adapter read replays a terminated container. The kubelet writes that state before it removes a
//! Pod it was asked to delete (kind probe, 2026-10-28). A watch that answers 410, stays silent, or
//! ends on a container last seen running keeps the policy, in the run's task, the open sweep and
//! the deferred sweep alike. A Pod marked `Failed` is not enough: `PodGC` marks a Pod `Failed`
//! before it force-deletes it.

use super::*;

/// The policy whose Pod the deferred sweep saw and then reads gone.
const DEFERRED: &str = "substrate-run-deferred-force";
/// The open sweep's one orphan: page 1's.
const ORPHAN: &str = "substrate-run-page-1";

fn policy_deleted(cluster: &Cluster, run: &str) -> bool {
    let delete =
        format!("DELETE /apis/networking.k8s.io/v1/namespaces/{NAMESPACE}/networkpolicies/{run}");
    cluster.requests.lock().contains(&delete)
}

/// A watch of `run`'s Pod by name, from a `resourceVersion`.
fn watched(cluster: &Cluster, run: &str) -> bool {
    cluster.requests.lock().iter().any(|request| {
        request.starts_with(&format!("GET /api/v1/namespaces/{NAMESPACE}/pods?watch="))
            && request.contains(&format!("fieldSelector=metadata.name%3D{run}"))
            && request.contains("resourceVersion=")
    })
}

fn cluster(watch: Watch) -> Arc<Cluster> {
    let cluster = Arc::new(Cluster {
        pages: 1,
        ..Cluster::default()
    });
    *cluster.watch.lock() = watch;
    cluster.running.store(true, Ordering::SeqCst);
    cluster
}

/// What each path did once the Pod read gone: whether it deleted the policy, and the run's phase.
struct Released {
    monitor: bool,
    phase: Phase,
    /// The limit the run's observation names once the task ended (design 36 D7).
    limit: Option<RunLimit>,
    open_sweep: bool,
    deferred_sweep: bool,
}

/// The run's task follows `RUN`, running on its node, until the Pod reads gone; with `kill`, the
/// adapter asked for its deletion first. Returns the phase and the observation's limit once the
/// task ends.
async fn follow(cluster: &Arc<Cluster>, api: Arc<Api>, kill: bool) -> (Phase, Option<RunLimit>) {
    let (run, sender) = tracked(RUN);
    let task = tokio::spawn(monitor(
        Arc::clone(&api),
        Arc::clone(&run),
        (sender, EndCell::new()),
        Instant::now() + Duration::from_mins(10),
        PodTrace::default(),
    ));
    // A read answered running, not one recorded: the fake records a request before it answers.
    let began = Instant::now();
    while cluster.run_reads_running.load(Ordering::SeqCst) == 0 {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the task never read the Pod"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if kill {
        request_stop(
            &api,
            &run,
            Signal::Kill,
            KILL_GRACE_SECONDS,
            StopOrigin::Embedder,
        )
        .await
        .expect("the deletion is asked for");
    }
    // The Pod is gone: removed by its kubelet, or force-deleted by someone else.
    cluster.pod_gone.store(true, Ordering::SeqCst);
    let finished = tokio::time::timeout(Duration::from_secs(30), task).await;
    assert!(finished.is_ok(), "the run's task did not end");
    let phase = *run.phase.borrow();
    (phase, PodAdapter::observation(&run).limit)
}

/// The open sweep's orphan and a policy the deferred sweep saw the Pod of, each read gone.
async fn sweeps(api: &Api, cluster: &Cluster) -> (bool, bool) {
    let open = async {
        sweep(api).await.expect("the open sweep");
    };
    let deferred = async {
        cluster.present.lock().insert(DEFERRED.to_owned());
        let mut deferred = vec![kept(DEFERRED)];
        resweep(api, &mut deferred).await.expect("a round");
        assert!(deferred[0].pod_seen, "the listed Pod was not recorded seen");
        cluster.present.lock().remove(DEFERRED);
        resweep(api, &mut deferred).await.expect("a round");
        assert!(
            deferred.is_empty() || !policy_deleted(cluster, DEFERRED),
            "a removed policy is still checked"
        );
    };
    tokio::join!(open, deferred);
    (
        policy_deleted(cluster, ORPHAN),
        policy_deleted(cluster, DEFERRED),
    )
}

fn kept(run: &str) -> Deferred {
    Deferred {
        run: run.to_owned(),
        created: (chrono::Utc::now() - chrono::Duration::hours(1)).fixed_offset(),
        pod_seen: false,
        trace: PodTrace::since(Some("1")),
    }
}

/// Every path at once against one fake, so a silent watch costs its bound once.
async fn released(watch: Watch, kill: bool) -> (Arc<Cluster>, Released) {
    released_by(cluster(watch), kill).await
}

/// [`released`], against a fake the case prepared.
async fn released_by(cluster: Arc<Cluster>, kill: bool) -> (Arc<Cluster>, Released) {
    let api = Arc::new(start(&cluster));
    let ((open_sweep, deferred_sweep), (phase, limit)) = tokio::join!(
        sweeps(&api, &cluster),
        follow(&cluster, Arc::clone(&api), kill)
    );
    let released = Released {
        monitor: policy_deleted(&cluster, RUN),
        phase,
        limit,
        open_sweep,
        deferred_sweep,
    };
    (cluster, released)
}

fn assert_kept(cluster: &Cluster, released: &Released, why: &str) {
    let requests = cluster.requests.lock().clone();
    assert!(
        !released.monitor,
        "{why}: the run's task deleted the policy of a Pod whose container may still run: \
         {requests:#?}"
    );
    assert!(
        matches!(released.phase, Phase::Unknown),
        "{why}: the run reads {:?}, not Unknown, while its container may still run",
        released.phase
    );
    assert!(
        !released.open_sweep,
        "{why}: the open sweep deleted the orphan's policy"
    );
    assert!(
        !released.deferred_sweep,
        "{why}: the deferred sweep deleted the policy"
    );
    for run in [RUN, ORPHAN, DEFERRED] {
        assert!(
            watched(cluster, run),
            "{why}: no watch of {run}'s Pod from a resourceVersion: {requests:#?}"
        );
    }
}

/// `PodGC` removed the Pod of a node that stopped answering: the Pod reads 404, and the last state
/// its kubelet reported is a running container. Every path keeps the policy.
#[tokio::test(flavor = "multi_thread")]
async fn a_pod_force_deleted_from_an_unreachable_node_keeps_its_policy() {
    let (cluster, released) = released(Watch::ForceDeleted, false).await;
    assert_kept(&cluster, &released, "force-deleted while running");
}

/// The adapter asked for the deletion itself (`Kill`), and then the Pod was force-deleted before
/// its kubelet answered: the Pod's removal is not the kubelet's confirmation.
#[tokio::test(flavor = "multi_thread")]
async fn a_kill_overtaken_by_a_forced_deletion_keeps_the_policy() {
    let (cluster, released) = released(Watch::ForceDeleted, true).await;
    assert_kept(
        &cluster,
        &released,
        "killed, then force-deleted while running",
    );
}

/// `story:force-deleted-pod-cases-wait-for-the-answer`: on a loaded machine the fake records the
/// task's first read of the Pod well before it answers it. The Pod is marked gone only once a read
/// was answered running, so the task still reads it running first and watches it from there.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_read_answered_late_still_reads_the_pod_running() {
    let cluster = cluster(Watch::ForceDeleted);
    *cluster.first_run_read_held.lock() = Some(Duration::from_millis(300));
    let (cluster, released) = released_by(cluster, false).await;
    assert_kept(&cluster, &released, "first read answered late");
}

/// The history after the last `resourceVersion` the adapter read is gone, answered as a 410 and
/// as a watch's `ERROR` event: the adapter cannot tell, and keeps the policy.
#[tokio::test(flavor = "multi_thread")]
async fn a_watch_answered_410_keeps_the_policy() {
    for watch in [Watch::Gone, Watch::Expired] {
        let (cluster, released) = released(watch, false).await;
        assert_kept(&cluster, &released, &format!("{watch:?}"));
    }
}

/// A watch that carries no event within its bound tells nothing, and keeps the policy.
#[tokio::test(flavor = "multi_thread")]
async fn a_watch_that_stays_silent_keeps_the_policy() {
    let began = Instant::now();
    let (cluster, released) = released(Watch::Silent, false).await;
    assert_kept(&cluster, &released, "silent watch");
    assert!(
        began.elapsed() < Duration::from_secs(20),
        "the silent watch held the paths {:?}",
        began.elapsed()
    );
}

/// The adapter's own kill: the kubelet wrote the container terminated and then removed the Pod.
/// Every path releases the policy, and the run ends on the signal.
#[tokio::test(flavor = "multi_thread")]
async fn a_kill_whose_pod_shows_its_container_terminated_releases_the_policy() {
    let (cluster, released) = released(Watch::Terminated, true).await;
    let requests = cluster.requests.lock().clone();
    assert!(
        released.monitor,
        "the run's task kept the policy: {requests:#?}"
    );
    assert!(
        matches!(released.phase, Phase::Ended(exit, _) if exit.signal == Some(Signal::Kill.number())),
        "the killed run reads {:?}",
        released.phase
    );
    assert!(
        released.open_sweep,
        "the open sweep kept the orphan's policy"
    );
    assert!(
        released.deferred_sweep,
        "the deferred sweep kept the policy"
    );
    assert_eq!(
        released.limit, None,
        "design 36 D7: an embedder's own kill names no limit"
    );
}

/// Design 36 D7, the `pod` row: the monitor's own stop past the deadline ends the run with
/// `WallTime`. The monitor is handed a give-up instant already past, so its first read of the
/// running Pod asks for the deletion; the kubelet then writes the container terminated and removes
/// the Pod, as it does for a deletion it was asked for. Design 36 D20, the `pod` row: the monitor
/// then writes the run's one `run_ended`, naming that limit, with no `observe` or `wait` called.
#[tokio::test(flavor = "multi_thread")]
async fn the_monitors_stop_past_the_deadline_names_the_wall_time_limit() {
    let cluster = cluster(Watch::Terminated);
    let api = Arc::new(start(&cluster));
    let (run, sender) = tracked(RUN);
    let (written, mut run_ended) = tokio::sync::mpsc::unbounded_channel();
    let admitted = crate::confinement::admission(
        &proof(),
        &ConfinementProfile {
            image: Some(format!("registry.invalid/run@sha256:{}", "a".repeat(64))),
            audit: Some(crate::confinement::AuditSink::new(
                move |record: &crate::confinement::AuditRecord| {
                    let value = serde_json::to_value(record).expect("a record");
                    if value["event"] == "run_ended" {
                        let _ = written.send(value);
                    }
                },
            )),
            ..ConfinementProfile::default()
        },
        |_| Ok(()),
    )
    .expect("admitted");
    let end = EndCell::new();
    let task = tokio::spawn(monitor(
        Arc::clone(&api),
        Arc::clone(&run),
        (sender, end.clone()),
        Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("an instant in the past"),
        PodTrace::default(),
    ));
    end.hand_record(admitted.audit_start(&run.id, "/bin/sleep"));
    let began = Instant::now();
    while cluster.pod_deletes.load(Ordering::SeqCst) == 0 {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the monitor never asked for the Pod's deletion past its give-up instant"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        matches!(*run.stop.lock(), Some(stop) if stop.origin == StopOrigin::Deadline),
        "the deletion was not recorded as the monitor's own"
    );
    cluster.pod_gone.store(true, Ordering::SeqCst);
    let finished = tokio::time::timeout(Duration::from_secs(30), task).await;
    assert!(finished.is_ok(), "the run's task did not end");
    let observation = PodAdapter::observation(&run);
    assert_eq!(observation.state, RunState::Ended, "{observation:?}");
    assert_eq!(
        observation.exit.and_then(|exit| exit.signal),
        Some(Signal::Kill.number()),
        "{observation:?}"
    );
    assert_eq!(
        observation.limit,
        Some(RunLimit::WallTime),
        "{observation:?}"
    );
    let record = tokio::time::timeout(Duration::from_secs(5), run_ended.recv())
        .await
        .expect("the monitor wrote run_ended")
        .expect("the sink lives");
    assert_eq!(record["run"], RUN, "{record}");
    assert_eq!(record["adapter"], "pod", "{record}");
    assert_eq!(record["profile"], admitted.id().as_str(), "{record}");
    assert_eq!(record["state"], "ended", "{record}");
    assert_eq!(record["limit"], "wall_time", "{record}");
    assert_eq!(
        record["exit"],
        serde_json::json!({"signal": Signal::Kill.number()}),
        "{record}"
    );
    assert!(run_ended.try_recv().is_err(), "a second run_ended");
}

/// A `pod` profile admitted with a sink that sends every `run_ended` it receives.
fn admitted_with_a_sink() -> (
    crate::confinement::AdmittedProfile,
    std::sync::mpsc::Receiver<serde_json::Value>,
) {
    let (written, run_ended) = std::sync::mpsc::channel();
    let written = parking_lot::Mutex::new(written);
    let admitted = crate::confinement::admission(
        &proof(),
        &ConfinementProfile {
            image: Some(format!("registry.invalid/run@sha256:{}", "a".repeat(64))),
            audit: Some(crate::confinement::AuditSink::new(
                move |record: &crate::confinement::AuditRecord| {
                    let value = serde_json::to_value(record).expect("a record");
                    if value["event"] == "run_ended" {
                        let _ = written.lock().send(value);
                    }
                },
            )),
            ..ConfinementProfile::default()
        },
        |_| Ok(()),
    )
    .expect("admitted");
    (admitted, run_ended)
}

/// A run's monitor side, as `monitor` builds it.
fn monitor_side(sender: watch::Sender<Phase>, end: &EndCell) -> Ending {
    Ending {
        phase: sender,
        end: end.clone(),
        published: false,
    }
}

fn exited_0() -> Phase {
    Phase::Ended(
        RunExit {
            code: Some(0),
            signal: None,
        },
        Some(0),
    )
}

fn current_thread() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// Design 36 D17, D20 and D22: `start`'s value first, then the publication. `publish` hands the
/// ended record to its thread before it returns, so a current-thread runtime dropped right after
/// `publish` returned, with no further poll, has the run's `run_ended` in the sink with
/// `state: ended`, never `unknown`; a second publication writes nothing.
#[test]
fn a_runtime_dropped_right_after_publish_has_the_ended_record_in_the_sink() {
    let (run, sender) = tracked(RUN);
    let (admitted, run_ended) = admitted_with_a_sink();
    let end = EndCell::new();
    let mut ending = monitor_side(sender, &end);
    end.hand_record(admitted.audit_start(&run.id, "/bin/true"));
    let runtime = current_thread();
    runtime.block_on(async {
        ending.publish(&run, exited_0());
        ending.publish(&run, Phase::Unknown);
    });
    drop(runtime);
    let record = run_ended
        .try_recv()
        .expect("run_ended is in the sink once the runtime's drop returned");
    assert_eq!(record["run"], RUN, "{record}");
    assert_eq!(record["state"], "ended", "{record}");
    assert_eq!(record["exit"], serde_json::json!({"code": 0}), "{record}");
    assert!(run_ended.try_recv().is_err(), "a second run_ended");
}

/// Design 36 D17, D20 and D22: the publication first, then `start`'s value, as where `start`'s
/// thread is held past the monitor's first poll. The value finds the ended observation and goes
/// to its thread before `hand_record` returns, so a current-thread runtime dropped right after,
/// with no further poll, has `run_ended` in the sink with `state: ended` and the adapter's
/// observation; a second publication writes nothing.
#[test]
fn a_value_handed_after_publish_then_a_runtime_dropped_records_the_ended_run() {
    let (run, sender) = tracked(RUN);
    let (admitted, run_ended) = admitted_with_a_sink();
    let end = EndCell::new();
    let mut ending = monitor_side(sender, &end);
    let runtime = current_thread();
    runtime.block_on(async {
        ending.publish(&run, exited_0());
        end.hand_record(admitted.audit_start(&run.id, "/bin/true"));
        ending.publish(&run, Phase::Unknown);
    });
    drop(runtime);
    let record = run_ended
        .try_recv()
        .expect("run_ended is in the sink once the runtime's drop returned");
    assert_eq!(record["run"], RUN, "{record}");
    assert_eq!(record["adapter"], "pod", "{record}");
    assert_eq!(record["state"], "ended", "{record}");
    assert_eq!(record["exit"], serde_json::json!({"code": 0}), "{record}");
    assert_eq!(record["memory_oom_kills"], 0, "{record}");
    assert!(run_ended.try_recv().is_err(), "a second run_ended");
}

/// Design 36 D17: a publication whose `start` never hands a value, as a start that failed once it
/// spawned the monitor, writes nothing, and nothing when the cell goes.
#[test]
fn a_publication_whose_start_never_hands_a_value_writes_nothing() {
    let (run, sender) = tracked(RUN);
    let (admitted, run_ended) = admitted_with_a_sink();
    let end = EndCell::new();
    let mut ending = monitor_side(sender, &end);
    let runtime = current_thread();
    runtime.block_on(async {
        ending.publish(&run, exited_0());
        ending.publish(&run, Phase::Unknown);
    });
    drop((ending, end, runtime));
    assert!(run_ended.try_recv().is_err(), "a run_ended without a start");
    drop(admitted);
}

/// Design 36 D22: a value handed to a monitor that is dropped before it published, as by a
/// runtime dropped while the run runs, writes `unknown` once the cell goes.
#[test]
fn a_handed_value_whose_monitor_never_publishes_writes_unknown_when_the_cell_goes() {
    let (run, sender) = tracked(RUN);
    let (admitted, run_ended) = admitted_with_a_sink();
    let end = EndCell::new();
    let ending = monitor_side(sender, &end);
    end.hand_record(admitted.audit_start(&run.id, "/bin/true"));
    drop(end);
    assert!(
        run_ended.try_recv().is_err(),
        "written while the monitor lives"
    );
    drop(ending);
    let record = run_ended
        .try_recv()
        .expect("run_ended once the cell's last holder went");
    assert_eq!(record["state"], "unknown", "{record}");
}
