//! Security review pass 2 of `story:pod-unknown-cleanup-outlives-adapter` (commit `d54ff038`,
//! `list_items`): a sweep's list that reads as empty or short, because its items are `null`, absent
//! or a truncated page, never deletes a `NetworkPolicy` by itself. Every deletion still waits for a
//! read of that exact Pod answering 404, and the open sweep still keeps a young policy. A 200 whose
//! body is a `Status` fails the sweep and deletes nothing.

use super::*;

/// A kept policy whose Pod the API no longer has: a read of it answers 404.
const GONE: &str = "substrate-run-deferred-pass2-gone";
/// A policy younger than [`SWEEP_AGE_FLOOR`] whose Pod the API does not have yet.
const YOUNG: &str = "substrate-run-deferred-pass2-young";

fn deletes(cluster: &Cluster) -> Vec<String> {
    cluster
        .requests
        .lock()
        .iter()
        .filter(|request| request.starts_with("DELETE "))
        .cloned()
        .collect()
}

fn kept(run: &str, pod_seen: bool) -> Deferred {
    Deferred {
        run: run.to_owned(),
        created: (chrono::Utc::now() - chrono::Duration::hours(1)).fixed_offset(),
        pod_seen,
    }
}

/// An owned policy's metadata as a `PartialObjectMetadataList` item, created at `created`.
fn owned_policy(run: &str, created: chrono::DateTime<chrono::Utc>) -> Value {
    json!({
        "kind": "PartialObjectMetadata",
        "apiVersion": "meta.k8s.io/v1",
        "metadata": {
            "name": run,
            "creationTimestamp": created.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "labels": {MANAGED_BY_LABEL: MANAGED_BY, RUN_LABEL: run},
        },
    })
}

/// A 200 that is an error, not a list.
fn status_body() -> String {
    json!({
        "kind": "Status",
        "apiVersion": "v1",
        "metadata": {},
        "status": "Failure",
        "message": "the server could not list",
        "code": 500,
        "items": null,
    })
    .to_string()
}

/// Bodies of the owned Pod list that name no Pod although `RUN`'s Pod exists (its read answers 200):
/// items `null`, items absent, and a page the server cut short whose `continue` is absent or `null`
/// while `remainingItemCount` says more remain.
fn short_pod_lists() -> Vec<String> {
    let metadata = "PartialObjectMetadataList";
    [
        json!({"kind": metadata, "apiVersion": "meta.k8s.io/v1", "metadata": {}, "items": null}),
        json!({"kind": metadata, "apiVersion": "meta.k8s.io/v1", "metadata": {"continue": ""}}),
        json!({"kind": metadata, "metadata": {"remainingItemCount": 7}, "items": []}),
        json!({"kind": "PodList", "metadata": {"continue": null, "remainingItemCount": 7}}),
    ]
    .iter()
    .map(Value::to_string)
    .collect()
}

/// The deferred sweep against a Pod list that reads as empty or short: the policy whose Pod still
/// answers 200 is kept, seen or not, and marked seen; only the policy whose Pod read answers 404 is
/// deleted. A `Status` answered 200 fails the round and deletes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn pass2_a_short_or_null_pod_list_never_deletes_a_policy_whose_pod_answers() {
    let cluster = Arc::new(Cluster::default());
    let api = start(&cluster);
    for body in short_pod_lists() {
        cluster.requests.lock().clear();
        *cluster.list_body.lock() = Some(body.clone());
        let mut deferred = vec![kept(RUN, true), kept(RUN, false), kept(GONE, false)];
        resweep(&api, &mut deferred)
            .await
            .unwrap_or_else(|error| panic!("a round failed on {body}: {error}"));
        assert_eq!(
            deletes(&cluster),
            vec![format!("DELETE {}/{GONE}", api.policies())],
            "on {body}: deleted other than the policy whose Pod read answered 404"
        );
        assert_eq!(deferred.len(), 2, "on {body}: {deferred:?}");
        assert!(
            deferred
                .iter()
                .all(|policy| policy.run == RUN && policy.pod_seen),
            "on {body}: the kept policies are not the present Pod's, marked seen: {deferred:?}"
        );
    }

    cluster.requests.lock().clear();
    *cluster.list_body.lock() = Some(status_body());
    let mut deferred = vec![kept(GONE, true)];
    assert!(
        resweep(&api, &mut deferred).await.is_err(),
        "a round read a Status answered 200 as a Pod list"
    );
    assert_eq!(deletes(&cluster), Vec::<String>::new());
    assert_eq!(deferred.len(), 1);
}

/// The open sweep against one policy-list page the server cut short (no `continue`, while
/// `remainingItemCount` says more remain): it deletes the old orphan whose Pod read answers 404,
/// keeps the young one whose Pod is also gone, and reads the old policy whose Pod exists as kept.
/// A `Status` answered 200 fails the sweep and deletes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn pass2_the_open_sweep_on_a_short_page_keeps_young_and_present_policies() {
    let cluster = Arc::new(Cluster::default());
    let api = start(&cluster);
    let old = chrono::Utc::now() - chrono::Duration::hours(1);
    let young = chrono::Utc::now() - chrono::Duration::seconds(20);
    *cluster.list_body.lock() = Some(
        json!({
            "kind": "PartialObjectMetadataList",
            "apiVersion": "meta.k8s.io/v1",
            "metadata": {"remainingItemCount": 3},
            "items": [owned_policy(GONE, old), owned_policy(YOUNG, young), owned_policy(RUN, old)],
        })
        .to_string(),
    );
    let deferred = sweep(&api).await.expect("the open sweep");
    assert_eq!(
        deletes(&cluster),
        vec![format!("DELETE {}/{GONE}", api.policies())],
        "the open sweep deleted other than the old orphan"
    );
    let mut runs: Vec<(&str, bool)> = deferred
        .iter()
        .map(|policy| (policy.run.as_str(), policy.pod_seen))
        .collect();
    runs.sort_unstable();
    assert_eq!(runs, vec![(RUN, true), (YOUNG, false)]);

    cluster.requests.lock().clear();
    *cluster.list_body.lock() = Some(status_body());
    assert!(
        sweep(&api).await.is_err(),
        "the open sweep read a Status answered 200 as a policy list"
    );
    assert_eq!(deletes(&cluster), Vec::<String>::new());
}
