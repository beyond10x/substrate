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

/// The listed policy's `creationTimestamp`, as the fake answers it.
const CREATED: &str = "2020-01-01T00:00:00Z";

/// Makes each listed policy created at `created`, with one `managedFields` entry per time in
/// `writes`: the adapter's create first, then another writer's (an operator adding a label);
/// `None` lists no `managedFields`.
fn written(cluster: &Cluster, created: &str, writes: Option<&[&str]>) {
    let mut metadata = json!({"creationTimestamp": created});
    if let Some(writes) = writes {
        let fields: Vec<Value> = writes
            .iter()
            .enumerate()
            .map(|(index, time)| {
                json!({
                    "manager": if index == 0 { "b10x-substrate-pod" } else { "kubectl-label" },
                    "operation": "Update",
                    "apiVersion": "networking.k8s.io/v1",
                    "time": time,
                    "fieldsType": "FieldsV1",
                    "fieldsV1": {"f:metadata": {"f:labels": {"f:team": {}}}},
                })
            })
            .collect();
        metadata["managedFields"] = json!(fields);
    }
    *cluster.policy_metadata.lock() = Some(metadata);
}

/// `story:pod-sweep-replays-from-the-policys-create`: a policy whose metadata shows a write after
/// its create, another `managedFields` entry (even in the same second, as times have second
/// precision) or its one entry's time after `creationTimestamp`, has a `resourceVersion` past its
/// create, so a replay from it may start past the Pod's whole history. The never-created rule does
/// not sweep it: open keeps it, logged, and does not check it again, as a policy whose container
/// is not shown stopped.
#[tokio::test(flavor = "multi_thread")]
async fn a_policy_written_after_its_create_is_not_swept_as_never_created() {
    let began = Instant::now();
    for writes in [
        &[CREATED, CREATED][..],
        &[CREATED, "2020-01-02T09:30:00Z"],
        &["2020-01-01T00:00:01Z"],
    ] {
        let cluster = cluster(Watch::NeverCreated);
        written(&cluster, CREATED, Some(writes));
        let api = start(&cluster);
        let deferred = sweep(&api).await.expect("the open sweep");
        let requests = cluster.requests.lock().clone();
        assert!(
            !deleted(&cluster, ORPHAN),
            "{writes:?}: open swept a policy written after its create as never created \
             (story:pod-sweep-replays-from-the-policys-create): {requests:#?}"
        );
        assert!(deferred.is_empty(), "{writes:?}: {deferred:?}");
    }
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "{:?}",
        began.elapsed()
    );
}

/// A policy unchanged since its create, one `managedFields` entry at or before its
/// `creationTimestamp`, or none listed, is swept as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_policy_unchanged_since_its_create_is_swept_as_never_created() {
    for writes in [
        Some(&[CREATED][..]),
        Some(&["2019-12-31T23:59:59Z"][..]),
        None,
    ] {
        let cluster = cluster(Watch::NeverCreated);
        written(&cluster, CREATED, writes);
        let api = start(&cluster);
        let deferred = sweep(&api).await.expect("the open sweep");
        let requests = cluster.requests.lock().clone();
        assert!(
            deleted(&cluster, ORPHAN),
            "{writes:?}: open kept the policy of a Pod a complete replay shows never created: \
             {requests:#?}"
        );
        assert!(deferred.is_empty(), "{writes:?}: {deferred:?}");
    }
}

/// A young policy written after its create, which open defers, is not swept as never created by
/// the deferred sweep once it is old: it is kept and checked no more.
#[tokio::test(flavor = "multi_thread")]
async fn a_deferred_policy_written_after_its_create_is_not_swept_as_never_created() {
    let cluster = cluster(Watch::NeverCreated);
    let now = chrono::Utc::now();
    let created =
        (now - chrono::Duration::minutes(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let later = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    written(&cluster, &created, Some(&[&created, &later]));
    let api = start(&cluster);
    let mut deferred = sweep(&api).await.expect("the open sweep");
    assert_eq!(
        deferred
            .iter()
            .map(|kept| kept.run.as_str())
            .collect::<Vec<_>>(),
        [ORPHAN],
        "open did not defer a young policy"
    );
    deferred[0].created = (now - chrono::Duration::hours(1)).fixed_offset();
    resweep(&api, &mut deferred).await.expect("a round");
    let requests = cluster.requests.lock().clone();
    assert!(
        !deleted(&cluster, ORPHAN),
        "the deferred sweep swept a policy written after its create as never created \
         (story:pod-sweep-replays-from-the-policys-create): {requests:#?}"
    );
    assert!(deferred.is_empty(), "a kept policy is still checked");
}

/// Adversary pass 1 of wave D354 (`story:pod-sweep-replays-from-the-policys-create`): the edges of
/// what the listed metadata shows of a write after the create.
mod d354_adv {
    use super::*;

    /// Lists the orphan with `metadata` merged into its own, sweeps once, and returns whether the
    /// policy was deleted, with what open deferred.
    async fn swept_with(cluster: &Arc<Cluster>, metadata: Value) -> (bool, Vec<Deferred>) {
        *cluster.policy_metadata.lock() = Some(metadata);
        let api = start(cluster);
        let deferred = sweep(&api).await.expect("the open sweep");
        (deleted(cluster, ORPHAN), deferred)
    }

    /// One `managedFields` entry whose `time` cannot be read (absent, `null`, not a string, not
    /// RFC 3339) does not show the policy unchanged since its create: it is kept, not deferred.
    #[tokio::test(flavor = "multi_thread")]
    async fn one_entry_without_a_readable_time_is_kept() {
        let began = Instant::now();
        for entry in [
            json!({"manager": "b10x-substrate-pod", "operation": "Update"}),
            json!({"manager": "b10x-substrate-pod", "operation": "Update", "time": null}),
            json!({"manager": "b10x-substrate-pod", "operation": "Update", "time": 0}),
            json!({"manager": "b10x-substrate-pod", "operation": "Update", "time": "yesterday"}),
            json!({"manager": "b10x-substrate-pod", "operation": "Update", "time": ""}),
        ] {
            let cluster = cluster(Watch::NeverCreated);
            let (deleted, deferred) = swept_with(
                &cluster,
                json!({"creationTimestamp": CREATED, "managedFields": [entry]}),
            )
            .await;
            let requests = cluster.requests.lock().clone();
            assert!(
                !deleted,
                "{entry}: an unreadable entry time swept the policy as never created \
                 (story:pod-sweep-replays-from-the-policys-create): {requests:#?}"
            );
            assert!(deferred.is_empty(), "{entry}: {deferred:?}");
        }
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "{:?}",
            began.elapsed()
        );
    }

    /// `managedFields` that is neither absent nor a list is not read as unchanged: kept.
    #[tokio::test(flavor = "multi_thread")]
    async fn managed_fields_that_is_not_a_list_is_kept() {
        for fields in [json!({}), json!("b10x-substrate-pod"), json!(1)] {
            let cluster = cluster(Watch::NeverCreated);
            let (deleted, deferred) = swept_with(
                &cluster,
                json!({"creationTimestamp": CREATED, "managedFields": fields}),
            )
            .await;
            assert!(
                !deleted,
                "{fields}: swept as never created (story:pod-sweep-replays-from-the-policys-create)"
            );
            assert!(deferred.is_empty(), "{fields}: {deferred:?}");
        }
    }

    /// `managedFields` listed empty or `null` shows no write, as none listed does: swept as before.
    #[tokio::test(flavor = "multi_thread")]
    async fn managed_fields_empty_or_null_is_swept_as_before() {
        for fields in [json!([]), Value::Null] {
            let cluster = cluster(Watch::NeverCreated);
            let (deleted, deferred) = swept_with(
                &cluster,
                json!({"creationTimestamp": CREATED, "managedFields": fields}),
            )
            .await;
            let requests = cluster.requests.lock().clone();
            assert!(
                deleted,
                "{fields}: open kept a policy whose metadata shows no write: {requests:#?}"
            );
            assert!(deferred.is_empty(), "{fields}: {deferred:?}");
        }
    }

    /// Times are compared as instants, not as text: the create's own instant written in another
    /// offset or with fractional zeros is swept, one second past it in another offset is kept.
    #[tokio::test(flavor = "multi_thread")]
    async fn one_entry_time_is_compared_as_an_instant() {
        for (time, swept) in [
            ("2020-01-01T01:00:00+01:00", true),
            ("2020-01-01T00:00:00.000Z", true),
            ("2019-12-31T19:00:00-05:00", true),
            ("2020-01-01T01:00:01+01:00", false),
            ("2019-12-31T19:00:01-05:00", false),
        ] {
            let cluster = cluster(Watch::NeverCreated);
            written(&cluster, CREATED, Some(&[time]));
            let api = start(&cluster);
            let deferred = sweep(&api).await.expect("the open sweep");
            let requests = cluster.requests.lock().clone();
            assert_eq!(
                deleted(&cluster, ORPHAN),
                swept,
                "{time}: (story:pod-sweep-replays-from-the-policys-create): {requests:#?}"
            );
            assert!(deferred.is_empty(), "{time}: {deferred:?}");
        }
    }

    /// Every page's policy is judged on its own metadata: written after its create on both pages
    /// of a two-page list, both are kept, and the list is still read whole and each Pod read.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_written_policy_on_every_page_is_kept() {
        let cluster = Arc::new(Cluster {
            pages: 2,
            ..Cluster::default()
        });
        *cluster.watch.lock() = Watch::NeverCreated;
        written(&cluster, CREATED, Some(&[CREATED, CREATED]));
        let api = start(&cluster);
        let deferred = sweep(&api).await.expect("the open sweep");
        let requests = cluster.requests.lock().clone();
        for page in 1..=2 {
            let run = page_orphan(page);
            assert!(
                !deleted(&cluster, &run),
                "{run}: swept as never created \
                 (story:pod-sweep-replays-from-the-policys-create): {requests:#?}"
            );
            let read = format!("GET /api/v1/namespaces/{NAMESPACE}/pods/{run}");
            assert!(
                requests.iter().any(|request| request == &read),
                "{run}'s Pod was not read: {requests:#?}"
            );
        }
        assert!(deferred.is_empty(), "{deferred:?}");
    }
}
