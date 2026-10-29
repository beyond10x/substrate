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
    open_sweep: bool,
    deferred_sweep: bool,
}

/// The run's task follows `RUN`, running on its node, until the Pod reads gone; with `kill`, the
/// adapter asked for its deletion first. Returns once the task ends.
async fn follow(cluster: &Arc<Cluster>, api: Arc<Api>, kill: bool) -> Phase {
    let (run, sender) = tracked(RUN);
    let task = tokio::spawn(monitor(
        Arc::clone(&api),
        Arc::clone(&run),
        sender,
        Instant::now() + Duration::from_mins(10),
        PodTrace::default(),
    ));
    let read = format!("GET {}/{RUN}", api.pods());
    let began = Instant::now();
    while !cluster.requests.lock().contains(&read) {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the task never read the Pod"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if kill {
        request_stop(&api, &run, Signal::Kill, KILL_GRACE_SECONDS)
            .await
            .expect("the deletion is asked for");
    }
    // The Pod is gone: removed by its kubelet, or force-deleted by someone else.
    cluster.pod_gone.store(true, Ordering::SeqCst);
    let finished = tokio::time::timeout(Duration::from_secs(30), task).await;
    assert!(finished.is_ok(), "the run's task did not end");
    *run.phase.borrow()
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
    let cluster = cluster(watch);
    let api = Arc::new(start(&cluster));
    let ((open_sweep, deferred_sweep), phase) = tokio::join!(
        sweeps(&api, &cluster),
        follow(&cluster, Arc::clone(&api), kill)
    );
    let released = Released {
        monitor: policy_deleted(&cluster, RUN),
        phase,
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
}
