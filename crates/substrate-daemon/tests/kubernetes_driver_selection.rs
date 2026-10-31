#![forbid(unsafe_code)]
//! `--driver kubernetes` at the composition root (design 34 § 3 and § 6.1, § 12 U10).
//!
//! The daemon opens the namespace driver from design 28's self-contained JSON kubeconfig and
//! nothing else. A host-only declaration beside it is a startup refusal, never a setting the
//! namespace driver silently ignores. An unreadable configuration stops the daemon before it
//! listens (§ 6.1 step 1). A configuration that loads but whose cluster does not open the driver
//! (steps 2 to 7) still serves the socket, with every driver fact absent: nothing is published
//! that was not observed (invariant 3). The subject stays the peer's uid; nothing in the
//! configuration can name one.
//!
//! No cluster is reached: the configuration names a loopback port nothing listens on, with a CA
//! generated here, so no step past the load can succeed. The fake API is not used, because its
//! feature is refused on every edge of a crate with a binary target (design 34 § 10.1), and no
//! ambient kubeconfig is read.

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use base64::Engine as _;
use serde_json::{Value, json};
use substrate_daemon::{
    App, DaemonConfig, DriverSelection, EgressAperture, GitSourceConfig, Identity,
    OpsListenerConfig, SecretSlot, StoreBackendLedger, SystemAuthority, WritableRootConfig, router,
    serve,
};
use substrate_host::{Driver, HostConfig, HostDriver};
use substrate_kubernetes::NamespaceDriver;
use substrate_store::Store;
use tempfile::TempDir;
use tower::ServiceExt as _;

#[path = "support/ports.rs"]
mod ports;
use ports::unused_address;

/// Sets one host-only declaration on a configuration.
type Declare = fn(&mut DaemonConfig, &Path);

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn tree() -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temporary tree");
    let base = std::fs::canonicalize(directory.path()).expect("canonical tree");
    std::fs::create_dir(base.join("other")).expect("tree directory");
    (directory, base)
}

/// A daemon selecting the namespace driver from `kubeconfig`, and nothing host-only.
fn config(base: &Path, kubeconfig: PathBuf) -> DaemonConfig {
    let mut config = DaemonConfig::minimum(
        base.join("other/api.sock"),
        base.join("state/state.db"),
        PathBuf::new(),
        "d",
        vec![nix::unistd::getuid().as_raw()],
    );
    config.driver = DriverSelection::Kubernetes { config: kubeconfig };
    config
}

async fn refusal(config: DaemonConfig) -> String {
    match tokio::time::timeout(Duration::from_secs(10), serve(config)).await {
        Ok(Err(error)) => error.to_string(),
        Ok(Ok(())) => "serve returned Ok(())".to_owned(),
        Err(_) => "serve did not return within 10 s".to_owned(),
    }
}

/// Every host-only declaration, one at a time, beside `--driver kubernetes`.
#[tokio::test(flavor = "multi_thread")]
async fn a_kubernetes_daemon_refuses_every_host_only_declaration() {
    let (_directory, base) = tree();
    let kubeconfig = base.join("other/kubeconfig.json");
    let cases: [(&str, Declare); 8] = [
        ("workspaces", |config, base| {
            config.workspaces = base.join("workspaces");
        }),
        ("cgroup_root", |config, base| {
            config.cgroup_root = Some(base.join("cgroup"));
        }),
        ("project_quota_ids", |config, _| {
            config.project_quota_ids = Some((200_000, 200_127));
        }),
        ("git_sources", |config, _| {
            config.git_sources = vec![GitSourceConfig {
                name: "origin".to_owned(),
                base_url: "https://git.invalid/".to_owned(),
            }];
        }),
        ("secret_slots", |config, base| {
            config.secret_slots = vec![SecretSlot {
                name: "token".to_owned(),
                path: base.join("other/token"),
            }];
        }),
        ("egress_apertures", |config, _| {
            config.egress_apertures = vec![EgressAperture {
                name: "model".to_owned(),
                host: "127.0.0.1".to_owned(),
                port: 443,
                max_bytes: None,
            }];
        }),
        ("writable_roots", |config, base| {
            config.writable_roots = vec![WritableRootConfig {
                name: "cache".to_owned(),
                path: base.join("cache"),
            }];
        }),
        ("ca_bundle", |config, base| {
            config.ca_bundle = Some(base.join("other/ca.pem"));
        }),
    ];
    for (field, declare) in cases {
        let mut declared = config(&base, kubeconfig.clone());
        declare(&mut declared, &base);
        let observed = refusal(declared).await;
        assert!(
            observed.starts_with("config.driver-conflict: ") && observed.contains(field),
            "{field} beside --driver kubernetes: {observed:?}"
        );
    }
    assert!(
        !base.join("other/api.sock").exists(),
        "a refused configuration bound the socket"
    );
}

/// § 6.1 step 1: a configuration that cannot be loaded stops the daemon, naming the reason code,
/// before anything listens.
#[tokio::test(flavor = "multi_thread")]
async fn an_unloadable_namespace_configuration_stops_the_daemon_before_it_listens() {
    let (_directory, base) = tree();
    let absent = refusal(config(&base, base.join("other/absent.json"))).await;
    assert!(
        absent.starts_with("kubernetes.config-invalid: "),
        "{absent:?}"
    );
    let without_extension = base.join("other/no-extension.json");
    std::fs::write(
        &without_extension,
        serde_json::to_vec(&unreachable_kubeconfig(None)).expect("kubeconfig"),
    )
    .expect("write kubeconfig");
    let refused = refusal(config(&base, without_extension)).await;
    assert!(
        refused.starts_with("kubernetes.config-invalid: "),
        "{refused:?}"
    );
    assert!(!base.join("other/api.sock").exists());
}

/// Steps 2 to 7 failing is not a startup refusal: the configuration loads, the cluster it names
/// answers nothing, so the driver never opens, and the daemon serves its socket publishing only
/// what it observes itself — the driver kind and the daemon-side facts, and no driver fact at all.
#[tokio::test(flavor = "multi_thread")]
async fn an_unopened_namespace_driver_is_served_with_every_driver_fact_absent() {
    let (_directory, base) = tree();
    let kubeconfig = unreachable_kubeconfig(Some(extension()));
    let path = base.join("other/kubeconfig.json");
    std::fs::write(&path, serde_json::to_vec(&kubeconfig).expect("kubeconfig"))
        .expect("write kubeconfig");
    let socket = base.join("other/api.sock");
    let daemon = tokio::spawn(serve(config(&base, path)));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(
            !daemon.is_finished(),
            "the daemon stopped: {:?}",
            daemon.await
        );
        assert!(Instant::now() < deadline, "the daemon never listened");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let machine = tokio::task::spawn_blocking(move || get(&socket, "/v1/machine"))
        .await
        .expect("the client thread");
    daemon.abort();
    let snapshot = &machine["result"];
    assert_eq!(snapshot["driver"], "kubernetes", "{machine}");
    let facts = snapshot["facts"].as_object().expect("facts");
    assert_eq!(facts.get("events.pull"), Some(&json!(true)), "{machine}");
    for absent in [
        "exec.namespaces",
        "exec.cgroup-limits",
        "exec.cgroup-kill",
        "exec.no-egress",
        "confinement.no-egress",
        "exec.argv-only",
        "exec.inline-capsule",
        "exec.max-current",
        "workspace.guarded-io",
        "workspace.max-current",
    ] {
        assert!(!facts.contains_key(absent), "{absent} published: {machine}");
    }
}

/// ADR 0045 D5 A and design 34 § 6.1: readiness reads one snapshot, taken once the namespace
/// driver's first open sequence has settled. Until then `driver` fails, even for a fact the
/// daemon publishes itself. Here the first open's connection is closed unanswered, a refusal the
/// open's retry can clear, so nothing settles and the daemon stays not ready. The retry's rules
/// review is held, then answered with a grant beyond the Role: `kubernetes.rbac-excess`, which no
/// retry clears, settles it, the snapshot taken then publishes `events.pull`, and the daemon is
/// ready. A host daemon's snapshot is taken as it starts, as before, so it is ready at once.
#[tokio::test(flavor = "multi_thread")]
async fn readiness_reads_the_snapshot_taken_once_the_first_open_sequence_has_settled() {
    let required = vec!["events.pull".to_owned()];
    let (_directory, base) = tree();
    let api = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("an API address");
    let server = format!("https://{}", api.local_addr().expect("API address"));
    let (ca_pem, acceptor) = api_identity();
    let path = base.join("other/kubeconfig.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&kubeconfig_naming(&server, &ca_pem, Some(extension())))
            .expect("kubeconfig"),
    )
    .expect("write kubeconfig");
    let ops = unused_address();
    let mut namespace = config(&base, path);
    namespace.ops = Some(OpsListenerConfig {
        listen: ops,
        require_facts: required.clone(),
    });
    let daemon = tokio::spawn(serve(namespace));
    let (unanswered, _) = tokio::time::timeout(Duration::from_secs(10), api.accept())
        .await
        .expect("the open sequence reached the API")
        .expect("accept the open's connection");
    let pending = "store ok\nschema ok\nmaintenance ok\ndriver failed\n";
    let socket = base.join("other/api.sock");
    let (status, body) = readiness_until(ops, |status, body| {
        socket.exists() && status == 503 && body == pending
    })
    .await;
    assert_eq!(
        (status, body.as_str()),
        (503, pending),
        "before the open settled"
    );
    drop(unanswered);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        readyz(ops).await,
        Some((503, pending.to_owned())),
        "an open refused for a reason its retry can clear has not settled"
    );

    // The retry, after the driver's 10 s: its rules review is held, then refused for good.
    let (retry, _) = tokio::time::timeout(Duration::from_secs(15), api.accept())
        .await
        .expect("the open is retried")
        .expect("accept the retry's connection");
    let mut review = acceptor.accept(retry).await.expect("the retry's TLS");
    let request = read_request(&mut review).await;
    assert!(
        request.starts_with("POST /apis/authorization.k8s.io/v1/selfsubjectrulesreviews "),
        "{request}"
    );
    assert_eq!(
        readyz(ops).await,
        Some((503, pending.to_owned())),
        "an open in flight is not ready"
    );
    answer_excess_rules(&mut review).await;
    let ready = "store ok\nschema ok\nmaintenance ok\ndriver ok\n";
    let settled = readiness_until(ops, |status, _| status == 200).await;
    assert_eq!(
        (settled.0, settled.1.as_str()),
        (200, ready),
        "the open settled"
    );
    daemon.abort();

    let (_host_directory, host_base) = tree();
    std::fs::create_dir(host_base.join("workspaces")).expect("workspaces");
    let host_ops = unused_address();
    let mut host = DaemonConfig::minimum(
        host_base.join("other/api.sock"),
        host_base.join("state/state.db"),
        host_base.join("workspaces"),
        "d",
        vec![nix::unistd::getuid().as_raw()],
    );
    host.ops = Some(OpsListenerConfig {
        listen: host_ops,
        require_facts: required,
    });
    let host_daemon = tokio::spawn(serve(host));
    let host_ready = readiness_until(host_ops, |status, _| status == 200).await;
    host_daemon.abort();
    assert_eq!(
        (host_ready.0, host_ready.1.as_str()),
        (200, ready),
        "a host daemon"
    );
}

/// `story:kubernetes-collisions-metric`, design 34 § 4: the operational listener of a daemon
/// running the Kubernetes driver serves `substrate_kubernetes_collisions_total`, the driver's own
/// count, here 0 because the driver never reached a cluster. A host daemon's scrape carries no
/// such series. That a real collision moves the count by one is the driver crate's case on the
/// fake API (`one_collision_on_the_api_moves_the_ports_collision_count_by_exactly_one`), which
/// this crate cannot link (design 34 § 10.1).
#[tokio::test(flavor = "multi_thread")]
async fn only_a_kubernetes_daemon_serves_the_collision_series() {
    let collisions = "substrate_kubernetes_collisions_total 0\n";
    let (_directory, base) = tree();
    let path = base.join("other/kubeconfig.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&unreachable_kubeconfig(Some(extension()))).expect("kubeconfig"),
    )
    .expect("write kubeconfig");
    let ops = unused_address();
    let mut namespace = config(&base, path);
    namespace.ops = Some(OpsListenerConfig {
        listen: ops,
        require_facts: Vec::new(),
    });
    let daemon = tokio::spawn(serve(namespace));
    let scrape = metrics_once_listening(ops).await;
    daemon.abort();
    assert!(
        scrape.contains("# TYPE substrate_kubernetes_collisions counter\n"),
        "{scrape}"
    );
    assert!(scrape.contains(collisions), "{scrape}");

    let (_host_directory, host_base) = tree();
    std::fs::create_dir(host_base.join("workspaces")).expect("workspaces");
    let host_ops = unused_address();
    let mut host = DaemonConfig::minimum(
        host_base.join("other/api.sock"),
        host_base.join("state/state.db"),
        host_base.join("workspaces"),
        "d",
        vec![nix::unistd::getuid().as_raw()],
    );
    host.ops = Some(OpsListenerConfig {
        listen: host_ops,
        require_facts: Vec::new(),
    });
    let host_daemon = tokio::spawn(serve(host));
    let host_scrape = metrics_once_listening(host_ops).await;
    host_daemon.abort();
    assert!(host_scrape.ends_with("# EOF\n"), "{host_scrape}");
    assert!(
        !host_scrape.contains("substrate_kubernetes"),
        "{host_scrape}"
    );
}

/// Design 30 § 6, ADR 0048 D7: the operational listener of a host daemon names the confinement
/// adapter it opened, `linux`, in `substrate_adapter_info`, a gauge that is always 1. A daemon
/// running `--driver kubernetes` opens no confinement adapter, so its scrape carries no such
/// series, and neither does any client answer.
#[tokio::test(flavor = "multi_thread")]
async fn only_a_daemon_that_opened_a_confinement_adapter_names_it_to_the_operator() {
    let (_directory, base) = tree();
    let path = base.join("other/kubeconfig.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&unreachable_kubeconfig(Some(extension()))).expect("kubeconfig"),
    )
    .expect("write kubeconfig");
    let ops = unused_address();
    let mut namespace = config(&base, path);
    namespace.ops = Some(OpsListenerConfig {
        listen: ops,
        require_facts: Vec::new(),
    });
    let daemon = tokio::spawn(serve(namespace));
    let scrape = metrics_once_listening(ops).await;
    daemon.abort();
    assert!(scrape.ends_with("# EOF\n"), "{scrape}");
    assert!(!scrape.contains("substrate_adapter"), "{scrape}");

    let (_host_directory, host_base) = tree();
    std::fs::create_dir(host_base.join("workspaces")).expect("workspaces");
    let host_ops = unused_address();
    let mut host = DaemonConfig::minimum(
        host_base.join("other/api.sock"),
        host_base.join("state/state.db"),
        host_base.join("workspaces"),
        "d",
        vec![nix::unistd::getuid().as_raw()],
    );
    host.ops = Some(OpsListenerConfig {
        listen: host_ops,
        require_facts: Vec::new(),
    });
    let host_daemon = tokio::spawn(serve(host));
    let host_scrape = metrics_once_listening(host_ops).await;
    host_daemon.abort();
    let adapter = host_scrape
        .lines()
        .filter(|line| line.contains("substrate_adapter_info"))
        .collect::<Vec<_>>();
    assert_eq!(
        adapter,
        [
            "# TYPE substrate_adapter_info gauge",
            "# HELP substrate_adapter_info The confinement adapter this daemon opened.",
            "substrate_adapter_info{adapter=\"linux\"} 1",
        ],
        "{host_scrape}"
    );
}

/// The first `/metrics` body the operational listener answers `200` with, within 10 s.
async fn metrics_once_listening(ops: std::net::SocketAddr) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some((200, body)) = ops_get(ops, "/metrics").await {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "the operational listener never answered"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `/readyz` until `done` holds for its answer or 10 s have passed; the last answer.
async fn readiness_until(
    ops: std::net::SocketAddr,
    mut done: impl FnMut(u16, &str) -> bool,
) -> (u16, String) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = (0, String::new());
    while Instant::now() < deadline {
        if let Some(answer) = readyz(ops).await {
            if done(answer.0, &answer.1) {
                return answer;
            }
            last = answer;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    last
}

/// One `GET /readyz` on the operational listener: its status and body, or `None` before it
/// listens.
async fn readyz(ops: std::net::SocketAddr) -> Option<(u16, String)> {
    ops_get(ops, "/readyz").await
}

/// One `GET` of `path` on the operational listener: its status and body, or `None` before it
/// listens.
async fn ops_get(ops: std::net::SocketAddr, path: &str) -> Option<(u16, String)> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut stream = tokio::net::TcpStream::connect(ops).await.ok()?;
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: substrate\r\n\r\n").as_bytes())
        .await
        .ok()?;
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8(answer).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head.get(9..12)?.parse().ok()?;
    Some((status, body.to_owned()))
}

/// Design 34 § 8 and § 14 Q3 A: every `workspace.adopt` on a namespace machine is
/// `workspace.adopt-unserved` (501), decided before any name is looked up. A free name, a name
/// another subject holds and a name the caller holds get the same answer, so the answer says
/// nothing about who holds which name. The names are held through a host driver over the same
/// store, as a host daemon would have left them; the namespace driver is never opened.
#[tokio::test(flavor = "multi_thread")]
async fn every_adopt_on_a_namespace_machine_is_unserved_before_any_name_is_looked_up() {
    const OWNER: &str = "local:1001";
    const CALLER: &str = "local:1000";
    let (_directory, base) = tree();
    std::fs::create_dir_all(base.join("workspaces/project")).expect("operator directory");
    let store = Arc::new(Store::open(base.join("state.db")).expect("state store"));
    let host = HostDriver::open(HostConfig::minimum(base.join("workspaces"))).expect("host driver");
    let host = App::new(Arc::clone(&store), host as Arc<dyn Driver>, "d");
    let held = adopt(&host, OWNER, "01JFXN0ADOPTHELD000000000", "project").await;
    assert_eq!(held.0, StatusCode::CREATED, "{}", held.1);
    let kubeconfig =
        serde_json::to_vec(&unreachable_kubeconfig(Some(extension()))).expect("kubeconfig");
    let ledger = Arc::new(StoreBackendLedger::new(
        Arc::clone(&store),
        "d",
        Arc::new(SystemAuthority),
    ));
    let namespace = NamespaceDriver::parse(&kubeconfig, "d", ledger).expect("the configuration");
    let namespace = App::new(store, Arc::new(namespace) as Arc<dyn Driver>, "d");
    for (subject, operation, root_name, holder) in [
        (CALLER, "01JFXN0ADOPTFREE000000000", "free", "nobody"),
        (
            CALLER,
            "01JFXN0ADOPTOTHER00000000",
            "project",
            "another subject",
        ),
        (OWNER, "01JFXN0ADOPTOWN0000000000", "project", "the caller"),
    ] {
        let (status, body) = adopt(&namespace, subject, operation, root_name).await;
        assert_eq!(
            status,
            StatusCode::NOT_IMPLEMENTED,
            "a name {holder} holds: {body}"
        );
        assert_eq!(
            body["error"]["code"],
            substrate_wire::WORKSPACE_ADOPT_UNSERVED,
            "a name {holder} holds: {body}"
        );
        assert_eq!(body["error"]["class"], "unserved", "{body}");
        assert!(body["error"].get("address").is_none(), "{body}");
    }
}

async fn adopt(
    app: &Arc<App>,
    subject: &str,
    operation: &str,
    root_name: &str,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/workspaces/adopt")
        .header("content-type", "application/json")
        .header("x-request-id", "req_namespace_adopt")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "op": operation,
                "input": {"root_name": root_name, "labels": {}},
            }))
            .expect("mutation"),
        ))
        .expect("request");
    let identity = Identity {
        subject: subject.to_owned(),
        actor: "namespace-adopt-test".to_owned(),
        principal: None,
    };
    let response = router(Arc::clone(app))
        .layer(Extension(identity))
        .oneshot(request)
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65_536)
        .await
        .expect("response body");
    (
        status,
        serde_json::from_slice(&bytes).expect("JSON response"),
    )
}

/// The namespace driver's context extension, naming images by digest and nothing host-only.
fn extension() -> Value {
    json!({
        "storage_class": "standard",
        "workspace_size_bytes": 1_073_741_824_u64,
        "executor_image": format!("registry.invalid/substrate/executor@sha256:{DIGEST}"),
        "exec_image": format!("registry.invalid/busybox@sha256:{DIGEST}"),
        "default_deny_policy": "default-deny",
        "egress_probe_address": "192.0.2.10:443",
        "pod_pids_limit": 1024,
        "container_log_max_bytes": 10_485_760,
    })
}

/// A self-contained JSON kubeconfig naming a loopback port nothing listens on, trusted through a
/// CA made here, with `extension` as the namespace driver's context extension.
fn unreachable_kubeconfig(extension: Option<Value>) -> Value {
    let (ca, _) = certificate_authority();
    kubeconfig_naming("https://127.0.0.1:1", &ca.pem(), extension)
}

/// A CA made here and its issuer.
fn certificate_authority() -> (rcgen::Certificate, rcgen::Issuer<'static, rcgen::KeyPair>) {
    let key = rcgen::KeyPair::generate().expect("a CA key");
    let mut params =
        rcgen::CertificateParams::new(Vec::<String>::new()).expect("CA certificate parameters");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = params.self_signed(&key).expect("a CA certificate");
    (ca, rcgen::Issuer::new(params, key))
}

/// A CA's PEM and a TLS acceptor holding a `127.0.0.1` certificate it issued: an API server the
/// kubeconfig naming that CA trusts.
fn api_identity() -> (String, tokio_rustls::TlsAcceptor) {
    let (ca, issuer) = certificate_authority();
    let key = rcgen::KeyPair::generate().expect("a server key");
    let server = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()])
        .expect("server certificate parameters")
        .signed_by(&key, &issuer)
        .expect("a server certificate");
    let private = rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der())
        .expect("a server key in DER");
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![server.der().clone()], private)
        .expect("the API's TLS configuration");
    (ca.pem(), tokio_rustls::TlsAcceptor::from(Arc::new(tls)))
}

/// One HTTP/1.1 request's head and body, read whole.
async fn read_request(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> String {
    use tokio::io::AsyncReadExt as _;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4_096];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buffer))
            .await
            .expect("the request arrives")
            .expect("read the request");
        assert_ne!(read, 0, "the request ended early");
        bytes.extend_from_slice(&buffer[..read]);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if body.len() >= length {
                return text;
            }
        }
    }
}

/// Answers a rules review with one grant beyond the driver's Role: `get` on `secrets`.
async fn answer_excess_rules(stream: &mut (impl tokio::io::AsyncWrite + Unpin)) {
    use tokio::io::AsyncWriteExt as _;
    let body = json!({
        "apiVersion": "authorization.k8s.io/v1",
        "kind": "SelfSubjectRulesReview",
        "status": {
            "resourceRules": [{"verbs": ["get"], "apiGroups": [""], "resources": ["secrets"]}],
            "nonResourceRules": [],
            "incomplete": false,
        },
    })
    .to_string();
    let answer = format!(
        "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(answer.as_bytes())
        .await
        .expect("answer the review");
    stream.shutdown().await.expect("close the answer");
}

/// A self-contained JSON kubeconfig naming `server`, trusted through the CA `ca_pem`, with
/// `extension` as the namespace driver's context extension.
fn kubeconfig_naming(server: &str, ca_pem: &str, extension: Option<Value>) -> Value {
    let mut context = json!({"cluster": "c", "user": "u", "namespace": "substrate"});
    if let Some(extension) = extension {
        context["extensions"] = json!([{
            "name": "b10x-substrate/namespace-driver",
            "extension": extension,
        }]);
    }
    json!({
        "apiVersion": "v1",
        "kind": "Config",
        "current-context": "c",
        "clusters": [{"name": "c", "cluster": {
            "server": server,
            "certificate-authority-data":
                base64::engine::general_purpose::STANDARD.encode(ca_pem),
        }}],
        "contexts": [{"name": "c", "context": context}],
        "users": [{"name": "u", "user": {"token": "unused-token"}}],
    })
}

/// One `GET` over the daemon's socket, answered as JSON.
fn get(socket: &Path, path: &str) -> Value {
    let mut stream = UnixStream::connect(socket).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: substrate\r\nConnection: close\r\n\r\n"
    )
    .expect("request");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("response");
    let text = String::from_utf8(response).expect("UTF-8 response");
    let (head, body) = text.split_once("\r\n\r\n").expect("HTTP response");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(body)
    } else {
        body.to_owned()
    };
    serde_json::from_str(&body).expect("JSON body")
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    loop {
        let (size, rest) = body.split_once("\r\n").expect("chunk size");
        let size = usize::from_str_radix(size.trim(), 16).expect("hex chunk size");
        if size == 0 {
            return out;
        }
        out.push_str(&rest[..size]);
        body = &rest[size + 2..];
    }
}
