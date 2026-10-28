//! `story:pod-unknown-cleanup-outlives-adapter`, finding A4: once a run reads `Unknown` because its
//! Pod's removal was not confirmed, the run's task keeps removing the Pod until the API confirms it
//! gone, and then removes its `NetworkPolicy` (design 28, § *A run*: "while the task keeps trying").
//!
//! A fake TLS API server stands in for a cluster whose kubelet stays away: it accepts every
//! deletion of the Pod and keeps the Pod until it has been asked [`RELEASED_AFTER`] times, more
//! than the two removal rounds the task used to make before it stopped. The removal pacing is
//! shortened so the case takes about a second; the task's logic is the adapter's own.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicU32;

use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::*;

const NAMESPACE: &str = "substrate-pod-cleanup";
const RUN: &str = "substrate-run-cleanup";
const PACED: Pacing = Pacing {
    poll: Duration::from_millis(10),
    remove_attempts: 3,
    notice: Duration::from_millis(10),
};
/// The deletion of the Pod after which the fake kubelet lets it go: past the two rounds of
/// `remove_attempts` the task made before this story.
const RELEASED_AFTER: u32 = 3 * PACED.remove_attempts + 1;

#[derive(Default)]
struct Cluster {
    pod_deletes: AtomicU32,
    pod_gone: AtomicBool,
    policy_gone: AtomicBool,
    /// While set, the kubelet keeps the Pod whatever it has been asked.
    kept: AtomicBool,
    /// How many pages the policy list answers, one owned orphan on each; `u32::MAX` never ends.
    pages: u32,
    /// Each request's method and path, in arrival order.
    requests: Mutex<Vec<String>>,
    /// The `substrate-run-deferred-*` Pods that exist: the owned Pod list names them and a read
    /// of one answers it; any other `substrate-run-deferred-*` Pod is gone.
    present: Mutex<std::collections::BTreeSet<String>>,
    /// Security review: each listed Pod carries an argv of this many bytes and a full environment
    /// at the adapter's own bounds, as a run the adapter admitted does; 0 lists bare metadata.
    pod_argv_bytes: usize,
    /// Security review: while set, the owned Pod list answers 500.
    pod_list_fails: AtomicBool,
    /// While set, the policy list and the owned Pod list answer 200 with a body that names no
    /// list kind and holds no items.
    kindless_lists: AtomicBool,
    /// Security review pass 2: while set, the policy list and the owned Pod list answer 200 with
    /// exactly this body.
    list_body: Mutex<Option<String>>,
}

/// The orphan the policy list's page `page` holds: owned, an hour old, its Pod gone.
fn page_orphan(page: u32) -> String {
    format!("substrate-run-page-{page}")
}

/// Whether `accept` asks for a `meta.k8s.io/v1` `PartialObjectMetadataList`, as an API server
/// reads it: one of its comma-separated media types is `application/json` with the parameters
/// `as=PartialObjectMetadataList`, `g=meta.k8s.io` and `v=v1`, in any order.
fn metadata_only(accept: &str) -> bool {
    accept.split(',').any(|media| {
        let mut parts = media.split(';').map(str::trim);
        let media_type = parts.next();
        let mut parameters: Vec<&str> = parts.collect();
        parameters.sort_unstable();
        media_type == Some("application/json")
            && parameters == ["as=PartialObjectMetadataList", "g=meta.k8s.io", "v=v1"]
    })
}

/// A list of `kind` holding `items` and ending at `token`, as the API server answers `accept`: each
/// item's `metadata` alone in a `PartialObjectMetadataList`, or the objects whole. An empty
/// `PartialObjectMetadataList` carries `"items": null`, as a real API server's does (CI Pod lane
/// run 40714688540: every open on kind failed on it).
fn list(accept: &str, kind: &str, items: Vec<Value>, token: &Value) -> String {
    if metadata_only(accept) && items.is_empty() {
        return json!({
            "kind": "PartialObjectMetadataList",
            "apiVersion": "meta.k8s.io/v1",
            "metadata": {"continue": token},
            "items": null,
        })
        .to_string();
    }
    if metadata_only(accept) {
        let items: Vec<Value> = items
            .into_iter()
            .map(|item| {
                json!({
                    "kind": "PartialObjectMetadata",
                    "apiVersion": "meta.k8s.io/v1",
                    "metadata": item["metadata"],
                })
            })
            .collect();
        return json!({
            "kind": "PartialObjectMetadataList",
            "apiVersion": "meta.k8s.io/v1",
            "metadata": {"continue": token},
            "items": items,
        })
        .to_string();
    }
    json!({"kind": format!("{kind}List"), "metadata": {"continue": token}, "items": items})
        .to_string()
}

/// Page `page` (from 1) of the policy list: one orphan, and a `continue` token unless it is the last.
fn page(cluster: &Cluster, page: u32, accept: &str) -> String {
    let name = page_orphan(page);
    let token = if page < cluster.pages {
        json!(format!("token-{}", page + 1))
    } else {
        json!("")
    };
    let policy = json!({
        "kind": "NetworkPolicy",
        "metadata": {
            "name": name,
            "creationTimestamp": "2020-01-01T00:00:00Z",
            "labels": {MANAGED_BY_LABEL: MANAGED_BY, RUN_LABEL: name},
        },
        "spec": {
            "podSelector": {"matchLabels": {RUN_LABEL: name}},
            "policyTypes": ["Ingress", "Egress"],
        },
    });
    list(accept, "NetworkPolicy", vec![policy], &token)
}

/// The owned Pod list: the `present` Pods, or 500 while `pod_list_fails` is set.
fn owned_pod_list(cluster: &Cluster, accept: &str) -> (u16, String) {
    if cluster.pod_list_fails.load(Ordering::SeqCst) {
        return (
            500,
            r#"{"kind":"Status","message":"unavailable"}"#.to_owned(),
        );
    }
    let items: Vec<Value> = cluster
        .present
        .lock()
        .iter()
        .map(|name| {
            let mut pod = json!({"metadata": {
                "name": name,
                "labels": {MANAGED_BY_LABEL: MANAGED_BY, RUN_LABEL: name},
            }});
            if cluster.pod_argv_bytes > 0 {
                let env: Vec<Value> = (0..crate::environment::MAX_SET)
                    .map(|index| json!({"name": format!("V{index}"), "value": "x".repeat(4096)}))
                    .collect();
                // The API server records the fields the create set in `metadata.managedFields`,
                // which a metadata-only list keeps: one key per variable, never its value.
                let fields: serde_json::Map<String, Value> = (0..crate::environment::MAX_SET)
                    .map(|index| {
                        (
                            format!("k:{{\"name\":\"V{index}\"}}"),
                            json!({".": {}, "f:name": {}, "f:value": {}}),
                        )
                    })
                    .collect();
                pod["metadata"]["managedFields"] = json!([{
                    "manager": "b10x-substrate-pod",
                    "operation": "Update",
                    "apiVersion": "v1",
                    "fieldsType": "FieldsV1",
                    "fieldsV1": {"f:spec": {"f:containers": {
                        format!("k:{{\"name\":\"{CONTAINER}\"}}"): {"f:args": {}, "f:env": fields},
                    }}},
                }]);
                pod["spec"] = json!({"containers": [{
                    "name": CONTAINER,
                    "command": ["sh"],
                    "args": ["x".repeat(cluster.pod_argv_bytes - 2)],
                    "env": env,
                }]});
            }
            pod
        })
        .collect();
    (200, list(accept, "Pod", items, &json!("")))
}

fn answer(cluster: &Cluster, method: &str, path: &str, accept: &str) -> (u16, String) {
    let pod = format!("/api/v1/namespaces/{NAMESPACE}/pods/{RUN}");
    let policies = format!("/apis/networking.k8s.io/v1/namespaces/{NAMESPACE}/networkpolicies");
    let policy = format!("{policies}/{RUN}");
    let gone = (404, r#"{"kind":"Status","code":404}"#.to_owned());
    let (pod_path, query) = path.split_once('?').unwrap_or((path, ""));
    let pods = format!("/api/v1/namespaces/{NAMESPACE}/pods");
    let listed = method == "GET"
        && query.starts_with("labelSelector=")
        && (pod_path == policies || pod_path == pods);
    if listed && cluster.kindless_lists.load(Ordering::SeqCst) {
        return (200, r#"{"metadata":{"continue":""}}"#.to_owned());
    }
    if listed && let Some(body) = cluster.list_body.lock().clone() {
        return (200, body);
    }
    // Open's probe: a list of at most one Pod, then the log of a Pod that does not exist.
    if method == "GET" && pod_path == pods && query == "limit=1" {
        return (200, list(JSON, "Pod", Vec::new(), &json!("")));
    }
    if method == "GET" && pod_path == format!("{pods}/{LOG_ACCESS_PROBE}/log") {
        return gone;
    }
    if method == "GET" && pod_path == policies && cluster.pages == 0 {
        return (200, list(accept, "NetworkPolicy", Vec::new(), &json!("")));
    }
    if method == "GET" && pod_path == policies && cluster.pages > 0 {
        let number = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("continue=token-"))
            .map_or(1, |number| number.parse().expect("a page number"));
        return (200, page(cluster, number, accept));
    }
    let orphan = |prefix: &str| {
        cluster.pages > 0
            && pod_path
                .strip_prefix(prefix)
                .is_some_and(|name| name.starts_with("substrate-run-page-"))
    };
    if method == "GET" && orphan(&format!("/api/v1/namespaces/{NAMESPACE}/pods/")) {
        return gone;
    }
    if method == "DELETE" && orphan(&format!("{policies}/")) {
        return (200, "{}".to_owned());
    }
    if method == "GET" && pod_path == pods && query.starts_with("labelSelector=") {
        return owned_pod_list(cluster, accept);
    }
    let deferred = |prefix: &str| {
        pod_path
            .strip_prefix(prefix)
            .filter(|name| name.starts_with("substrate-run-deferred-"))
    };
    if method == "GET"
        && let Some(name) = deferred(&format!("{pods}/"))
    {
        return if cluster.present.lock().contains(name) {
            (200, json!({"metadata": {"name": name}}).to_string())
        } else {
            gone
        };
    }
    if method == "DELETE" && deferred(&format!("{policies}/")).is_some() {
        return (200, "{}".to_owned());
    }
    match method {
        "GET" if pod_path == format!("{pod}/log") => (200, "hello\n".to_owned()),
        "GET" if path == pod && cluster.pod_gone.load(Ordering::SeqCst) => gone,
        "GET" if path == pod => (
            200,
            json!({
                "metadata": {"name": RUN},
                "status": {
                    "phase": "Succeeded",
                    "containerStatuses": [{
                        "name": CONTAINER,
                        "state": {"terminated": {"exitCode": 0, "reason": "Completed"}},
                    }],
                },
            })
            .to_string(),
        ),
        "DELETE" if path == pod => {
            let asked = cluster.pod_deletes.fetch_add(1, Ordering::SeqCst) + 1;
            if cluster.pod_gone.load(Ordering::SeqCst) {
                gone
            } else {
                // The kubelet answers again only after the task's former two rounds.
                if asked >= RELEASED_AFTER && !cluster.kept.load(Ordering::SeqCst) {
                    cluster.pod_gone.store(true, Ordering::SeqCst);
                }
                (200, "{}".to_owned())
            }
        }
        "GET" | "DELETE" if path == policy && cluster.policy_gone.load(Ordering::SeqCst) => gone,
        "GET" if path == policy => (200, "{}".to_owned()),
        "DELETE" if path == policy => {
            cluster.policy_gone.store(true, Ordering::SeqCst);
            (200, "{}".to_owned())
        }
        _ => (
            500,
            r#"{"kind":"Status","message":"unexpected"}"#.to_owned(),
        ),
    }
}

fn serve(stream: TcpStream, config: Arc<ServerConfig>, cluster: &Cluster) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let tls = ServerConnection::new(config).map_err(std::io::Error::other)?;
    let mut stream = BufReader::new(StreamOwned::new(tls, stream));
    let mut line = String::new();
    stream.read_line(&mut line)?;
    let mut length = 0_usize;
    let mut accept = String::new();
    loop {
        let mut header = String::new();
        stream.read_line(&mut header)?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().unwrap_or(0);
        } else if name.eq_ignore_ascii_case("accept") {
            value.trim().clone_into(&mut accept);
        }
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body)?;
    let mut parts = line.split(' ');
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    cluster.requests.lock().push(format!("{method} {path}"));
    let (status, payload) = answer(cluster, method, path, &accept);
    let writer = stream.get_mut();
    write!(
        writer,
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         date: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len(),
        chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT"),
    )?;
    writer.flush()?;
    writer.conn.send_close_notify();
    writer.flush()
}

/// Starts the fake on loopback and returns the API the adapter would open against it.
fn start(cluster: &Arc<Cluster>) -> Api {
    let kubeconfig = fake_kubeconfig(cluster);
    let (mut api, _) = parse_cluster(kubeconfig.to_string().as_bytes()).expect("the fake parses");
    api.pacing = PACED;
    api
}

/// Starts the fake on loopback and returns the cluster configuration that names it.
fn fake_kubeconfig(cluster: &Arc<Cluster>) -> Value {
    let identity =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).expect("identity");
    let config = Arc::new(
        ServerConfig::builder_with_provider(
            Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        )
        .with_safe_default_protocol_versions()
        .expect("protocols")
        .with_no_client_auth()
        .with_single_cert(
            vec![identity.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(identity.signing_key.serialize_der())
                .into(),
        )
        .expect("TLS config"),
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let served = Arc::clone(cluster);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (cluster, config) = (Arc::clone(&served), Arc::clone(&config));
            std::thread::spawn(move || serve(stream, config, &cluster));
        }
    });
    json!({
        "apiVersion": "v1",
        "kind": "Config",
        "current-context": "fake",
        "clusters": [{"name": "fake", "cluster": {
            "server": format!("https://{address}"),
            "certificate-authority-data":
                base64::engine::general_purpose::STANDARD.encode(identity.cert.pem()),
        }}],
        "contexts": [{"name": "fake", "context": {
            "cluster": "fake", "user": "fake", "namespace": NAMESPACE,
            "extensions": [{"name": POD_SETTINGS_EXTENSION, "extension": {"pod_pids_limit": 1024}}],
        }}],
        "users": [{"name": "fake", "user": {"token": "fake-token"}}],
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_read_unknown_is_removed_when_the_kubelet_outlasts_two_removal_rounds() {
    let cluster = Arc::new(Cluster::default());
    let api = Arc::new(start(&cluster));
    let (sender, phase) = watch::channel(Phase::Running);
    let run = Arc::new(PodRun {
        id: RunId::new(RUN.to_owned()),
        phase: phase.clone(),
        output_limit: 1024,
        captured: Mutex::new(Captured::default()),
        started: AtomicBool::new(false),
        stop: Mutex::new(None),
        pod_gone: AtomicBool::new(false),
    });
    let task = tokio::spawn(monitor(
        api,
        run,
        sender,
        Instant::now() + Duration::from_mins(10),
    ));
    let mut observed = phase.clone();
    let unknown = tokio::time::timeout(
        Duration::from_secs(10),
        observed.wait_for(|phase| matches!(phase, Phase::Unknown)),
    )
    .await;
    assert!(
        matches!(unknown, Ok(Ok(_))),
        "a run whose Pod removal is not confirmed reads Unknown"
    );
    let finished = tokio::time::timeout(Duration::from_secs(20), task).await;
    let deletes = cluster.pod_deletes.load(Ordering::SeqCst);
    let requests = cluster.requests.lock().clone();
    assert!(
        finished.is_ok(),
        "the task still runs 20 s after Unknown, the Pod asked deleted {deletes} times"
    );
    assert!(
        cluster.policy_gone.load(Ordering::SeqCst),
        "story:pod-unknown-cleanup-outlives-adapter: the task stopped after {deletes} deletions of \
         the Pod, before the kubelet let it go after {RELEASED_AFTER}, and left its NetworkPolicy"
    );
    assert!(
        deletes >= RELEASED_AFTER,
        "the Pod went after {deletes} deletions"
    );
    let first_policy_request = requests
        .iter()
        .position(|request| request.contains("/networkpolicies/"))
        .expect("the policy was asked for");
    assert_eq!(
        requests[..first_policy_request]
            .iter()
            .filter(|request| request.starts_with("DELETE ") && request.contains("/pods/"))
            .count(),
        usize::try_from(RELEASED_AFTER).expect("small"),
        "the policy was touched before the Pod was confirmed gone: {requests:#?}"
    );
    assert!(
        matches!(*phase.borrow(), Phase::Unknown),
        "the run keeps reading Unknown after the late removal"
    );
}

/// A run tracked by `adapter`, whose phase `monitor` drives when it is given the sender.
fn tracked(name: &str) -> (Arc<PodRun>, watch::Sender<Phase>) {
    let (sender, phase) = watch::channel(Phase::Running);
    let run = Arc::new(PodRun {
        id: RunId::new(name.to_owned()),
        phase,
        output_limit: 1024,
        captured: Mutex::new(Captured::default()),
        started: AtomicBool::new(false),
        stop: Mutex::new(None),
        pod_gone: AtomicBool::new(false),
    });
    (run, sender)
}

/// Security finding S2 (decision D4): a run read `Unknown` whose Pod's removal is still pending
/// stays counted against `POD_MAX_TRACKED_RUNS` until the API confirms the Pod gone, so tasks that
/// keep removing never grow past the cap; at the cap a start is refused `exec.tracking-capacity`.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_read_unknown_with_its_removal_pending_counts_against_the_tracking_cap() {
    let cluster = Arc::new(Cluster::default());
    cluster.kept.store(true, Ordering::SeqCst);
    let api = Arc::new(start(&cluster));
    let adapter = PodAdapter {
        facts: proof(),
        api: Arc::clone(&api),
        settings: PodSettings {
            namespace: NAMESPACE.to_owned(),
            run_as_user: POD_DEFAULT_RUN_AS_USER,
            pod_pids_limit: Some(1024),
            ephemeral_storage_bytes: POD_DEFAULT_EPHEMERAL_STORAGE_BYTES,
        },
        runs: Mutex::new(Vec::new()),
    };
    let (run, sender) = tracked(RUN);
    let mut observed = run.phase.clone();
    let task = tokio::spawn(monitor(
        api,
        Arc::clone(&run),
        sender,
        Instant::now() + Duration::from_mins(10),
    ));
    {
        let mut runs = adapter.runs.lock();
        runs.push(run);
        // Every other tracked run is still running; their senders are dropped, which keeps the phase.
        for index in 1..POD_MAX_TRACKED_RUNS {
            runs.push(tracked(&format!("substrate-run-busy-{index}")).0);
        }
    }
    let unknown = tokio::time::timeout(
        Duration::from_secs(10),
        observed.wait_for(|phase| matches!(phase, Phase::Unknown)),
    )
    .await;
    assert!(
        matches!(unknown, Ok(Ok(_))),
        "the run whose Pod the kubelet keeps reads Unknown"
    );
    let refused = adapter.reserve().err().map(|error| error.code());
    assert_eq!(
        refused,
        Some("exec.tracking-capacity"),
        "a run read Unknown whose Pod's removal is still pending was forgotten to make room, so its \
         removal task no longer counts against the cap"
    );
    assert_eq!(adapter.runs.lock().len(), POD_MAX_TRACKED_RUNS);
    cluster.kept.store(false, Ordering::SeqCst);
    let finished = tokio::time::timeout(Duration::from_secs(20), task).await;
    assert!(
        finished.is_ok(),
        "the task ends once the kubelet lets the Pod go"
    );
    assert!(
        adapter.reserve().is_ok(),
        "the run whose Pod is confirmed gone still holds its place"
    );
    assert_eq!(adapter.runs.lock().len(), POD_MAX_TRACKED_RUNS - 1);
}

/// Security finding S3 (decision D5): the open sweep reads the policy list in pages of 100 and
/// follows `continue`, so an orphan on a later page is removed as one on the first is.
#[tokio::test(flavor = "multi_thread")]
async fn the_open_sweep_follows_the_policy_list_across_its_pages() {
    let cluster = Arc::new(Cluster {
        pages: 2,
        ..Cluster::default()
    });
    let api = start(&cluster);
    let swept = sweep(&api).await;
    let requests = cluster.requests.lock().clone();
    assert!(swept.is_ok(), "the sweep failed: {swept:?}; {requests:#?}");
    let lists: Vec<&String> = requests
        .iter()
        .filter(|request| request.starts_with("GET ") && request.contains("/networkpolicies?"))
        .collect();
    assert_eq!(lists.len(), 2, "{requests:#?}");
    assert!(
        lists.iter().all(|list| list.contains("&limit=100")),
        "a list asked for more than a page of 100: {lists:#?}"
    );
    assert!(
        lists[1].contains("&continue=token-2"),
        "the second list did not continue the first: {lists:#?}"
    );
    for number in 1..=2 {
        let deleted = format!("DELETE {}/{}", api.policies(), page_orphan(number));
        assert!(
            requests.contains(&deleted),
            "the orphan on page {number} was not removed: {requests:#?}"
        );
    }
}

/// Decision D5: the sweep reads at most 64 pages. A list that has not ended by then fails the
/// sweep, and with it the open (`confinement.adapter-unserved`), before anything is deleted.
#[tokio::test(flavor = "multi_thread")]
async fn the_open_sweep_fails_past_its_page_bound_and_deletes_nothing() {
    let cluster = Arc::new(Cluster {
        pages: u32::MAX,
        ..Cluster::default()
    });
    let api = start(&cluster);
    let swept = sweep(&api).await;
    let requests = cluster.requests.lock().clone();
    assert!(swept.is_err(), "an endless list was swept");
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("/networkpolicies?"))
            .count(),
        64,
        "{requests:#?}"
    );
    assert!(
        !requests
            .iter()
            .any(|request| request.starts_with("DELETE ")),
        "the sweep deleted before it had read the whole list: {requests:#?}"
    );
}

/// CI Pod lane run 40714688540: on kind every open failed `confinement.adapter-unserved` with "the
/// policy list holds no items", because an API server answers an empty `PartialObjectMetadataList`
/// without an items array. An open against a namespace holding no owned policy and no Pod, whose
/// lists the fake answers as that server does, succeeds and deletes nothing.
#[test]
fn an_open_with_no_policies_and_no_pods_succeeds_on_an_empty_metadata_list() {
    let cluster = Arc::new(Cluster::default());
    let directory = tempfile::tempdir().expect("a directory");
    let path = directory.path().join("cluster.json");
    std::fs::write(&path, fake_kubeconfig(&cluster).to_string()).expect("the configuration");
    let opened = open_pod(PodRequest {
        cluster: Some(path),
    });
    let requests = cluster.requests.lock().clone();
    assert!(
        opened.is_ok(),
        "the open failed: {:?}; {requests:#?}",
        opened.as_ref().err()
    );
    assert!(
        requests
            .iter()
            .any(|request| request.starts_with("GET ") && request.contains("/networkpolicies?")),
        "the open did not list the policies: {requests:#?}"
    );
    assert!(
        !requests
            .iter()
            .any(|request| request.starts_with("DELETE ")),
        "an open with nothing to sweep deleted: {requests:#?}"
    );
}

/// Failing closed: a 200 to either list whose body names no list kind and holds no items is no
/// evidence that nothing is there. The deferred round and the open sweep fail, and nothing is
/// deleted; the same round against a list answer deletes the policy.
#[tokio::test(flavor = "multi_thread")]
async fn a_list_answer_without_a_kind_or_items_fails_the_round_and_deletes_nothing() {
    const GONE: &str = "substrate-run-deferred-kindless";
    let cluster = Arc::new(Cluster::default());
    cluster.kindless_lists.store(true, Ordering::SeqCst);
    let api = start(&cluster);
    // Its Pod was seen and is gone: a round that read the Pod list deletes this policy.
    let mut deferred = vec![Deferred {
        run: GONE.to_owned(),
        created: (chrono::Utc::now() - chrono::Duration::hours(1)).fixed_offset(),
        pod_seen: true,
    }];
    let round = resweep(&api, &mut deferred).await;
    assert!(round.is_err(), "a round read a kindless list as empty");
    let swept = sweep(&api).await;
    assert!(
        swept.is_err(),
        "the open sweep read a kindless list as empty"
    );
    let deletes = |cluster: &Cluster| {
        cluster
            .requests
            .lock()
            .iter()
            .filter(|request| request.starts_with("DELETE "))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        deletes(&cluster),
        Vec::<String>::new(),
        "a kindless list answer deleted something"
    );
    assert_eq!(deferred.len(), 1, "the kept policy is no longer checked");
    cluster.kindless_lists.store(false, Ordering::SeqCst);
    resweep(&api, &mut deferred).await.expect("a round");
    assert_eq!(
        deletes(&cluster),
        vec![format!("DELETE {}/{GONE}", api.policies())],
        "the control round did not delete the policy whose Pod is gone"
    );
}

/// What a sweep reads as a list: either kind it may be answered, with items an array, `null` or
/// absent. Nothing else reads as one, empty or not.
#[test]
fn a_sweep_reads_only_a_list_kind_with_array_null_or_absent_items() {
    let metadata = "PartialObjectMetadataList";
    for (body, count) in [
        (json!({"kind": metadata, "items": null}), 0),
        (json!({"kind": metadata, "metadata": {}}), 0),
        (json!({"kind": metadata, "items": [{"metadata": {}}]}), 1),
        (json!({"kind": "PodList", "items": null}), 0),
        (json!({"kind": "PodList", "items": [{}, {}]}), 2),
    ] {
        assert_eq!(
            list_items(&body, "PodList", "Pod").map(<[Value]>::len),
            Ok(count),
            "{body}"
        );
    }
    for body in [
        json!({}),
        json!({"items": []}),
        json!({"metadata": {"continue": ""}}),
        json!({"kind": "Status", "items": null}),
        json!({"kind": "NetworkPolicyList", "items": []}),
        json!({"kind": metadata, "items": {}}),
        json!({"kind": metadata, "items": "none"}),
        json!({"kind": metadata, "items": 0}),
        json!([]),
    ] {
        assert!(list_items(&body, "PodList", "Pod").is_err(), "{body}");
    }
}

#[path = "pod_cleanup_adversary.rs"]
mod adversary_pass1;

#[path = "pod_cleanup_deferred.rs"]
mod deferred;

#[path = "pod_cleanup_review.rs"]
mod review;

#[path = "pod_cleanup_security_pass2.rs"]
mod security_pass2;
