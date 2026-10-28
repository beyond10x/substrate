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
    pod::{self, PodAdapter, PodRequest, PodSettings, PodTiming},
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
    /// After the Pod is created and `start` has read its policy back (design 28), accept every
    /// connection and never answer it.
    blackhole_after_pod_create: bool,
    /// The log endpoint's answer.
    log: (u16, String),
    /// Answer a list of `NetworkPolicy` objects 403, as to a credential that may not list them.
    refuse_policy_list: bool,
    /// What goes wrong with the run's `NetworkPolicy` between its create and `start`'s read-back.
    policy_fault: PolicyFault,
}

#[derive(Clone, Copy, Default)]
enum PolicyFault {
    #[default]
    None,
    /// Remove the run's `NetworkPolicy` when its Pod is stored, as another adapter's sweep that
    /// read the Pod absent just before would.
    GoneWhenThePodIsStored,
    /// Answer every read of one `NetworkPolicy` by name with this status instead.
    ReadAnswers(u16),
    /// Answer a `NetworkPolicy` create without the `metadata.uid` the API server assigns.
    CreateAnswersNoUid,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            lose_pod_create_answer: false,
            blackhole_after_pod_create: false,
            log: (200, "hello\n".to_owned()),
            refuse_policy_list: false,
            policy_fault: PolicyFault::None,
        }
    }
}

/// The API server's form of an instant `age` ago: RFC 3339, whole seconds, UTC.
fn timestamp(age: Duration) -> String {
    let at = chrono::Utc::now() - chrono::Duration::from_std(age).expect("an age");
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

struct Fake {
    kubeconfig: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    objects: Arc<Mutex<BTreeMap<String, Value>>>,
    /// Each deleted Pod's last object, by name, which a watch of it from a `resourceVersion`
    /// replays as its `DELETED` event (`story:pod-force-deleted-keeps-its-policy`).
    deleted: Arc<Mutex<BTreeMap<String, Value>>>,
    stop: Arc<AtomicBool>,
    _directory: tempfile::TempDir,
}

struct Shared {
    behaviour: Behaviour,
    requests: Arc<Mutex<Vec<String>>>,
    objects: Arc<Mutex<BTreeMap<String, Value>>>,
    deleted: Arc<Mutex<BTreeMap<String, Value>>>,
    blackhole: AtomicBool,
    /// A Pod was created; the next read of a policy by name arms `blackhole`.
    pod_created: AtomicBool,
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
        let deleted = Arc::new(Mutex::new(BTreeMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            behaviour,
            requests: Arc::clone(&requests),
            objects: Arc::clone(&objects),
            deleted: Arc::clone(&deleted),
            blackhole: AtomicBool::new(false),
            pod_created: AtomicBool::new(false),
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
            deleted,
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

    /// Opens `pod` against the fake with `timing` instead of the adapter's own windows.
    fn open_timed(&self, timing: PodTiming) -> Arc<PodAdapter> {
        pod::open_pod_timed(
            PodRequest {
                cluster: Some(self.kubeconfig.clone()),
            },
            timing,
        )
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
    } else if method == "GET" && path == pods && query.starts_with("watch=") {
        // A watch of one Pod replays its deletion, when the fake deleted it; otherwise it ends
        // without an event.
        let name = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("fieldSelector=metadata.name%3D"))
            .unwrap_or_default();
        let replay = shared
            .deleted
            .lock()
            .get(name)
            .map(|object| json!({"type": "DELETED", "object": object}).to_string() + "\n");
        (200, replay.unwrap_or_default())
    } else if method == "GET" && path == policies {
        if shared.behaviour.refuse_policy_list {
            (
                403,
                r#"{"kind":"Status","message":"networkpolicies is forbidden"}"#.to_owned(),
            )
        } else {
            // Every policy in the namespace, whatever the selector asks: the adapter checks the
            // labels itself.
            let items: Vec<Value> = shared
                .objects
                .lock()
                .iter()
                .filter(|(key, _)| key.starts_with("networkpolicies/"))
                .map(|(_, object)| object.clone())
                .collect();
            (
                200,
                json!({"kind": "NetworkPolicyList", "apiVersion": "networking.k8s.io/v1",
                       "metadata": {}, "items": items})
                .to_string(),
            )
        }
    } else if method == "POST" && (path == pods || path == policies) {
        let mut object: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if object["metadata"].is_object() {
            object["metadata"]["creationTimestamp"] = json!(timestamp(Duration::ZERO));
            object["metadata"]["resourceVersion"] = json!("1");
        }
        let name = object["metadata"]["name"]
            .as_str()
            .unwrap_or("?")
            .to_owned();
        let collection = if path == pods {
            "pods"
        } else {
            "networkpolicies"
        };
        // The API server assigns every object a uid; a Pod's ownerReference names its policy's.
        if object["metadata"].is_object()
            && !(collection == "networkpolicies"
                && matches!(
                    shared.behaviour.policy_fault,
                    PolicyFault::CreateAnswersNoUid
                ))
        {
            object["metadata"]["uid"] = json!(format!("uid-{collection}-{name}"));
        }
        shared
            .objects
            .lock()
            .insert(key(collection, &name), object.clone());
        if collection == "pods"
            && matches!(
                shared.behaviour.policy_fault,
                PolicyFault::GoneWhenThePodIsStored
            )
        {
            shared.objects.lock().remove(&key("networkpolicies", &name));
        }
        if collection == "pods" && shared.behaviour.lose_pod_create_answer {
            // Committed; the answer is lost.
            let _ = stream.get_mut().sock.shutdown(std::net::Shutdown::Both);
            return Ok(());
        }
        if collection == "pods" {
            shared.pod_created.store(true, Ordering::SeqCst);
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
                    Some(object) => {
                        // The kubelet stopped the container, wrote its state, then removed the Pod.
                        let mut last = object.clone();
                        last["status"] = pod_status();
                        shared.deleted.lock().insert(rest.to_owned(), last);
                        (200, object.to_string())
                    }
                    None => (404, r#"{"kind":"Status","code":404}"#.to_owned()),
                },
                _ => (405, "{}".to_owned()),
            }
        }
    } else if let Some(rest) = path.strip_prefix(&policies) {
        let id = key("networkpolicies", rest);
        match method.as_str() {
            "GET" if let PolicyFault::ReadAnswers(status) = shared.behaviour.policy_fault => (
                status,
                r#"{"kind":"Status","message":"the policy read failed"}"#.to_owned(),
            ),
            "GET" => {
                // `start`'s read-back of the policy once the Pod is stored is answered; then the
                // server goes silent.
                if shared.behaviour.blackhole_after_pod_create
                    && shared.pod_created.load(Ordering::SeqCst)
                {
                    shared.blackhole.store(true, Ordering::SeqCst);
                }
                match shared.objects.lock().get(&id) {
                    Some(object) => (200, object.to_string()),
                    None => (404, r#"{"kind":"Status","code":404}"#.to_owned()),
                }
            }
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
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\ndate: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len(),
        chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT"),
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

/// `story:pod-unknown-cleanup-outlives-adapter`, security finding S1 (decision D2): once the Pod
/// is stored, `start` reads the run's `NetworkPolicy` back. A policy another adapter's sweep removed
/// meanwhile (404), a read that fails, or a policy create answered without the uid the Pod's
/// ownerReference names (D3) each remove the Pod and refuse the start `confinement.pod-api-failed`,
/// so no started run's Pod is without its default-deny policy.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_a_start_whose_policy_is_not_read_back_removes_its_pod_and_is_refused() {
    let cases = [
        (
            "the policy gone when the Pod was stored",
            PolicyFault::GoneWhenThePodIsStored,
        ),
        (
            "the policy read answered 500",
            PolicyFault::ReadAnswers(500),
        ),
        (
            "the policy create answered no uid",
            PolicyFault::CreateAnswersNoUid,
        ),
    ];
    for (case, policy_fault) in cases {
        let behaviour = Behaviour {
            policy_fault,
            ..Behaviour::default()
        };
        let fake = Fake::start(behaviour);
        let adapter = fake.open();
        let admitted = adapter.admit(&admitted_profile()).expect("admitted");
        let started = adapter.start(&admitted, &echo()).await;
        assert_eq!(
            started
                .as_ref()
                .err()
                .map(substrate_host::confinement::ConfinementError::code),
            Some("confinement.pod-api-failed"),
            "{case}: start answered {started:?}; requests: {:#?}",
            fake.requests()
        );
        let left = fake.objects();
        assert!(
            left.is_empty(),
            "{case}: the refused start left {left:?}; requests: {:#?}",
            fake.requests()
        );
    }
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
    assert_eq!(
        ended.exit.and_then(|exit| exit.code),
        Some(0),
        "wait reported {ended:?}"
    );
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

/// How long the restarted kubelet may take to report its node `Ready` and untainted.
const NODE_RETURN_LIMIT: Duration = Duration::from_mins(2);

/// The kind node's kubelet, stopped; dropping it starts the kubelet again and waits for the node,
/// so a case that fails while the kubelet is away does not hand the next case a stopped node.
struct KubeletStopped(Option<String>);

impl KubeletStopped {
    fn stop(kind_node: &str) -> Self {
        node(kind_node, &["systemctl", "stop", "kubelet"]);
        Self(Some(kind_node.to_owned()))
    }

    /// Starts the kubelet and requires the node back: `Ready` and carrying none of the node
    /// lifecycle taints (`node.kubernetes.io/unreachable`, `not-ready`) its absence added. Nothing
    /// orders the run's objects going before the node lifecycle controller lifts those taints, so
    /// the next case's start is ordered after this, not after the cleanup wait below.
    fn restore(mut self) {
        let kind_node = self.0.take().expect("stopped once");
        let state = Self::start_and_wait(&kind_node);
        assert!(
            state.is_ok(),
            "the node did not return within {NODE_RETURN_LIMIT:?}: {state:?}"
        );
    }

    fn start_and_wait(kind_node: &str) -> Result<(), String> {
        node(kind_node, &["systemctl", "start", "kubelet"]);
        let deadline = std::time::Instant::now() + NODE_RETURN_LIMIT;
        loop {
            // The lane's ServiceAccount may not read Nodes; the kind node's own admin.conf may.
            let output = std::process::Command::new("docker")
                .args(["exec", kind_node, "kubectl", "--kubeconfig"])
                .args(["/etc/kubernetes/admin.conf", "get", "nodes", "--output"])
                .arg(
                    "jsonpath={range .items[*]}{.status.conditions[?(@.type==\"Ready\")].status} \
                     {.spec.taints[*].key}{\"\\n\"}{end}",
                )
                .output()
                .expect("docker runs");
            let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let back = output.status.success()
                && !state.is_empty()
                && state
                    .lines()
                    .all(|line| line.starts_with("True") && !line.contains("node.kubernetes.io/"));
            if back {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "{state} {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

impl Drop for KubeletStopped {
    fn drop(&mut self) {
        if let Some(kind_node) = self.0.take()
            && let Err(state) = Self::start_and_wait(&kind_node)
        {
            println!("adversary pod lane: the node did not return: {state}");
        }
    }
}

/// `wait` "Returns once the run has ended" (`ConfinementAdapter::wait`). `signal: Kill` deletes the
/// Pod with a grace period of 0, which the API server treats as a force deletion: the object goes
/// at once, without the kubelet confirming the container stopped. On a node whose kubelet is not
/// answering (stopped here; partitioned in production), `wait` then reports the run ended by
/// SIGKILL while its container still runs. `SUBSTRATE_POD_KIND_NODE` names the kind node
/// container; the kubelet is started again, and the node observed `Ready` and untainted, before
/// the assertion, so the case after this one starts on a node that schedules and runs its Pod.
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
        cluster: Some(kubeconfig.clone()),
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
    let stopped = KubeletStopped::stop(&kind_node);
    adapter
        .signal(&run, substrate_wire::Signal::Kill)
        .await
        .expect("killed");
    let ended = tokio::time::timeout(Duration::from_mins(3), adapter.wait(&run)).await;
    let still_running = running();
    tokio::task::block_in_place(|| stopped.restore());
    let ended = ended.expect("wait returned").expect("waited");
    println!("wait reported {ended:?}");
    assert!(
        ended.state != RunState::Ended || still_running.is_empty(),
        "wait reported {:?} {:?} while container {still_running} still runs",
        ended.state,
        ended.exit
    );
    // story:pod-lane-runs-adversary-and-security: a run read `Unknown` is removed by the adapter
    // once the kubelet answers again ("the adapter keeps trying", pod.rs). Wait for that here, in
    // the process that holds the adapter, so the CI pod lane's no-leftover check sees the result
    // instead of a NetworkPolicy left only because the test ended first.
    let deadline = std::time::Instant::now() + Duration::from_mins(3);
    loop {
        let left = leftovers(&kubeconfig, &adapter.settings().namespace, run.as_str());
        if left.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the adapter left {left} after the kubelet returned"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// The run's objects still in the cluster, read with `kubectl` rather than the adapter.
fn leftovers(kubeconfig: &std::path::Path, namespace: &str, run: &str) -> String {
    let output = std::process::Command::new("kubectl")
        .arg("--kubeconfig")
        .arg(kubeconfig)
        .args(["--namespace", namespace, "get", "pods,networkpolicies"])
        .arg(format!("--selector=b10x-substrate/run={run}"))
        .args(["--output", "name"])
        .output()
        .expect("kubectl runs");
    assert!(
        output.status.success(),
        "kubectl: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8")
}

/// The windows `adversary_wait_returns_when_the_api_server_goes_silent` opens the adapter with:
/// seconds where the adapter's own are 30 s per API call and two minutes unreachable.
const SILENT_TIMING: PodTiming = PodTiming {
    api_timeout: Duration::from_secs(5),
    unreachable_limit: Duration::from_secs(5),
};

/// `UNREACHABLE_LIMIT` (pod.rs): "How long the API may stay unreachable before a run is reported
/// `Unknown`". With the API server gone silent, `wait` must return within that limit plus one API
/// timeout plus a bounded cleanup. The adapter's own windows are two minutes and 30 s; the case
/// opens it with [`SILENT_TIMING`] and allows one minute, so it holds the same promise in seconds
/// and fails if `wait` waits out the adapter's own two minutes instead of the windows it was given.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_wait_returns_when_the_api_server_goes_silent() {
    assert_ne!(SILENT_TIMING, PodTiming::default());
    assert!(PodTiming::default().unreachable_limit > Duration::from_mins(1));
    let fake = Fake::start(Behaviour {
        blackhole_after_pod_create: true,
        ..Behaviour::default()
    });
    let adapter = fake.open_timed(SILENT_TIMING);
    let admitted = adapter.admit(&admitted_profile()).expect("admitted");
    let run = adapter.start(&admitted, &echo()).await.expect("started");
    let started = std::time::Instant::now();
    let waited = tokio::time::timeout(Duration::from_mins(1), adapter.wait(&run)).await;
    let observed = adapter.observe(&run).await.expect("observed");
    println!(
        "waited {:?}, returned {}; the run reads {:?}",
        started.elapsed(),
        waited.is_ok(),
        observed.state
    );
    assert!(
        waited.is_ok(),
        "wait did not return {:?} after the API server went silent; the run still reads {:?}",
        started.elapsed(),
        observed.state
    );
    let ended = waited.expect("wait returned").expect("waited");
    assert_eq!(
        ended.state,
        RunState::Unknown,
        "a run whose API went silent is reported {:?}, not Unknown",
        ended.exit
    );
}

/// A `NetworkPolicy` as the fake stores it, created `age` ago, with `labels`.
fn policy(name: &str, labels: &[(&str, &str)], age: Duration) -> Value {
    let mut object = json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": {"name": name, "namespace": NAMESPACE, "labels": {},
                     "resourceVersion": "1", "creationTimestamp": timestamp(age)},
        "spec": {"podSelector": {}, "policyTypes": ["Ingress", "Egress"]},
    });
    for (label, value) in labels {
        object["metadata"]["labels"][*label] = json!(value);
    }
    object
}

/// `story:pod-unknown-cleanup-outlives-adapter`, finding A3 (decision D124): a process that ends
/// before a run's cleanup leaves the run's `NetworkPolicy`. Open removes a policy carrying the
/// adapter's owner label whose run's Pod is gone, and nothing else: not a policy without the owner
/// label, not one whose Pod exists (here in a terminal phase), and not one younger than the
/// sweep's 90 s age floor, which may fence a run another adapter is still starting.
/// `story:pod-force-deleted-keeps-its-policy`: the orphan's Pod is gone and a watch of it replays
/// its container terminated; the Pod of `substrate-run-forced` was force-deleted with its
/// container last seen running, and its policy stays.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_open_removes_only_an_old_owned_policy_whose_pod_is_gone() {
    let fake = Fake::start(Behaviour::default());
    let old = Duration::from_hours(1);
    let owned = |run| {
        [
            (pod::MANAGED_BY_LABEL, pod::MANAGED_BY),
            (pod::RUN_LABEL, run),
        ]
    };
    let seeded = [
        (
            "substrate-run-orphan",
            policy("substrate-run-orphan", &owned("substrate-run-orphan"), old),
        ),
        (
            "substrate-run-live",
            policy("substrate-run-live", &owned("substrate-run-live"), old),
        ),
        (
            "substrate-run-forced",
            policy("substrate-run-forced", &owned("substrate-run-forced"), old),
        ),
        (
            "substrate-run-unowned",
            policy(
                "substrate-run-unowned",
                &[(pod::RUN_LABEL, "substrate-run-unowned")],
                old,
            ),
        ),
        (
            "substrate-run-starting",
            policy(
                "substrate-run-starting",
                &owned("substrate-run-starting"),
                Duration::from_secs(2),
            ),
        ),
    ];
    {
        let mut objects = fake.objects.lock();
        for (name, object) in seeded {
            objects.insert(format!("networkpolicies/{name}"), object);
        }
        objects.insert(
            "pods/substrate-run-live".to_owned(),
            json!({"metadata": {"name": "substrate-run-live"}}),
        );
        let gone = |name: &str, state: Value| {
            json!({"metadata": {"name": name, "resourceVersion": "2"},
                   "status": {"phase": "Failed",
                              "containerStatuses": [{"name": "run", "state": state}]}})
        };
        let mut deleted = fake.deleted.lock();
        deleted.insert(
            "substrate-run-orphan".to_owned(),
            gone(
                "substrate-run-orphan",
                json!({"terminated": {"exitCode": 0}}),
            ),
        );
        deleted.insert(
            "substrate-run-forced".to_owned(),
            gone("substrate-run-forced", json!({"running": {}})),
        );
    }
    let _adapter = fake.open();
    assert_eq!(
        fake.objects(),
        [
            "networkpolicies/substrate-run-forced",
            "networkpolicies/substrate-run-live",
            "networkpolicies/substrate-run-starting",
            "networkpolicies/substrate-run-unowned",
            "pods/substrate-run-live",
        ],
        "open swept the wrong policies; requests: {:#?}",
        fake.requests()
    );
    let deletions: Vec<String> = fake
        .requests()
        .into_iter()
        .filter(|request| request.starts_with("DELETE "))
        .collect();
    assert_eq!(
        deletions.len(),
        1,
        "open deleted more than the orphan: {deletions:?}"
    );
}

/// A sweep that cannot list the namespace's policies does not open as if it had swept: the
/// adapter is refused by name, as a failed probe is.
#[test]
fn adversary_open_is_refused_when_its_sweep_cannot_list_policies() {
    let fake = Fake::start(Behaviour {
        refuse_policy_list: true,
        ..Behaviour::default()
    });
    let error = pod::open_pod(PodRequest {
        cluster: Some(fake.kubeconfig.clone()),
    })
    .err()
    .expect("open is refused");
    assert_eq!(error.code(), "confinement.adapter-unserved", "{error}");
    assert!(error.to_string().contains("sweep"), "{error}");
}

/// The open sweep on a cluster: a `NetworkPolicy` carrying the adapter's labels whose Pod never
/// existed is gone once a fresh adapter opens. It waits out the sweep's 90 s age floor first, by
/// the API server's own clock. That wait is too long for the pod lane, so the case is manual like
/// `cleanup_pass1`: run it by hand with the lane variables and `SUBSTRATE_POD_MANUAL_CLEANUP=1`.
/// `adversary_open_removes_only_an_old_owned_policy_whose_pod_is_gone` holds the sweep in seconds.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_lane_open_sweeps_an_orphaned_policy() {
    let Some(kubeconfig) = lane("SUBSTRATE_POD_KUBECONFIG") else {
        return;
    };
    if std::env::var_os("SUBSTRATE_POD_MANUAL_CLEANUP").as_deref() != Some("1".as_ref()) {
        println!(
            "adversary pod sweep lane: manual; SUBSTRATE_POD_MANUAL_CLEANUP is not 1; skipped"
        );
        return;
    }
    let namespace = pod::open_pod(PodRequest {
        cluster: Some(kubeconfig.clone()),
    })
    .expect("opens")
    .settings()
    .namespace
    .clone();
    let run = format!(
        "substrate-run-sweep-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_nanos()
    );
    let directory = tempfile::tempdir().expect("directory");
    let manifest = directory.path().join("policy.json");
    let mut object = policy(
        &run,
        &[
            (pod::MANAGED_BY_LABEL, pod::MANAGED_BY),
            (pod::RUN_LABEL, &run),
        ],
        Duration::ZERO,
    );
    object["metadata"]
        .as_object_mut()
        .expect("metadata")
        .remove("creationTimestamp");
    object["metadata"]["namespace"] = json!(namespace);
    std::fs::write(&manifest, serde_json::to_vec(&object).expect("json")).expect("write");
    let created = std::process::Command::new("kubectl")
        .arg("--kubeconfig")
        .arg(&kubeconfig)
        .args(["--namespace", &namespace, "create", "--filename"])
        .arg(&manifest)
        .output()
        .expect("kubectl runs");
    assert!(
        created.status.success(),
        "kubectl create: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    tokio::time::sleep(Duration::from_secs(92)).await;
    assert!(
        !leftovers(&kubeconfig, &namespace, &run).is_empty(),
        "the policy is there before the sweep"
    );
    let _adapter = pod::open_pod(PodRequest {
        cluster: Some(kubeconfig.clone()),
    })
    .expect("opens");
    let left = leftovers(&kubeconfig, &namespace, &run);
    assert!(left.is_empty(), "open left the orphaned policy: {left}");
}

mod cleanup_pass1 {
    //! Adversary pass 1 on `story:pod-lane-runs-adversary-and-security`: what removes a run read
    //! `Unknown`.
    //!
    //! pod.rs, `monitor`: "a removal not confirmed within the limit reports the run `Unknown` while
    //! the adapter keeps trying". The trying is a task on the embedding process's runtime, and it is
    //! two bounded rounds of `REMOVE_ATTEMPTS` polls, not an open-ended retry. Both cases stop the
    //! kind node's kubelet, kill a run, see `wait` report `Unknown`, start the kubelet again, wait for
    //! the kubelet to remove the Pod, and then require the run's default-deny `NetworkPolicy` to be
    //! gone after a few of the adapter's own polls:
    //!
    //! * after the process that held the adapter has ended (its runtime is dropped, which is what
    //!   process exit does to the cleanup task), with a fresh adapter opened on the same cluster, as a
    //!   restarted daemon would;
    //! * with the adapter alive throughout, when the kubelet stays away longer than the two rounds.
    //!
    //! No wait here is longer than an adapter bound: reaching `Unknown` takes the adapter's
    //! `STOP_CONFIRM` (60 s), which no case can shorten. The lane variables are the pod lane's
    //! (`scripts/pod-lane.sh`). That lane runs this binary with every case selected, but these two
    //! cases are manual: with the lane variables set and `SUBSTRATE_POD_MANUAL_CLEANUP` not `1`, each
    //! prints a `skipped` line, not `ABSENT`, and returns, so the lane does not run them. Run them by
    //! hand with the lane variables, `SUBSTRATE_POD_MANUAL_CLEANUP=1` and `--test-threads=1`: they
    //! stop the kubelet.

    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use substrate_host::confinement::{
        Command, ConfinementAdapter, ConfinementProfile, RunState,
        pod::{self, PodAdapter, PodRequest},
    };

    const IMAGE: &str = "docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e";
    /// pod.rs `UNREACHABLE_LIMIT`: the adapter's longest bound on one removal.
    const UNREACHABLE_LIMIT: Duration = Duration::from_mins(2);
    /// Ten of pod.rs's `POLL` (500 ms): a policy removal follows its Pod's within one.
    const POLICY_GRACE: Duration = Duration::from_secs(5);
    /// pod.rs `monitor` after `Unknown`: two removal rounds of `REMOVE_ATTEMPTS` (60) polls of
    /// `POLL` (500 ms) plus a delete and a get each, about 31 s a round on a local cluster.
    const BOTH_ROUNDS: Duration = Duration::from_secs(75);

    fn lane() -> Option<(PathBuf, String)> {
        if std::env::var_os("SUBSTRATE_POD_LANE").as_deref() != Some("1".as_ref()) {
            println!("adversary pod cleanup lane: ABSENT; 0 executed");
            return None;
        }
        if std::env::var_os("SUBSTRATE_POD_MANUAL_CLEANUP").as_deref() != Some("1".as_ref()) {
            println!(
                "adversary pod cleanup lane: manual; SUBSTRATE_POD_MANUAL_CLEANUP is not 1; skipped; \
             0 executed"
            );
            return None;
        }
        let kubeconfig = std::env::var_os("SUBSTRATE_POD_KUBECONFIG").map(PathBuf::from);
        let node = std::env::var("SUBSTRATE_POD_KIND_NODE").ok();
        if let (Some(kubeconfig), Some(node)) = (kubeconfig, node) {
            Some((kubeconfig, node))
        } else {
            println!(
                "adversary pod cleanup lane: SUBSTRATE_POD_KUBECONFIG or SUBSTRATE_POD_KIND_NODE \
             unset; ABSENT; 0 executed"
            );
            None
        }
    }

    fn node(node: &str, args: &[&str]) -> String {
        let output = std::process::Command::new("docker")
            .args(["exec", node])
            .args(args)
            .output()
            .expect("docker runs");
        assert!(
            output.status.success(),
            "docker exec {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// Starts the kubelet again however the case ends.
    struct KubeletStopped(String);

    impl KubeletStopped {
        fn stop(kind_node: &str) -> Self {
            node(kind_node, &["systemctl", "stop", "kubelet"]);
            Self(kind_node.to_owned())
        }
    }

    impl Drop for KubeletStopped {
        fn drop(&mut self) {
            let _ = std::process::Command::new("docker")
                .args(["exec", &self.0, "systemctl", "start", "kubelet"])
                .status();
        }
    }

    fn objects(kubeconfig: &Path, namespace: &str, run: &str, kinds: &str) -> String {
        let output = std::process::Command::new("kubectl")
            .arg("--kubeconfig")
            .arg(kubeconfig)
            .args(["--namespace", namespace, "get", kinds])
            .arg(format!("--selector=b10x-substrate/run={run}"))
            .args(["--output", "name"])
            .output()
            .expect("kubectl runs");
        assert!(
            output.status.success(),
            "kubectl: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("utf-8")
            .trim()
            .to_owned()
    }

    /// Waits, at most `UNREACHABLE_LIMIT`, for the kubelet to remove the run's Pod, then
    /// `POLICY_GRACE`; returns what of the run is left.
    fn left_once_the_pod_is_gone(kubeconfig: &Path, namespace: &str, run: &str) -> String {
        let deadline = Instant::now() + UNREACHABLE_LIMIT;
        while !objects(kubeconfig, namespace, run, "pods").is_empty() {
            assert!(
                Instant::now() < deadline,
                "the kubelet did not remove the Pod"
            );
            std::thread::sleep(Duration::from_secs(1));
        }
        std::thread::sleep(POLICY_GRACE);
        objects(kubeconfig, namespace, run, "pods,networkpolicies")
    }

    fn open(kubeconfig: &Path) -> Arc<PodAdapter> {
        pod::open_pod(PodRequest {
            cluster: Some(kubeconfig.to_path_buf()),
        })
        .expect("opens")
    }

    /// Starts `sleep 600`, stops the kubelet, kills the run and requires `wait` to report `Unknown`.
    /// Returns the run id; the kubelet stays stopped until the returned guard drops.
    async fn killed_while_the_kubelet_is_away(
        adapter: &PodAdapter,
        kind_node: &str,
    ) -> (String, KubeletStopped) {
        let admitted = adapter
            .admit(&ConfinementProfile {
                image: Some(IMAGE.to_owned()),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let command = Command {
            argv: vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                "echo started; exec sleep 600".to_owned(),
            ],
            env: BTreeMap::new(),
        };
        let run = adapter.start(&admitted, &command).await.expect("started");
        let deadline = Instant::now() + Duration::from_mins(1);
        while !String::from_utf8_lossy(&adapter.output(&run).await.expect("output").stdout)
            .contains("started")
        {
            assert!(Instant::now() < deadline, "did not start");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let stopped = KubeletStopped::stop(kind_node);
        adapter
            .signal(&run, substrate_wire::Signal::Kill)
            .await
            .expect("killed");
        let ended = tokio::time::timeout(UNREACHABLE_LIMIT, adapter.wait(&run))
            .await
            .expect("wait returned")
            .expect("waited");
        assert_eq!(
            ended.state,
            RunState::Unknown,
            "the setup did not reach a run read Unknown"
        );
        println!("run {} reads Unknown", run.as_str());
        (run.as_str().to_owned(), stopped)
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    /// Held by `story:pod-unknown-cleanup-outlives-adapter`. Manual: `scripts/pod-lane.sh`
    /// runs this binary but not this case, which skips without `SUBSTRATE_POD_MANUAL_CLEANUP=1`. Run
    /// it by hand against a kind cluster with the pod lane's variables,
    /// `SUBSTRATE_POD_MANUAL_CLEANUP=1` and `--test-threads=1`.
    #[test]
    fn adversary_lane_a_run_read_unknown_leaves_nothing_once_its_process_has_ended() {
        let Some((kubeconfig, kind_node)) = lane() else {
            return;
        };
        let started = Instant::now();
        let first = runtime();
        let (namespace, run, stopped) = first.block_on(async {
            let adapter = open(&kubeconfig);
            let (run, stopped) = killed_while_the_kubelet_is_away(&adapter, &kind_node).await;
            (adapter.settings().namespace.clone(), run, stopped)
        });
        // The embedding process ends right after `wait` returned: its runtime, and the adapter's
        // cleanup task on it, end with it.
        drop(first);
        drop(stopped);
        // The restarted embedder opens `pod` again on the same cluster and stays up.
        let second = runtime();
        let _adapter = second.block_on(async { open(&kubeconfig) });
        let left = left_once_the_pod_is_gone(&kubeconfig, &namespace, &run);
        println!("wall time {:?}", started.elapsed());
        assert!(
            left.is_empty(),
            "the kubelet removed the Pod of a run read Unknown whose process ended, and {left} is left"
        );
    }

    /// Held by `story:pod-unknown-cleanup-outlives-adapter`. Manual: `scripts/pod-lane.sh`
    /// runs this binary but not this case, which skips without `SUBSTRATE_POD_MANUAL_CLEANUP=1`. Run
    /// it by hand against a kind cluster with the pod lane's variables,
    /// `SUBSTRATE_POD_MANUAL_CLEANUP=1` and `--test-threads=1`.
    #[tokio::test(flavor = "multi_thread")]
    async fn adversary_lane_a_run_read_unknown_is_removed_after_a_longer_kubelet_absence() {
        let Some((kubeconfig, kind_node)) = lane() else {
            return;
        };
        let started = Instant::now();
        let adapter = open(&kubeconfig);
        let namespace = adapter.settings().namespace.clone();
        let (run, stopped) = killed_while_the_kubelet_is_away(&adapter, &kind_node).await;
        // The node stays away for both of `monitor`'s removal rounds, and no longer.
        tokio::time::sleep(BOTH_ROUNDS).await;
        drop(stopped);
        let left = tokio::task::spawn_blocking(move || {
            left_once_the_pod_is_gone(&kubeconfig, &namespace, &run)
        })
        .await
        .expect("polled");
        println!("wall time {:?}", started.elapsed());
        assert!(
            left.is_empty(),
            "the kubelet removed the Pod of a run read Unknown, the adapter is alive, and {left} is \
         left"
        );
        drop(adapter);
    }
}

#[path = "adversary_pod/adversary_pass1.rs"]
mod adversary_pass1;
#[path = "adversary_pod/security_pass1.rs"]
mod security_pass1;
