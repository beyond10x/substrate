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
    App, DaemonConfig, DriverSelection, EgressAperture, GitSourceConfig, Identity, SecretSlot,
    StoreBackendLedger, SystemAuthority, WritableRootConfig, router, serve,
};
use substrate_host::{Driver, HostConfig, HostDriver};
use substrate_kubernetes::NamespaceDriver;
use substrate_store::Store;
use tempfile::TempDir;
use tower::ServiceExt as _;

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
    let key = rcgen::KeyPair::generate().expect("a CA key");
    let mut params =
        rcgen::CertificateParams::new(Vec::<String>::new()).expect("CA certificate parameters");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = params.self_signed(&key).expect("a CA certificate");
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
            "server": "https://127.0.0.1:1",
            "certificate-authority-data":
                base64::engine::general_purpose::STANDARD.encode(ca.pem()),
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
