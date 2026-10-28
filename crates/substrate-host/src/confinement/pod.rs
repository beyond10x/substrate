//! The `pod` adapter: one Kubernetes Pod per run (ADR 0109 decision 3,
//! `docs/design/28-pod-confinement-adapter.md`).
//!
//! It renders an admitted profile into one Pod in the namespace the operator's cluster
//! configuration names, beside a default-deny `NetworkPolicy`, creates both through the cluster API,
//! polls the Pod's phase, reads its log once it ends and deletes the Pod, then the policy, before
//! `wait` reports the run ended. A Pod whose removal the API does not confirm keeps its policy,
//! and the run reads `Unknown` while its task keeps removing it; open removes a policy whose run's
//! Pod is gone. It serves `process_run`, `process_watch`, `output_read`, `wait`,
//! `workspace` (an `emptyDir` at `/workspace`), `limits` and `audit`, and proves
//! `image_digest_pinned`. It serves no `filesystem_view` (a Pod reaches no host path) and neither
//! network family: a `NetworkPolicy`'s enforcement depends on the cluster's network plugin, which
//! the API does not let a client prove.
//!
//! The adapter speaks to the API server itself over HTTP/1.1 and TLS with the workspace's own
//! `hyper` and `rustls`; it admits no Kubernetes client crate and runs no `kubectl`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::header::{self, HeaderValue};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use rustls::ClientConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use serde_json::{Map, Value, json};
use substrate_wire::Signal;
use tokio::sync::watch;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use super::local::LOCAL_POLICY_ENV;
use super::{
    AdapterFacts, AdapterName, AdmittedProfile, Command, ConfinementAdapter, ConfinementError,
    ConfinementProfile, ContractFamily, Fact, KeptOutput, ProcessBounds, RenderedCommand, RunExit,
    RunId, RunObservation, RunOutput, RunState, image_is_pinned,
};
use crate::DriverError;

/// The kubeconfig context extension that carries the operator's Pod settings.
pub const POD_SETTINGS_EXTENSION: &str = "b10x-substrate/pod";
/// The uid a run's container runs as when the operator names none.
pub const POD_DEFAULT_RUN_AS_USER: u32 = 65_532;
/// The largest output bound a profile may state, and the bound when it states none.
pub const POD_OUTPUT_LIMIT_BYTES: u64 = 1_048_576;
/// Runs one adapter tracks; past it, ended runs are forgotten oldest first.
pub const POD_MAX_TRACKED_RUNS: usize = 128;
/// The label every object the adapter creates carries, with [`MANAGED_BY`].
pub const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
pub const MANAGED_BY: &str = "b10x-substrate";
/// The label naming the run an object belongs to; its value is the run id.
pub const RUN_LABEL: &str = "b10x-substrate/run";
/// The one container of a run's Pod.
pub const CONTAINER: &str = "run";
/// Where the run's `emptyDir` workspace is mounted.
pub const WORKSPACE_MOUNT: &str = "/workspace";
/// The workspace's `sizeLimit` and the container's `ephemeral-storage` limit when the operator
/// states none.
pub const POD_DEFAULT_EPHEMERAL_STORAGE_BYTES: u64 = 1 << 30;
/// The Pod open asks for a log of, to learn whether the credential may read `pods/log`: no run is
/// ever named this, so an authorized read answers 404.
pub const LOG_ACCESS_PROBE: &str = "substrate-log-access-probe";

/// A `processes` bound the cluster does not hold: no `pod_pids_limit`, or one above the bound.
pub const POD_PROCESSES_UNSERVED: &str = "confinement.pod-processes-unserved";
/// A signal the cluster API cannot deliver.
pub const POD_SIGNAL_UNSERVED: &str = "confinement.pod-signal-unserved";
/// The cluster API refused or failed a request the run needed.
pub const POD_API_FAILED: &str = "confinement.pod-api-failed";

/// The linux driver's defaults, for a profile that states no process bounds.
const DEFAULT_BOUNDS: ProcessBounds = ProcessBounds {
    timeout_ms: 600_000,
    processes: 256,
    memory_bytes: 1 << 30,
    cpu_millis: 600_000,
};
const MAX_CLUSTER_FILE_BYTES: u64 = 1_048_576;
const MAX_API_BODY_BYTES: usize = 4 * 1_048_576;
const MAX_ARGV_BYTES: usize = 262_144;
/// What a request asks the API server for.
const JSON: &str = "application/json";
/// What a sweep's list asks for: a `PartialObjectMetadataList`, whose items carry each object's
/// `metadata` alone. The sweeps read only names, labels and creation times, and a full Pod of a run
/// the adapter admits holds up to 256 KiB of argv and 256 KiB of environment, so a page of
/// [`SWEEP_PAGE_ITEMS`] full Pods of ten such runs is past [`MAX_API_BODY_BYTES`].
const METADATA_LIST: &str = "application/json;as=PartialObjectMetadataList;g=meta.k8s.io;v=v1";
const API_TIMEOUT: Duration = Duration::from_secs(30);
/// What the open sweep adds to `start`'s two creates: whole-second `creationTimestamp` and `Date`
/// values, and skew between API server members, one stamping the policy and another answering.
const SWEEP_MARGIN: Duration = Duration::from_secs(30);
/// The age below which the open sweep keeps an owned policy whose Pod is absent: `start` creates
/// the policy and then the Pod, each call bounded by [`API_TIMEOUT`], and the API server stamps a
/// `creationTimestamp` before admission and storage, so a younger policy may be the only fence of a
/// run another adapter is still starting.
const SWEEP_AGE_FLOOR: Duration = API_TIMEOUT.saturating_mul(2).saturating_add(SWEEP_MARGIN);
/// The policies one page of the sweep's list asks for.
const SWEEP_PAGE_ITEMS: usize = 100;
/// The pages the sweep reads before it fails, and with it the open.
const SWEEP_MAX_PAGES: usize = 64;
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(500);
/// How long past its wall bound a Pod may stay unended before the adapter deletes it.
const DEADLINE_GRACE: Duration = Duration::from_mins(1);
/// How long the API may stay unreachable, or a Pod's removal unconfirmed, before a run is reported
/// `Unknown`.
const UNREACHABLE_LIMIT: Duration = Duration::from_mins(2);
/// How long after a deletion the adapter waits for the kubelet to confirm the container stopped
/// before it reports the run `Unknown`.
const STOP_CONFIRM: Duration = Duration::from_mins(1);
/// Polls of a Pod in a terminal phase whose container has no terminated state yet.
const TERMINAL_POLLS: u32 = 10;
const REMOVE_ATTEMPTS: u32 = 60;
/// How soon the adapter notices that a Pod it waits on is gone: the pause between two reads of a
/// Pod whose removal is not confirmed after `Unknown`, and between two rounds of the deferred sweep.
const NOTICE: Duration = Duration::from_secs(2);
/// How the adapter paces a removal: [`POLL`], [`REMOVE_ATTEMPTS`] and [`NOTICE`], which a unit
/// case shortens.
#[derive(Debug, Clone, Copy)]
struct Pacing {
    poll: Duration,
    remove_attempts: u32,
    notice: Duration,
}
const PACING: Pacing = Pacing {
    poll: POLL,
    remove_attempts: REMOVE_ATTEMPTS,
    notice: NOTICE,
};
/// Reads of a log before the adapter reports it unreadable.
const LOG_ATTEMPTS: u32 = 5;
const TERM_GRACE_SECONDS: u64 = 5;
/// The shortest grace the API leaves to the kubelet: 0 is a force deletion, which removes the
/// object without the kubelet confirming the container stopped.
const KILL_GRACE_SECONDS: u64 = 1;

/// What opening `pod` takes.
#[derive(Debug, Clone)]
pub struct PodRequest {
    /// The cluster configuration the operator provides, the JSON form of a kubeconfig (design 28);
    /// absent means no cluster.
    pub cluster: Option<PathBuf>,
}

/// What the operator's cluster configuration fixes for every run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodSettings {
    /// The current context's namespace: the only one the adapter touches.
    pub namespace: String,
    /// The numeric uid and gid the container runs as.
    pub run_as_user: u32,
    /// The kubelet's `podPidsLimit` on every node, as the operator states it. Without it no
    /// profile is admitted: every run has a processes bound, the default one included.
    pub pod_pids_limit: Option<u32>,
    /// The workspace `emptyDir`'s `sizeLimit` and the container's `ephemeral-storage` request and
    /// limit, in bytes.
    pub ephemeral_storage_bytes: u64,
}

/// `pod`'s declared set (`spec/confinement/confinement.yaml`, above `OpenAdapter`).
pub fn proof() -> AdapterFacts {
    AdapterFacts::new(
        AdapterName::Pod,
        None,
        &[
            (ContractFamily::ProcessRun, true),
            (ContractFamily::ProcessWatch, true),
            (ContractFamily::OutputRead, true),
            (ContractFamily::Wait, true),
            (ContractFamily::Workspace, true),
            (ContractFamily::Limits, true),
            (ContractFamily::Audit, true),
        ],
        &[(Fact::ImageDigestPinned, true)],
    )
}

pub(crate) fn open(request: PodRequest) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    Ok(open_pod(request)?)
}

/// Opens `pod` and returns the adapter itself, for [`PodAdapter::settings`].
/// `confinement::open` answers the same.
///
/// # Errors
///
/// `confinement.adapter-unserved` when there is no cluster configuration, it is unusable
/// (design 28 names each refused member), the cluster does not answer the probe, the credential
/// may not read `pods/log`, or the sweep of orphaned `NetworkPolicy` objects fails.
pub fn open_pod(request: PodRequest) -> Result<Arc<PodAdapter>, ConfinementError> {
    let unserved = |detail: String| ConfinementError::AdapterUnserved {
        adapter: AdapterName::Pod,
        detail,
    };
    let path = request
        .cluster
        .ok_or_else(|| unserved("no cluster configuration".to_owned()))?;
    let bytes = read_bounded(&path).map_err(unserved)?;
    let (api, settings) = parse_cluster(&bytes).map_err(unserved)?;
    let api = Arc::new(api);
    let kept = probe(&api).map_err(unserved)?;
    let adapter = Arc::new(PodAdapter {
        facts: proof(),
        api,
        settings,
        runs: Mutex::new(Vec::new()),
    });
    super::check_open(adapter.facts())?;
    if !kept.is_empty() {
        defer_sweep(&adapter.api, kept).map_err(|error| {
            unserved(format!(
                "the deferred sweep of the NetworkPolicies open kept cannot start: {error}"
            ))
        })?;
    }
    Ok(adapter)
}

fn read_bounded(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path)
        .map_err(|error| format!("the cluster configuration cannot be opened: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("the cluster configuration cannot be inspected: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_CLUSTER_FILE_BYTES {
        return Err("the cluster configuration must be one regular file of at most 1 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CLUSTER_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("the cluster configuration cannot be read: {error}"))?;
    Ok(bytes)
}

// --- the cluster configuration ---------------------------------------------------------------

fn named<'a>(config: &'a Value, list: &str, name: &str, member: &str) -> Result<&'a Value, String> {
    config[list]
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["name"] == name))
        .map(|entry| &entry[member])
        .filter(|value| value.is_object())
        .ok_or_else(|| format!("the current context names no {member} {name:?}"))
}

fn text<'a>(object: &'a Value, member: &str) -> Result<Option<&'a str>, String> {
    match &object[member] {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value)),
        _ => Err(format!("{member} is not a string")),
    }
}

fn decode(object: &Value, member: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    text(object, member)?
        .map(|data| {
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .map(Zeroizing::new)
                .map_err(|_| format!("{member} is not base64"))
        })
        .transpose()
}

/// A DNS-1123 label: what a namespace name must be.
fn is_dns_label(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
}

fn parse_cluster(bytes: &[u8]) -> Result<(Api, PodSettings), String> {
    let config: Value = serde_json::from_slice(bytes)
        .map_err(|_| "the cluster configuration is not the JSON form of a kubeconfig".to_owned())?;
    let current = config["current-context"]
        .as_str()
        .ok_or("the cluster configuration names no current context")?;
    let context = named(&config, "contexts", current, "context")?;
    let namespace = text(context, "namespace")?
        .ok_or("the current context names no namespace")?
        .to_owned();
    if !is_dns_label(&namespace) {
        return Err("the context namespace is not a DNS-1123 label".into());
    }
    let settings = settings(context, namespace)?;
    let cluster_name = text(context, "cluster")?.ok_or("the context names no cluster")?;
    let user_name = text(context, "user")?.ok_or("the context names no user")?;
    let cluster = named(&config, "clusters", cluster_name, "cluster")?;
    let user = named(&config, "users", user_name, "user")?;
    let api = Api::new(cluster, user, &settings.namespace)?;
    Ok((api, settings))
}

fn settings(context: &Value, namespace: String) -> Result<PodSettings, String> {
    let mut settings = PodSettings {
        namespace,
        run_as_user: POD_DEFAULT_RUN_AS_USER,
        pod_pids_limit: None,
        ephemeral_storage_bytes: POD_DEFAULT_EPHEMERAL_STORAGE_BYTES,
    };
    let Some(extension) = context["extensions"].as_array().and_then(|extensions| {
        extensions
            .iter()
            .find(|extension| extension["name"] == POD_SETTINGS_EXTENSION)
    }) else {
        return Ok(settings);
    };
    let members = extension["extension"]
        .as_object()
        .ok_or_else(|| format!("the {POD_SETTINGS_EXTENSION} extension is not an object"))?;
    let positive = |value: &Value, member: &str| {
        value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("{member} must be a positive 32-bit integer"))
    };
    for (member, value) in members {
        match member.as_str() {
            "run_as_user" => settings.run_as_user = positive(value, member)?,
            "pod_pids_limit" => settings.pod_pids_limit = Some(positive(value, member)?),
            "ephemeral_storage_bytes" => {
                settings.ephemeral_storage_bytes = value
                    .as_u64()
                    .filter(|bytes| (1_048_576..=1_099_511_627_776).contains(bytes))
                    .ok_or_else(|| format!("{member} must be an integer from 1 MiB to 1 TiB"))?;
            }
            other => {
                return Err(format!(
                    "the {POD_SETTINGS_EXTENSION} extension has {other:?}"
                ));
            }
        }
    }
    Ok(settings)
}

/// The kubeconfig members design 28 refuses, because each changes who the adapter is or whom it
/// trusts.
const REFUSED_CLUSTER_MEMBERS: [&str; 2] = ["proxy-url", "certificate-authority"];
const REFUSED_USER_MEMBERS: [&str; 11] = [
    "exec",
    "auth-provider",
    "username",
    "password",
    "tokenFile",
    "client-certificate",
    "client-key",
    "as",
    "as-uid",
    "as-groups",
    "as-user-extra",
];

/// The cluster API, reached as the operator's configuration says.
struct Api {
    host: String,
    port: u16,
    authority: String,
    base_path: String,
    server_name: ServerName<'static>,
    connector: TlsConnector,
    bearer: Option<Zeroizing<String>>,
    namespace: String,
    pacing: Pacing,
}

impl Api {
    fn new(cluster: &Value, user: &Value, namespace: &str) -> Result<Self, String> {
        if cluster["insecure-skip-tls-verify"] == json!(true) {
            return Err("insecure-skip-tls-verify is refused".into());
        }
        for member in REFUSED_CLUSTER_MEMBERS {
            if !cluster[member].is_null() {
                return Err(format!("the cluster member {member} is refused"));
            }
        }
        for member in REFUSED_USER_MEMBERS {
            if !user[member].is_null() {
                return Err(format!("the user member {member} is refused"));
            }
        }
        let server =
            url::Url::parse(text(cluster, "server")?.ok_or("the cluster names no server")?)
                .map_err(|_| "the cluster server is not a URL".to_owned())?;
        if server.scheme() != "https"
            || !server.username().is_empty()
            || server.password().is_some()
            || server.query().is_some()
            || server.fragment().is_some()
        {
            return Err("the cluster server must be a plain https URL".into());
        }
        let host = match server.host() {
            Some(url::Host::Domain(domain)) => domain.to_owned(),
            Some(url::Host::Ipv4(address)) => address.to_string(),
            Some(url::Host::Ipv6(address)) => address.to_string(),
            None => return Err("the cluster server names no host".into()),
        };
        let port = server
            .port_or_known_default()
            .ok_or("the cluster server names no port")?;
        let name = text(cluster, "tls-server-name")?.map_or_else(|| host.clone(), str::to_owned);
        let server_name = ServerName::try_from(name)
            .map_err(|_| "the cluster server name is not a DNS name or an address".to_owned())?;
        let authority = server[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
        let base_path = server.path().trim_end_matches('/').to_owned();
        let bearer = text(user, "token")?
            .filter(|token| !token.is_empty())
            .map(|token| Zeroizing::new(token.to_owned()));
        let connector = connector(cluster, user, bearer.is_some())?;
        Ok(Self {
            host,
            port,
            authority,
            base_path,
            server_name,
            connector,
            bearer,
            namespace: namespace.to_owned(),
            pacing: PACING,
        })
    }

    fn pods(&self) -> String {
        format!("/api/v1/namespaces/{}/pods", self.namespace)
    }

    fn policies(&self) -> String {
        format!(
            "/apis/networking.k8s.io/v1/namespaces/{}/networkpolicies",
            self.namespace
        )
    }

    /// One request on its own connection, bounded in time and in body size.
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        limit: usize,
    ) -> Result<(u16, Bytes), String> {
        let (status, _, bytes) = self.timed(method, path, body, JSON, limit).await?;
        Ok((status, bytes))
    }

    /// A GET of one page of the list at `path`, its items' `metadata` alone ([`METADATA_LIST`]),
    /// bounded at [`MAX_API_BODY_BYTES`], with the answer's `Date` header: the API server's clock.
    async fn list_metadata(&self, path: &str) -> Result<(u16, Option<String>, Bytes), String> {
        self.timed(Method::GET, path, None, METADATA_LIST, MAX_API_BODY_BYTES)
            .await
    }

    async fn timed(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        accept: &'static str,
        limit: usize,
    ) -> Result<(u16, Option<String>, Bytes), String> {
        tokio::time::timeout(
            API_TIMEOUT,
            self.exchange(method, path, body, accept, limit),
        )
        .await
        .map_err(|_| "the cluster API did not answer in time".to_owned())?
    }

    async fn exchange(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        accept: &'static str,
        limit: usize,
    ) -> Result<(u16, Option<String>, Bytes), String> {
        let tcp = tokio::net::TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|error| format!("connect: {error}"))?;
        let tls = self
            .connector
            .connect(self.server_name.clone(), tcp)
            .await
            .map_err(|error| format!("TLS: {error}"))?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(tls))
                .await
                .map_err(|error| format!("HTTP: {error}"))?;
        let driver = tokio::spawn(connection);
        let mut request = Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.base_path))
            .header(header::HOST, &self.authority)
            .header(header::ACCEPT, accept)
            .header(header::USER_AGENT, "b10x-substrate-pod");
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        if let Some(token) = &self.bearer {
            let line = Zeroizing::new(format!("Bearer {}", token.as_str()));
            let mut value = HeaderValue::from_str(&line)
                .map_err(|_| "the token is not a header value".to_owned())?;
            value.set_sensitive(true);
            request = request.header(header::AUTHORIZATION, value);
        }
        let payload = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|error| format!("encode: {error}"))?
            .unwrap_or_default();
        let request = request
            .body(Full::new(Bytes::from(payload)))
            .map_err(|error| format!("request: {error}"))?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|error| format!("HTTP: {error}"))?;
        let status = response.status().as_u16();
        let date = response
            .headers()
            .get(header::DATE)
            .and_then(|date| date.to_str().ok())
            .map(str::to_owned);
        let bytes = Limited::new(response.into_body(), limit)
            .collect()
            .await
            .map_err(|_| "the response body failed or exceeded its bound".to_owned())?
            .to_bytes();
        driver.abort();
        Ok((status, date, bytes))
    }

    /// Creates `object` in `collection` and returns the API's answer, `Null` when it is not JSON.
    async fn create(&self, collection: &str, object: &Value) -> Result<Value, String> {
        match self
            .call(Method::POST, collection, Some(object), MAX_API_BODY_BYTES)
            .await
        {
            Ok((200..=202, body)) => Ok(serde_json::from_slice(&body).unwrap_or(Value::Null)),
            Ok((status, body)) => Err(format!(
                "create answered {status}: {}",
                status_message(&body)
            )),
            Err(error) => Err(error),
        }
    }

    /// The object at `path`, or `None` once it is gone.
    async fn get(&self, path: &str) -> Result<Option<Value>, String> {
        match self
            .call(Method::GET, path, None, MAX_API_BODY_BYTES)
            .await?
        {
            (200, body) => serde_json::from_slice(&body)
                .map(Some)
                .map_err(|_| "the API answered something other than JSON".to_owned()),
            (404, _) => Ok(None),
            (status, body) => Err(format!("get answered {status}: {}", status_message(&body))),
        }
    }

    /// Asks the API to delete the object at `path`; `true` once it answers the object is gone.
    async fn delete(&self, path: &str, grace_seconds: u64) -> Result<bool, String> {
        let options = json!({
            "apiVersion": "v1",
            "kind": "DeleteOptions",
            "gracePeriodSeconds": grace_seconds,
            "propagationPolicy": "Background",
        });
        match self
            .call(Method::DELETE, path, Some(&options), MAX_API_BODY_BYTES)
            .await?
        {
            (404, _) => Ok(true),
            (200 | 202, _) => Ok(false),
            (status, body) => Err(format!(
                "delete answered {status}: {}",
                status_message(&body)
            )),
        }
    }

    /// Deletes the object at `path` with `grace_seconds` and polls until the API no longer has it.
    /// A Pod is deleted with a grace of at least one second, never forced, so its disappearance is
    /// the kubelet's confirmation that its container stopped.
    async fn remove(&self, path: &str, grace_seconds: u64) -> bool {
        for _ in 0..self.pacing.remove_attempts {
            if self.remove_once(path, grace_seconds).await {
                return true;
            }
            tokio::time::sleep(self.pacing.poll).await;
        }
        false
    }

    /// Deletes and reads the object at `path` until the API no longer has it, with no bound on the
    /// tries: the pause between two deletions doubles from one poll to one round of
    /// `remove_attempts` polls (500 ms to 30 s), and in that pause the object is read every
    /// `notice` (2 s), so its removal by the kubelet is seen within one `notice`, not at the next
    /// deletion. An object that never goes costs a read every 2 seconds and a deletion every 30.
    async fn remove_until_gone(&self, path: &str, grace_seconds: u64) {
        let ceiling = self.pacing.poll * self.pacing.remove_attempts;
        let mut pause = self.pacing.poll;
        while !self.remove_once(path, grace_seconds).await {
            let next = tokio::time::Instant::now() + pause;
            pause = (pause * 2).min(ceiling);
            loop {
                let wake = next.min(tokio::time::Instant::now() + self.pacing.notice);
                tokio::time::sleep_until(wake).await;
                if wake >= next {
                    break;
                }
                if matches!(self.get(path).await, Ok(None)) {
                    return;
                }
            }
        }
    }

    /// One deletion of the object at `path` and one read of it; `true` once it is gone.
    async fn remove_once(&self, path: &str, grace_seconds: u64) -> bool {
        matches!(self.delete(path, grace_seconds).await, Ok(true))
            || matches!(self.get(path).await, Ok(None))
    }

    /// The API's answer to a read of the container's log, at most `limit` bytes plus one.
    async fn log(&self, name: &str, limit: usize) -> Result<(u16, Bytes), String> {
        let path = format!(
            "{}/{name}/log?container={CONTAINER}&limitBytes={}",
            self.pods(),
            limit + 1
        );
        self.call(Method::GET, &path, None, limit.max(MAX_API_BODY_BYTES) + 1)
            .await
    }
}

fn connector(cluster: &Value, user: &Value, bearer: bool) -> Result<TlsConnector, String> {
    let authority = decode(cluster, "certificate-authority-data")?
        .ok_or("the cluster names no certificate-authority-data")?;
    let mut roots = rustls::RootCertStore::empty();
    for certificate in rustls_pemfile::certs(&mut authority.as_slice()) {
        roots
            .add(certificate.map_err(|_| "certificate-authority-data is not PEM".to_owned())?)
            .map_err(|_| "certificate-authority-data holds an invalid certificate".to_owned())?;
    }
    if roots.is_empty() {
        return Err("certificate-authority-data holds no certificate".into());
    }
    let builder = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| "no TLS protocol version is available".to_owned())?
    .with_root_certificates(roots);
    let certificate = decode(user, "client-certificate-data")?;
    let key = decode(user, "client-key-data")?;
    let mut config = match (certificate, key) {
        (Some(certificate), Some(key)) => {
            let chain: Vec<CertificateDer<'static>> =
                rustls_pemfile::certs(&mut certificate.as_slice())
                    .collect::<Result<_, _>>()
                    .map_err(|_| "client-certificate-data is not PEM".to_owned())?;
            let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key.as_slice())
                .ok()
                .flatten()
                .ok_or("client-key-data holds no private key")?;
            builder
                .with_client_auth_cert(chain, key)
                .map_err(|_| "the client certificate and key do not agree".to_owned())?
        }
        (None, None) if bearer => builder.with_no_client_auth(),
        (None, None) => {
            return Err("the user carries neither a client certificate nor a token".into());
        }
        _ => return Err("a client certificate needs both its certificate and its key".into()),
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsConnector::from(Arc::new(config)))
}

/// The API's own `Status.message`, bounded, or nothing.
fn status_message(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|status| status["message"].as_str().map(str::to_owned))
        .map(|message| message.chars().take(300).collect())
        .unwrap_or_default()
}

fn api_failed(detail: &str) -> ConfinementError {
    DriverError::failed(POD_API_FAILED, format!("pod: the cluster API: {detail}")).into()
}

/// Open's requests, on a thread of its own so `open` stays synchronous inside or outside a
/// runtime: a list of at most one Pod in the namespace, then a read of the log of a Pod no run is
/// named, which the API answers 404 only to a credential that may read `pods/log` and 403 to one
/// that may not. Without that access every run's output would be unreadable. Then [`sweep`], whose
/// kept policies it returns.
fn probe(api: &Arc<Api>) -> Result<Vec<Deferred>, String> {
    let api = Arc::clone(api);
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("the probe has no runtime: {error}"))?;
                runtime.block_on(async {
                    let api: &Api = &api;
                    let ask = |path: String| async move {
                        match tokio::time::timeout(
                            OPEN_TIMEOUT,
                            api.call(Method::GET, &path, None, MAX_API_BODY_BYTES),
                        )
                        .await
                        {
                            Ok(Ok(answer)) => Ok(answer),
                            Ok(Err(error)) => Err(format!("the cluster did not answer: {error}")),
                            Err(_) => {
                                Err("the cluster did not answer within 10 seconds".to_owned())
                            }
                        }
                    };
                    match ask(format!("{}?limit=1", api.pods())).await? {
                        (200, _) => {}
                        (status, body) => {
                            return Err(format!(
                                "the cluster answered the probe {status}: {}",
                                status_message(&body)
                            ));
                        }
                    }
                    let log = format!(
                        "{}/{LOG_ACCESS_PROBE}/log?container={CONTAINER}&limitBytes=1",
                        api.pods()
                    );
                    match ask(log).await? {
                        // Authorized: no such Pod, or (should one exist) its log or its refusal
                        // to name a container it does not have.
                        (200 | 400 | 404, _) => {}
                        (status @ (401 | 403), body) => {
                            return Err(format!(
                                "the credential may not read pods/log in the namespace, so no \
                                 run's output could be read ({status}: {})",
                                status_message(&body)
                            ));
                        }
                        (status, body) => {
                            return Err(format!(
                                "the cluster answered the pods/log probe {status}: {}",
                                status_message(&body)
                            ));
                        }
                    }
                    sweep(api).await.map_err(|error| {
                        format!("the open sweep of orphaned NetworkPolicies failed: {error}")
                    })
                })
            })
            .join()
            .map_err(|_| "the probe failed".to_owned())?
    })
}

/// The label selector of the objects the adapter owns, URL-encoded.
fn owner_selector() -> String {
    url::form_urlencoded::byte_serialize(format!("{MANAGED_BY_LABEL}={MANAGED_BY}").as_bytes())
        .collect()
}

/// Deletes the `NetworkPolicy` objects a process that ended before its runs' cleanup left behind
/// (`approval-record:pod-orphan-policy-sweep-d124`): each carrying the adapter's owner label, named
/// for the run that label names, at least [`SWEEP_AGE_FLOOR`] old by the API server's clock, whose
/// run's Pod the API no longer has, in any phase. A policy without the label is never a candidate.
/// The list is read whole first, in at most [`SWEEP_MAX_PAGES`] pages of [`SWEEP_PAGE_ITEMS`],
/// each policy's metadata alone ([`METADATA_LIST`]) and each page bounded in bytes; every request is bounded by [`API_TIMEOUT`] and there are at most two per
/// owned policy, so open stays bounded. Any failure, a list longer than that included, fails the
/// sweep, and with it the open.
///
/// Returns the owned policies it kept, younger than the floor or with their Pod present, each
/// marked with whether the API showed its Pod; [`defer_sweep`] checks them again.
async fn sweep(api: &Api) -> Result<Vec<Deferred>, String> {
    let first = format!(
        "{}?labelSelector={}&limit={SWEEP_PAGE_ITEMS}",
        api.policies(),
        owner_selector()
    );
    let mut candidates = Vec::new();
    let mut path = first.clone();
    let mut pages = 0;
    loop {
        if pages == SWEEP_MAX_PAGES {
            return Err(format!(
                "the policy list did not end within {SWEEP_MAX_PAGES} pages of {SWEEP_PAGE_ITEMS}"
            ));
        }
        pages += 1;
        let Some(next) = sweep_page(api, &path, &mut candidates).await? else {
            break;
        };
        let token: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
        path = format!("{first}&continue={token}");
    }
    let mut deferred = Vec::new();
    for (run, created, old) in candidates {
        let pod_seen = api.get(&format!("{}/{run}", api.pods())).await?.is_some();
        if !old || pod_seen {
            deferred.push(Deferred {
                run,
                created,
                pod_seen,
            });
            continue;
        }
        api.delete(&format!("{}/{run}", api.policies()), 0).await?;
        tracing::info!(
            policy = run,
            "pod: open removed a NetworkPolicy whose run's Pod is gone"
        );
    }
    Ok(deferred)
}

/// Reads one page of the sweep's list at `path` and adds its owned policies to `candidates`, each
/// with its creation time and whether it is old enough to be an orphan; the `continue` token of
/// the next page, or `None` on the last.
async fn sweep_page(
    api: &Api,
    path: &str,
    candidates: &mut Vec<(String, chrono::DateTime<chrono::FixedOffset>, bool)>,
) -> Result<Option<String>, String> {
    let (status, date, body) = api.list_metadata(path).await?;
    if status != 200 {
        return Err(format!(
            "the policy list answered {status}: {}",
            status_message(&body)
        ));
    }
    // The API server's clock, never this host's: a skewed host would read a young policy as old.
    let now = date.and_then(|date| chrono::DateTime::parse_from_rfc2822(&date).ok());
    let list: Value = serde_json::from_slice(&body)
        .map_err(|_| "the API answered something other than JSON".to_owned())?;
    let items = list["items"]
        .as_array()
        .ok_or("the policy list holds no items")?;
    for policy in items {
        let Some((run, created)) = owned(policy) else {
            continue;
        };
        let now = now.ok_or("the policy list carries no Date the adapter can read")?;
        let old = now
            .signed_duration_since(created)
            .to_std()
            .is_ok_and(|age| age >= SWEEP_AGE_FLOOR);
        candidates.push((run.to_owned(), created, old));
    }
    Ok(continuation(&list))
}

/// A list's `continue` token, or `None` on its last page.
fn continuation(list: &Value) -> Option<String> {
    list["metadata"]["continue"]
        .as_str()
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

/// An owned policy the open sweep kept: younger than [`SWEEP_AGE_FLOOR`], or its run's Pod exists.
#[derive(Debug, Clone)]
struct Deferred {
    run: String,
    created: chrono::DateTime<chrono::FixedOffset>,
    /// The API has shown the run's Pod: its start stored it and creates it no more, so once it is
    /// gone the policy fences nothing, whatever its age.
    pod_seen: bool,
}

/// Starts the deferred sweep of what open kept, on a thread of its own with its own runtime, so it
/// runs whether or not the embedder opened the adapter inside one: a round of [`resweep`] every
/// `notice` (2 s) until nothing is kept, or until the adapter and every task of its runs are gone.
/// One thread per open, one list per round, and at most what one sweep can list
/// ([`SWEEP_MAX_PAGES`] pages of [`SWEEP_PAGE_ITEMS`]) kept. A failed round is tried again at the
/// next. Returns once the thread has its runtime, or why it has none. The runtime is built on the
/// thread, never on the caller's, which may be inside a runtime that must not drop another.
fn defer_sweep(
    api: &Arc<Api>,
    mut deferred: Vec<Deferred>,
) -> Result<std::thread::JoinHandle<()>, String> {
    let api = Arc::downgrade(api);
    let (ready, started) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("substrate-pod-sweep".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = ready.send(Err(format!("no runtime: {error}")));
                    return;
                }
            };
            let _ = ready.send(Ok(()));
            runtime.block_on(async move {
                while !deferred.is_empty() {
                    let Some(notice) = api.upgrade().map(|api| api.pacing.notice) else {
                        return;
                    };
                    tokio::time::sleep(notice).await;
                    let Some(api) = api.upgrade() else {
                        return;
                    };
                    if let Err(error) = resweep(&api, &mut deferred).await {
                        tracing::warn!(
                            error,
                            "pod: a round of the deferred NetworkPolicy sweep failed"
                        );
                    }
                }
            });
        })
        .map_err(|error| format!("no thread: {error}"))?;
    started
        .recv()
        .map_err(|_| "the thread ended before its runtime was built".to_owned())??;
    Ok(thread)
}

/// One round of the deferred sweep. Reads the namespace's owned Pods whole, then, for each kept
/// policy whose Pod that list lacks and whose Pod the API showed before or which is at least
/// [`SWEEP_AGE_FLOOR`] old by the list's `Date`, reads the Pod once more and deletes the policy
/// when that read says the Pod is gone, as the open sweep does. A Pod listed or read marks its
/// policy seen. A list that fails fails the round before anything is deleted; a read or a deletion
/// that fails keeps its policy for the next round.
async fn resweep(api: &Api, deferred: &mut Vec<Deferred>) -> Result<(), String> {
    let (now, present) = owned_pods(api).await?;
    let mut index = 0;
    while let Some(policy) = deferred.get_mut(index) {
        if present.contains(&policy.run) {
            policy.pod_seen = true;
            index += 1;
            continue;
        }
        let old = now.is_some_and(|now| {
            now.signed_duration_since(policy.created)
                .to_std()
                .is_ok_and(|age| age >= SWEEP_AGE_FLOOR)
        });
        if !policy.pod_seen && !old {
            index += 1;
            continue;
        }
        match api.get(&format!("{}/{}", api.pods(), policy.run)).await {
            Ok(None) => {}
            Ok(Some(_)) => {
                policy.pod_seen = true;
                index += 1;
                continue;
            }
            Err(_) => {
                index += 1;
                continue;
            }
        }
        if api
            .delete(&format!("{}/{}", api.policies(), policy.run), 0)
            .await
            .is_err()
        {
            index += 1;
            continue;
        }
        tracing::info!(
            policy = policy.run,
            "pod: the adapter removed a NetworkPolicy its open kept, once its run's Pod was gone"
        );
        deferred.swap_remove(index);
    }
    Ok(())
}

/// The names of the namespace's Pods that carry the adapter's owner label, read whole in at most
/// [`SWEEP_MAX_PAGES`] pages of [`SWEEP_PAGE_ITEMS`], each Pod's metadata alone
/// ([`METADATA_LIST`]), and the API server's clock on the first page.
async fn owned_pods(
    api: &Api,
) -> Result<
    (
        Option<chrono::DateTime<chrono::FixedOffset>>,
        std::collections::BTreeSet<String>,
    ),
    String,
> {
    let first = format!(
        "{}?labelSelector={}&limit={SWEEP_PAGE_ITEMS}",
        api.pods(),
        owner_selector()
    );
    let mut path = first.clone();
    let mut now = None;
    let mut names = std::collections::BTreeSet::new();
    for page in 0..SWEEP_MAX_PAGES {
        let (status, date, body) = api.list_metadata(&path).await?;
        if status != 200 {
            return Err(format!(
                "the Pod list answered {status}: {}",
                status_message(&body)
            ));
        }
        if page == 0 {
            now = date.and_then(|date| chrono::DateTime::parse_from_rfc2822(&date).ok());
        }
        let list: Value = serde_json::from_slice(&body)
            .map_err(|_| "the API answered something other than JSON".to_owned())?;
        let items = list["items"]
            .as_array()
            .ok_or("the Pod list holds no items")?;
        names.extend(
            items
                .iter()
                .filter_map(|pod| pod["metadata"]["name"].as_str())
                .map(str::to_owned),
        );
        let Some(next) = continuation(&list) else {
            return Ok((now, names));
        };
        let token: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
        path = format!("{first}&continue={token}");
    }
    Err(format!(
        "the Pod list did not end within {SWEEP_MAX_PAGES} pages of {SWEEP_PAGE_ITEMS}"
    ))
}

/// The run a policy fences and when the API server created it, for a policy that carries the
/// adapter's owner label, whose name is a DNS label equal to its run label (the adapter names both
/// objects of a run for it) and whose creation time the sweep can read. Any other policy is never a
/// candidate: the sweep reads the Pod the label names and deletes the policy the name names, so the
/// two must be one.
fn owned(policy: &Value) -> Option<(&str, chrono::DateTime<chrono::FixedOffset>)> {
    let metadata = &policy["metadata"];
    if metadata["labels"][MANAGED_BY_LABEL] != MANAGED_BY {
        return None;
    }
    let run = metadata["labels"][RUN_LABEL]
        .as_str()
        .filter(|run| is_dns_label(run))?;
    if metadata["name"].as_str() != Some(run) {
        return None;
    }
    let created = metadata["creationTimestamp"]
        .as_str()
        .and_then(|created| chrono::DateTime::parse_from_rfc3339(created).ok())?;
    Some((run, created))
}

// --- rendering ------------------------------------------------------------------------------

/// The bounds a run is rendered with.
struct Resolved {
    deadline_seconds: u64,
    millicores: u64,
    memory_bytes: u64,
    output_limit: usize,
}

fn limit_unserved(detail: &str) -> ConfinementError {
    DriverError::exhausted(
        "exec.limit-unserved",
        format!("pod: {detail}"),
        "process_bounds",
    )
    .into()
}

fn resolve(
    settings: &PodSettings,
    profile: &ConfinementProfile,
) -> Result<Resolved, ConfinementError> {
    let bounds = profile.process_bounds.unwrap_or(DEFAULT_BOUNDS);
    if bounds.timeout_ms == 0
        || bounds.timeout_ms > 86_400_000
        || bounds.processes == 0
        || bounds.processes > 4096
        || bounds.memory_bytes < 1_048_576
        || bounds.memory_bytes > 1_099_511_627_776
        || bounds.cpu_millis == 0
        || bounds.cpu_millis > 86_400_000
    {
        return Err(limit_unserved(
            "a bound is outside the linux driver's ranges",
        ));
    }
    let deadline_seconds = bounds.timeout_ms.div_ceil(1000);
    // A rate that, held for the whole wall bound, spends at most `cpu_millis` of CPU time.
    let millicores = bounds.cpu_millis / deadline_seconds;
    if millicores == 0 {
        return Err(limit_unserved(
            "the CPU bound is under one millicore over the wall bound",
        ));
    }
    // Every run has a processes bound, the default one included, and the Pod API holds none of
    // its own: without the operator's `pod_pids_limit` no profile is admitted. A profile that
    // states no bound runs under the cluster's.
    let held = match (settings.pod_pids_limit, profile.process_bounds) {
        (None, _) => false,
        (Some(limit), Some(stated)) => limit <= stated.processes,
        (Some(_), None) => true,
    };
    if !held {
        return Err(DriverError::unserved(
            POD_PROCESSES_UNSERVED,
            "pod: the cluster holds no per-Pod process bound at or below this one",
            "process_bounds.processes",
        )
        .into());
    }
    let output = profile
        .io_bounds
        .map_or(POD_OUTPUT_LIMIT_BYTES, |io| io.output_bytes);
    if output == 0 || output > POD_OUTPUT_LIMIT_BYTES {
        return Err(limit_unserved(
            "the output bound is outside 1 byte to 1 MiB",
        ));
    }
    Ok(Resolved {
        deadline_seconds,
        millicores,
        memory_bytes: bounds.memory_bytes,
        output_limit: usize::try_from(output).unwrap_or(usize::MAX),
    })
}

/// The refusals designs 28 and 29 add to the contract's order, for fields a Pod cannot apply.
///
/// # Errors
///
/// `exec.workspace-access-invalid`, `confinement.writable-subtree-git` and
/// `exec.workspace-scoped-write-unserved` for a workspace access other than the whole workspace,
/// `confinement.output-retention-unserved` for a stream kept by its tail,
/// `confinement.family-unserved` for a host path or a network policy, `exec.secret-slots-unserved`,
/// `exec.workdir-invalid`, `exec.limit-unserved` and `confinement.pod-processes-unserved`.
pub fn check_profile(
    settings: &PodSettings,
    profile: &ConfinementProfile,
) -> Result<(), ConfinementError> {
    let family = |family| ConfinementError::FamilyUnserved {
        adapter: AdapterName::Pod,
        family,
    };
    // Its workspace is an `emptyDir` the adapter creates, so no part of it can be held read-only.
    super::refuse_workspace_access("pod", profile)?;
    // The log API bounds a read only by `limitBytes`, which keeps the first bytes; keeping the
    // last ones would mean reading a log whose size the adapter cannot bound (design 29 § 6).
    if !profile.output_retention.is_head() {
        return Err(DriverError::unserved(
            super::OUTPUT_RETENTION_UNSERVED,
            "The pod adapter keeps only the first bytes of each output stream.",
            "output_retention",
        )
        .into());
    }
    if profile.workspace.is_some()
        || !profile.writable_roots.is_empty()
        || !profile.read_only_roots.is_empty()
        || !profile.read_only_files.is_empty()
    {
        return Err(family(ContractFamily::FilesystemView));
    }
    if profile.network.is_some() {
        return Err(family(ContractFamily::NetworkEgress));
    }
    if !profile.secret_slots.is_empty() {
        return Err(DriverError::unserved(
            "exec.secret-slots-unserved",
            "Sealed secret slots are not served by the pod adapter.",
            "secret_slots",
        )
        .into());
    }
    if let Some(workdir) = &profile.workdir {
        super::relative_workdir(workdir)?;
    }
    resolve(settings, profile).map(|_| ())
}

fn check_pod_command(command: &Command) -> Result<(), ConfinementError> {
    super::check_command(command)?;
    if command.argv.iter().map(String::len).sum::<usize>() > MAX_ARGV_BYTES
        || command.argv.iter().any(|arg| arg.contains('\0'))
    {
        return Err(DriverError::refused(
            "exec.argv-invalid",
            "The argv exceeds 256 KiB or holds a NUL.",
            "argv",
        )
        .into());
    }
    // The portable shape every adapter holds a run's environment to, and the one variable only
    // the adapter sets.
    crate::environment::check(&command.env)?;
    if command.env.contains_key(LOCAL_POLICY_ENV) {
        return Err(crate::environment::refused().into());
    }
    Ok(())
}

fn metadata(settings: &PodSettings, run: &str) -> Value {
    json!({
        "name": run,
        "namespace": settings.namespace,
        "labels": {MANAGED_BY_LABEL: MANAGED_BY, RUN_LABEL: run},
    })
}

fn security_context(settings: &PodSettings, container: bool) -> Value {
    let mut context = json!({
        "runAsNonRoot": true,
        "runAsUser": settings.run_as_user,
        "runAsGroup": settings.run_as_user,
        "seccompProfile": {"type": "RuntimeDefault"},
    });
    if container {
        let members = context.as_object_mut().expect("an object");
        members.insert("allowPrivilegeEscalation".into(), json!(false));
        members.insert("privileged".into(), json!(false));
        members.insert("readOnlyRootFilesystem".into(), json!(true));
        members.insert("capabilities".into(), json!({"drop": ["ALL"]}));
    }
    context
}

/// `text` as the kubelet passes it on: it expands `$(NAME)` and turns `$$` into `$` in a
/// container's `command`, `args` and `env` values, so every `$` is written `$$`.
fn literal(text: &str) -> String {
    text.replace('$', "$$")
}

/// The `NetworkPolicy` and the Pod one run creates, in that order.
fn render_objects(
    settings: &PodSettings,
    profile: &ConfinementProfile,
    command: &Command,
    run: &str,
) -> Result<(Value, Value, Resolved), ConfinementError> {
    check_pod_command(command)?;
    let Some(image) = profile
        .image
        .as_deref()
        .filter(|image| image_is_pinned(image))
    else {
        return Err(ConfinementError::ImageUnpinned {
            adapter: AdapterName::Pod,
        });
    };
    check_profile(settings, profile)?;
    let resolved = resolve(settings, profile)?;
    let working = match &profile.workdir {
        Some(workdir) => {
            let relative = super::relative_workdir(workdir)?;
            let parts: Vec<String> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            format!("{WORKSPACE_MOUNT}/{}", parts.join("/"))
        }
        None => WORKSPACE_MOUNT.to_owned(),
    };
    let mut env: Vec<Value> = command
        .env
        .iter()
        .map(|(name, value)| json!({"name": name, "value": literal(value)}))
        .collect();
    // ADR 0109 F3 A: inside `pod` the host refuses `local`.
    env.push(json!({"name": LOCAL_POLICY_ENV, "value": "refuse"}));
    let args: Vec<String> = command.argv[1..].iter().map(|arg| literal(arg)).collect();
    let storage = settings.ephemeral_storage_bytes.to_string();
    let resources = json!({
        "cpu": format!("{}m", resolved.millicores),
        "memory": resolved.memory_bytes.to_string(),
        "ephemeral-storage": storage,
    });
    let policy = json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": metadata(settings, run),
        "spec": {
            "podSelector": {"matchLabels": {RUN_LABEL: run}},
            "policyTypes": ["Ingress", "Egress"],
        },
    });
    let pod = json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": metadata(settings, run),
        "spec": {
            "activeDeadlineSeconds": resolved.deadline_seconds,
            "automountServiceAccountToken": false,
            "enableServiceLinks": false,
            "hostIPC": false,
            "hostNetwork": false,
            "hostPID": false,
            "restartPolicy": "Never",
            "terminationGracePeriodSeconds": TERM_GRACE_SECONDS,
            "securityContext": security_context(settings, false),
            "volumes": [{"name": "workspace", "emptyDir": {"sizeLimit": storage}}],
            "containers": [{
                "name": CONTAINER,
                "image": image,
                "imagePullPolicy": "IfNotPresent",
                "command": [literal(&command.argv[0])],
                "args": args,
                "workingDir": working,
                "env": env,
                "resources": {"requests": resources, "limits": resources},
                "securityContext": security_context(settings, true),
                "volumeMounts": [{"name": "workspace", "mountPath": WORKSPACE_MOUNT}],
                "stdin": false,
                "tty": false,
            }],
        },
    });
    Ok((policy, pod, resolved))
}

/// The objects one run of `command` under `profile` creates, as a Kubernetes `List`: the
/// `NetworkPolicy`, then the Pod. Nothing is sent anywhere.
///
/// # Errors
///
/// Every refusal of design 28 that `start` would answer for this profile and command.
pub fn render_manifests(
    settings: &PodSettings,
    profile: &ConfinementProfile,
    command: &Command,
    run: &str,
) -> Result<Value, ConfinementError> {
    let (policy, pod, _) = render_objects(settings, profile, command, run)?;
    let mut list = Map::new();
    list.insert("apiVersion".into(), json!("v1"));
    list.insert("kind".into(), json!("List"));
    list.insert("items".into(), json!([policy, pod]));
    Ok(Value::Object(list))
}

// --- runs -----------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Phase {
    Running,
    /// The exit, and the run's OOM kills when the API showed the container's terminated state.
    Ended(RunExit, Option<u64>),
    Unknown,
}

#[derive(Default)]
struct Captured {
    stdout: Vec<u8>,
    truncated: bool,
    /// Final: read once the container ended or before the adapter deleted it, or never readable.
    complete: bool,
    /// Why the log could not be read; `output_read` reports it instead of an empty output.
    error: Option<String>,
}

/// A deletion of the run's Pod the adapter asked for: the signal the run ends with, and when.
#[derive(Debug, Clone, Copy)]
struct Stop {
    signal: Signal,
    at: Instant,
}

struct PodRun {
    id: RunId,
    phase: watch::Receiver<Phase>,
    output_limit: usize,
    captured: Mutex<Captured>,
    /// Whether the API has shown the container running or terminated.
    started: AtomicBool,
    stop: Mutex<Option<Stop>>,
    /// Whether the API has confirmed the run's Pod gone after the adapter's removal.
    pod_gone: AtomicBool,
}

/// The `pod` adapter, opened against one cluster and one namespace.
pub struct PodAdapter {
    facts: AdapterFacts,
    api: Arc<Api>,
    settings: PodSettings,
    runs: Mutex<Vec<Arc<PodRun>>>,
}

impl PodAdapter {
    /// What the operator's cluster configuration fixes for every run.
    pub fn settings(&self) -> &PodSettings {
        &self.settings
    }

    fn run(&self, id: &RunId) -> Result<Arc<PodRun>, ConfinementError> {
        self.runs
            .lock()
            .iter()
            .find(|run| &run.id == id)
            .cloned()
            .ok_or_else(super::unknown_run)
    }

    fn observation(run: &PodRun) -> RunObservation {
        let (state, exit, memory_oom_kills) = match *run.phase.borrow() {
            Phase::Running => (RunState::Running, None, None),
            Phase::Ended(exit, kills) => (RunState::Ended, Some(exit), kills),
            Phase::Unknown => (RunState::Unknown, None, None),
        };
        RunObservation {
            run: run.id.clone(),
            adapter: AdapterName::Pod,
            inner: None,
            declared_insecure: None,
            state,
            exit,
            memory_oom_kills,
        }
    }

    /// Makes room for one more run, forgetting runs that are over oldest first. A run read
    /// `Unknown` whose Pod's removal the API has not confirmed is not over: its task is still
    /// removing it, and the tasks never outnumber [`POD_MAX_TRACKED_RUNS`].
    fn reserve(&self) -> Result<(), ConfinementError> {
        let mut runs = self.runs.lock();
        while runs.len() >= POD_MAX_TRACKED_RUNS {
            let Some(ended) = runs.iter().position(|run| {
                !matches!(*run.phase.borrow(), Phase::Running)
                    && run.pod_gone.load(Ordering::SeqCst)
            }) else {
                return Err(DriverError::exhausted(
                    "exec.tracking-capacity",
                    "Every tracked pod run is still running or its Pod's removal is not confirmed.",
                    "run",
                )
                .into());
            };
            runs.remove(ended);
        }
        Ok(())
    }

    /// Every refusal and failure of a start, before the run exists.
    async fn start_run(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RunId, ConfinementError> {
        admitted.check_on(&self.facts, true)?;
        let name = format!(
            "substrate-run-{}",
            ulid::Ulid::generate().to_string().to_ascii_lowercase()
        );
        let (policy, pod, resolved) =
            render_objects(&self.settings, admitted.profile(), command, &name)?;
        self.reserve()?;
        let policy_path = format!("{}/{name}", self.api.policies());
        // The policy first, so the Pod is never without it.
        let created = match self.api.create(&self.api.policies(), &policy).await {
            Ok(created) => created,
            Err(error) => {
                // No Pod exists yet; a policy stored before its answer was lost is removed.
                self.api.remove(&policy_path, 0).await;
                return Err(api_failed(&error));
            }
        };
        let Some(uid) = created["metadata"]["uid"]
            .as_str()
            .filter(|uid| !uid.is_empty())
            .map(str::to_owned)
        else {
            self.api.remove(&policy_path, 0).await;
            return Err(api_failed("the NetworkPolicy create answered no uid"));
        };
        // The garbage collector removes a Pod whose owner is gone, so a Pod never outlives its
        // policy for long, whoever removed the policy. Not a controller and never blocking the
        // policy's deletion, which would need a further permission.
        let mut pod = pod;
        pod["metadata"]["ownerReferences"] = json!([{
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "name": name,
            "uid": uid,
            "controller": false,
        }]);
        if let Err(error) = self.api.create(&self.api.pods(), &pod).await {
            // An answer lost after the API stored the Pod looks like a refusal.
            return Err(self.abandon(&name, &error).await);
        }
        // Another adapter's open sweep may have removed the policy while the Pod's create was in
        // flight: a run starts only when its policy is read back after its Pod is stored.
        let unfenced = match self.api.get(&policy_path).await {
            Ok(Some(read)) if read["metadata"]["uid"].as_str() == Some(uid.as_str()) => None,
            Ok(Some(_)) => {
                Some("the run's NetworkPolicy was replaced before its Pod was stored".into())
            }
            Ok(None) => {
                Some("the run's NetworkPolicy was removed before its Pod was stored".into())
            }
            Err(error) => Some(format!(
                "the run's NetworkPolicy was not read back: {error}"
            )),
        };
        if let Some(detail) = unfenced {
            return Err(self.abandon(&name, &detail).await);
        }
        let id = RunId::new(name);
        let (sender, phase) = watch::channel(Phase::Running);
        let run = Arc::new(PodRun {
            id: id.clone(),
            phase,
            output_limit: resolved.output_limit,
            captured: Mutex::new(Captured::default()),
            started: AtomicBool::new(false),
            stop: Mutex::new(None),
            pod_gone: AtomicBool::new(false),
        });
        let wall = Duration::from_secs(resolved.deadline_seconds);
        tokio::spawn(monitor(
            Arc::clone(&self.api),
            Arc::clone(&run),
            sender,
            Instant::now() + wall + DEADLINE_GRACE,
        ));
        self.runs.lock().push(run);
        admitted.audit_start(&id, &command.argv[0]);
        Ok(id)
    }

    /// A start that fails once its Pod may be stored: deletes the Pod by name and only once it is
    /// confirmed gone its policy. The failure names the Pod when its removal was not confirmed.
    async fn abandon(&self, name: &str, error: &str) -> ConfinementError {
        let pod_path = format!("{}/{name}", self.api.pods());
        if !self.api.remove(&pod_path, KILL_GRACE_SECONDS).await {
            return api_failed(&format!(
                "{error}; the Pod {name} may have been stored and its removal was not confirmed, \
                 so its NetworkPolicy stays"
            ));
        }
        self.api
            .remove(&format!("{}/{name}", self.api.policies()), 0)
            .await;
        api_failed(error)
    }
}

/// Reads the container's log, retried. A log that cannot be read is an error, never an empty
/// output; only a container the API has not shown started has printed nothing yet.
async fn read_log(api: &Api, run: &PodRun) -> Result<(Vec<u8>, bool), String> {
    let mut failure = String::new();
    for attempt in 0..LOG_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(POLL).await;
        }
        match api.log(run.id.as_str(), run.output_limit).await {
            Ok((200, body)) => {
                let mut log = body.to_vec();
                let truncated = log.len() > run.output_limit;
                log.truncate(run.output_limit);
                return Ok((log, truncated));
            }
            // The kubelet keeps no log yet: the Pod has no node or its container waits to start.
            Ok((400, _)) if !run.started.load(Ordering::SeqCst) => {
                return Ok((Vec::new(), false));
            }
            Ok((status, body)) => {
                failure = format!("the log read answered {status}: {}", status_message(&body));
            }
            Err(error) => failure = error,
        }
    }
    Err(failure)
}

fn log_unread(detail: &str) -> ConfinementError {
    api_failed(&format!("the run's log cannot be read: {detail}"))
}

/// Reads the container's log once into `run`, unless it was read already; a failed read is kept
/// as the reason no output can be reported.
async fn capture(api: &Api, run: &PodRun) {
    if run.captured.lock().complete {
        return;
    }
    let read = read_log(api, run).await;
    let mut captured = run.captured.lock();
    if captured.complete {
        return;
    }
    match read {
        Ok((stdout, truncated)) => {
            *captured = Captured {
                stdout,
                truncated,
                complete: true,
                error: None,
            };
        }
        Err(error) => captured.error = Some(error),
    }
}

/// Makes the output final before the run is reported over: what was read, or why nothing was.
fn seal(run: &PodRun) {
    let mut captured = run.captured.lock();
    if !captured.complete {
        captured.complete = true;
        captured
            .error
            .get_or_insert_with(|| "the log was not read before the run was lost".to_owned());
    }
}

/// Asks the API to delete the run's Pod with `grace_seconds`, at least one: the kubelet stops the
/// container and only then does the Pod go. The stop is recorded first, so a Pod seen gone
/// afterwards is read as this deletion's.
async fn request_stop(
    api: &Api,
    run: &PodRun,
    signal: Signal,
    grace_seconds: u64,
) -> Result<(), String> {
    let first = {
        let mut stop = run.stop.lock();
        let first = stop.is_none();
        stop.get_or_insert(Stop {
            signal,
            at: Instant::now(),
        });
        first
    };
    let path = format!("{}/{}", api.pods(), run.id.as_str());
    match api.delete(&path, grace_seconds).await {
        Ok(_) => Ok(()),
        Err(error) => {
            if first {
                *run.stop.lock() = None;
            }
            Err(error)
        }
    }
}

fn container(pod: &Value) -> &Value {
    pod["status"]["containerStatuses"]
        .as_array()
        .and_then(|containers| containers.iter().find(|c| c["name"] == CONTAINER))
        .map_or(&Value::Null, |container| &container["state"])
}

fn terminal(pod: &Value) -> bool {
    matches!(
        pod["status"]["phase"].as_str(),
        Some("Succeeded" | "Failed")
    )
}

/// How the run's container ended, once the API shows it terminated: the exit and its OOM kills.
/// With `force`, a Pod in a terminal phase that never reports its container ends without either.
fn ended(pod: &Value, force: bool) -> Option<(RunExit, Option<u64>)> {
    let terminated = &container(pod)["terminated"];
    if !terminated.is_object() {
        return (force && terminal(pod)).then_some((
            RunExit {
                code: None,
                signal: None,
            },
            None,
        ));
    }
    let kills = u64::from(terminated["reason"] == "OOMKilled");
    let exit = RunExit {
        code: terminated["exitCode"]
            .as_i64()
            .and_then(|code| i32::try_from(code).ok()),
        signal: terminated["signal"]
            .as_i64()
            .filter(|signal| *signal != 0)
            .and_then(|signal| i32::try_from(signal).ok()),
    };
    Some((exit, Some(kills)))
}

/// Follows one run until the API shows its container ended, or the run is lost, then removes the
/// Pod and, once the Pod is confirmed gone, its policy.
async fn monitor(api: Arc<Api>, run: Arc<PodRun>, phase: watch::Sender<Phase>, give_up: Instant) {
    let name = run.id.as_str().to_owned();
    let pod_path = format!("{}/{name}", api.pods());
    let policy_path = format!("{}/{name}", api.policies());
    let mut answered = Instant::now();
    let mut terminal_polls = 0_u32;
    let outcome = loop {
        tokio::time::sleep(POLL).await;
        let answer = api.get(&pod_path).await;
        // Read after the answer: a stop recorded before its deletion is seen with the deletion.
        let stop = *run.stop.lock();
        let stopped = |exit: RunExit| {
            stop.map_or(exit, |stop| RunExit {
                code: None,
                signal: Some(stop.signal.number()),
            })
        };
        match answer {
            Ok(Some(pod)) => {
                answered = Instant::now();
                let state = container(&pod);
                if state["running"].is_object() || state["terminated"].is_object() {
                    run.started.store(true, Ordering::SeqCst);
                }
                if let Some((exit, kills)) = ended(&pod, terminal_polls >= TERMINAL_POLLS) {
                    capture(&api, &run).await;
                    break Phase::Ended(stopped(exit), kills);
                }
                if terminal(&pod) {
                    terminal_polls += 1;
                }
                match stop {
                    // The kubelet has not confirmed the container stopped: it may still run.
                    Some(stop) if stop.at.elapsed() >= STOP_CONFIRM => break Phase::Unknown,
                    None if Instant::now() >= give_up => {
                        capture(&api, &run).await;
                        // A failed deletion is asked for again at the next poll.
                        let _ = request_stop(&api, &run, Signal::Kill, KILL_GRACE_SECONDS).await;
                    }
                    _ => {}
                }
            }
            // Gone after the adapter's own graceful deletion: the kubelet confirmed the container
            // stopped. Gone otherwise: deleted by someone else, which the adapter cannot account for.
            Ok(None) => {
                break match stop {
                    Some(_) => Phase::Ended(
                        stopped(RunExit {
                            code: None,
                            signal: None,
                        }),
                        None,
                    ),
                    None => Phase::Unknown,
                };
            }
            Err(_) => {
                if answered.elapsed() >= UNREACHABLE_LIMIT {
                    break Phase::Unknown;
                }
            }
        }
    };
    if matches!(outcome, Phase::Unknown) {
        seal(&run);
        phase.send_replace(Phase::Unknown);
    }
    // The Pod first, never forced, and its policy only once the Pod is confirmed gone; a removal
    // not confirmed within the limit reports the run `Unknown` while the adapter keeps trying.
    let deadline = tokio::time::Instant::from_std(Instant::now() + UNREACHABLE_LIMIT);
    let removed = tokio::time::timeout_at(deadline, api.remove(&pod_path, KILL_GRACE_SECONDS))
        .await
        .unwrap_or(false);
    if removed {
        run.pod_gone.store(true, Ordering::SeqCst);
        let policy_removed = tokio::time::timeout_at(deadline, api.remove(&policy_path, 0))
            .await
            .unwrap_or(false);
        seal(&run);
        phase.send_replace(outcome);
        if !policy_removed {
            api.remove_until_gone(&policy_path, 0).await;
        }
        return;
    }
    seal(&run);
    phase.send_replace(Phase::Unknown);
    // A kubelet may stay away longer than any bound the adapter could pick: the task asks until the
    // API confirms the Pod gone, for as long as its runtime lives, and only then removes the policy.
    // Until then the run counts against `POD_MAX_TRACKED_RUNS`, so these tasks stay bounded.
    api.remove_until_gone(&pod_path, KILL_GRACE_SECONDS).await;
    run.pod_gone.store(true, Ordering::SeqCst);
    api.remove_until_gone(&policy_path, 0).await;
}

#[async_trait]
impl ConfinementAdapter for PodAdapter {
    fn facts(&self) -> &AdapterFacts {
        &self.facts
    }

    fn check(&self, profile: &ConfinementProfile) -> Result<(), ConfinementError> {
        check_profile(&self.settings, profile)
    }

    /// The program and arguments the container runs, after every refusal `start` would make. A
    /// refused render is audited as a refused start is.
    fn render(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RenderedCommand, ConfinementError> {
        let rendered = admitted.check_on(&self.facts, false).and_then(|()| {
            render_objects(
                &self.settings,
                admitted.profile(),
                command,
                "substrate-run-render",
            )
        });
        admitted.audit_refusal(
            &self.facts,
            rendered.map(|_| RenderedCommand {
                program: PathBuf::from(&command.argv[0]),
                args: command.argv[1..].to_vec(),
            }),
        )
    }

    /// Every refusal, from the contract's recheck to the cluster API's, writes its audit record.
    async fn start(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RunId, ConfinementError> {
        let started = self.start_run(admitted, command).await;
        admitted.audit_refusal(&self.facts, started)
    }

    async fn observe(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
        Ok(Self::observation(&*self.run(run)?))
    }

    /// The container's one log stream as `stdout`: the kubelet interleaves stdout and stderr, and
    /// the API returns them as one (design 28). A log the API does not return is
    /// `confinement.pod-api-failed`, never an empty output.
    async fn output(&self, run: &RunId) -> Result<RunOutput, ConfinementError> {
        let run = self.run(run)?;
        let stored = {
            let captured = run.captured.lock();
            captured.complete.then(|| match &captured.error {
                Some(error) => Err(error.clone()),
                None => Ok((captured.stdout.clone(), captured.truncated)),
            })
        };
        let read = match stored {
            Some(read) => read,
            None => read_log(&self.api, &run).await,
        };
        let (stdout, truncated) = read.map_err(|error| log_unread(&error))?;
        // The log API returns the first bytes up to the bound and says nothing of the rest, so
        // the stream's total is absent, never guessed (design 29 § 6).
        let stdout_kept = KeptOutput::unobserved_head(stdout.len());
        Ok(RunOutput {
            stdout,
            stderr: Vec::new(),
            stdout_truncated: truncated,
            stderr_truncated: false,
            stdout_kept,
            stderr_kept: KeptOutput::unobserved_head(0),
        })
    }

    async fn wait(&self, run: &RunId) -> Result<RunObservation, ConfinementError> {
        let run = self.run(run)?;
        let mut phase = run.phase.clone();
        // The monitor holds the sender until it has sent the end.
        let _ = phase
            .wait_for(|phase| !matches!(phase, Phase::Running))
            .await;
        Ok(Self::observation(&run))
    }

    /// `Kill` deletes the Pod with a grace of 1 second and `Term` with 5, so the kubelet sends
    /// `SIGTERM`, then `SIGKILL`; the API delivers no other signal. The run reads `Ended` with that
    /// signal once the kubelet confirms the container stopped, and `Unknown` if it does not.
    async fn signal(&self, run: &RunId, signal: Signal) -> Result<(), ConfinementError> {
        let run = self.run(run)?;
        let grace = match signal {
            Signal::Kill => KILL_GRACE_SECONDS,
            Signal::Term => TERM_GRACE_SECONDS,
            Signal::Int => {
                return Err(DriverError::unserved(
                    POD_SIGNAL_UNSERVED,
                    "pod: the cluster API delivers only termination, by deleting the Pod",
                    "signal",
                )
                .into());
            }
        };
        if !matches!(*run.phase.borrow(), Phase::Running) {
            return Ok(());
        }
        capture(&self.api, &run).await;
        request_stop(&self.api, &run, signal, grace)
            .await
            .map_err(|error| api_failed(&error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confinement::{OutputRetention, Retention, WorkspaceAccess};

    const IMAGE: &str = "registry.invalid/run@sha256:0000000000000000000000000000000000000000000000000000000000000000";

    fn settings() -> PodSettings {
        PodSettings {
            namespace: "substrate-pod-unit".to_owned(),
            run_as_user: POD_DEFAULT_RUN_AS_USER,
            pod_pids_limit: Some(1024),
            ephemeral_storage_bytes: POD_DEFAULT_EPHEMERAL_STORAGE_BYTES,
        }
    }

    fn profile() -> ConfinementProfile {
        ConfinementProfile {
            image: Some(IMAGE.to_owned()),
            ..ConfinementProfile::default()
        }
    }

    #[test]
    fn the_unit_profile_is_admitted() {
        check_profile(&settings(), &profile()).expect("admitted");
    }

    // A8: the workspace is an `emptyDir` the adapter creates, so a narrower one is refused.
    #[test]
    fn a_narrower_workspace_is_refused_scoped_write_unserved() {
        for access in [
            WorkspaceAccess::ReadOnly,
            WorkspaceAccess::Scoped {
                writable_subtrees: vec!["out".to_owned()],
            },
        ] {
            let error = check_profile(
                &settings(),
                &ConfinementProfile {
                    workspace_access: access.clone(),
                    ..profile()
                },
            )
            .expect_err("refused");
            assert_eq!(
                error.code(),
                substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED,
                "{access:?}: {error}"
            );
            assert_eq!(error.status(), 501);
        }
    }

    // B11: the log API keeps only the first bytes, so a tail mode on either stream is refused.
    #[test]
    fn a_tail_mode_on_either_stream_is_refused_output_retention_unserved() {
        for retention in [
            OutputRetention {
                stdout: Retention::Tail,
                stderr: Retention::Head,
            },
            OutputRetention {
                stdout: Retention::Head,
                stderr: Retention::HeadAndTail,
            },
            OutputRetention {
                stdout: Retention::HeadAndTail,
                stderr: Retention::Tail,
            },
        ] {
            let error = check_profile(
                &settings(),
                &ConfinementProfile {
                    output_retention: retention,
                    ..profile()
                },
            )
            .expect_err("refused");
            assert_eq!(
                error.code(),
                super::super::OUTPUT_RETENTION_UNSERVED,
                "{retention:?}: {error}"
            );
            assert_eq!(error.status(), 501);
        }
        check_profile(
            &settings(),
            &ConfinementProfile {
                output_retention: OutputRetention::default(),
                ..profile()
            },
        )
        .expect("head on both streams is served");
    }
}

#[cfg(test)]
#[path = "pod_cleanup_tests.rs"]
mod cleanup_tests;
