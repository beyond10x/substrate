//! Security pass 1 on `story:pod-unknown-cleanup-outlives-adapter` (decision D124, the open sweep).
//!
//! The invariant: no running confined Pod is left without its default-deny `NetworkPolicy`
//! because another adapter's open swept it. `start` creates the policy, then the Pod, each call
//! bounded by the adapter's 30 s API timeout. The API server stamps `creationTimestamp` when it
//! begins handling a create, before admission and storage, so the Pod a start is creating may be
//! stored later than 30 s after its policy's `creationTimestamp` while both calls stay inside
//! their own bound. A sweep that reads such a policy as old and the Pod as absent removes the only
//! network fence of a run whose start then succeeds.
//!
//! The fake below is this module's own: it stamps a policy's `creationTimestamp` a configured span
//! back (the time the API server spent before the policy's answer reached the client) and can hold
//! a Pod create until the test releases it.

use super::*;

struct Cluster {
    objects: Mutex<BTreeMap<String, Value>>,
    requests: Mutex<Vec<String>>,
    /// How far before its answer the server stamps a created policy's `creationTimestamp`.
    policy_stamped_back: Duration,
    /// Hold every Pod create until `release_pod_create` is set.
    hold_pod_create: bool,
    pod_create_arrived: AtomicBool,
    release_pod_create: AtomicBool,
    /// The `Date` header every answer carries; `None` omits it.
    date: Option<String>,
}

impl Cluster {
    fn new() -> Self {
        Self {
            objects: Mutex::new(BTreeMap::new()),
            requests: Mutex::new(Vec::new()),
            policy_stamped_back: Duration::ZERO,
            hold_pod_create: false,
            pod_create_arrived: AtomicBool::new(false),
            release_pod_create: AtomicBool::new(false),
            date: Some(String::new()),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.objects.lock().keys().cloned().collect()
    }
}

fn not_found() -> (u16, String) {
    (404, r#"{"kind":"Status","code":404}"#.to_owned())
}

#[allow(clippy::too_many_lines)] // One fake API server keeps every route it answers adjacent.
fn answer(cluster: &Cluster, method: &str, target: &str, body: &[u8]) -> (u16, String) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let pods = format!("/api/v1/namespaces/{NAMESPACE}/pods");
    let policies = format!("/apis/networking.k8s.io/v1/namespaces/{NAMESPACE}/networkpolicies");
    if method == "GET" && path == pods && query == "limit=1" {
        return (200, r#"{"kind":"PodList","items":[]}"#.to_owned());
    }
    if method == "GET" && path == policies {
        let items: Vec<Value> = cluster
            .objects
            .lock()
            .iter()
            .filter(|(key, _)| key.starts_with("networkpolicies/"))
            .map(|(_, object)| object.clone())
            .collect();
        return (
            200,
            json!({"kind": "NetworkPolicyList", "metadata": {}, "items": items}).to_string(),
        );
    }
    if method == "POST" && (path == pods || path == policies) {
        let mut object: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let name = object["metadata"]["name"]
            .as_str()
            .unwrap_or("?")
            .to_owned();
        // The API server assigns every object a uid; a Pod's ownerReference names its policy's.
        object["metadata"]["uid"] = json!(format!("uid-{name}"));
        if path == policies {
            object["metadata"]["creationTimestamp"] = json!(timestamp(cluster.policy_stamped_back));
            cluster
                .objects
                .lock()
                .insert(format!("networkpolicies/{name}"), object.clone());
        } else {
            if cluster.hold_pod_create {
                cluster.pod_create_arrived.store(true, Ordering::SeqCst);
                let until = std::time::Instant::now() + Duration::from_secs(10);
                while !cluster.release_pod_create.load(Ordering::SeqCst)
                    && std::time::Instant::now() < until
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            object["metadata"]["creationTimestamp"] = json!(timestamp(Duration::ZERO));
            cluster
                .objects
                .lock()
                .insert(format!("pods/{name}"), object.clone());
        }
        return (201, object.to_string());
    }
    if let Some(rest) = path.strip_prefix(&format!("{pods}/")) {
        if let Some(name) = rest.strip_suffix("/log") {
            return if cluster.objects.lock().contains_key(&format!("pods/{name}")) {
                (200, "hello\n".to_owned())
            } else {
                not_found()
            };
        }
        let key = format!("pods/{rest}");
        return match method {
            "GET" => match cluster.objects.lock().get(&key) {
                Some(object) => {
                    let mut object = object.clone();
                    // Running, so the run's own task neither ends it nor removes its policy.
                    object["status"] = json!({"phase": "Running", "containerStatuses": [
                        {"name": "run", "state": {"running": {}}}]});
                    (200, object.to_string())
                }
                None => not_found(),
            },
            "DELETE" => match cluster.objects.lock().remove(&key) {
                Some(object) => (200, object.to_string()),
                None => not_found(),
            },
            _ => (405, "{}".to_owned()),
        };
    }
    if let Some(rest) = path.strip_prefix(&format!("{policies}/")) {
        let key = format!("networkpolicies/{rest}");
        return match method {
            "GET" => cluster
                .objects
                .lock()
                .get(&key)
                .map_or_else(not_found, |object| (200, object.to_string())),
            "DELETE" => cluster
                .objects
                .lock()
                .remove(&key)
                .map_or_else(not_found, |object| (200, object.to_string())),
            _ => (405, "{}".to_owned()),
        };
    }
    (
        500,
        r#"{"kind":"Status","message":"unexpected"}"#.to_owned(),
    )
}

fn serve_one(
    stream: TcpStream,
    config: Arc<ServerConfig>,
    cluster: &Cluster,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let tls = ServerConnection::new(config).map_err(std::io::Error::other)?;
    let mut stream = BufReader::new(StreamOwned::new(tls, stream));
    let mut line = String::new();
    stream.read_line(&mut line)?;
    let mut length = 0_usize;
    loop {
        let mut header = String::new();
        stream.read_line(&mut header)?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body)?;
    let mut parts = line.trim_end().split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    cluster.requests.lock().push(format!("{method} {target}"));
    let (status, payload) = answer(cluster, method, target, &body);
    let date = match &cluster.date {
        Some(fixed) if !fixed.is_empty() => format!("date: {fixed}\r\n"),
        Some(_) => format!(
            "date: {}\r\n",
            chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT")
        ),
        None => String::new(),
    };
    let writer = stream.get_mut();
    write!(
        writer,
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{date}\
         connection: close\r\n\r\n{payload}",
        payload.len()
    )?;
    writer.flush()?;
    writer.conn.send_close_notify();
    writer.flush()
}

/// Serves `cluster` on loopback and returns a kubeconfig for it, in a directory the caller holds.
fn serve_cluster(cluster: &Arc<Cluster>) -> (tempfile::TempDir, PathBuf) {
    let identity =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).expect("identity");
    let config = Arc::new(
        ServerConfig::builder()
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
            std::thread::spawn(move || serve_one(stream, config, &cluster));
        }
    });
    let kubeconfig = json!({
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
            "extensions": [{"name": "b10x-substrate/pod", "extension": {"pod_pids_limit": 1024}}],
        }}],
        "users": [{"name": "fake", "user": {"token": "fake-token"}}],
    });
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("kubeconfig.json");
    std::fs::write(&path, serde_json::to_vec(&kubeconfig).expect("json")).expect("write");
    (directory, path)
}

fn open_against(kubeconfig: &std::path::Path) -> Result<Arc<PodAdapter>, String> {
    pod::open_pod(PodRequest {
        cluster: Some(kubeconfig.to_path_buf()),
    })
    .map_err(|error| format!("{}: {error}", error.code()))
}

fn owned_orphan(name: &str, created: &str) -> Value {
    json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": {"name": name, "namespace": NAMESPACE, "creationTimestamp": created,
                     "labels": {pod::MANAGED_BY_LABEL: pod::MANAGED_BY, pod::RUN_LABEL: name}},
        "spec": {"podSelector": {"matchLabels": {pod::RUN_LABEL: name}},
                 "policyTypes": ["Ingress", "Egress"]},
    })
}

/// A start whose policy the API server stamped 29 s before its answer reached the adapter (a slow
/// admission or a slow answer, inside the 30 s bound of that call), and whose Pod create is still
/// in flight (inside the 30 s bound of its own call), while a second adapter opens. The start
/// succeeds; its running Pod must still have its default-deny policy.
#[tokio::test(flavor = "multi_thread")]
async fn security_a_pod_whose_start_succeeds_keeps_its_policy_through_another_adapters_open() {
    let cluster = Arc::new(Cluster {
        policy_stamped_back: Duration::from_secs(29),
        hold_pod_create: true,
        ..Cluster::new()
    });
    let (_directory, kubeconfig) = serve_cluster(&cluster);
    let first = open_against(&kubeconfig).expect("the first adapter opens");
    let admitted = first.admit(&admitted_profile()).expect("admitted");
    let starting = {
        let first = Arc::clone(&first);
        tokio::spawn(async move { first.start(&admitted, &echo()).await })
    };
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !cluster.pod_create_arrived.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < until,
            "the Pod create never arrived"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The policy's age by the server's clock passes 30 s while the Pod create is 2 s in flight.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let second = {
        let kubeconfig = kubeconfig.clone();
        tokio::task::spawn_blocking(move || open_against(&kubeconfig))
            .await
            .expect("open joins")
    };
    cluster.release_pod_create.store(true, Ordering::SeqCst);
    let started = starting.await.expect("start joins");
    let run = started.expect("the start succeeds: its Pod was stored inside its call's bound");
    let policy = format!("networkpolicies/{}", run.as_str());
    let pod = format!("pods/{}", run.as_str());
    let held = cluster.keys();
    assert!(held.contains(&pod), "the started Pod is stored: {held:?}");
    assert!(
        held.contains(&policy),
        "the run started and its Pod {pod} is running without its default-deny NetworkPolicy, \
         which the second adapter's open sweep deleted (second open: {:?}); requests: {:#?}",
        second.as_ref().map(|_| "opened"),
        cluster.requests.lock()
    );
    drop(second);
}

/// The sweep reads age only by the API server's clock: an orphan candidate under an answer whose
/// `Date` is missing or unreadable refuses open by name and deletes nothing.
#[test]
fn security_a_sweep_without_a_readable_server_date_refuses_open_and_deletes_nothing() {
    for date in [None, Some("not a date".to_owned())] {
        let cluster = Arc::new(Cluster {
            date: date.clone(),
            ..Cluster::new()
        });
        cluster.objects.lock().insert(
            "networkpolicies/substrate-run-orphan".to_owned(),
            owned_orphan("substrate-run-orphan", "2020-01-01T00:00:00Z"),
        );
        let (_directory, kubeconfig) = serve_cluster(&cluster);
        let refused = open_against(&kubeconfig).err();
        assert!(
            refused
                .as_deref()
                .is_some_and(|error| error.starts_with("confinement.adapter-unserved")),
            "date {date:?}: open was not refused by name: {refused:?}"
        );
        assert_eq!(
            cluster.keys(),
            ["networkpolicies/substrate-run-orphan"],
            "date {date:?}: the sweep deleted without a server clock"
        );
    }
}

/// A `creationTimestamp` ahead of the server's `Date` (a skewed control-plane member) is young, not
/// old; an owned policy whose run label is empty or not a DNS label is never a candidate.
#[test]
fn security_a_future_or_unnamed_policy_is_never_swept() {
    let cluster = Arc::new(Cluster::new());
    let future = (chrono::Utc::now() + chrono::Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    {
        let mut objects = cluster.objects.lock();
        objects.insert(
            "networkpolicies/substrate-run-future".to_owned(),
            owned_orphan("substrate-run-future", &future),
        );
        for (name, run) in [
            ("substrate-run-empty", ""),
            ("substrate-run-bad", "Bad_Label"),
        ] {
            let mut object = owned_orphan(name, "2020-01-01T00:00:00Z");
            object["metadata"]["labels"][pod::RUN_LABEL] = json!(run);
            objects.insert(format!("networkpolicies/{name}"), object);
        }
    }
    let (_directory, kubeconfig) = serve_cluster(&cluster);
    let opened = open_against(&kubeconfig);
    assert!(opened.is_ok(), "open: {:?}", opened.err());
    assert_eq!(
        cluster.keys(),
        [
            "networkpolicies/substrate-run-bad",
            "networkpolicies/substrate-run-empty",
            "networkpolicies/substrate-run-future",
        ],
        "requests: {:#?}",
        cluster.requests.lock()
    );
}
