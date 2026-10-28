//! Security review of `story:pod-unknown-cleanup-outlives-adapter` (commit `b13033b7`): the
//! deferred sweep deletes a kept `NetworkPolicy` only once a read of its Pod answers 404, and it
//! keeps working in a namespace holding runs the adapter itself admitted.

use super::*;

const DONE: &str = "substrate-run-deferred-review-done";
const UNREADABLE: &str = "substrate-run-review-unreadable";

fn kept(run: &str, age: Duration, pod_seen: bool) -> Deferred {
    Deferred {
        run: run.to_owned(),
        created: (chrono::Utc::now() - chrono::Duration::from_std(age).expect("an age"))
            .fixed_offset(),
        pod_seen,
        trace: PodTrace::since(Some("1")),
    }
}

fn deletes(cluster: &Cluster) -> Vec<String> {
    cluster
        .requests
        .lock()
        .iter()
        .filter(|request| request.starts_with("DELETE "))
        .cloned()
        .collect()
}

/// The safety fact the deferred sweep rests on: absence from the owned Pod list never deletes a
/// policy by itself. A Pod read answering anything but 404 (here 500), or a Pod list that fails,
/// keeps every policy, however old and whether or not its Pod was seen.
#[tokio::test(flavor = "multi_thread")]
async fn review_the_deferred_sweep_deletes_only_after_a_pod_read_answers_404() {
    let cluster = Arc::new(Cluster::default());
    let api = start(&cluster);
    let mut deferred = vec![
        kept(UNREADABLE, Duration::from_hours(1), true),
        kept(UNREADABLE, Duration::from_hours(1), false),
    ];
    resweep(&api, &mut deferred).await.expect("a round");
    assert_eq!(
        deletes(&cluster),
        Vec::<String>::new(),
        "a policy was deleted while its Pod's read answered 500"
    );
    assert_eq!(
        deferred.len(),
        2,
        "an unconfirmed policy is no longer checked"
    );

    cluster.pod_list_fails.store(true, Ordering::SeqCst);
    let mut deferred = vec![kept(DONE, Duration::from_hours(1), true)];
    let round = resweep(&api, &mut deferred).await;
    assert!(round.is_err(), "a round whose Pod list failed succeeded");
    assert_eq!(
        deletes(&cluster),
        Vec::<String>::new(),
        "a policy was deleted in a round whose Pod list failed"
    );
    assert_eq!(deferred.len(), 1);
}

/// The deferred sweep lists the namespace's owned Pods whole, as full objects, in pages of 100
/// bounded at `MAX_API_BODY_BYTES` (4 MiB). A run the adapter admits may carry 256 KiB of argv
/// and 64 variables of 4 KiB (`check_pod_command`), about 520 KiB in its Pod, so ten such runs
/// alive in the namespace put the first page past the bound: every round then fails and no kept
/// policy is ever removed, its Pod long gone, while the list is still asked for every 2 s.
#[tokio::test(flavor = "multi_thread")]
async fn review_ten_admitted_runs_alive_do_not_stall_the_deferred_sweep() {
    let mut cluster = Cluster {
        pod_argv_bytes: MAX_ARGV_BYTES,
        ..Cluster::default()
    };
    for index in 0..10 {
        cluster
            .present
            .get_mut()
            .insert(format!("substrate-run-deferred-review-busy-{index}"));
    }
    let cluster = Arc::new(cluster);
    let api = start(&cluster);
    // Its Pod was seen and is gone: the policy fences nothing and should go this round.
    let mut deferred = vec![kept(DONE, Duration::from_secs(10), true)];
    let round = resweep(&api, &mut deferred).await;
    assert!(
        round.is_ok(),
        "the round failed with runs the adapter admitted alive in the namespace: {round:?}"
    );
    assert!(
        deletes(&cluster).contains(&format!("DELETE {}/{DONE}", api.policies())),
        "the policy whose Pod is gone was kept: {:#?}",
        cluster.requests.lock()
    );
    assert!(deferred.is_empty(), "a removed policy is still checked");
}
