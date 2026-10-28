//! `story:pod-unknown-cleanup-outlives-adapter`, the two manual kind cases (2026-10-28):
//!
//! * a fresh adapter's open sweep keeps an owned policy younger than [`SWEEP_AGE_FLOOR`] or whose
//!   Pod exists, and the adapter checks it again, so the policy goes once its Pod does, as
//!   `cleanup_pass1::adversary_lane_a_run_read_unknown_leaves_nothing_once_its_process_has_ended`
//!   requires; never before the floor while the Pod was never seen (security finding S1);
//! * after `Unknown` the task reads the Pod between two backed-off deletions, so it notices the
//!   kubelet's removal within [`Pacing::notice`], as
//!   `cleanup_pass1::adversary_lane_a_run_read_unknown_is_removed_after_a_longer_kubelet_absence`
//!   requires.

use super::*;

const YOUNG: &str = "substrate-run-deferred-young";
const SEEN: &str = "substrate-run-deferred-seen";

fn kept(run: &str, age: Duration) -> Deferred {
    Deferred {
        run: run.to_owned(),
        created: (chrono::Utc::now() - chrono::Duration::from_std(age).expect("an age"))
            .fixed_offset(),
        pod_seen: false,
    }
}

fn policy_deleted(cluster: &Cluster, api: &Api, run: &str) -> bool {
    cluster
        .requests
        .lock()
        .contains(&format!("DELETE {}/{run}", api.policies()))
}

/// Security finding S1 holds in the deferred sweep: a policy whose Pod the API never showed stays
/// until it is [`SWEEP_AGE_FLOOR`] old, because its start may still create the Pod; then it goes.
#[tokio::test(flavor = "multi_thread")]
async fn the_deferred_sweep_keeps_a_young_policy_until_it_passes_the_floor() {
    let cluster = Arc::new(Cluster::default());
    let api = start(&cluster);
    let mut deferred = vec![kept(YOUNG, Duration::from_secs(10))];
    resweep(&api, &mut deferred).await.expect("a round");
    assert!(
        !policy_deleted(&cluster, &api, YOUNG),
        "a policy 10 s old whose Pod was never seen was removed while its start may still create \
         the Pod"
    );
    assert_eq!(deferred.len(), 1, "the young policy is no longer checked");
    deferred[0].created -= chrono::Duration::from_std(SWEEP_AGE_FLOOR).expect("the floor");
    resweep(&api, &mut deferred).await.expect("a round");
    assert!(
        policy_deleted(&cluster, &api, YOUNG),
        "a policy past the floor whose Pod is gone was kept: {:#?}",
        cluster.requests.lock()
    );
    assert!(deferred.is_empty(), "a removed policy is still checked");
}

/// A policy whose Pod the API showed goes once the Pod is gone, whatever its age: its start stored
/// the Pod, so nothing creates it again.
#[tokio::test(flavor = "multi_thread")]
async fn the_deferred_sweep_removes_a_young_policy_once_the_pod_it_saw_is_gone() {
    let cluster = Arc::new(Cluster::default());
    cluster.present.lock().insert(SEEN.to_owned());
    let api = start(&cluster);
    let mut deferred = vec![kept(SEEN, Duration::from_secs(10))];
    resweep(&api, &mut deferred).await.expect("a round");
    assert!(
        !policy_deleted(&cluster, &api, SEEN),
        "a policy whose Pod exists was removed"
    );
    assert!(deferred[0].pod_seen, "the listed Pod was not recorded seen");
    cluster.present.lock().remove(SEEN);
    resweep(&api, &mut deferred).await.expect("a round");
    assert!(
        policy_deleted(&cluster, &api, SEEN),
        "the Pod the sweep saw is gone and its policy stays: {:#?}",
        cluster.requests.lock()
    );
    assert!(deferred.is_empty(), "a removed policy is still checked");
}

/// What open keeps is checked again on a thread of its own, outside any runtime as `open` may be,
/// every `notice`, until nothing is left; then the thread ends.
#[test]
fn the_adapter_checks_what_its_open_kept_until_nothing_is_left() {
    let cluster = Arc::new(Cluster::default());
    cluster.present.lock().insert(SEEN.to_owned());
    let api = Arc::new(start(&cluster));
    let thread = defer_sweep(&api, vec![kept(SEEN, Duration::from_secs(10))]).expect("spawned");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !policy_deleted(&cluster, &api, SEEN),
        "a policy whose Pod exists was removed"
    );
    cluster.present.lock().remove(SEEN);
    let gone = Instant::now();
    while !thread.is_finished() && gone.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        policy_deleted(&cluster, &api, SEEN),
        "the Pod is gone and its policy stays: {:#?}",
        cluster.requests.lock()
    );
    assert!(
        thread.is_finished(),
        "the deferred sweep still runs with nothing left to check"
    );
    assert!(
        gone.elapsed() < Duration::from_secs(1),
        "the policy went {:?} after its Pod, more than a few rounds of {:?}",
        gone.elapsed(),
        api.pacing.notice
    );
}

/// The thread ends with the adapter and its runs' tasks, whatever it still checks.
#[test]
fn the_deferred_sweep_ends_with_the_adapter() {
    let cluster = Arc::new(Cluster::default());
    cluster.present.lock().insert(SEEN.to_owned());
    let api = Arc::new(start(&cluster));
    let thread = defer_sweep(&api, vec![kept(SEEN, Duration::from_secs(10))]).expect("spawned");
    std::thread::sleep(Duration::from_millis(100));
    drop(api);
    let dropped = Instant::now();
    while !thread.is_finished() && dropped.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        thread.is_finished(),
        "the deferred sweep outlived the adapter"
    );
}

/// After `Unknown` the deletions back off to one round of polls, and the task reads the Pod every
/// `notice` in between: the kubelet's removal is noticed within a few of those, not at the next
/// deletion.
#[tokio::test(flavor = "multi_thread")]
async fn a_removal_notices_the_pod_gone_between_backed_off_deletions() {
    let cluster = Arc::new(Cluster::default());
    cluster.kept.store(true, Ordering::SeqCst);
    let mut api = start(&cluster);
    // Deletions back off to 10 s; the reads come every 20 ms.
    api.pacing = Pacing {
        poll: Duration::from_millis(10),
        remove_attempts: 1000,
        notice: Duration::from_millis(20),
    };
    let api = Arc::new(api);
    let pod = format!("{}/{RUN}", api.pods());
    let removing = Arc::clone(&api);
    let task =
        tokio::spawn(async move { removing.remove_until_gone(&pod, KILL_GRACE_SECONDS).await });
    // Nine deletions by now, the next one about 2.5 s away.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!task.is_finished(), "the kept Pod was read gone");
    cluster.pod_gone.store(true, Ordering::SeqCst);
    let gone = Instant::now();
    let finished = tokio::time::timeout(Duration::from_secs(20), task).await;
    let noticed = gone.elapsed();
    assert!(finished.is_ok(), "the removal never noticed the Pod gone");
    assert!(
        noticed < Duration::from_secs(1),
        "the removal noticed the Pod gone {noticed:?} after the kubelet removed it, at its next \
         backed-off deletion"
    );
}
