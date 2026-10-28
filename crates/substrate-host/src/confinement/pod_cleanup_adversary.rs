//! Adversary pass 1 on `story:pod-unknown-cleanup-outlives-adapter`, finding A4: the pause between
//! two removal tries doubles from one poll and stops at one round of polls (design 28: "from
//! 500 ms to 30 seconds"). The cleanup case outlasts the former rounds in about 0.6 s with or
//! without that ceiling, so nothing else holds it.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn adv1_the_pause_between_removal_tries_stops_doubling_at_one_round() {
    let cluster = Arc::new(Cluster::default());
    let api = start(&cluster);
    let pod = format!("{}/{RUN}", api.pods());
    let began = std::time::Instant::now();
    let removed = tokio::time::timeout(
        Duration::from_secs(20),
        api.remove_until_gone(&pod, KILL_GRACE_SECONDS),
    )
    .await;
    let took = began.elapsed();
    assert!(removed.is_ok(), "the Pod was not removed within 20 s");
    assert_eq!(cluster.pod_deletes.load(Ordering::SeqCst), RELEASED_AFTER);
    // Nine pauses: 10 + 20 + 7 x 30 = 240 ms with the ceiling of one round (3 x 10 ms); without it
    // 10 x (2^9 - 1) = 5110 ms.
    assert!(
        took < Duration::from_millis(2500),
        "the pause kept doubling past one round of polls: {took:?} for {RELEASED_AFTER} tries"
    );
}
