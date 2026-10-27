//! Adversarial cases for the `pod` adapter (`story:pod-adapter`, design 28).
//!
//! Each case states what design 28, the story's acceptance or the adapter's own documentation
//! promises, and drives the adapter against a fake TLS API server on loopback (or the renderer
//! alone) to check it.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use parking_lot::Mutex;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};
use substrate_host::confinement::{
    Command, ConfinementAdapter, ConfinementProfile, RunState,
    pod::{self, PodAdapter, PodRequest, PodSettings},
};

const IMAGE: &str = "docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e";
const NAMESPACE: &str = "substrate-fake";

// --- Kubernetes' own `$(VAR)` expansion, as the kubelet applies it ----------------------------
//
// `k8s.io/kubernetes/third_party/forked/golang/expansion/expand.go`: the kubelet expands
// `$(NAME)` in every env value (against the variables defined before it) and in `command` and
// `args` (against the container's whole environment); `$$` becomes `$`; an undefined reference
// is left as written.

fn expand(input: &str, defined: &BTreeMap<String, String>) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut checkpoint = 0;
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && cursor + 1 < bytes.len() {
            out.extend_from_slice(&bytes[checkpoint..cursor]);
            let rest = &bytes[cursor + 1..];
            let advance = match rest[0] {
                b'$' => {
                    out.push(b'$');
                    1
                }
                b'(' => {
                    if let Some(close) = rest.iter().position(|byte| *byte == b')') {
                        let name = std::str::from_utf8(&rest[1..close]).expect("utf-8");
                        if let Some(value) = defined.get(name) {
                            out.extend_from_slice(value.as_bytes());
                        } else {
                            out.extend_from_slice(format!("$({name})").as_bytes());
                        }
                        close + 1
                    } else {
                        out.extend_from_slice(b"$(");
                        1
                    }
                }
                other => {
                    out.push(b'$');
                    out.push(other);
                    1
                }
            };
            cursor += advance;
            checkpoint = cursor + 1;
        }
        cursor += 1;
    }
    out.extend_from_slice(&bytes[checkpoint.min(bytes.len())..]);
    String::from_utf8(out).expect("utf-8")
}

/// The argv and environment the kubelet hands the container for a rendered Pod.
fn what_the_container_receives(pod: &Value) -> (Vec<String>, BTreeMap<String, String>) {
    let container = &pod["spec"]["containers"][0];
    let mut env = BTreeMap::new();
    for entry in container["env"].as_array().expect("env") {
        let value = expand(entry["value"].as_str().expect("value"), &env);
        env.insert(entry["name"].as_str().expect("name").to_owned(), value);
    }
    let mut argv: Vec<String> = Vec::new();
    for part in container["command"]
        .as_array()
        .expect("command")
        .iter()
        .chain(container["args"].as_array().expect("args"))
    {
        argv.push(expand(part.as_str().expect("text"), &env));
    }
    (argv, env)
}

/// Design 28: container `command` and `args` are "the argv, split at its first element; no
/// shell", and `env` is "the command's environment plus `SUBSTRATE_CONFINEMENT_LOCAL=refuse`".
/// `render` reports the argv itself as what the container runs. The kubelet rewrites `$$` and
/// `$(NAME)` in all three, so the adapter must escape them for the argv to arrive unchanged.
#[test]
fn adversary_the_container_receives_the_argv_and_environment_unchanged() {
    let settings = PodSettings {
        namespace: NAMESPACE.to_owned(),
        run_as_user: 65_532,
        pod_pids_limit: Some(1024),
        // story:pod-adapter, security F4: the operator's settings state the ephemeral storage bound.
        ephemeral_storage_bytes: pod::POD_DEFAULT_EPHEMERAL_STORAGE_BYTES,
    };
    let profile = ConfinementProfile {
        image: Some(IMAGE.to_owned()),
        ..ConfinementProfile::default()
    };
    let command = Command {
        argv: vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "echo $$ > pid; echo $(LANG)".to_owned(),
        ],
        env: BTreeMap::from([
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("PRICE".to_owned(), "5$$ $(LANG)".to_owned()),
        ]),
    };
    let rendered = pod::render_manifests(&settings, &profile, &command, "substrate-run-adv")
        .expect("the profile renders");
    let pod = &rendered["items"][1];
    assert_eq!(pod["kind"], "Pod");
    let (argv, env) = what_the_container_receives(pod);
    assert_eq!(
        argv, command.argv,
        "the kubelet's $(VAR) expansion rewrites the argv the adapter rendered"
    );
    for (name, value) in &command.env {
        assert_eq!(
            env.get(name),
            Some(value),
            "the kubelet's $(VAR) expansion rewrites env {name}"
        );
    }
}

// --- a fake API server ------------------------------------------------------------------------

#[derive(Clone)]
struct Behaviour {
    /// Store the Pod a create names, then close the connection without answering: the create
    /// committed and its answer was lost (a timeout, a dropped connection).
    lose_pod_create_answer: bool,
    /// After the Pod is created, accept every connection and never answer it.
    blackhole_after_pod_create: bool,
    /// The log endpoint's answer.
    log: (u16, String),
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            lose_pod_create_answer: false,
            blackhole_after_pod_create: false,
            log: (200, "hello\n".to_owned()),
        }
    }
}

struct Fake {
    kubeconfig: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    objects: Arc<Mutex<BTreeMap<String, Value>>>,
    stop: Arc<AtomicBool>,
    _directory: tempfile::TempDir,
}

struct Shared {
    behaviour: Behaviour,
    requests: Arc<Mutex<Vec<String>>>,
    objects: Arc<Mutex<BTreeMap<String, Value>>>,
    blackhole: AtomicBool,
    held: Mutex<Vec<TcpStream>>,
}

impl Fake {
    fn start(behaviour: Behaviour) -> Self {
        let directory = tempfile::tempdir().expect("directory");
        let identity =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).expect("identity");
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
                .expect("TLS config"),
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        listener.set_nonblocking(true).expect("nonblocking");
        let address = listener.local_addr().expect("address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let objects = Arc::new(Mutex::new(BTreeMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            behaviour,
            requests: Arc::clone(&requests),
            objects: Arc::clone(&objects),
            blackhole: AtomicBool::new(false),
            held: Mutex::new(Vec::new()),
        });
        let stopped = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                if shared.blackhole.load(Ordering::SeqCst) {
                    shared.held.lock().push(socket);
                    continue;
                }
                let (shared, config) = (Arc::clone(&shared), Arc::clone(&config));
                std::thread::spawn(move || {
                    socket.set_nonblocking(false).expect("blocking");
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .expect("timeout");
                    let tls = ServerConnection::new(config).expect("session");
                    let mut stream = BufReader::new(StreamOwned::new(tls, socket));
                    let _ = serve(&mut stream, &shared);
                });
            }
        });
        let config = json!({
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
        let kubeconfig = directory.path().join("kubeconfig.json");
        std::fs::write(&kubeconfig, serde_json::to_vec(&config).expect("json")).expect("write");
        Self {
            kubeconfig,
            requests,
            objects,
            stop,
            _directory: directory,
        }
    }

    fn open(&self) -> Arc<PodAdapter> {
        pod::open_pod(PodRequest {
            cluster: Some(self.kubeconfig.clone()),
        })
        .expect("pod opens against the fake cluster")
    }

    fn objects(&self) -> Vec<String> {
        self.objects.lock().keys().cloned().collect()
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().clone()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn pod_status() -> Value {
    json!({
        "phase": "Succeeded",
        "containerStatuses": [{
            "name": "run",
            "state": {"terminated": {"exitCode": 0, "reason": "Completed"}},
        }],
    })
}

#[allow(clippy::too_many_lines)] // One fake API server keeps every route it answers adjacent.
fn serve(
    stream: &mut BufReader<StreamOwned<ServerConnection, TcpStream>>,
    shared: &Shared,
) -> std::io::Result<()> {
    let mut line = String::new();
    stream.read_line(&mut line)?;
    let request = line.trim_end().to_owned();
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
    shared.requests.lock().push(request.clone());
    let mut parts = request.split(' ');
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let pods = format!("/api/v1/namespaces/{NAMESPACE}/pods");
    let policies = format!("/apis/networking.k8s.io/v1/namespaces/{NAMESPACE}/networkpolicies");
    let key =
        |collection: &str, rest: &str| format!("{collection}/{}", rest.trim_start_matches('/'));
    let (status, payload): (u16, String) = if method == "GET" && path == pods && query == "limit=1"
    {
        (
            200,
            r#"{"kind":"PodList","apiVersion":"v1","items":[]}"#.to_owned(),
        )
    } else if method == "POST" && (path == pods || path == policies) {
        let object: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let name = object["metadata"]["name"]
            .as_str()
            .unwrap_or("?")
            .to_owned();
        let collection = if path == pods {
            "pods"
        } else {
            "networkpolicies"
        };
        shared
            .objects
            .lock()
            .insert(key(collection, &name), object.clone());
        if collection == "pods" && shared.behaviour.lose_pod_create_answer {
            // Committed; the answer is lost.
            let _ = stream.get_mut().sock.shutdown(std::net::Shutdown::Both);
            return Ok(());
        }
        if collection == "pods" && shared.behaviour.blackhole_after_pod_create {
            shared.blackhole.store(true, Ordering::SeqCst);
        }
        (201, object.to_string())
    } else if let Some(rest) = path.strip_prefix(&pods) {
        let rest = rest.trim_start_matches('/');
        if let Some(name) = rest.strip_suffix("/log") {
            // story:pod-adapter, adversary decision 3: `open` reads the log of a Pod no run is
            // named and refuses a credential the API answers 403. An authorized API server answers
            // 404 for a Pod it does not hold, so the run's own log keeps the configured answer:
            // here a 403 models a log that became unreadable after open.
            if shared.objects.lock().contains_key(&key("pods", name)) {
                shared.behaviour.log.clone()
            } else {
                (404, r#"{"kind":"Status","code":404}"#.to_owned())
            }
        } else {
            let id = key("pods", rest);
            match method.as_str() {
                "GET" => match shared.objects.lock().get(&id) {
                    Some(object) => {
                        let mut object = object.clone();
                        object["status"] = pod_status();
                        (200, object.to_string())
                    }
                    None => (404, r#"{"kind":"Status","code":404}"#.to_owned()),
                },
                "DELETE" => match shared.objects.lock().remove(&id) {
                    Some(object) => (200, object.to_string()),
                    None => (404, r#"{"kind":"Status","code":404}"#.to_owned()),
                },
                _ => (405, "{}".to_owned()),
            }
        }
    } else if let Some(rest) = path.strip_prefix(&policies) {
        let id = key("networkpolicies", rest);
        match method.as_str() {
            "GET" => match shared.objects.lock().get(&id) {
                Some(object) => (200, object.to_string()),
                None => (404, r#"{"kind":"Status","code":404}"#.to_owned()),
            },
            "DELETE" => match shared.objects.lock().remove(&id) {
                Some(object) => (200, object.to_string()),
                None => (404, r#"{"kind":"Status","code":404}"#.to_owned()),
            },
            _ => (405, "{}".to_owned()),
        }
    } else {
        (
            500,
            r#"{"kind":"Status","message":"unexpected"}"#.to_owned(),
        )
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

fn admitted_profile() -> ConfinementProfile {
    ConfinementProfile {
        image: Some(IMAGE.to_owned()),
        ..ConfinementProfile::default()
    }
}

fn echo() -> Command {
    Command {
        argv: vec!["/bin/echo".to_owned(), "hello".to_owned()],
        env: BTreeMap::new(),
    }
}

/// Design 28: "a Pod refused by the API deletes the policy", and the story: no run object is left
/// after an API error. A create whose answer is lost (the API server committed it; the client
/// timed out or the connection dropped) leaves a Pod the adapter does not track and never deletes,
/// and the adapter deletes the default-deny policy that was its only network fence.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_a_pod_whose_create_answer_was_lost_is_not_left_behind() {
    let fake = Fake::start(Behaviour {
        lose_pod_create_answer: true,
        ..Behaviour::default()
    });
    let adapter = fake.open();
    let admitted = adapter.admit(&admitted_profile()).expect("admitted");
    let error = adapter
        .start(&admitted, &echo())
        .await
        .expect_err("the create's answer was lost");
    assert_eq!(error.code(), "confinement.pod-api-failed", "{error}");
    let left = fake.objects();
    assert!(
        left.is_empty(),
        "after a failed start the cluster still holds {left:?}; requests: {:#?}",
        fake.requests()
    );
}

/// The adapter's own unread log must not read as a complete, untruncated, empty output: the open
/// probe checks `list pods` and not `get pods/log`, so a Role without `pods/log` opens and then
/// answers every run's `output_read` with nothing, as if the program printed nothing.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_an_unreadable_log_is_not_reported_as_an_empty_output() {
    let fake = Fake::start(Behaviour {
        log: (
            403,
            r#"{"kind":"Status","message":"pods \"x\" is forbidden: cannot get resource \"pods/log\""}"#
                .to_owned(),
        ),
        ..Behaviour::default()
    });
    let adapter = fake.open();
    let admitted = adapter.admit(&admitted_profile()).expect("admitted");
    let run = adapter.start(&admitted, &echo()).await.expect("started");
    let ended = tokio::time::timeout(Duration::from_mins(1), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    let output = adapter.output(&run).await;
    assert!(
        output.is_err(),
        "a log the API refused (403) is reported as a complete output: {output:?}"
    );
}

// --- on a real cluster (the pod lane's variables) ---------------------------------------------

fn lane(variable: &str) -> Option<PathBuf> {
    if std::env::var_os("SUBSTRATE_POD_LANE").as_deref() != Some("1".as_ref()) {
        println!("adversary pod lane: ABSENT; 0 executed");
        return None;
    }
    let path = std::env::var_os(variable).map(PathBuf::from);
    if path.is_none() {
        println!("adversary pod lane: {variable} unset; ABSENT; 0 executed");
    }
    path
}

/// The same promise as the portable case, observed on a cluster: `/bin/echo` (no shell) prints
/// its arguments as the client passed them.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_lane_echo_prints_its_argv_unchanged() {
    let Some(kubeconfig) = lane("SUBSTRATE_POD_KUBECONFIG") else {
        return;
    };
    let adapter = pod::open_pod(PodRequest {
        cluster: Some(kubeconfig),
    })
    .expect("opens");
    let admitted = adapter.admit(&admitted_profile()).expect("admitted");
    let command = Command {
        argv: vec![
            "/bin/echo".to_owned(),
            "a$$b".to_owned(),
            "$(LANG)".to_owned(),
        ],
        env: BTreeMap::from([("LANG".to_owned(), "C.UTF-8".to_owned())]),
    };
    let run = adapter.start(&admitted, &command).await.expect("started");
    let ended = tokio::time::timeout(Duration::from_mins(3), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.exit.and_then(|exit| exit.code), Some(0));
    let output = adapter.output(&run).await.expect("output");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "a$$b $(LANG)\n",
        "the container did not receive the argv the client passed"
    );
}

/// The same promise as the portable log case, observed on a cluster whose `ServiceAccount` holds
/// design 28's `pods`/`networkpolicies` Role and not its `pods/log` one: `output_read` must not
/// read as a complete, empty output. (story:pod-adapter, adversary decision 3: `pod` does not
/// open there at all.)
#[tokio::test(flavor = "multi_thread")]
async fn adversary_lane_a_role_without_pods_log_does_not_read_as_empty_output() {
    let Some(kubeconfig) = lane("SUBSTRATE_POD_KUBECONFIG_NOLOG") else {
        return;
    };
    // story:pod-adapter, adversary decision 3: `open` checks `pods/log` access and is refused
    // `confinement.adapter-unserved` without it, so no run can reach an unreadable output.
    let opened = pod::open_pod(PodRequest {
        cluster: Some(kubeconfig),
    });
    let error = opened.err().expect("pod does not open without pods/log");
    assert_eq!(error.code(), "confinement.adapter-unserved", "{error}");
    assert!(error.to_string().contains("pods/log"), "{error}");
}

fn node(node: &str, args: &[&str]) -> String {
    let output = std::process::Command::new("docker")
        .args(["exec", node])
        .args(args)
        .output()
        .expect("docker runs");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// `wait` "Returns once the run has ended" (`ConfinementAdapter::wait`). `signal: Kill` deletes the
/// Pod with a grace period of 0, which the API server treats as a force deletion: the object goes
/// at once, without the kubelet confirming the container stopped. On a node whose kubelet is not
/// answering (stopped here; partitioned in production), `wait` then reports the run ended by
/// SIGKILL while its container still runs. `SUBSTRATE_POD_KIND_NODE` names the kind node
/// container; the kubelet is started again before the assertion.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_lane_a_kill_reported_ended_left_no_running_container() {
    let Some(kubeconfig) = lane("SUBSTRATE_POD_KUBECONFIG") else {
        return;
    };
    let Some(kind_node) = std::env::var("SUBSTRATE_POD_KIND_NODE").ok() else {
        println!("adversary pod lane: SUBSTRATE_POD_KIND_NODE unset; ABSENT; 0 executed");
        return;
    };
    let adapter = pod::open_pod(PodRequest {
        cluster: Some(kubeconfig),
    })
    .expect("opens");
    let admitted = adapter.admit(&admitted_profile()).expect("admitted");
    let command = Command {
        argv: vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "echo started; exec sleep 600".to_owned(),
        ],
        env: BTreeMap::new(),
    };
    let run = adapter.start(&admitted, &command).await.expect("started");
    let deadline = std::time::Instant::now() + Duration::from_mins(2);
    while !String::from_utf8_lossy(&adapter.output(&run).await.expect("output").stdout)
        .contains("started")
    {
        assert!(std::time::Instant::now() < deadline, "did not start");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let label = format!("io.kubernetes.pod.name={}", run.as_str());
    let running = || {
        node(
            &kind_node,
            &[
                "crictl", "ps", "--state", "running", "--label", &label, "-q",
            ],
        )
    };
    assert!(!running().is_empty(), "the container runs before the kill");
    node(&kind_node, &["systemctl", "stop", "kubelet"]);
    adapter
        .signal(&run, substrate_wire::Signal::Kill)
        .await
        .expect("killed");
    let ended = tokio::time::timeout(Duration::from_mins(3), adapter.wait(&run)).await;
    let still_running = running();
    node(&kind_node, &["systemctl", "start", "kubelet"]);
    let ended = ended.expect("wait returned").expect("waited");
    println!("wait reported {ended:?}");
    assert!(
        ended.state != RunState::Ended || still_running.is_empty(),
        "wait reported {:?} {:?} while container {still_running} still runs",
        ended.state,
        ended.exit
    );
}

/// `UNREACHABLE_LIMIT` (pod.rs): "How long the API may stay unreachable before a run is reported
/// `Unknown`" is two minutes. With the API server gone silent, `wait` must return within that
/// limit plus one 30 s API timeout plus a bounded cleanup; this case allows five minutes in all.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_wait_returns_when_the_api_server_goes_silent() {
    let fake = Fake::start(Behaviour {
        blackhole_after_pod_create: true,
        ..Behaviour::default()
    });
    let adapter = fake.open();
    let admitted = adapter.admit(&admitted_profile()).expect("admitted");
    let run = adapter.start(&admitted, &echo()).await.expect("started");
    let started = std::time::Instant::now();
    let waited = tokio::time::timeout(Duration::from_mins(5), adapter.wait(&run)).await;
    let observed = adapter.observe(&run).await.expect("observed");
    assert!(
        waited.is_ok(),
        "wait did not return {:?} after the API server went silent; the run still reads {:?}",
        started.elapsed(),
        observed.state
    );
}
