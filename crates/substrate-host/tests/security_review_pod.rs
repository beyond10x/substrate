//! Security review of `story:pod-adapter` (`docs/design/28-pod-confinement-adapter.md`).
//!
//! Conformance cases for the cluster-credential and Pod-authority rules design 28 states, run
//! against a fake TLS API server on loopback that keeps the objects it is sent.

#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use parking_lot::Mutex;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};
use substrate_host::confinement::{
    AdapterName, Command, ConfinementAdapter, ConfinementError, ConfinementProfile, IoBounds,
    ProcessBounds, RunState,
    pod::{self, PodAdapter, PodRequest, PodSettings},
};

const IMAGE: &str = "docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e";
const NAMESPACE: &str = "substrate-sec";
const TOKEN: &str = "sec-review-bearer-0123456789abcdef";

fn b64(bytes: impl AsRef<[u8]>) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// One request the fake API server read.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: String,
    authorization: Option<String>,
}

/// What the fake server answers.
enum Reply {
    Answer(u16, Value),
    /// Read the request, then close the connection without an answer.
    Drop,
}

type Handler = Arc<dyn Fn(&Seen, &mut BTreeMap<String, Value>) -> Reply + Send + Sync>;

/// A named way a Pod create's answer is lost.
type LostAnswer = (&'static str, fn() -> Reply);

struct FakeApi {
    address: std::net::SocketAddr,
    ca_pem: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    objects: Arc<Mutex<BTreeMap<String, Value>>>,
    stop: Arc<AtomicBool>,
    directory: tempfile::TempDir,
}

impl Drop for FakeApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl FakeApi {
    fn start(names: &[&str], handler: Handler) -> Self {
        let identity = rcgen::generate_simple_self_signed(
            names
                .iter()
                .map(|n| (*n).to_owned())
                .collect::<Vec<String>>(),
        )
        .expect("identity");
        let config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![identity.cert.der().clone()],
                    rustls::pki_types::PrivatePkcs8KeyDer::from(
                        identity.signing_key.serialize_der(),
                    )
                    .into(),
                )
                .expect("server TLS"),
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        listener.set_nonblocking(true).expect("nonblocking");
        let address = listener.local_addr().expect("address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let objects = Arc::new(Mutex::new(BTreeMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, objects2, stop2) = (Arc::clone(&seen), Arc::clone(&objects), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                let socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                let (config, seen, objects, handler) = (
                    Arc::clone(&config),
                    Arc::clone(&seen2),
                    Arc::clone(&objects2),
                    Arc::clone(&handler),
                );
                std::thread::spawn(move || {
                    socket.set_nonblocking(false).expect("blocking");
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .expect("timeout");
                    let tls = ServerConnection::new(config).expect("session");
                    let mut stream = BufReader::new(StreamOwned::new(tls, socket));
                    let _ = serve(&mut stream, &seen, &objects, &*handler);
                });
            }
        });
        Self {
            address,
            ca_pem: identity.cert.pem(),
            seen,
            objects,
            stop,
            directory: tempfile::tempdir().expect("directory"),
        }
    }

    fn server(&self) -> String {
        format!("https://{}", self.address)
    }

    /// A kubeconfig for this server with a bearer token, after `edit`.
    fn kubeconfig(&self, extension: Option<Value>, edit: &dyn Fn(&mut Value)) -> PathBuf {
        let mut context = json!({"cluster": "c", "user": "u", "namespace": NAMESPACE});
        if let Some(extension) = extension {
            context["extensions"] = json!([{"name": "b10x-substrate/pod", "extension": extension}]);
        }
        let mut config = json!({
            "apiVersion": "v1",
            "kind": "Config",
            "current-context": "x",
            "clusters": [{"name": "c", "cluster": {
                "server": self.server(),
                "certificate-authority-data": b64(&self.ca_pem),
            }}],
            "contexts": [{"name": "x", "context": context}],
            "users": [{"name": "u", "user": {"token": TOKEN}}],
        });
        edit(&mut config);
        let path = self.directory.path().join(format!("k{}.json", ulid_like()));
        std::fs::write(&path, serde_json::to_vec(&config).expect("json")).expect("write");
        path
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().clone()
    }
}

fn ulid_like() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos()
}

fn serve(
    stream: &mut BufReader<StreamOwned<ServerConnection, std::net::TcpStream>>,
    seen: &Mutex<Vec<Seen>>,
    objects: &Mutex<BTreeMap<String, Value>>,
    handler: &(dyn Fn(&Seen, &mut BTreeMap<String, Value>) -> Reply + Send + Sync),
) -> std::io::Result<()> {
    let mut line = String::new();
    stream.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut length = 0_usize;
    let mut authorization = None;
    loop {
        let mut header = String::new();
        stream.read_line(&mut header)?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let lower = header.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("authorization:") {
            authorization = Some(header["authorization:".len()..].trim().to_owned());
        }
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body)?;
    let request = Seen {
        method,
        path,
        body: String::from_utf8_lossy(&body).into_owned(),
        authorization,
    };
    seen.lock().push(request.clone());
    let reply = handler(&request, &mut objects.lock());
    let (status, payload) = match reply {
        Reply::Drop => return Ok(()),
        Reply::Answer(status, payload) => (status, payload.to_string()),
    };
    let writer = stream.get_mut();
    write!(
        writer,
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len()
    )?;
    writer.flush()?;
    writer.conn.send_close_notify();
    let _ = writer.flush();
    Ok(())
}

fn status(code: u16, message: &str) -> Reply {
    Reply::Answer(
        code,
        json!({"kind": "Status", "code": code, "message": message}),
    )
}

/// The object key a path names: `pods/<name>` or `networkpolicies/<name>`.
fn key(path: &str) -> Option<String> {
    let path = path.split('?').next().unwrap_or_default();
    let mut parts = path.rsplit('/');
    let name = parts.next()?;
    let collection = parts.next()?;
    matches!(collection, "pods" | "networkpolicies").then(|| format!("{collection}/{name}"))
}

/// A cluster that stores what it is sent. `pod_create` decides the answer to a Pod create after
/// the Pod is stored; `phase` is the phase every stored Pod reports.
fn storing(pod_create: fn() -> Reply, phase: &'static str) -> Handler {
    Arc::new(
        move |request: &Seen, objects: &mut BTreeMap<String, Value>| {
            let path = request.path.split('?').next().unwrap_or_default();
            match request.method.as_str() {
                "GET" if path.ends_with("/pods") => {
                    Reply::Answer(200, json!({"kind": "PodList", "items": []}))
                }
                "GET" if path.ends_with("/log") => Reply::Answer(200, json!("hi")),
                // Open's sweep (design 28): the stored policies, which carry no creation time
                // and so are never its candidates.
                "GET" if path.ends_with("/networkpolicies") => {
                    let items: Vec<Value> = objects
                        .iter()
                        .filter(|(key, _)| key.starts_with("networkpolicies/"))
                        .map(|(_, object)| object.clone())
                        .collect();
                    Reply::Answer(200, json!({"kind": "NetworkPolicyList", "items": items}))
                }
                "POST" => {
                    let object: Value = serde_json::from_str(&request.body).expect("object");
                    let name = object["metadata"]["name"]
                        .as_str()
                        .expect("name")
                        .to_owned();
                    let collection = if path.ends_with("/pods") {
                        "pods"
                    } else {
                        "networkpolicies"
                    };
                    objects.insert(format!("{collection}/{name}"), object.clone());
                    if collection == "pods" {
                        pod_create()
                    } else {
                        Reply::Answer(201, object)
                    }
                }
                "GET" => match key(path).and_then(|key| objects.get(&key).cloned()) {
                    Some(mut object) => {
                        if key(path).is_some_and(|key| key.starts_with("pods/")) {
                            object["status"] = json!({
                                "phase": phase,
                                "containerStatuses": [{"name": "run", "state": {"terminated":
                                    {"exitCode": 0, "reason": "Completed"}}}],
                            });
                        }
                        Reply::Answer(200, object)
                    }
                    None => status(404, "not found"),
                },
                "DELETE" => match key(path).and_then(|key| objects.remove(&key)) {
                    Some(object) => Reply::Answer(200, object),
                    None => status(404, "not found"),
                },
                _ => status(500, "unexpected"),
            }
        },
    )
}

fn admitted_profile() -> ConfinementProfile {
    ConfinementProfile {
        image: Some(IMAGE.to_owned()),
        ..ConfinementProfile::default()
    }
}

fn command(argv: &[&str], env: &[(&str, &str)]) -> Command {
    Command {
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        env: env
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
    }
}

fn open(path: PathBuf) -> Result<Arc<PodAdapter>, ConfinementError> {
    pod::open_pod(PodRequest {
        cluster: Some(path),
    })
}

// --- 1. the Pod is never without its NetworkPolicy -----------------------------------------------

/// Design 28: "The policy is created first and deleted last" and `start`'s own comment: "The
/// policy first, so the Pod is never without it." A Pod create the API server stored but whose
/// answer did not arrive (a dropped connection, a 504 from a load balancer, an admission webhook
/// past the 30 s client timeout) is not a Pod the API refused. Deleting the policy then leaves a
/// stored Pod with no default-deny policy and no run tracking it.
#[tokio::test(flavor = "multi_thread")]
async fn a_pod_create_with_a_lost_answer_never_leaves_the_pod_without_its_policy() {
    let cases: [LostAnswer; 2] = [
        ("the connection closed after the Pod was stored", || {
            Reply::Drop
        }),
        ("a 504 after the Pod was stored", || {
            status(
                504,
                "the server was unable to return a response in the time allotted",
            )
        }),
    ];
    let mut failures = Vec::new();
    for (name, pod_create) in cases {
        let api = FakeApi::start(&["127.0.0.1"], storing(pod_create, "Running"));
        // story:pod-adapter, adversary decision 7: without a declared pods PID limit no profile
        // is admitted, so the cluster declares one.
        let adapter =
            open(api.kubeconfig(Some(json!({"pod_pids_limit": 1024})), &|_| {})).expect("opens");
        let admitted = adapter.admit(&admitted_profile()).expect("admitted");
        let error = adapter
            .start(&admitted, &command(&["/bin/sleep", "600"], &[]))
            .await
            .expect_err("start reports the lost answer");
        let objects = api.objects.lock().clone();
        let pods: BTreeSet<String> = objects
            .keys()
            .filter_map(|k| k.strip_prefix("pods/").map(str::to_owned))
            .collect();
        let policies: BTreeSet<String> = objects
            .keys()
            .filter_map(|k| k.strip_prefix("networkpolicies/").map(str::to_owned))
            .collect();
        let unguarded: Vec<&String> = pods.difference(&policies).collect();
        if !unguarded.is_empty() {
            let trail: Vec<String> = api
                .seen()
                .iter()
                .map(|s| format!("{} {}", s.method, s.path))
                .collect();
            failures.push(format!(
                "{name}: start answered {:?} and left Pod(s) {unguarded:?} stored without a \
                 NetworkPolicy; requests: {trail:#?}",
                error.code()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// --- 2. the argv and environment reach the container as requested ------------------------------

/// The kubelet's expansion of `command`, `args` and `env[].value` (the Pod API's documented
/// rule: `$(NAME)` becomes the variable's value when it is defined, `$$` becomes `$`, anything
/// else is kept).
fn kubelet_expand(input: &str, env: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(index) = rest.find('$') {
        out.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            out.push('$');
            rest = tail;
        } else if let Some(inner) = after.strip_prefix('(')
            && let Some(close) = inner.find(')')
        {
            let name = &inner[..close];
            if let Some(value) = env.get(name) {
                out.push_str(value);
            } else {
                out.push_str("$(");
                out.push_str(name);
                out.push(')');
            }
            rest = &inner[close + 1..];
        } else {
            out.push('$');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Design 28: container `command` and `args` are "the argv, split at its first element; no
/// shell", and `env` is "the command's environment". The Pod API expands `$(VAR)` and `$$` in
/// all three, so a rendered argv or value holding `$` runs as something other than what was
/// requested, and the `run_started` record names a program the container did not run.
#[test]
fn the_container_runs_the_requested_argv_and_environment_after_kubelet_expansion() {
    let settings = PodSettings {
        namespace: NAMESPACE.to_owned(),
        run_as_user: 65_532,
        // story:pod-adapter, adversary decision 7: without a declared pods PID limit no profile
        // renders; security F4: the settings state the ephemeral storage bound.
        pod_pids_limit: Some(1024),
        ephemeral_storage_bytes: pod::POD_DEFAULT_EPHEMERAL_STORAGE_BYTES,
    };
    let requested = command(
        &["$(PROG)", "-c", "echo cost: $$5 $(LANG)"],
        &[("LANG", "C.UTF-8"), ("PROG", "/bin/sh"), ("PRICE", "$$5")],
    );
    let rendered = pod::render_manifests(
        &settings,
        &admitted_profile(),
        &requested,
        "substrate-run-x",
    )
    .expect("renders");
    let container = &rendered["items"][1]["spec"]["containers"][0];
    let mut env = BTreeMap::new();
    let mut ran_env = BTreeMap::new();
    for entry in container["env"].as_array().expect("env") {
        let name = entry["name"].as_str().expect("name").to_owned();
        let value = kubelet_expand(entry["value"].as_str().expect("value"), &env);
        env.insert(name.clone(), value.clone());
        ran_env.insert(name, value);
    }
    let mut ran: Vec<String> = Vec::new();
    for part in container["command"]
        .as_array()
        .expect("command")
        .iter()
        .chain(container["args"].as_array().expect("args"))
    {
        ran.push(kubelet_expand(part.as_str().expect("string"), &env));
    }
    ran_env.remove("SUBSTRATE_CONFINEMENT_LOCAL");
    assert_eq!(
        (ran, ran_env),
        (requested.argv.clone(), requested.env.clone()),
        "the container would run a different argv or environment than the one requested"
    );
}

// --- 3. TLS to the API server is verified ------------------------------------------------------

#[test]
fn the_api_server_certificate_is_verified_against_the_kubeconfig_trust_roots_and_name() {
    let other = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).expect("other");
    let api = FakeApi::start(&["localhost"], storing(|| status(201, ""), "Running"));
    let cases: Vec<(&str, PathBuf)> = vec![
        (
            "another CA",
            api.kubeconfig(None, &|config| {
                config["clusters"][0]["cluster"]["certificate-authority-data"] =
                    json!(b64(other.cert.pem()));
                config["clusters"][0]["cluster"]["tls-server-name"] = json!("localhost");
            }),
        ),
        // The certificate names only `localhost`; the server is addressed by IP.
        (
            "a name the certificate does not carry",
            api.kubeconfig(None, &|_| {}),
        ),
        (
            "a tls-server-name the certificate does not carry",
            api.kubeconfig(None, &|config| {
                config["clusters"][0]["cluster"]["tls-server-name"] = json!("kubernetes.invalid");
            }),
        ),
    ];
    for (name, path) in cases {
        let error = open(path).err().unwrap_or_else(|| panic!("{name}: opened"));
        assert_eq!(
            error.code(),
            "confinement.adapter-unserved",
            "{name}: {error}"
        );
    }
    assert!(
        api.seen().is_empty(),
        "an unverified server received a request: {:?}",
        api.seen()
    );
    // The control: the right name verifies.
    let good = api.kubeconfig(None, &|config| {
        config["clusters"][0]["cluster"]["tls-server-name"] = json!("localhost");
    });
    open(good).unwrap_or_else(|error| panic!("the verified server does not open: {error}"));
    let seen = api.seen();
    // story:pod-adapter, adversary decision 3: open's second request checks `pods/log` access;
    // story:pod-unknown-cleanup-outlives-adapter: its third lists policies for the open sweep.
    assert_eq!(
        seen.len(),
        3,
        "story:pod-unknown-cleanup-outlives-adapter: two probes and the sweep's list: {seen:?}"
    );
    for request in &seen {
        assert_eq!(
            request.authorization.as_deref(),
            Some(&*format!("Bearer {TOKEN}"))
        );
    }
}

// --- 4. kubeconfig members that change who the adapter is are refused by name -----------------

#[test]
fn every_refused_kubeconfig_member_is_refused_by_its_name() {
    let api = FakeApi::start(&["127.0.0.1"], storing(|| status(201, ""), "Running"));
    let cluster_members: [(&str, Value); 3] = [
        ("insecure-skip-tls-verify", json!(true)),
        ("proxy-url", json!("http://127.0.0.1:3128")),
        ("certificate-authority", json!("/etc/kubernetes/ca.crt")),
    ];
    let user_members: [(&str, Value); 11] = [
        (
            "exec",
            json!({"command": "/bin/true", "apiVersion": "client.authentication.k8s.io/v1"}),
        ),
        ("auth-provider", json!({"name": "oidc", "config": {}})),
        ("username", json!("admin")),
        ("password", json!("pw")),
        ("tokenFile", json!("/var/run/secrets/token")),
        (
            "client-certificate",
            json!("/etc/kubernetes/pki/client.crt"),
        ),
        ("client-key", json!("/etc/kubernetes/pki/client.key")),
        ("as", json!("system:admin")),
        ("as-uid", json!("0")),
        ("as-groups", json!(["system:masters"])),
        ("as-user-extra", json!({"scopes": ["x"]})),
    ];
    let mut failures = Vec::new();
    for (list, members) in [
        ("clusters", &cluster_members[..]),
        ("users", &user_members[..]),
    ] {
        for (member, value) in members {
            let inner = if list == "clusters" {
                "cluster"
            } else {
                "user"
            };
            let path = api.kubeconfig(None, &|config| {
                config[list][0][inner][*member] = value.clone();
            });
            match open(path) {
                Ok(_) => failures.push(format!("{member}: opened")),
                Err(error) => {
                    let text = error.to_string();
                    if error.code() != "confinement.adapter-unserved" || !text.contains(member) {
                        failures.push(format!("{member}: {} {text}", error.code()));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(api.seen().is_empty(), "{:?}", api.seen());
}

// --- 5. credentials never appear in an error or Debug output -----------------------------------

#[test]
fn credentials_never_appear_in_an_error_or_its_debug_form() {
    let client = rcgen::generate_simple_self_signed(vec!["client".to_owned()]).expect("client");
    let key_pem = client.signing_key.serialize_pem();
    let key_body: String = key_pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let refusing: Handler =
        Arc::new(|_: &Seen, _: &mut BTreeMap<String, Value>| status(401, "Unauthorized"));
    let api = FakeApi::start(&["127.0.0.1"], refusing);
    let with_key = |config: &mut Value| {
        config["users"][0]["user"]["client-certificate-data"] = json!(b64(client.cert.pem()));
        config["users"][0]["user"]["client-key-data"] = json!(b64(&key_pem));
    };
    let cases: Vec<(&str, PathBuf)> = vec![
        ("a 401", api.kubeconfig(None, &with_key)),
        (
            "a TLS failure",
            api.kubeconfig(None, &|config| {
                with_key(config);
                config["clusters"][0]["cluster"]["tls-server-name"] = json!("nope.invalid");
            }),
        ),
        (
            "a key without its certificate",
            api.kubeconfig(None, &|config| {
                config["users"][0]["user"]["client-key-data"] = json!(b64(&key_pem));
            }),
        ),
        (
            "a token that is not a header value",
            api.kubeconfig(None, &|config| {
                config["users"][0]["user"]["token"] = json!(format!("{TOKEN}\n"));
            }),
        ),
        (
            "an extension with an unknown member holding a token",
            api.kubeconfig(Some(json!({"token": TOKEN})), &|_| {}),
        ),
    ];
    for (name, path) in cases {
        let error = open(path).err().unwrap_or_else(|| panic!("{name}: opened"));
        for form in [error.to_string(), format!("{error:?}")] {
            let own = form;
            assert!(!own.contains(TOKEN), "{name}: the token is in {own}");
            assert!(
                !own.contains(&key_body[key_body.len() - 40..]),
                "{name}: the key is in {own}"
            );
            assert!(
                !own.contains(&b64(&key_pem)[100..140]),
                "{name}: the key is in {own}"
            );
        }
    }
}

// --- 6. one run's objects, deleted by name, in order, and only by the adapter that owns them ---

#[tokio::test(flavor = "multi_thread")]
async fn a_run_creates_the_rendered_objects_and_deletes_only_its_own_by_name_pod_first() {
    let api = FakeApi::start(&["127.0.0.1"], storing(|| status(201, ""), "Succeeded"));
    // story:pod-adapter, adversary decision 7: without a declared pods PID limit no profile is
    // admitted, so the cluster declares one.
    let limit = || Some(json!({"pod_pids_limit": 1024}));
    let adapter = open(api.kubeconfig(limit(), &|_| {})).expect("opens");
    let other = open(api.kubeconfig(limit(), &|_| {})).expect("a second adapter opens");
    let profile = ConfinementProfile {
        io_bounds: Some(IoBounds { output_bytes: 64 }),
        ..admitted_profile()
    };
    let admitted = adapter.admit(&profile).expect("admitted");
    let requested = command(&["/bin/echo", "hi"], &[("LANG", "C")]);
    let run = adapter.start(&admitted, &requested).await;
    // `status(201, "")` answers the Pod create with a Status object, which the adapter accepts.
    let run = run.expect("started");
    assert_eq!(
        other
            .observe(&run)
            .await
            .expect_err("another adapter sees the run")
            .code(),
        "resource.not-found"
    );
    assert!(
        other
            .signal(&run, substrate_wire::Signal::Kill)
            .await
            .is_err(),
        "another adapter signalled the run"
    );
    let ended = tokio::time::timeout(Duration::from_mins(1), adapter.wait(&run))
        .await
        .expect("ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    assert_eq!(ended.adapter, AdapterName::Pod);
    let seen = api.seen();
    for request in &seen {
        assert_eq!(
            request.authorization.as_deref(),
            Some(&*format!("Bearer {TOKEN}")),
            "{request:?}"
        );
        assert!(
            request
                .path
                .starts_with(&format!("/api/v1/namespaces/{NAMESPACE}/"))
                || request.path.starts_with(&format!(
                    "/apis/networking.k8s.io/v1/namespaces/{NAMESPACE}/"
                )),
            "a request outside the operator's namespace: {request:?}"
        );
    }
    let posts: Vec<&Seen> = seen.iter().filter(|s| s.method == "POST").collect();
    assert_eq!(posts.len(), 2, "{seen:?}");
    assert!(posts[0].path.ends_with("/networkpolicies"), "{seen:?}");
    assert!(posts[1].path.ends_with("/pods"), "{seen:?}");
    let sent_policy: Value = serde_json::from_str(&posts[0].body).expect("policy");
    let sent_pod: Value = serde_json::from_str(&posts[1].body).expect("pod");
    let settings = adapter.settings().clone();
    let rendered =
        pod::render_manifests(&settings, &profile, &requested, run.as_str()).expect("renders");
    assert_eq!(
        sent_policy, rendered["items"][0],
        "the policy sent is not the rendered one"
    );
    assert_eq!(
        sent_pod, rendered["items"][1],
        "the Pod sent is not the rendered one"
    );
    let deletes: Vec<&Seen> = seen.iter().filter(|s| s.method == "DELETE").collect();
    assert!(!deletes.is_empty(), "{seen:?}");
    for delete in &deletes {
        assert!(!delete.path.contains('?'), "a selector delete: {delete:?}");
        assert!(
            delete.path.ends_with(&format!("/pods/{}", run.as_str()))
                || delete
                    .path
                    .ends_with(&format!("/networkpolicies/{}", run.as_str())),
            "a delete of something other than this run's objects: {delete:?}"
        );
    }
    let first_policy_delete = seen
        .iter()
        .position(|s| s.method == "DELETE" && s.path.contains("/networkpolicies/"))
        .expect("the policy is deleted");
    let pod_gone = seen[..first_policy_delete]
        .iter()
        .any(|s| s.path.ends_with(&format!("/pods/{}", run.as_str())) && s.method == "DELETE");
    assert!(pod_gone, "the policy was deleted before the Pod: {seen:?}");
    assert!(
        api.objects.lock().is_empty(),
        "objects left: {:?}",
        api.objects.lock()
    );
    assert_eq!(
        adapter.output(&run).await.expect("output").stdout,
        b"\"hi\"".to_vec()
    );
}

// --- 7. the posture is fixed and no request value lands outside its mapped field ----------------

fn leaves<'a>(value: &'a Value, pointer: String, out: &mut Vec<(String, &'a str)>) {
    match value {
        Value::String(text) => out.push((pointer, text)),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                leaves(item, format!("{pointer}/{index}"), out);
            }
        }
        Value::Object(members) => {
            for (name, member) in members {
                leaves(member, format!("{pointer}/{name}"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn the_pod_posture_is_fixed_and_request_values_land_only_in_their_mapped_fields() {
    let settings = PodSettings {
        namespace: NAMESPACE.to_owned(),
        run_as_user: 4242,
        pod_pids_limit: Some(64),
        // story:pod-adapter, security F4: the settings state the ephemeral storage bound.
        ephemeral_storage_bytes: 7 << 20,
    };
    let marker = "MARKERZ";
    let profiles = [
        admitted_profile(),
        ConfinementProfile {
            workdir: Some(format!("{marker}/deep")),
            process_bounds: Some(ProcessBounds {
                timeout_ms: 7_001,
                processes: 64,
                memory_bytes: 3 << 20,
                cpu_millis: 8_000,
            }),
            io_bounds: Some(IoBounds { output_bytes: 1 }),
            local_reason: Some(marker.to_owned()),
            ..admitted_profile()
        },
    ];
    let requested = command(
        &[
            &format!("/{marker}"),
            marker,
            "--name=x",
            "app.kubernetes.io/name",
        ],
        &[("ZZ", marker), ("NODE_NAME", "evil"), ("HOSTNAME", marker)],
    );
    for profile in profiles {
        let rendered = pod::render_manifests(&settings, &profile, &requested, "substrate-run-y")
            .expect("renders");
        let pod = &rendered["items"][1];
        let spec = &pod["spec"];
        for (field, want) in [
            ("automountServiceAccountToken", json!(false)),
            ("hostNetwork", json!(false)),
            ("hostPID", json!(false)),
            ("hostIPC", json!(false)),
            ("enableServiceLinks", json!(false)),
            ("restartPolicy", json!("Never")),
        ] {
            assert_eq!(spec[field], want, "{field}");
        }
        for absent in [
            "serviceAccountName",
            "serviceAccount",
            "nodeName",
            "nodeSelector",
            "affinity",
            "tolerations",
            "runtimeClassName",
            "hostUsers",
            "shareProcessNamespace",
            "initContainers",
            "ephemeralContainers",
            "imagePullSecrets",
            "hostAliases",
            "priorityClassName",
        ] {
            assert!(spec[absent].is_null(), "{absent} is set: {}", spec[absent]);
        }
        // story:pod-adapter, security F4: the emptyDir has a sizeLimit, the operator's value.
        assert_eq!(
            spec["volumes"],
            json!([{"name": "workspace", "emptyDir": {"sizeLimit": "7340032"}}])
        );
        assert_eq!(
            spec["containers"][0]["resources"]["limits"]["ephemeral-storage"],
            json!("7340032")
        );
        assert_eq!(spec["containers"].as_array().expect("containers").len(), 1);
        let container = &spec["containers"][0];
        let context = &container["securityContext"];
        assert_eq!(context["runAsNonRoot"], json!(true));
        assert_eq!(context["runAsUser"], json!(4242));
        assert_eq!(context["runAsGroup"], json!(4242));
        assert_eq!(context["allowPrivilegeEscalation"], json!(false));
        assert_eq!(context["privileged"], json!(false));
        assert_eq!(context["readOnlyRootFilesystem"], json!(true));
        assert_eq!(context["capabilities"], json!({"drop": ["ALL"]}));
        assert_eq!(context["seccompProfile"], json!({"type": "RuntimeDefault"}));
        assert_eq!(spec["securityContext"]["runAsNonRoot"], json!(true));
        assert_eq!(
            spec["securityContext"]["seccompProfile"],
            json!({"type": "RuntimeDefault"})
        );
        assert_eq!(
            container["resources"]["requests"], container["resources"]["limits"],
            "requests differ from limits"
        );
        let bounds = profile.process_bounds.unwrap_or(ProcessBounds {
            timeout_ms: 600_000,
            processes: 256,
            memory_bytes: 1 << 30,
            cpu_millis: 600_000,
        });
        assert_eq!(
            container["resources"]["limits"]["memory"],
            json!(bounds.memory_bytes.to_string())
        );
        assert_eq!(
            spec["activeDeadlineSeconds"],
            json!(bounds.timeout_ms.div_ceil(1000))
        );
        assert_eq!(pod["metadata"]["namespace"], json!(NAMESPACE));
        assert_eq!(
            rendered["items"][0]["metadata"]["namespace"],
            json!(NAMESPACE)
        );
        assert_eq!(
            rendered["items"][0]["spec"],
            json!({
                "podSelector": {"matchLabels": {"b10x-substrate/run": "substrate-run-y"}},
                "policyTypes": ["Ingress", "Egress"],
            })
        );
        let mut found = Vec::new();
        leaves(&rendered, String::new(), &mut found);
        for (pointer, text) in found {
            if !text.contains(marker) {
                continue;
            }
            let allowed = pointer == "/items/1/spec/containers/0/command/0"
                || pointer.starts_with("/items/1/spec/containers/0/args/")
                || pointer == "/items/1/spec/containers/0/workingDir"
                || (pointer.starts_with("/items/1/spec/containers/0/env/")
                    && pointer.ends_with("/value"));
            assert!(allowed, "a request value reached {pointer}: {text}");
        }
    }
}
