//! An in-process Kubernetes API server for tests (`docs/design/34-kubernetes-namespace-driver.md`
//! § 10.1): HTTPS under its own `rcgen` CA, one namespace, an in-memory store with real `uid`s and
//! `resourceVersion`s, and a fault plan.
//!
//! It serves Pods, `PersistentVolumeClaim`s, `NetworkPolicy` objects and `Lease`s: create (with
//! `dryRun=All`), get, update (refused 409 on a stale `resourceVersion`), paged lists (whole or
//! metadata alone), chunked watches (an `ERROR` event with code 410 for a version older than
//! [`FakeCluster::compact`]), and delete with `gracePeriodSeconds` and `preconditions.uid` (409 on
//! a mismatch); `SelfSubjectRulesReview` answering [`FakeCluster::set_rules`]; `pods/log` with
//! `limitBytes`; and `pods/exec` over WebSocket `v5.channel.k8s.io`, answered by the
//! [`ExecHandler`] a test installs. Every request needs the fake's bearer token. Pod Security
//! admission is switchable ([`FakeCluster::enforce_restricted`]): off, as in a namespace without
//! the `pod-security.kubernetes.io/enforce` label, every Pod is admitted; on, a Pod with a
//! `privileged` container is refused 403, with or without `dryRun=All`.
//!
//! What it does not do yet, and where that lands: no kubelet (a Pod is never bound or run, and a
//! deletion removes it at once) and no executor behind `pods/exec`; the namespace driver's slices
//! add a stepped kubelet and an [`ExecHandler`] that runs the real executor. It confines nothing.
//!
//! Compiled for this crate's tests and under the `fake` feature, which is legal only on a
//! `[dev-dependencies]` edge.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::fmt;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::{Frame, Incoming};
use hyper::header::{self, HeaderValue};
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;

use crate::exec::EXEC_PROTOCOL;

type Body = BoxBody<Bytes, Infallible>;

/// The resources the fake stores: the API prefix and resource name a path names, the kind, and
/// the `apiVersion`.
const RESOURCES: [(&str, &str, &str, &str); 4] = [
    ("/api/v1", "pods", "Pod", "v1"),
    (
        "/api/v1",
        "persistentvolumeclaims",
        "PersistentVolumeClaim",
        "v1",
    ),
    (
        "/apis/networking.k8s.io/v1",
        "networkpolicies",
        "NetworkPolicy",
        "networking.k8s.io/v1",
    ),
    (
        "/apis/coordination.k8s.io/v1",
        "leases",
        "Lease",
        "coordination.k8s.io/v1",
    ),
];
const RULES_REVIEW: &str = "/apis/authorization.k8s.io/v1/selfsubjectrulesreviews";
/// The longest a watch stays open, whatever `timeoutSeconds` asks.
const MAX_WATCH: Duration = Duration::from_secs(30);
const MAX_REQUEST_BYTES: usize = 4 * 1_048_576;

/// One planned fault: the next request whose method is `method` and whose path and query start
/// with `path` meets it, once.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Fault {
    /// The fake acts on the request, stores what it stores, and closes the connection without
    /// answering: an answer lost after the API acted.
    DropAnswer {
        /// The request's method.
        method: Method,
        /// The prefix of its path and query.
        path: String,
    },
    /// The fake answers `status` with `body` and does not act on the request.
    Answer {
        /// The request's method.
        method: Method,
        /// The prefix of its path and query.
        path: String,
        /// The status it answers.
        status: u16,
        /// The JSON body it answers.
        body: Value,
    },
    /// The fake waits `by` before it acts on the request and answers: an API server whose
    /// response head is late.
    Delay {
        /// The request's method.
        method: Method,
        /// The prefix of its path and query.
        path: String,
        /// How long it waits.
        by: Duration,
    },
}

impl Fault {
    fn matches(&self, request_method: &Method, target: &str) -> bool {
        let (Self::DropAnswer { method, path }
        | Self::Answer { method, path, .. }
        | Self::Delay { method, path, .. }) = self;
        method == request_method && target.starts_with(path.as_str())
    }
}

/// One `pods/exec` call as the fake received it: the whole input, once the client closed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    /// The Pod's name.
    pub pod: String,
    /// The `container` the call names.
    pub container: String,
    /// The `command` parts, in order.
    pub command: Vec<String>,
    /// Everything the client sent on stdin.
    pub stdin: Vec<u8>,
}

/// What a `pods/exec` call answers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecAnswer {
    /// Sent on channel 1.
    pub stdout: Vec<u8>,
    /// Sent on channel 2.
    pub stderr: Vec<u8>,
    /// Stated in the `Status` on channel 3.
    pub exit_code: i32,
}

/// What answers `pods/exec`: the seam where a test, or a later slice's executor, decides what the
/// command does. It runs on a blocking thread.
pub trait ExecHandler: Send + Sync + 'static {
    /// Runs one call.
    fn exec(&self, request: ExecRequest) -> ExecAnswer;
}

impl<F> ExecHandler for F
where
    F: Fn(ExecRequest) -> ExecAnswer + Send + Sync + 'static,
{
    fn exec(&self, request: ExecRequest) -> ExecAnswer {
        self(request)
    }
}

/// A running fake API server; it stops when dropped.
pub struct FakeCluster {
    shared: Arc<Shared>,
    address: SocketAddr,
    ca_pem: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl fmt::Debug for FakeCluster {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FakeCluster")
            .field("address", &self.address)
            .field("namespace", &self.shared.namespace)
            .finish_non_exhaustive()
    }
}

struct Shared {
    namespace: String,
    token: String,
    store: Mutex<Store>,
    events: tokio::sync::broadcast::Sender<Stored>,
    exec: Mutex<Option<Arc<dyn ExecHandler>>>,
}

#[derive(Default)]
struct Store {
    version: u64,
    uids: u64,
    objects: BTreeMap<&'static str, BTreeMap<String, Value>>,
    history: Vec<Stored>,
    compacted: u64,
    /// What a paged list's later pages read: a resource's objects at the version its first page
    /// answered, until [`FakeCluster::compact`] passes that version.
    snapshots: BTreeMap<(&'static str, u64), BTreeMap<String, Value>>,
    faults: Vec<Fault>,
    requests: Vec<String>,
    rules: Vec<Value>,
    logs: BTreeMap<String, Vec<u8>>,
    log_refusal: Option<(u16, String)>,
    restricted: bool,
}

/// One stored change, as a watch replays it.
#[derive(Debug, Clone)]
struct Stored {
    version: u64,
    resource: &'static str,
    kind: &'static str,
    object: Value,
}

/// A refusal the fake answers as a Kubernetes `Status`.
struct Refusal {
    code: u16,
    reason: &'static str,
    message: String,
}

impl Refusal {
    fn new(code: u16, reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            reason,
            message: message.into(),
        }
    }
}

/// The service's error: the connection closes without an answer.
#[derive(Debug)]
struct Dropped;

impl fmt::Display for Dropped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the fake dropped the answer")
    }
}

impl std::error::Error for Dropped {}

impl FakeCluster {
    /// Starts a fake serving `namespace` to the bearer `token`, on loopback, on a thread of its
    /// own.
    ///
    /// # Errors
    ///
    /// When the CA, the listener or the thread cannot be made.
    pub fn start(namespace: &str, token: &str) -> std::io::Result<Self> {
        let other = |error: &dyn fmt::Display| std::io::Error::other(error.to_string());
        let ca_key = rcgen::KeyPair::generate().map_err(|error| other(&error))?;
        let mut ca_params =
            rcgen::CertificateParams::new(Vec::<String>::new()).map_err(|error| other(&error))?;
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "fake cluster CA");
        let ca = ca_params
            .self_signed(&ca_key)
            .map_err(|error| other(&error))?;
        let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
        let server_key = rcgen::KeyPair::generate().map_err(|error| other(&error))?;
        let server = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()])
            .and_then(|params| params.signed_by(&server_key, &issuer))
            .map_err(|error| other(&error))?;
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|error| other(&error))?
        .with_no_client_auth()
        .with_single_cert(
            vec![server.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
        )
        .map_err(|error| other(&error))?;
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));

        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let (events, _) = tokio::sync::broadcast::channel(1024);
        let shared = Arc::new(Shared {
            namespace: namespace.to_owned(),
            token: token.to_owned(),
            store: Mutex::new(Store::default()),
            events,
            exec: Mutex::new(None),
        });
        let (shutdown, stop) = tokio::sync::oneshot::channel();
        let state = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("fake-kube-apiserver".to_owned())
            .spawn(move || serve(listener, acceptor, state, stop))?;
        Ok(Self {
            shared,
            address,
            ca_pem: ca.pem(),
            shutdown: Some(shutdown),
            thread: Some(thread),
        })
    }

    /// Where the fake listens.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The namespace it serves.
    pub fn namespace(&self) -> &str {
        &self.shared.namespace
    }

    /// The JSON kubeconfig that names the fake: its CA data, its server and its bearer token, in a
    /// context `fake` naming its namespace.
    pub fn kubeconfig(&self) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "Config",
            "current-context": "fake",
            "clusters": [{"name": "fake", "cluster": {
                "server": format!("https://{}", self.address),
                "certificate-authority-data":
                    base64::engine::general_purpose::STANDARD.encode(&self.ca_pem),
            }}],
            "contexts": [{"name": "fake", "context": {
                "cluster": "fake", "user": "fake", "namespace": self.shared.namespace,
            }}],
            "users": [{"name": "fake", "user": {"token": self.shared.token}}],
        })
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.shared.lock()
    }

    /// The stored object `name` of `resource` (`pods`, `leases`, …), if any.
    pub fn object(&self, resource: &str, name: &str) -> Option<Value> {
        self.store()
            .objects
            .get(resource)
            .and_then(|objects| objects.get(name))
            .cloned()
    }

    /// Stores `object` in `resource` as a create through the API would.
    ///
    /// # Errors
    ///
    /// The `Status` message the API would answer: an unknown resource, a missing name or one taken.
    pub fn seed(&self, resource: &str, object: Value) -> Result<Value, String> {
        let (_, resource, kind, api_version) = RESOURCES
            .into_iter()
            .find(|(_, name, _, _)| *name == resource)
            .ok_or_else(|| format!("the fake serves no {resource}"))?;
        let created = self.shared.create(
            &mut self.store(),
            resource,
            kind,
            api_version,
            object,
            false,
        );
        created.map_err(|refusal| refusal.message)
    }

    /// Plans `fault` for the next matching request.
    pub fn fault(&self, fault: Fault) {
        self.store().faults.push(fault);
    }

    /// Every request so far, as `METHOD path?query`, in arrival order.
    pub fn requests(&self) -> Vec<String> {
        self.store().requests.clone()
    }

    /// Forgets the history up to now: a watch from any older `resourceVersion` answers an `ERROR`
    /// event with code 410, and a list continued from an older page answers 410.
    pub fn compact(&self) {
        let mut store = self.store();
        let compacted = store.version;
        store.compacted = compacted;
        store.history.clear();
        store
            .snapshots
            .retain(|(_, version), _| *version >= compacted);
    }

    /// The `resourceRules` every `SelfSubjectRulesReview` answers.
    pub fn set_rules(&self, rules: Vec<Value>) {
        self.store().rules = rules;
    }

    /// The log `pods/log` answers for the Pod `pod`.
    pub fn set_log(&self, pod: &str, log: &[u8]) {
        self.store().logs.insert(pod.to_owned(), log.to_vec());
    }

    /// Every `pods/log` read from now on is refused `status` `Forbidden` with `message`, as the
    /// API answers a Role without `pods/log` `get`.
    pub fn refuse_logs(&self, status: u16, message: &str) {
        self.store().log_refusal = Some((status, message.to_owned()));
    }

    /// Whether Pod Security admission enforces `restricted` from now on. Off, the default, admits
    /// every Pod; on, a Pod any of whose containers or init containers sets
    /// `securityContext.privileged: true` is refused 403 `Forbidden` with the message the API
    /// server's admission writes, before anything is stored and also under `dryRun=All`. Only the
    /// `privileged` field is checked: the fake does not evaluate the rest of the profile.
    pub fn enforce_restricted(&self, on: bool) {
        self.store().restricted = on;
    }

    /// What answers `pods/exec` from now on. Without one, a call is answered a `Failure` status.
    pub fn set_exec(&self, handler: impl ExecHandler) {
        *self
            .shared
            .exec
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(handler));
    }
}

impl Drop for FakeCluster {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(
    listener: std::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    shared: Arc<Shared>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    runtime.block_on(async move {
        let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
            return;
        };
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                _ = &mut stop => return,
            };
            let Ok((tcp, _)) = accepted else {
                continue;
            };
            let (acceptor, shared) = (acceptor.clone(), Arc::clone(&shared));
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let service = hyper::service::service_fn(move |request| {
                    let shared = Arc::clone(&shared);
                    async move { shared.handle(request).await }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), service)
                    .with_upgrades()
                    .await;
            });
        }
    });
}

/// What a path names beneath the namespace.
struct Target {
    resource: &'static str,
    kind: &'static str,
    api_version: &'static str,
    name: Option<String>,
    subresource: Option<String>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn handle(
        self: Arc<Self>,
        request: Request<Incoming>,
    ) -> Result<Response<Body>, Dropped> {
        let method = request.method().clone();
        let target = request
            .uri()
            .path_and_query()
            .map_or_else(|| request.uri().path().to_owned(), ToString::to_string);
        self.lock().requests.push(format!("{method} {target}"));
        let authorized = request
            .headers()
            .get(header::AUTHORIZATION)
            .is_some_and(|value| value.as_bytes() == format!("Bearer {}", self.token).as_bytes());
        if !authorized {
            return Ok(status(&Refusal::new(401, "Unauthorized", "Unauthorized")));
        }
        let fault = {
            let mut store = self.lock();
            let planned = store
                .faults
                .iter()
                .position(|fault| fault.matches(&method, &target));
            planned.map(|index| store.faults.remove(index))
        };
        let drop = match fault {
            Some(Fault::Answer { status, body, .. }) => return Ok(answer(status, &body)),
            Some(Fault::DropAnswer { .. }) => true,
            Some(Fault::Delay { by, .. }) => {
                tokio::time::sleep(by).await;
                false
            }
            None => false,
        };
        let answered = self.route(request, &method).await;
        if drop {
            return Err(Dropped);
        }
        Ok(answered.unwrap_or_else(|refusal| status(&refusal)))
    }

    async fn route(
        self: &Arc<Self>,
        request: Request<Incoming>,
        method: &Method,
    ) -> Result<Response<Body>, Refusal> {
        let path = request.uri().path().to_owned();
        let query: BTreeMap<String, Vec<String>> =
            request.uri().query().map_or_else(BTreeMap::new, |query| {
                let mut parsed: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
                    parsed
                        .entry(key.into_owned())
                        .or_default()
                        .push(value.into_owned());
                }
                parsed
            });
        let first = |key: &str| query.get(key).and_then(|values| values.first()).cloned();
        let dry_run = first("dryRun").as_deref() == Some("All");
        if path == RULES_REVIEW && method == Method::POST {
            let review = read_json(request).await?;
            let rules = self.lock().rules.clone();
            let mut answered = review;
            answered["status"] = json!({
                "resourceRules": rules,
                "nonResourceRules": [],
                "incomplete": false,
            });
            return Ok(answer(201, &answered));
        }
        let target = self.target(&path)?;
        let metadata = request
            .headers()
            .get(header::ACCEPT)
            .and_then(|accept| accept.to_str().ok())
            .is_some_and(|accept| accept.contains("as=PartialObjectMetadataList"));
        match (method, target.name.clone(), target.subresource.as_deref()) {
            (&Method::POST, None, None) => {
                let object = read_json(request).await?;
                let created = self.create(
                    &mut self.lock(),
                    target.resource,
                    target.kind,
                    target.api_version,
                    object,
                    dry_run,
                )?;
                Ok(answer(201, &created))
            }
            (&Method::GET, None, None)
                if first("watch").is_some_and(|w| w == "1" || w == "true") =>
            {
                Ok(self.watch(&target, &first))
            }
            (&Method::GET, None, None) => Ok(answer(200, &self.list(&target, &first, metadata)?)),
            (&Method::GET, Some(name), None) => self
                .lock()
                .objects
                .get(target.resource)
                .and_then(|objects| objects.get(&name))
                .map(|object| answer(200, object))
                .ok_or_else(|| not_found(target.resource, &name)),
            (&Method::PUT, Some(name), None) => {
                let object = read_json(request).await?;
                Ok(answer(200, &self.update(&target, &name, object, dry_run)?))
            }
            (&Method::DELETE, Some(name), None) => {
                let options = read_optional_json(request).await?;
                Ok(answer(
                    200,
                    &self.delete(&target, &name, &options, dry_run)?,
                ))
            }
            (&Method::GET, Some(name), Some("log")) if target.resource == "pods" => {
                self.log(&name, &first)
            }
            (&Method::GET | &Method::POST, Some(name), Some("exec"))
                if target.resource == "pods" =>
            {
                self.exec(request, &name, &query)
            }
            _ => Err(Refusal::new(
                405,
                "MethodNotAllowed",
                format!("the fake does not serve {method} {path}"),
            )),
        }
    }

    fn target(&self, path: &str) -> Result<Target, Refusal> {
        let unknown = || Refusal::new(404, "NotFound", format!("the fake serves no {path}"));
        let (prefix, rest) = path.split_once("/namespaces/").ok_or_else(unknown)?;
        let mut parts = rest.split('/');
        let namespace = parts.next().ok_or_else(unknown)?;
        let resource = parts.next().ok_or_else(unknown)?;
        let name = parts.next().map(str::to_owned);
        let subresource = parts.next().map(str::to_owned);
        if parts.next().is_some() {
            return Err(unknown());
        }
        let (_, resource, kind, api_version) = RESOURCES
            .into_iter()
            .find(|(api, name, _, _)| *api == prefix && *name == resource)
            .ok_or_else(unknown)?;
        if namespace != self.namespace {
            return Err(Refusal::new(
                403,
                "Forbidden",
                format!("{resource} is forbidden in namespace {namespace}"),
            ));
        }
        Ok(Target {
            resource,
            kind,
            api_version,
            name,
            subresource,
        })
    }

    fn create(
        &self,
        store: &mut Store,
        resource: &'static str,
        kind: &'static str,
        api_version: &'static str,
        mut object: Value,
        dry_run: bool,
    ) -> Result<Value, Refusal> {
        self.same_namespace(&object)?;
        let Some(name) = object["metadata"]["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
        else {
            return Err(Refusal::new(
                422,
                "Invalid",
                "metadata.name: Required value",
            ));
        };
        if resource == "pods"
            && store.restricted
            && let Some(container) = privileged_container(&object)
        {
            return Err(Refusal::new(
                403,
                "Forbidden",
                format!(
                    "pods \"{name}\" is forbidden: violates PodSecurity \"restricted:latest\": \
                     privileged (container \"{container}\" must not set \
                     securityContext.privileged=true)"
                ),
            ));
        }
        if store
            .objects
            .get(resource)
            .is_some_and(|objects| objects.contains_key(&name))
        {
            return Err(Refusal::new(
                409,
                "AlreadyExists",
                format!("{resource} \"{name}\" already exists"),
            ));
        }
        object["kind"] = json!(kind);
        object["apiVersion"] = json!(api_version);
        object["metadata"]["namespace"] = json!(self.namespace);
        object["metadata"]["creationTimestamp"] =
            json!(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
        store.uids += 1;
        object["metadata"]["uid"] = json!(format!("00000000-0000-4000-8000-{:012x}", store.uids));
        if dry_run {
            return Ok(object);
        }
        store.version += 1;
        object["metadata"]["resourceVersion"] = json!(store.version.to_string());
        store
            .objects
            .entry(resource)
            .or_default()
            .insert(name, object.clone());
        self.record(store, resource, "ADDED", &object);
        Ok(object)
    }

    /// A body naming another namespace than the request's is refused 400, as the API's handler
    /// does before anything is stored.
    fn same_namespace(&self, object: &Value) -> Result<(), Refusal> {
        match object["metadata"]["namespace"].as_str() {
            Some(namespace) if !namespace.is_empty() && namespace != self.namespace => {
                Err(Refusal::new(
                    400,
                    "BadRequest",
                    "the namespace of the provided object does not match the namespace sent on \
                     the request",
                ))
            }
            _ => Ok(()),
        }
    }

    /// A replacement: refused 400 for a body naming another object or namespace, 409 for a stale
    /// `resourceVersion` and 422 for another `metadata.uid`, which is immutable; under `dryRun=All`
    /// answered and not stored.
    fn update(
        &self,
        target: &Target,
        name: &str,
        mut object: Value,
        dry_run: bool,
    ) -> Result<Value, Refusal> {
        if let Some(named) = object["metadata"]["name"].as_str()
            && !named.is_empty()
            && named != name
        {
            return Err(Refusal::new(
                400,
                "BadRequest",
                format!(
                    "the name of the object ({named}) does not match the name on the URL ({name})"
                ),
            ));
        }
        self.same_namespace(&object)?;
        let mut store = self.lock();
        let store = &mut *store;
        let stored = store
            .objects
            .get(target.resource)
            .and_then(|objects| objects.get(name))
            .ok_or_else(|| not_found(target.resource, name))?;
        if object["metadata"]["resourceVersion"] != stored["metadata"]["resourceVersion"] {
            return Err(Refusal::new(
                409,
                "Conflict",
                format!(
                    "Operation cannot be fulfilled on {} \"{name}\": the object has been modified; \
                     please apply your changes to the latest version and try again",
                    target.resource
                ),
            ));
        }
        let held = &stored["metadata"]["uid"];
        if let Some(uid) = object["metadata"]["uid"].as_str()
            && !uid.is_empty()
            && Some(uid) != held.as_str()
        {
            return Err(Refusal::new(
                422,
                "Invalid",
                format!("metadata.uid: Invalid value: \"{uid}\": field is immutable"),
            ));
        }
        for member in ["uid", "creationTimestamp", "namespace", "name"] {
            object["metadata"][member] = stored["metadata"][member].clone();
        }
        object["kind"] = json!(target.kind);
        object["apiVersion"] = json!(target.api_version);
        if dry_run {
            return Ok(object);
        }
        store.version += 1;
        object["metadata"]["resourceVersion"] = json!(store.version.to_string());
        store
            .objects
            .entry(target.resource)
            .or_default()
            .insert(name.to_owned(), object.clone());
        self.record(store, target.resource, "MODIFIED", &object);
        Ok(object)
    }

    fn delete(
        &self,
        target: &Target,
        name: &str,
        options: &Value,
        dry_run: bool,
    ) -> Result<Value, Refusal> {
        let mut store = self.lock();
        let store = &mut *store;
        let stored = store
            .objects
            .get(target.resource)
            .and_then(|objects| objects.get(name))
            .ok_or_else(|| not_found(target.resource, name))?;
        if let Some(uid) = options["preconditions"]["uid"].as_str() {
            let held = stored["metadata"]["uid"].as_str().unwrap_or_default();
            if uid != held {
                return Err(Refusal::new(
                    409,
                    "Conflict",
                    format!(
                        "Precondition failed: UID in precondition: {uid}, UID in object meta: {held}"
                    ),
                ));
            }
        }
        let dry_run = dry_run
            || options["dryRun"]
                .as_array()
                .is_some_and(|modes| modes.iter().any(|mode| mode == "All"));
        if dry_run {
            return Ok(stored.clone());
        }
        let Some(mut object) = store
            .objects
            .get_mut(target.resource)
            .and_then(|objects| objects.remove(name))
        else {
            return Err(not_found(target.resource, name));
        };
        store.version += 1;
        object["metadata"]["resourceVersion"] = json!(store.version.to_string());
        object["metadata"]["deletionTimestamp"] =
            json!(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
        if let Some(grace) = options["gracePeriodSeconds"].as_u64() {
            object["metadata"]["deletionGracePeriodSeconds"] = json!(grace);
        }
        self.record(store, target.resource, "DELETED", &object);
        Ok(object)
    }

    /// Records one change for watches: in the history, and to every open watch.
    fn record(
        &self,
        store: &mut Store,
        resource: &'static str,
        kind: &'static str,
        object: &Value,
    ) {
        let stored = Stored {
            version: store.version,
            resource,
            kind,
            object: object.clone(),
        };
        store.history.push(stored.clone());
        let _ = self.events.send(stored);
    }

    /// A list, paged as the API pages one: every page is read at the version the first page
    /// answered (the continue token carries it), and a token whose version
    /// [`FakeCluster::compact`] passed answers 410 `Expired`.
    fn list(
        &self,
        target: &Target,
        first: &dyn Fn(&str) -> Option<String>,
        metadata: bool,
    ) -> Result<Value, Refusal> {
        let selector = Selector::parse(
            first("labelSelector").as_deref(),
            first("fieldSelector").as_deref(),
        )?;
        let limit = first("limit")
            .and_then(|limit| limit.parse::<usize>().ok())
            .filter(|limit| *limit > 0);
        let none = BTreeMap::new();
        let mut store = self.lock();
        let (version, after) = match first("continue") {
            None => (store.version, None),
            Some(token) => {
                let (version, after) = token
                    .split_once('/')
                    .and_then(|(version, after)| Some((version.parse::<u64>().ok()?, after)))
                    .ok_or_else(|| Refusal::new(400, "BadRequest", "continue key is not valid"))?;
                (version, Some(after.to_owned()))
            }
        };
        let objects = if after.is_some() {
            store
                .snapshots
                .get(&(target.resource, version))
                .filter(|_| version >= store.compacted)
                .ok_or_else(|| {
                    Refusal::new(
                        410,
                        "Expired",
                        "The provided continue parameter is too old to display a consistent list \
                         result. You can start a new list without the continue parameter.",
                    )
                })?
        } else {
            store.objects.get(target.resource).unwrap_or(&none)
        };
        let mut items: Vec<&Value> = objects
            .iter()
            .filter(|(name, _)| after.as_ref().is_none_or(|after| *name > after))
            .map(|(_, object)| object)
            .filter(|object| selector.matches(object))
            .collect();
        let mut next = None;
        if let Some(limit) = limit
            && items.len() > limit
        {
            items.truncate(limit);
            next = items
                .last()
                .and_then(|object| object["metadata"]["name"].as_str())
                .map(|name| format!("{version}/{name}"));
        }
        let snapshot = (after.is_none() && next.is_some()).then(|| objects.clone());
        let items: Vec<Value> = items
            .into_iter()
            .map(|object| {
                if metadata {
                    json!({
                        "kind": "PartialObjectMetadata",
                        "apiVersion": "meta.k8s.io/v1",
                        "metadata": object["metadata"],
                    })
                } else {
                    object.clone()
                }
            })
            .collect();
        if let Some(snapshot) = snapshot {
            store.snapshots.insert((target.resource, version), snapshot);
        }
        let mut list_metadata = json!({"resourceVersion": version.to_string()});
        if let Some(next) = next {
            list_metadata["continue"] = json!(next);
        }
        Ok(if metadata {
            json!({
                "kind": "PartialObjectMetadataList",
                "apiVersion": "meta.k8s.io/v1",
                "metadata": list_metadata,
                "items": items,
            })
        } else {
            json!({
                "kind": format!("{}List", target.kind),
                "apiVersion": target.api_version,
                "metadata": list_metadata,
                "items": items,
            })
        })
    }

    /// A chunked watch: an `ERROR` event with code 410 for a version older than the compacted
    /// history; otherwise every stored change after the version (or the present objects, for no
    /// version), then changes as they happen until `timeoutSeconds`.
    fn watch(&self, target: &Target, first: &dyn Fn(&str) -> Option<String>) -> Response<Body> {
        let selector = match Selector::parse(
            first("labelSelector").as_deref(),
            first("fieldSelector").as_deref(),
        ) {
            Ok(selector) => selector,
            Err(refusal) => return status(&refusal),
        };
        let timeout = first("timeoutSeconds")
            .and_then(|seconds| seconds.parse().ok())
            .map_or(MAX_WATCH, Duration::from_secs)
            .min(MAX_WATCH);
        let from = first("resourceVersion")
            .filter(|version| !version.is_empty() && version != "0")
            .map(|version| version.parse::<u64>().unwrap_or(0));
        let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(64);
        let (replay, mut updates) = {
            let store = self.lock();
            let updates = self.events.subscribe();
            let replay: Result<Vec<Value>, ()> = match from {
                Some(from) if from < store.compacted => Err(()),
                Some(from) => Ok(store
                    .history
                    .iter()
                    .filter(|stored| stored.version > from && stored.resource == target.resource)
                    .filter(|stored| selector.matches(&stored.object))
                    .map(|stored| json!({"type": stored.kind, "object": stored.object}))
                    .collect()),
                None => Ok(store
                    .objects
                    .get(target.resource)
                    .into_iter()
                    .flat_map(BTreeMap::values)
                    .filter(|object| selector.matches(object))
                    .map(|object| json!({"type": "ADDED", "object": object}))
                    .collect()),
            };
            (replay, updates)
        };
        let resource = target.resource;
        tokio::spawn(async move {
            let line = |event: &Value| {
                let mut line = serde_json::to_vec(event).unwrap_or_default();
                line.push(b'\n');
                Bytes::from(line)
            };
            let Ok(replay) = replay else {
                let _ = sender.send(line(&expired())).await;
                return;
            };
            for event in &replay {
                if sender.send(line(event)).await.is_err() {
                    return;
                }
            }
            let deadline = tokio::time::Instant::now() + timeout;
            loop {
                let stored = tokio::select! {
                    stored = next_change(&mut updates) => stored,
                    () = tokio::time::sleep_until(deadline) => return,
                };
                let stored = match stored {
                    Ok(stored) => stored,
                    Err(last) => {
                        if let Some(last) = last {
                            let _ = sender.send(line(&last)).await;
                        }
                        return;
                    }
                };
                if stored.resource != resource || !selector.matches(&stored.object) {
                    continue;
                }
                let event = json!({"type": stored.kind, "object": stored.object});
                if sender.send(line(&event)).await.is_err() {
                    return;
                }
            }
        });
        let mut response = Response::new(ChannelBody(receiver).boxed());
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        response
    }

    fn log(
        &self,
        name: &str,
        first: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Response<Body>, Refusal> {
        let store = self.lock();
        if !store
            .objects
            .get("pods")
            .is_some_and(|pods| pods.contains_key(name))
        {
            return Err(not_found("pods", name));
        }
        if let Some((status, message)) = &store.log_refusal {
            return Err(Refusal::new(*status, "Forbidden", message.clone()));
        }
        let mut log = store.logs.get(name).cloned().unwrap_or_default();
        if let Some(limit) = first("limitBytes").and_then(|limit| limit.parse::<usize>().ok()) {
            log.truncate(limit);
        }
        let mut response = Response::new(Full::new(Bytes::from(log)).boxed());
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        Ok(response)
    }

    /// Upgrades a `pods/exec` call to WebSocket `v5.channel.k8s.io`, reads its whole input until
    /// the client closes stdin, and answers what the installed [`ExecHandler`] returns.
    fn exec(
        self: &Arc<Self>,
        mut request: Request<Incoming>,
        pod: &str,
        query: &BTreeMap<String, Vec<String>>,
    ) -> Result<Response<Body>, Refusal> {
        if !self
            .lock()
            .objects
            .get("pods")
            .is_some_and(|pods| pods.contains_key(pod))
        {
            return Err(not_found("pods", pod));
        }
        let headers = request.headers();
        let offered = headers
            .get(header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(',').any(|offer| offer.trim() == EXEC_PROTOCOL));
        let key = headers.get(header::SEC_WEBSOCKET_KEY).cloned();
        let (true, Some(key)) = (offered, key) else {
            return Err(Refusal::new(
                400,
                "BadRequest",
                "pods/exec needs a WebSocket upgrade offering v5.channel.k8s.io",
            ));
        };
        let call = ExecRequest {
            pod: pod.to_owned(),
            container: query
                .get("container")
                .and_then(|values| values.first())
                .cloned()
                .unwrap_or_default(),
            command: query.get("command").cloned().unwrap_or_default(),
            stdin: Vec::new(),
        };
        let handler = self
            .exec
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let upgrade = hyper::upgrade::on(&mut request);
        tokio::spawn(answer_exec(upgrade, call, handler));
        let mut response = Response::new(Full::new(Bytes::new()).boxed());
        *response.status_mut() = hyper::StatusCode::SWITCHING_PROTOCOLS;
        let headers = response.headers_mut();
        headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(EXEC_PROTOCOL),
        );
        let accept = derive_accept_key(key.as_bytes());
        headers.insert(
            header::SEC_WEBSOCKET_ACCEPT,
            HeaderValue::from_str(&accept).map_err(|_| {
                Refusal::new(400, "BadRequest", "the WebSocket key is not a header value")
            })?,
        );
        Ok(response)
    }
}

/// The `ERROR` event a watch ends with when it cannot deliver every change after its version.
fn expired() -> Value {
    json!({"type": "ERROR", "object": {
        "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
        "message": "too old resource version", "reason": "Expired", "code": 410,
    }})
}

/// The next change for a watch; at the channel's end, the watch's last event, if it has one. A
/// watch that fell behind the channel has lost changes it cannot replay, so it ends 410.
async fn next_change(
    updates: &mut tokio::sync::broadcast::Receiver<Stored>,
) -> Result<Stored, Option<Value>> {
    use tokio::sync::broadcast::error::RecvError;
    match updates.recv().await {
        Ok(stored) => Ok(stored),
        Err(RecvError::Lagged(_)) => Err(Some(expired())),
        Err(RecvError::Closed) => Err(None),
    }
}

/// The server side of one upgraded `pods/exec`: reads stdin until the client closes it, runs the
/// installed handler off the runtime, and answers both streams and the final `Status`.
async fn answer_exec(
    upgrade: hyper::upgrade::OnUpgrade,
    mut call: ExecRequest,
    handler: Option<Arc<dyn ExecHandler>>,
) {
    let Ok(upgraded) = upgrade.await else {
        return;
    };
    let mut socket =
        WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
    while let Some(Ok(message)) = socket.next().await {
        let Message::Binary(data) = message else {
            if message.is_close() {
                break;
            }
            continue;
        };
        match data.split_first() {
            Some((0, input)) => call.stdin.extend_from_slice(input),
            Some((255, [0])) => break,
            _ => {}
        }
    }
    let answered = match handler {
        Some(handler) => tokio::task::spawn_blocking(move || handler.exec(call))
            .await
            .ok(),
        None => None,
    };
    let status = match &answered {
        Some(answer) if answer.exit_code == 0 => {
            json!({"metadata": {}, "status": "Success"})
        }
        Some(answer) => json!({
            "metadata": {}, "status": "Failure",
            "message": format!(
                "command terminated with non-zero exit code: exit status {}",
                answer.exit_code
            ),
            "reason": "NonZeroExitCode",
            "details": {"causes": [{
                "reason": "ExitCode", "message": answer.exit_code.to_string(),
            }]},
        }),
        None => json!({
            "metadata": {}, "status": "Failure",
            "message": "the fake serves no exec handler", "reason": "InternalError",
        }),
    };
    let framed = |channel: u8, payload: &[u8]| {
        let mut message = Vec::with_capacity(payload.len() + 1);
        message.push(channel);
        message.extend_from_slice(payload);
        Message::binary(message)
    };
    if let Some(answer) = &answered {
        for (channel, stream) in [(1, &answer.stdout), (2, &answer.stderr)] {
            for chunk in stream.chunks(32 * 1024) {
                if socket.feed(framed(channel, chunk)).await.is_err() {
                    return;
                }
            }
        }
    }
    let status = serde_json::to_vec(&status).unwrap_or_default();
    let _ = socket.send(framed(3, &status)).await;
    let _ = socket.close(None).await;
}

/// Equality label and field selectors.
struct Selector {
    labels: Vec<(String, String)>,
    name: Option<String>,
}

impl Selector {
    /// Reads `key=value` and `key==value` terms alone. Every other requirement (`!=`, `in`,
    /// `notin`, existence) is refused 400: the fake does not serve it, and reading it as an
    /// equality on another key would answer a list the API never would.
    fn parse(labels: Option<&str>, fields: Option<&str>) -> Result<Self, Refusal> {
        let key_char = |c: char| c.is_ascii_alphanumeric() || "-_./".contains(c);
        let value_char = |c: char| c.is_ascii_alphanumeric() || "-_.".contains(c);
        let pairs = |text: &str| -> Result<Vec<(String, String)>, Refusal> {
            text.split(',')
                .filter(|term| !term.is_empty())
                .map(|term| {
                    term.split_once("==")
                        .or_else(|| term.split_once('='))
                        .filter(|(key, value)| {
                            !key.is_empty()
                                && key.chars().all(key_char)
                                && value.chars().all(value_char)
                        })
                        .map(|(key, value)| (key.to_owned(), value.to_owned()))
                        .ok_or_else(|| {
                            Refusal::new(400, "BadRequest", format!("unread selector {term:?}"))
                        })
                })
                .collect()
        };
        let labels = labels.map(pairs).transpose()?.unwrap_or_default();
        let mut name = None;
        for (field, value) in fields.map(pairs).transpose()?.unwrap_or_default() {
            if field != "metadata.name" {
                return Err(Refusal::new(
                    400,
                    "BadRequest",
                    format!("the fake selects no field {field}"),
                ));
            }
            name = Some(value);
        }
        Ok(Self { labels, name })
    }

    fn matches(&self, object: &Value) -> bool {
        let metadata = &object["metadata"];
        self.name
            .as_ref()
            .is_none_or(|name| metadata["name"].as_str() == Some(name))
            && self
                .labels
                .iter()
                .all(|(key, value)| metadata["labels"][key].as_str() == Some(value))
    }
}

/// A body fed from a channel: a watch's lines as they happen.
struct ChannelBody(tokio::sync::mpsc::Receiver<Bytes>);

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.get_mut()
            .0
            .poll_recv(context)
            .map(|line| line.map(|line| Ok(Frame::data(line))))
    }
}

async fn read_json(request: Request<Incoming>) -> Result<Value, Refusal> {
    let bytes = read_body(request).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| Refusal::new(400, "BadRequest", "the body is not JSON"))
}

/// The name of the first container or init container of the Pod `object` that sets
/// `securityContext.privileged: true`.
fn privileged_container(object: &Value) -> Option<String> {
    ["initContainers", "containers"]
        .iter()
        .filter_map(|list| object["spec"][list].as_array())
        .flatten()
        .find(|container| container["securityContext"]["privileged"] == Value::Bool(true))
        .map(|container| container["name"].as_str().unwrap_or_default().to_owned())
}

/// A body that may be absent (a `DELETE`'s options): `null` for none, otherwise JSON. A body past
/// its bound or not JSON is refused; it is never read as no options at all.
async fn read_optional_json(request: Request<Incoming>) -> Result<Value, Refusal> {
    let bytes = read_body(request).await?;
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| Refusal::new(400, "BadRequest", "the body is not JSON"))
}

async fn read_body(request: Request<Incoming>) -> Result<Bytes, Refusal> {
    Ok(Limited::new(request.into_body(), MAX_REQUEST_BYTES)
        .collect()
        .await
        .map_err(|_| Refusal::new(413, "RequestEntityTooLarge", "the body is past its bound"))?
        .to_bytes())
}

fn not_found(resource: &str, name: &str) -> Refusal {
    Refusal::new(404, "NotFound", format!("{resource} \"{name}\" not found"))
}

fn answer(code: u16, body: &Value) -> Response<Body> {
    let mut response =
        Response::new(Full::new(Bytes::from(serde_json::to_vec(body).unwrap_or_default())).boxed());
    *response.status_mut() =
        hyper::StatusCode::from_u16(code).unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn status(refusal: &Refusal) -> Response<Body> {
    answer(
        refusal.code,
        &json!({
            "kind": "Status",
            "apiVersion": "v1",
            "metadata": {},
            "status": "Failure",
            "message": refusal.message,
            "reason": refusal.reason,
            "code": refusal.code,
        }),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Stored, next_change};

    /// A watch whose reader fell behind the event channel cannot deliver what it missed: it ends
    /// with an `ERROR` event of code 410, so the client lists again, rather than ending as a watch
    /// whose timeout came does.
    #[tokio::test]
    async fn a_watch_that_fell_behind_its_channel_ends_with_410() {
        let (events, mut updates) = tokio::sync::broadcast::channel(1);
        for version in 1..=2 {
            let change = Stored {
                version,
                resource: "pods",
                kind: "ADDED",
                object: json!({}),
            };
            events.send(change).expect("a receiver");
        }

        let last = next_change(&mut updates)
            .await
            .expect_err("the reader fell behind")
            .expect("a last event");
        assert_eq!(last["type"], "ERROR");
        assert_eq!(last["object"]["code"], 410);
        assert_eq!(last["object"]["reason"], "Expired");

        let (events, mut updates) = tokio::sync::broadcast::channel::<Stored>(1);
        drop(events);
        assert_eq!(
            next_change(&mut updates).await.err(),
            Some(None),
            "a closed channel ends the watch with no event"
        );
    }
}
