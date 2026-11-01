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

/// Wave D364: a policy written after its create is released on a replay that shows its run's
/// container terminated, as before D354 (`story:written-policy-swept-when-shown-stopped`), and one
/// kept because of that write says so once, where it is old and its Pod gone
/// (`story:kept-written-policy-log-names-the-write`).
mod d364 {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// What the reason of a policy kept for a write after its create names.
    const WRITE: &str = "written after its create";

    /// The cases' `managedFields`: the adapter's create, then another writer's in the same second.
    const WRITES: &[&str] = &[CREATED, CREATED];

    /// One event logged on a thread while a [`Log`] was held: its level and each field's name with
    /// its value.
    #[derive(Debug, Clone)]
    struct Line {
        level: tracing::Level,
        fields: Vec<(String, String)>,
    }

    impl Line {
        fn field(&self, name: &str) -> Option<&str> {
            self.fields
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value.as_str())
        }

        /// Whether any field of the line names the write after the create.
        fn names_the_write(&self) -> bool {
            self.fields.iter().any(|(_, value)| value.contains(WRITE))
        }
    }

    impl tracing::field::Visit for Line {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.fields
                .push((field.name().to_owned(), value.to_owned()));
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.fields
                .push((field.name().to_owned(), format!("{value:?}")));
        }
    }

    thread_local! {
        /// The lines of the [`Log`] this thread holds, if any.
        static CAPTURE: RefCell<Option<Rc<RefCell<Vec<Line>>>>> = const { RefCell::new(None) };
    }

    /// The process's global subscriber, installed once: it keeps the events of a thread that holds
    /// a [`Log`] and drops every other. A scoped default would race other cases' first use of a
    /// callsite, which can cache its interest as never (`probe_tests.rs` in the namespace driver).
    struct Capture;

    impl tracing::Subscriber for Capture {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            CAPTURE.with(|capture| {
                if let Some(lines) = capture.borrow().as_ref() {
                    let mut line = Line {
                        level: *event.metadata().level(),
                        fields: Vec::new(),
                    };
                    event.record(&mut line);
                    lines.borrow_mut().push(line);
                }
            });
        }

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// Every event the adapter logs on this thread while it is held. A sweep a case awaits runs on
    /// the case's own thread.
    struct Log {
        lines: Rc<RefCell<Vec<Line>>>,
    }

    impl Log {
        fn start() -> Self {
            static INSTALLED: std::sync::Once = std::sync::Once::new();
            INSTALLED.call_once(|| {
                tracing::subscriber::set_global_default(Capture).expect("one global subscriber");
            });
            let lines = Rc::new(RefCell::new(Vec::new()));
            CAPTURE.with(|capture| *capture.borrow_mut() = Some(Rc::clone(&lines)));
            Self { lines }
        }

        /// The lines logged for the policy of `run`.
        fn of(&self, run: &str) -> Vec<Line> {
            self.lines
                .borrow()
                .iter()
                .filter(|line| line.field("policy") == Some(run))
                .cloned()
                .collect()
        }
    }

    impl Drop for Log {
        fn drop(&mut self) {
            CAPTURE.with(|capture| capture.borrow_mut().take());
        }
    }

    /// A policy listed young, written after its create, which open defers: the API and
    /// the kept record, aged to an hour.
    async fn deferred_written(cluster: &Arc<Cluster>) -> (Api, Vec<Deferred>) {
        let now = chrono::Utc::now();
        let created =
            (now - chrono::Duration::minutes(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let later = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        written(cluster, &created, Some(&[&created, &later]));
        let api = start(cluster);
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
        (api, deferred)
    }

    /// The red probe of the D354 adversary pass, rebuilt: a policy written after its create, old
    /// and its Pod gone, whose replay from its `resourceVersion` shows its run's container
    /// terminated, is swept by open as stopped, whichever way its metadata shows the write.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_written_policy_shown_stopped_is_swept_by_open() {
        for writes in [
            &[CREATED, CREATED][..],
            &[CREATED, "2020-01-02T09:30:00Z"],
            &["2020-01-01T00:00:01Z"],
        ] {
            let fake = cluster(Watch::Terminated);
            written(&fake, CREATED, Some(writes));
            let api = start(&fake);
            let deferred = sweep(&api).await.expect("the open sweep");
            let requests = fake.requests.lock().clone();
            assert!(
                deleted(&fake, ORPHAN),
                "{writes:?}: open kept a policy written after its create whose replay shows its \
                 container terminated (story:written-policy-swept-when-shown-stopped): \
                 {requests:#?}"
            );
            assert!(deferred.is_empty(), "{writes:?}: {deferred:?}");
            let watched = watches(&fake, ORPHAN);
            assert!(
                watched.len() == 1 && watched[0].contains("&resourceVersion=1&"),
                "{writes:?}: the replay did not start at the policy's version: {watched:?}"
            );
        }
    }

    /// A young policy written after its create, which open defers, is swept by the deferred sweep
    /// once it is old and its replay shows its run's container terminated.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_deferred_written_policy_shown_stopped_is_swept() {
        let fake = cluster(Watch::Terminated);
        let (api, mut deferred) = deferred_written(&fake).await;
        resweep(&api, &mut deferred).await.expect("a round");
        let requests = fake.requests.lock().clone();
        assert!(
            deleted(&fake, ORPHAN),
            "the deferred sweep kept a policy written after its create whose replay shows its \
             container terminated (story:written-policy-swept-when-shown-stopped): {requests:#?}"
        );
        assert!(deferred.is_empty(), "a removed policy is still checked");
    }

    /// A written policy whose replay is one `BOOKMARK` and no event of its Pod is still kept, by
    /// open and by the deferred sweep, though its replay now runs.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_written_policy_whose_replay_is_a_bookmark_alone_is_kept() {
        let fake = cluster(Watch::NeverCreated);
        written(&fake, CREATED, Some(WRITES));
        let api = start(&fake);
        let deferred = sweep(&api).await.expect("the open sweep");
        let requests = fake.requests.lock().clone();
        assert!(
            !deleted(&fake, ORPHAN),
            "open swept a written policy on a bookmark alone \
             (story:written-policy-swept-when-shown-stopped): {requests:#?}"
        );
        assert!(deferred.is_empty(), "{deferred:?}");

        let fake = cluster(Watch::NeverCreated);
        let (api, mut deferred) = deferred_written(&fake).await;
        resweep(&api, &mut deferred).await.expect("a round");
        let requests = fake.requests.lock().clone();
        assert!(
            !deleted(&fake, ORPHAN),
            "the deferred sweep swept a written policy on a bookmark alone \
             (story:written-policy-swept-when-shown-stopped): {requests:#?}"
        );
        assert!(deferred.is_empty(), "a kept policy is still checked");
    }

    /// The one warn of a kept written policy, old and its Pod gone, in open and in the deferred
    /// sweep: its reason names the write after the create.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_kept_written_policy_logs_one_warn_naming_the_write() {
        let fake = cluster(Watch::NeverCreated);
        written(&fake, CREATED, Some(WRITES));
        let api = start(&fake);
        let log = Log::start();
        sweep(&api).await.expect("the open sweep");
        let lines = log.of(ORPHAN);
        drop(log);
        let warns: Vec<&Line> = lines
            .iter()
            .filter(|line| line.level == tracing::Level::WARN)
            .collect();
        assert!(
            warns.len() == 1 && warns[0].field("why").is_some_and(|why| why.contains(WRITE)),
            "open kept a written policy without one warn whose reason names the write \
             (story:kept-written-policy-log-names-the-write): {lines:#?}"
        );

        let fake = cluster(Watch::NeverCreated);
        let (api, mut deferred) = deferred_written(&fake).await;
        let log = Log::start();
        resweep(&api, &mut deferred).await.expect("a round");
        let lines = log.of(ORPHAN);
        drop(log);
        let warns: Vec<&Line> = lines
            .iter()
            .filter(|line| line.level == tracing::Level::WARN)
            .collect();
        assert!(
            warns.len() == 1 && warns[0].field("why").is_some_and(|why| why.contains(WRITE)),
            "the deferred sweep kept a written policy without one warn whose reason names the \
             write (story:kept-written-policy-log-names-the-write): {lines:#?}"
        );
    }

    /// A young written policy, and an old one whose Pod is present, log no line naming the write:
    /// open defers both, and nothing of the write is decided yet.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_young_or_present_written_policy_logs_no_such_line() {
        let fake = cluster(Watch::NeverCreated);
        let now = chrono::Utc::now();
        let created =
            (now - chrono::Duration::minutes(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let later = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        written(&fake, &created, Some(&[&created, &later]));
        let api = start(&fake);
        let log = Log::start();
        let deferred = sweep(&api).await.expect("the open sweep");
        let young = log.of(ORPHAN);
        drop(log);
        assert_eq!(deferred.len(), 1, "open did not defer a young policy");
        assert!(
            !young.iter().any(Line::names_the_write),
            "a young written policy logged its write \
             (story:kept-written-policy-log-names-the-write): {young:#?}"
        );

        let fake = cluster(Watch::NeverCreated);
        fake.page_pods_present.store(true, Ordering::SeqCst);
        written(&fake, CREATED, Some(WRITES));
        let api = start(&fake);
        let log = Log::start();
        let deferred = sweep(&api).await.expect("the open sweep");
        let present = log.of(ORPHAN);
        drop(log);
        assert!(
            deferred.len() == 1 && deferred[0].pod_seen,
            "open did not defer a policy whose Pod is present: {deferred:?}"
        );
        assert!(
            !present.iter().any(Line::names_the_write),
            "a written policy whose Pod is present logged its write \
             (story:kept-written-policy-log-names-the-write): {present:#?}"
        );
        assert!(
            watches(&fake, ORPHAN).is_empty(),
            "a present Pod was replayed"
        );
    }
}
