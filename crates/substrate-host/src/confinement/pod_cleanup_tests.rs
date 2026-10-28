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
};
/// The deletion of the Pod after which the fake kubelet lets it go: past the two rounds of
/// `remove_attempts` the task made before this story.
const RELEASED_AFTER: u32 = 3 * PACED.remove_attempts + 1;

#[derive(Default)]
struct Cluster {
    pod_deletes: AtomicU32,
    pod_gone: AtomicBool,
    policy_gone: AtomicBool,
    /// Each request's method and path, in arrival order.
    requests: Mutex<Vec<String>>,
}

fn answer(cluster: &Cluster, method: &str, path: &str) -> (u16, String) {
    let pod = format!("/api/v1/namespaces/{NAMESPACE}/pods/{RUN}");
    let policy = format!("/apis/networking.k8s.io/v1/namespaces/{NAMESPACE}/networkpolicies/{RUN}");
    let gone = (404, r#"{"kind":"Status","code":404}"#.to_owned());
    let (pod_path, _) = path.split_once('?').unwrap_or((path, ""));
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
                if asked >= RELEASED_AFTER {
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
    let mut parts = line.split(' ');
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    cluster.requests.lock().push(format!("{method} {path}"));
    let (status, payload) = answer(cluster, method, path);
    let writer = stream.get_mut();
    write!(
        writer,
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{payload}",
        payload.len()
    )?;
    writer.flush()?;
    writer.conn.send_close_notify();
    writer.flush()
}

/// Starts the fake on loopback and returns the API the adapter would open against it.
fn start(cluster: &Arc<Cluster>) -> Api {
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
            "extensions": [{"name": POD_SETTINGS_EXTENSION, "extension": {"pod_pids_limit": 1024}}],
        }}],
        "users": [{"name": "fake", "user": {"token": "fake-token"}}],
    });
    let (mut api, _) = parse_cluster(kubeconfig.to_string().as_bytes()).expect("the fake parses");
    api.pacing = PACED;
    api
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
