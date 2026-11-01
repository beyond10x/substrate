//! `story:pod-orphaned-policy-is-swept`: a policy whose run's Pod was never created (the adapter
//! stopped between creating the policy and creating the Pod) is swept once a complete replay from
//! the policy's `resourceVersion` shows no Pod of the run's name was ever added. A watch that asks
//! for bookmarks replays every event since that version and then sends a `BOOKMARK` shortly before
//! its `timeoutSeconds` ends; for a Pod never created the `BOOKMARK` is its only line (kind v1.37.0,
//! 2026-11-01). A replay that does not end in a `BOOKMARK` (410, an `ERROR` event, a stream that
//! ends or stays silent) keeps the policy, and events of the name decide as they did before.

use super::*;

/// The open sweep's one orphan: page 1's, its Pod gone and never read.
const ORPHAN: &str = "substrate-run-page-1";

fn deleted(cluster: &Cluster, run: &str) -> bool {
    let delete =
        format!("DELETE /apis/networking.k8s.io/v1/namespaces/{NAMESPACE}/networkpolicies/{run}");
    cluster.requests.lock().contains(&delete)
}

/// The watches of `run`'s Pod the fake was asked for.
fn watches(cluster: &Cluster, run: &str) -> Vec<String> {
    cluster
        .requests
        .lock()
        .iter()
        .filter(|request| {
            request.starts_with(&format!("GET /api/v1/namespaces/{NAMESPACE}/pods?watch="))
                && request.contains(&format!("fieldSelector=metadata.name%3D{run}&"))
        })
        .cloned()
        .collect()
}

fn cluster(watch: Watch) -> Arc<Cluster> {
    let cluster = Arc::new(Cluster {
        pages: 1,
        ..Cluster::default()
    });
    *cluster.watch.lock() = watch;
    cluster
}

/// A policy the open sweep kept, created `age` ago, whose run's Pod no read has shown.
fn kept(run: &str, age: chrono::Duration) -> Deferred {
    Deferred {
        run: run.to_owned(),
        created: (chrono::Utc::now() - age).fixed_offset(),
        pod_seen: false,
        trace: PodTrace::since(Some("1")),
    }
}

/// An old owned policy whose Pod reads 404 and whose replay from the policy's `resourceVersion` is
/// one `BOOKMARK`: the Pod was never created, and open deletes the policy. The watch asks for
/// bookmarks, with a `timeoutSeconds` the client's own bound outlasts.
#[tokio::test(flavor = "multi_thread")]
async fn an_old_policy_whose_pod_was_never_created_is_swept() {
    let cluster = cluster(Watch::NeverCreated);
    let api = start(&cluster);
    let began = Instant::now();
    let deferred = sweep(&api).await.expect("the open sweep");
    let requests = cluster.requests.lock().clone();
    assert!(
        deleted(&cluster, ORPHAN),
        "open kept the policy of a Pod a complete replay shows never created: {requests:#?}"
    );
    assert!(deferred.is_empty(), "{deferred:?}");
    let watched = watches(&cluster, ORPHAN);
    assert_eq!(watched.len(), 1, "{requests:#?}");
    assert!(
        watched[0].contains("&resourceVersion=1&")
            && watched[0].contains("&allowWatchBookmarks=true")
            && watched[0].contains(&format!(
                "&timeoutSeconds={}",
                kubernetes::gone::WATCH_LIMIT.as_secs()
            )),
        "the replay did not ask for bookmarks from the policy's version: {watched:?}"
    );
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "{:?}",
        began.elapsed()
    );
}

/// A replay that does not end in a `BOOKMARK` cannot say no Pod was added: a stream that ends, a
/// 410 answered as a status or as an `ERROR` event, and one that stays silent past the client's
/// bound each keep the policy, in the open sweep and in the deferred one.
#[tokio::test(flavor = "multi_thread")]
async fn an_incomplete_replay_keeps_the_policy() {
    const DEFERRED: &str = "substrate-run-deferred-incomplete";
    for watch in [Watch::Ended, Watch::Gone, Watch::Expired, Watch::Silent] {
        let cluster = cluster(watch);
        let api = start(&cluster);
        let mut deferred = vec![kept(DEFERRED, chrono::Duration::hours(1))];
        let (open, round) = tokio::join!(sweep(&api), resweep(&api, &mut deferred));
        let requests = cluster.requests.lock().clone();
        assert_eq!(open.expect("the open sweep").len(), 0, "{watch:?}");
        round.expect("a round");
        for run in [ORPHAN, DEFERRED] {
            assert!(
                !deleted(&cluster, run),
                "{watch:?}: {run}'s policy was deleted on an incomplete replay: {requests:#?}"
            );
            assert_eq!(watches(&cluster, run).len(), 1, "{watch:?}: {requests:#?}");
        }
        assert!(
            deferred.is_empty(),
            "{watch:?}: a policy the API cannot show free is still checked"
        );
    }
}

/// Events of the run's name decide as before, `BOOKMARK` or not: a bound Pod added and deleted
/// with its container not shown terminated keeps the policy, and so does a Pod added whose end the
/// replay never showed.
#[tokio::test(flavor = "multi_thread")]
async fn a_replay_showing_the_pod_added_keeps_the_policy_without_its_end() {
    for watch in [Watch::BoundDeleted, Watch::Added] {
        let cluster = cluster(watch);
        let api = start(&cluster);
        let deferred = sweep(&api).await.expect("the open sweep");
        let requests = cluster.requests.lock().clone();
        assert!(
            !deleted(&cluster, ORPHAN),
            "{watch:?}: the policy of a Pod the replay shows added was deleted: {requests:#?}"
        );
        assert!(deferred.is_empty(), "{watch:?}: {deferred:?}");
    }
}

/// A replay that goes unanswered says nothing: the open sweep defers the policy, as before.
#[tokio::test(flavor = "multi_thread")]
async fn an_unanswered_replay_defers_the_policy() {
    let cluster = cluster(Watch::Unavailable);
    let api = start(&cluster);
    let deferred = sweep(&api).await.expect("the open sweep");
    assert!(!deleted(&cluster, ORPHAN));
    assert_eq!(
        deferred
            .iter()
            .map(|kept| kept.run.as_str())
            .collect::<Vec<_>>(),
        [ORPHAN]
    );
}

/// A policy the open sweep kept because it was young, whose Pod was never created: once it is
/// old, a round of the deferred sweep reads the Pod 404, replays a `BOOKMARK` alone and deletes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_deferred_young_policy_whose_pod_was_never_created_is_swept_once_old() {
    const YOUNG: &str = "substrate-run-deferred-young";
    let cluster = cluster(Watch::NeverCreated);
    let api = start(&cluster);
    let mut deferred = vec![kept(YOUNG, chrono::Duration::zero())];
    resweep(&api, &mut deferred).await.expect("a round");
    assert_eq!(deferred.len(), 1, "a young policy was dropped");
    assert!(
        watches(&cluster, YOUNG).is_empty(),
        "a young policy was replayed"
    );
    deferred[0].created = (chrono::Utc::now() - chrono::Duration::hours(1)).fixed_offset();
    resweep(&api, &mut deferred).await.expect("a round");
    let requests = cluster.requests.lock().clone();
    assert!(
        deleted(&cluster, YOUNG),
        "the deferred sweep kept the policy of a Pod never created: {requests:#?}"
    );
    assert!(deferred.is_empty(), "a removed policy is still checked");
}

/// The never-created rule is only for a run no read has shown a Pod of: a policy whose Pod a read
/// showed, even without a `uid`, is released only on its container shown stopped, and a replay of
/// a `BOOKMARK` alone shows no such thing.
#[tokio::test(flavor = "multi_thread")]
async fn a_policy_whose_pod_was_seen_is_not_swept_on_a_bookmark() {
    const SEEN: &str = "substrate-run-deferred-seen";
    let cluster = cluster(Watch::NeverCreated);
    let api = start(&cluster);
    let mut deferred = vec![Deferred {
        pod_seen: true,
        ..kept(SEEN, chrono::Duration::hours(1))
    }];
    resweep(&api, &mut deferred).await.expect("a round");
    let requests = cluster.requests.lock().clone();
    assert!(
        !deleted(&cluster, SEEN),
        "a bookmark released the policy of a Pod a read showed: {requests:#?}"
    );
    assert!(
        watches(&cluster, SEEN)
            .iter()
            .all(|watch| !watch.contains("allowWatchBookmarks")),
        "{requests:#?}"
    );
}
