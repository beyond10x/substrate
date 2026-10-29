//! Adversary pass on `story:pod-force-deleted-keeps-its-policy` (decision D173): a gone Pod's
//! `NetworkPolicy` goes only once the API shows the run's container stopped, and a run the adapter
//! itself killed leaves nothing behind (the decision's own reason: a killed run "leaves nothing").
//!
//! A scripted fake API server holds one run's Pod and policy; each case states what the Pod's
//! creation and deletion do and what a watch of the Pod replays, and drives the adapter through
//! `start`, `signal` and `wait`, so the cases do not depend on how the adapter tracks the Pod.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};

use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::*;

const IMAGE: &str =
    "registry.invalid/run@sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// What the scripted API server holds.
#[derive(Default)]
struct World {
    pod: Option<Value>,
    policy: Option<Value>,
    /// A watch's answer: its status and the events it replays. `None` ends the stream at once.
    watch: Option<(u16, Vec<Value>)>,
    /// While set, a read of the Pod by name answers 503.
    pod_reads_fail: bool,
    requests: Vec<String>,
}

type OnCreate = Box<dyn Fn(&mut World, &Value) -> bool + Send + Sync>;
type OnDelete = Box<dyn Fn(&mut World, &Value) + Send + Sync>;

struct Scripted {
    world: Mutex<World>,
    /// A Pod's create: stores what the API server would; `true` loses the answer.
    on_create: OnCreate,
    /// A deletion of the Pod the API holds, with that Pod.
    on_delete: OnDelete,
}

impl Scripted {
    fn new(on_create: OnCreate, on_delete: OnDelete) -> Arc<Self> {
        Arc::new(Self {
            world: Mutex::new(World::default()),
            on_create,
            on_delete,
        })
    }

    fn requests(&self) -> Vec<String> {
        self.world.lock().requests.clone()
    }

    fn watches(&self) -> usize {
        self.world
            .lock()
            .requests
            .iter()
            .filter(|request| request.contains("/pods?watch="))
            .count()
    }

    fn policy_deleted(&self) -> bool {
        self.world
            .lock()
            .requests
            .iter()
            .any(|request| request.starts_with("DELETE ") && request.contains("/networkpolicies/"))
    }
}

fn status(code: u16) -> String {
    json!({"kind": "Status", "code": code}).to_string()
}

/// The answer to one request, or `None` to drop the connection unanswered.
fn reply(scripted: &Scripted, method: &str, target: &str, body: &[u8]) -> Option<(u16, String)> {
    let mut world = scripted.world.lock();
    world.requests.push(format!("{method} {target}"));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let pods = format!("/api/v1/namespaces/{NAMESPACE}/pods");
    let policies = format!("/apis/networking.k8s.io/v1/namespaces/{NAMESPACE}/networkpolicies");
    let posted: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let gone = Some((404, status(404)));
    if method == "GET" && path == pods && query.starts_with("watch=") {
        return Some(match &world.watch {
            None => (200, String::new()),
            Some((code, events)) => (
                *code,
                events
                    .iter()
                    .map(|event| event.to_string() + "\n")
                    .collect(),
            ),
        });
    }
    if method == "POST" && path == policies {
        let mut policy = posted;
        policy["metadata"]["uid"] = json!("uid-policy");
        policy["metadata"]["resourceVersion"] = json!("1");
        world.policy = Some(policy.clone());
        return Some((201, policy.to_string()));
    }
    if method == "POST" && path == pods {
        if (scripted.on_create)(&mut world, &posted) {
            return None;
        }
        let pod = world.pod.clone().unwrap_or(posted);
        return Some((201, pod.to_string()));
    }
    if path.starts_with(&format!("{policies}/")) {
        return match method {
            "GET" => world
                .policy
                .clone()
                .map_or(gone, |policy| Some((200, policy.to_string()))),
            "DELETE" => world
                .policy
                .take()
                .map_or(gone, |policy| Some((200, policy.to_string()))),
            _ => Some((405, status(405))),
        };
    }
    if let Some(rest) = path.strip_prefix(&format!("{pods}/")) {
        if rest.ends_with("/log") {
            return Some((200, "hello\n".to_owned()));
        }
        return match method {
            "GET" if world.pod_reads_fail => Some((503, status(503))),
            "GET" => world
                .pod
                .clone()
                .map_or(gone, |pod| Some((200, pod.to_string()))),
            "DELETE" => match world.pod.clone() {
                Some(pod) => {
                    (scripted.on_delete)(&mut world, &pod);
                    Some((200, pod.to_string()))
                }
                None => gone,
            },
            _ => Some((405, status(405))),
        };
    }
    Some((500, status(500)))
}

fn serve_scripted(
    stream: TcpStream,
    config: Arc<ServerConfig>,
    scripted: &Scripted,
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
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body)?;
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let Some((code, payload)) = reply(scripted, method, target, &body) else {
        // Stored; the answer is lost.
        let _ = stream.get_mut().sock.shutdown(std::net::Shutdown::Both);
        return Ok(());
    };
    let writer = stream.get_mut();
    write!(
        writer,
        "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         date: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len(),
        chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT"),
    )?;
    writer.flush()?;
    writer.conn.send_close_notify();
    writer.flush()
}

/// The adapter, opened against the scripted fake without open's probes and sweep.
fn scripted_adapter(scripted: &Arc<Scripted>) -> PodAdapter {
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
    let served = Arc::clone(scripted);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (scripted, config) = (Arc::clone(&served), Arc::clone(&config));
            std::thread::spawn(move || serve_scripted(stream, config, &scripted));
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
    let (mut api, settings) =
        parse_cluster(kubeconfig.to_string().as_bytes()).expect("the fake parses");
    api.pacing = PACED;
    PodAdapter {
        facts: proof(),
        api: Arc::new(api),
        settings,
        runs: Mutex::new(Vec::new()),
    }
}

async fn start_echo(adapter: &PodAdapter) -> Result<RunId, ConfinementError> {
    let profile = ConfinementProfile {
        image: Some(IMAGE.to_owned()),
        ..ConfinementProfile::default()
    };
    let admitted = adapter.admit(&profile).expect("admitted");
    let command = Command {
        argv: vec!["/bin/echo".to_owned(), "hello".to_owned()],
        env: std::collections::BTreeMap::new(),
    };
    adapter.start(&admitted, &command).await
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let began = Instant::now();
    while !ready() {
        assert!(began.elapsed() < Duration::from_secs(20), "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn waited(adapter: &PodAdapter, run: &RunId) -> RunObservation {
    tokio::time::timeout(Duration::from_secs(30), adapter.wait(run))
        .await
        .expect("the run ends within 30 s")
        .expect("waited")
}

/// The Pod the API server stored from `posted`, at `resourceVersion` 2.
fn stored(posted: &Value, node: Option<&str>, restart: &str, status: Value) -> Value {
    let mut pod = posted.clone();
    pod["metadata"]["uid"] = json!("uid-pod");
    pod["metadata"]["resourceVersion"] = json!("2");
    if let Some(node) = node {
        pod["spec"]["nodeName"] = json!(node);
    }
    pod["spec"]["restartPolicy"] = json!(restart);
    pod["status"] = status;
    pod
}

/// `pod` at `version` with `status`.
fn at(pod: &Value, version: u32, status: Value) -> Value {
    let mut pod = pod.clone();
    pod["metadata"]["resourceVersion"] = json!(version.to_string());
    pod["status"] = status;
    pod
}

fn event(kind: &str, object: &Value) -> Value {
    json!({"type": kind, "object": object})
}

fn running() -> Value {
    json!({"phase": "Running", "containerStatuses": [{
        "name": CONTAINER, "restartCount": 0,
        "state": {"running": {"startedAt": "2026-10-29T00:00:00Z"}},
    }]})
}

fn terminated() -> Value {
    json!({"phase": "Failed", "containerStatuses": [{
        "name": CONTAINER, "restartCount": 0,
        "state": {"terminated": {"exitCode": 137, "reason": "Error"}},
    }]})
}

/// `PodGC`'s mark before a forced deletion, the container last reported running.
fn force_deleted(status: Value) -> Value {
    let mut status = status;
    status["phase"] = json!("Failed");
    status["conditions"] = json!([{"type": "DisruptionTarget", "status": "True",
                                   "reason": "DeletionByPodGC"}]);
    status
}

fn no_delete() -> OnDelete {
    Box::new(|_, _| {})
}

/// Waits until the run's task has asked the API about the gone Pod, then `settle` more.
async fn after_the_watch(scripted: &Scripted, settle: Duration) {
    until("the adapter never watched the gone Pod", || {
        scripted.watches() > 0
    })
    .await;
    tokio::time::sleep(settle).await;
}

/// The adapter's own kill of a run whose Pod no node took (the cluster has no room): the API server
/// deletes an unscheduled Pod at once, no kubelet is involved, and no container ever ran. Design 28
/// ends a killed run with its signal and deletes its policy; decision D173 rejected keeping the
/// policy after the adapter's own kill because a killed run must leave nothing.
#[tokio::test(flavor = "multi_thread")]
async fn adv_watch_a_killed_run_whose_pod_was_never_scheduled_leaves_no_policy() {
    let scripted = Scripted::new(
        Box::new(|world, posted| {
            let pending = json!({"phase": "Pending", "conditions": [{
                "type": "PodScheduled", "status": "False", "reason": "Unschedulable",
            }]});
            world.pod = Some(stored(posted, None, "Never", pending));
            false
        }),
        Box::new(|world, pod| {
            // No node holds the Pod: the API server removes it with a grace of 0.
            let last = at(pod, 3, pod["status"].clone());
            world.watch = Some((200, vec![event("DELETED", &last)]));
            world.pod = None;
        }),
    );
    let adapter = scripted_adapter(&scripted);
    let run = start_echo(&adapter).await.expect("started");
    let read = format!("GET {}/{}", adapter.api.pods(), run.as_str());
    until("the run's task never read its Pod", || {
        scripted.requests().contains(&read)
    })
    .await;
    adapter
        .signal(&run, Signal::Kill)
        .await
        .expect("the kill is asked for");
    let ended = waited(&adapter, &run).await;
    after_the_watch(&scripted, Duration::from_millis(500)).await;
    assert!(
        scripted.world.lock().policy.is_none(),
        "the adapter killed a run whose Pod no node ever held and left its NetworkPolicy; the run \
         reads {:?}; requests: {:#?}",
        ended.state,
        scripted.requests()
    );
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
}

/// The adapter's own kill, before its task's first successful read of the Pod (the API answered
/// the reads 503 meanwhile, or the kubelet was faster than the first poll): the kubelet wrote the
/// container terminated and removed the Pod. Design 28 § *A gone Pod*: before any Pod object was
/// read, the watch starts from the policy's `resourceVersion`, which `start` read.
#[tokio::test(flavor = "multi_thread")]
async fn adv_watch_a_run_killed_before_its_pod_was_read_watches_from_the_policy() {
    let scripted = Scripted::new(
        Box::new(|world, posted| {
            world.pod = Some(stored(posted, Some("node-a"), "Never", running()));
            world.pod_reads_fail = true;
            false
        }),
        Box::new(|world, pod| {
            // The kubelet stops the container, writes its state and removes the Pod.
            world.watch = Some((
                200,
                vec![
                    event("MODIFIED", &at(pod, 3, terminated())),
                    event("DELETED", &at(pod, 4, terminated())),
                ],
            ));
            world.pod = None;
            world.pod_reads_fail = false;
        }),
    );
    let adapter = scripted_adapter(&scripted);
    let run = start_echo(&adapter).await.expect("started");
    adapter
        .signal(&run, Signal::Kill)
        .await
        .expect("the kill is asked for");
    let ended = waited(&adapter, &run).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        scripted.world.lock().policy.is_none(),
        "a run the adapter killed, whose kubelet confirmed the container terminated, kept its \
         NetworkPolicy ({} watches of the Pod); the run reads {:?}; requests: {:#?}",
        scripted.watches(),
        ended.state,
        scripted.requests()
    );
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
}

/// A start whose Pod create answer was lost after the API server stored it (design 28: such a
/// start is abandoned): meanwhile the Pod was scheduled, its container started, and it was
/// force-deleted. The abandon reads the Pod 404 and, unlike the run's task and both sweeps,
/// releases the policy with no evidence the container stopped.
#[tokio::test(flavor = "multi_thread")]
async fn adv_watch_an_abandoned_start_whose_pod_was_force_deleted_keeps_its_policy() {
    let scripted = Scripted::new(
        Box::new(|world, posted| {
            let pod = stored(posted, Some("node-a"), "Never", running());
            world.watch = Some((
                200,
                vec![
                    event("ADDED", &at(&pod, 2, running())),
                    event("DELETED", &at(&pod, 3, force_deleted(running()))),
                ],
            ));
            world.pod = None;
            true
        }),
        no_delete(),
    );
    let adapter = scripted_adapter(&scripted);
    let refused = start_echo(&adapter).await;
    assert_eq!(
        refused.as_ref().err().map(ConfinementError::code),
        Some("confinement.pod-api-failed"),
        "{refused:?}"
    );
    assert!(
        scripted.world.lock().policy.is_some(),
        "the abandoned start released the NetworkPolicy of a Pod that was force-deleted while its \
         container ran; requests: {:#?}",
        scripted.requests()
    );
}

/// `PodTrace` reads a container shown `terminated` as stopped for good because the rendered Pod
/// has `restartPolicy: Never`, but it never checks the Pod it read says so. A Pod whose restart
/// policy an admission webhook changed restarts its container: the replay shows it terminated,
/// then running again, then force-deleted while running.
#[tokio::test(flavor = "multi_thread")]
async fn adv_watch_a_terminated_container_that_restarted_keeps_the_policy() {
    let scripted = Scripted::new(
        Box::new(|world, posted| {
            world.pod = Some(stored(posted, Some("node-a"), "Always", running()));
            false
        }),
        no_delete(),
    );
    let adapter = scripted_adapter(&scripted);
    let run = start_echo(&adapter).await.expect("started");
    let read = format!("GET {}/{}", adapter.api.pods(), run.as_str());
    until("the run's task never read its Pod", || {
        scripted.requests().contains(&read)
    })
    .await;
    {
        let mut world = scripted.world.lock();
        let pod = world.pod.take().expect("the Pod");
        let mut restarted = running();
        restarted["containerStatuses"][0]["restartCount"] = json!(1);
        restarted["containerStatuses"][0]["lastState"] =
            json!({"terminated": {"exitCode": 1, "reason": "Error"}});
        world.watch = Some((
            200,
            vec![
                event("MODIFIED", &at(&pod, 3, terminated())),
                event("MODIFIED", &at(&pod, 4, restarted.clone())),
                event("DELETED", &at(&pod, 5, force_deleted(restarted))),
            ],
        ));
    }
    let ended = waited(&adapter, &run).await;
    after_the_watch(&scripted, Duration::from_millis(500)).await;
    assert!(
        !scripted.policy_deleted(),
        "the policy was released on a terminated state the replay later showed running again; \
         the run reads {:?}; requests: {:#?}",
        ended.state,
        scripted.requests()
    );
}

/// A watch that keeps answering something other than 200, 401, 403 or 410 is asked again every
/// `notice` for as long as the runtime lives. The run's Pod is gone, so the run no longer counts
/// against `POD_MAX_TRACKED_RUNS` and is forgotten to make room while its task still asks: the
/// tasks are no longer bounded by the cap (design 28: "the removal tasks never outnumber the cap").
#[tokio::test(flavor = "multi_thread")]
async fn adv_watch_a_task_still_asking_about_a_gone_pod_counts_against_the_cap() {
    let scripted = Scripted::new(
        Box::new(|world, posted| {
            world.pod = Some(stored(posted, Some("node-a"), "Never", running()));
            false
        }),
        no_delete(),
    );
    let adapter = scripted_adapter(&scripted);
    let run = start_echo(&adapter).await.expect("started");
    let read = format!("GET {}/{}", adapter.api.pods(), run.as_str());
    until("the run's task never read its Pod", || {
        scripted.requests().contains(&read)
    })
    .await;
    {
        let mut world = scripted.world.lock();
        world.pod = None;
        world.watch = Some((500, vec![json!({"kind": "Status", "code": 500})]));
    }
    let ended = waited(&adapter, &run).await;
    assert_eq!(ended.state, RunState::Unknown);
    until("the task asked the watch once only", || {
        scripted.watches() >= 2
    })
    .await;
    {
        let mut runs = adapter.runs.lock();
        for index in 1..POD_MAX_TRACKED_RUNS {
            runs.push(tracked(&format!("substrate-run-busy-{index}")).0);
        }
    }
    let reserved = adapter.reserve();
    let before = scripted.watches();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = scripted.watches();
    assert!(
        reserved.is_err(),
        "the run was forgotten to make room while its task still asks about its gone Pod: {} \
         watches in 300 ms after it was forgotten",
        after - before
    );
}

/// What a watch of the gone Pod answers, from the Pod the adapter last read.
type Replay = Box<dyn Fn(&Value) -> (u16, Vec<Value>) + Send + Sync>;

/// Replays that must keep the policy, each after a forced deletion of a Pod read running.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // Every replay case is listed beside the one flow they share.
async fn adv_watch_replays_without_the_run_container_terminated_keep_the_policy() {
    let sidecar = {
        let mut status = running();
        status["containerStatuses"] = json!([
            {"name": "aaa-sidecar", "state": {"terminated": {"exitCode": 0, "reason": "Completed"}}},
            status["containerStatuses"][0].clone(),
        ]);
        status
    };
    let init = {
        let mut status = running();
        status["initContainerStatuses"] = json!([{"name": CONTAINER,
            "state": {"terminated": {"exitCode": 0, "reason": "Completed"}}}]);
        status
    };
    let last_state = {
        let mut status = running();
        status["containerStatuses"][0]["lastState"] =
            json!({"terminated": {"exitCode": 0, "reason": "Completed"}});
        status
    };
    let ephemeral = {
        let mut status = running();
        status["ephemeralContainerStatuses"] = json!([{"name": CONTAINER,
            "state": {"terminated": {"exitCode": 0, "reason": "Completed"}}}]);
        status
    };
    let cases: Vec<(&str, Replay)> = vec![
        (
            "a bookmark only",
            Box::new(|pod| {
                let mut bookmark = json!({"kind": "Pod", "metadata": {"resourceVersion": "9"}});
                bookmark["metadata"]["name"] = pod["metadata"]["name"].clone();
                (200, vec![event("BOOKMARK", &bookmark)])
            }),
        ),
        (
            "a stream that ends after a running container",
            Box::new(|pod| (200, vec![event("MODIFIED", &at(pod, 3, running()))])),
        ),
        (
            "another uid shown terminated",
            Box::new(|pod| {
                let mut other = at(pod, 3, terminated());
                other["metadata"]["uid"] = json!("uid-earlier");
                (
                    200,
                    vec![
                        event("MODIFIED", &other),
                        event("DELETED", &at(pod, 4, force_deleted(running()))),
                    ],
                )
            }),
        ),
        (
            "a sidecar terminated",
            Box::new(move |pod| {
                (
                    200,
                    vec![event(
                        "DELETED",
                        &at(pod, 3, force_deleted(sidecar.clone())),
                    )],
                )
            }),
        ),
        (
            "an init container of the same name terminated",
            Box::new(move |pod| {
                (
                    200,
                    vec![event("DELETED", &at(pod, 3, force_deleted(init.clone())))],
                )
            }),
        ),
        (
            "the last state terminated",
            Box::new(move |pod| {
                (
                    200,
                    vec![event(
                        "DELETED",
                        &at(pod, 3, force_deleted(last_state.clone())),
                    )],
                )
            }),
        ),
        (
            "an ephemeral container of the same name terminated",
            Box::new(move |pod| {
                (
                    200,
                    vec![event(
                        "DELETED",
                        &at(pod, 3, force_deleted(ephemeral.clone())),
                    )],
                )
            }),
        ),
        (
            "a Failed phase with no container status",
            Box::new(|pod| {
                (
                    200,
                    vec![event("DELETED", &at(pod, 3, force_deleted(json!({}))))],
                )
            }),
        ),
        (
            "an ERROR event answering 500",
            Box::new(|_| {
                (
                    200,
                    vec![event(
                        "ERROR",
                        &json!({"kind": "Status", "code": 500, "message": "internal"}),
                    )],
                )
            }),
        ),
        (
            "a watch answered 500",
            Box::new(|_| (500, vec![json!({"kind": "Status", "code": 500})])),
        ),
        (
            "out of order: the deletion first, a terminated state after it",
            Box::new(|pod| {
                (
                    200,
                    vec![
                        event("DELETED", &at(pod, 4, force_deleted(running()))),
                        event("MODIFIED", &at(pod, 3, terminated())),
                    ],
                )
            }),
        ),
    ];
    let mut tasks = Vec::new();
    for (case, replay) in cases {
        tasks.push(tokio::spawn(async move {
            let scripted = Scripted::new(
                Box::new(|world, posted| {
                    world.pod = Some(stored(posted, Some("node-a"), "Never", running()));
                    false
                }),
                no_delete(),
            );
            let adapter = scripted_adapter(&scripted);
            let run = start_echo(&adapter).await.expect("started");
            let read = format!("GET {}/{}", adapter.api.pods(), run.as_str());
            until("the run's task never read its Pod", || {
                scripted.requests().contains(&read)
            })
            .await;
            {
                let mut world = scripted.world.lock();
                let pod = world.pod.take().expect("the Pod");
                world.watch = Some(replay(&pod));
            }
            let ended = waited(&adapter, &run).await;
            after_the_watch(&scripted, Duration::from_millis(500)).await;
            (case, ended.state, scripted.policy_deleted())
        }));
    }
    for task in tasks {
        let (case, state, deleted) = task.await.expect("the case ran");
        assert!(!deleted, "{case}: the policy was released");
        assert_eq!(state, RunState::Unknown, "{case}");
    }
}
