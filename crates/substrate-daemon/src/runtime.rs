#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read as _};
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::app::{
    ConnectionGauge, Listener, MaintenanceTask, Operations, RequestMetrics, development_router,
    hosted_router, is_capability_fact_name, record_request,
};
use crate::delegation::{DelegatedContextPolicy, TrustedKey};
use crate::hosted::{HostedAdmission, HostedIdentityConfig, require_hosted_authority};
use crate::tls::{TlsDaemonConfig, load_server_config};
use crate::{App, Identity, SystemAuthority, router};
use anyhow::{Context as _, anyhow, bail};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Router};
use ed25519_dalek::VerifyingKey;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use parking_lot::Mutex;
use substrate_host::{HostConfig, HostDriver};
use substrate_store::Store;
use subtle::ConstantTimeEq as _;
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_rustls::TlsAcceptor;
use tower::Layer as _;
use tracing::{info, warn};
use zeroize::Zeroizing;

use sha2::{Digest as _, Sha256};

#[derive(Debug, Clone, Copy)]
struct UnixTransportPolicy {
    global_connections: usize,
    connections_per_uid: usize,
    header_read_timeout: std::time::Duration,
    connection_lifetime: std::time::Duration,
    accept_retry_delay: std::time::Duration,
    max_buffer_bytes: usize,
    max_headers: usize,
    keep_alive: bool,
}

impl UnixTransportPolicy {
    const fn production() -> Self {
        Self {
            global_connections: 128,
            connections_per_uid: 32,
            header_read_timeout: std::time::Duration::from_secs(5),
            connection_lifetime: std::time::Duration::from_mins(5),
            accept_retry_delay: std::time::Duration::from_millis(25),
            max_buffer_bytes: 64 * 1024,
            max_headers: 64,
            // WebSocket session attachment requires HTTP/1.1 upgrade/keep-alive semantics.
            //
            // This lifetime is the finite bound for a peer that stays un-upgraded, idle or not.
            // It is not the bound on an upgraded one: hyper resolves the connection future when it
            // hands the socket to the upgrade, so the timeout around that future is already over
            // before the upgraded socket serves a byte. An upgraded peer is bounded by its own
            // stream's lifetime instead — one hour for an event stream, a metrics stream and a
            // pipe attachment — and by its stream's caps: the upgraded task hands this
            // connection's transport slot over to the stream permit it holds
            // (`hand_over_transport`, ADR 0042), so the transport budget counts connections that
            // have not upgraded and the published stream caps count the ones that have.
            // `app::tests::upgraded_transport_slot` measures both halves.
            keep_alive: true,
        }
    }
}

/// The transport admission one connection was accepted under, shared with everything that
/// connection is still serving until an upgrade hands it over (ADR 0042).
///
/// Every listener publishes a clone of this handle in the request extensions
/// (`admitted_service`), and `enforce_connection_lifetime` holds one for the connection future.
/// For a connection that never upgrades, the slot returns when the connection and every clone are
/// gone. For one that upgrades, the upgraded task calls [`hand_over_transport`] with the stream
/// permit it holds, and the slot returns then, whatever clones remain: from that point the socket
/// is counted by its stream's per-subject and deployment caps, not by the transport's.
/// `app::metrics::tests::every_websocket_upgrade_hands_its_transport_admission_to_its_stream` is
/// the check that a route added later cannot skip the hand-over.
#[derive(Clone)]
pub(crate) struct TransportPermit {
    slot: Arc<Mutex<Option<TransportSlot>>>,
}

/// The two admissions one connection holds together: the listener's global bound, and its scope's
/// — per uid on unix, per source address over TCP and TLS.
struct TransportSlot {
    _global: OwnedSemaphorePermit,
    _scope: OwnedSemaphorePermit,
}

impl TransportPermit {
    fn new(global: OwnedSemaphorePermit, scope: OwnedSemaphorePermit) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Some(TransportSlot {
                _global: global,
                _scope: scope,
            }))),
        }
    }
}

/// A stream family's own admission, held for as long as an upgraded socket serves: an event or
/// metrics stream permit, or a session attachment permit. Each is counted by a per-subject (or
/// per-session) cap and a deployment-wide cap, which the capability snapshot publishes
/// (`events.stream-bounds`, `metrics.stream-bounds`, `sessions.attachment-bounds`).
pub(crate) trait StreamSlot {}

/// Hands an upgraded connection's transport admission over to the stream it became (ADR 0042).
///
/// Releases the transport slot now, even while clones of the admission remain in request
/// extensions or in `enforce_connection_lifetime`. It borrows the stream permit the caller holds,
/// so it cannot be called before a stream slot is taken: the socket is counted by the stream's
/// caps before the transport stops counting it, and at no instant by neither. Called first in
/// every upgraded task, which runs only after the `101`; a request that fails to upgrade never
/// reaches it, and its connection keeps the transport slot until it ends. `None` — a route driven
/// without a transport, as the crate's own tests do — hands over nothing.
pub(crate) fn hand_over_transport<S: StreamSlot>(
    transport_admission: Option<TransportPermit>,
    _stream: &S,
) {
    if let Some(admission) = transport_admission {
        drop(admission.slot.lock().take());
    }
}

struct ConnectionLimits {
    global: Arc<Semaphore>,
    global_limit: usize,
    by_uid: BTreeMap<u32, Arc<Semaphore>>,
    /// Admissions refused at capacity, global or per uid (ADR 0045); an unmapped uid is not one.
    rejected: Arc<AtomicU64>,
}

impl ConnectionLimits {
    fn new(allowed_uids: &BTreeSet<u32>, policy: UnixTransportPolicy) -> Self {
        assert!(
            policy.global_connections > 0,
            "global connection limit must be nonzero"
        );
        assert!(
            policy.connections_per_uid > 0,
            "per-UID connection limit must be nonzero"
        );
        let by_uid = allowed_uids
            .iter()
            .copied()
            .map(|uid| (uid, Arc::new(Semaphore::new(policy.connections_per_uid))))
            .collect();
        Self {
            global: Arc::new(Semaphore::new(policy.global_connections)),
            global_limit: policy.global_connections,
            by_uid,
            rejected: Arc::new(AtomicU64::new(0)),
        }
    }

    fn acquire(&self, uid: u32) -> Option<TransportPermit> {
        let subject_limit = Arc::clone(self.by_uid.get(&uid)?);
        let permit = Arc::clone(&self.global)
            .try_acquire_owned()
            .ok()
            .and_then(|global| {
                let subject = subject_limit.try_acquire_owned().ok()?;
                Some(TransportPermit::new(global, subject))
            });
        if permit.is_none() {
            self.rejected.fetch_add(1, Ordering::Relaxed);
        }
        permit
    }

    fn gauge(&self) -> ConnectionGauge {
        ConnectionGauge {
            global: Arc::clone(&self.global),
            limit: self.global_limit,
            rejected: Arc::clone(&self.rejected),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PeerIdentity {
    uid: u32,
    pid: Option<u32>,
}

trait PeerCredentialSource {
    fn peer_identity(&self) -> io::Result<PeerIdentity>;
}

impl PeerCredentialSource for UnixStream {
    fn peer_identity(&self) -> io::Result<PeerIdentity> {
        let credentials = self.peer_cred()?;
        Ok(PeerIdentity {
            uid: credentials.uid(),
            pid: credentials.pid().and_then(|pid| u32::try_from(pid).ok()),
        })
    }
}

#[async_trait::async_trait]
trait ConnectionSource {
    type Stream: PeerCredentialSource + Send + 'static;

    async fn accept_stream(&self) -> io::Result<Self::Stream>;
}

#[async_trait::async_trait]
impl ConnectionSource for UnixListener {
    type Stream = UnixStream;

    async fn accept_stream(&self) -> io::Result<Self::Stream> {
        self.accept().await.map(|(stream, _address)| stream)
    }
}

struct AcceptedConnection<S> {
    stream: S,
    peer: PeerIdentity,
    permit: TransportPermit,
}

async fn accept_authorized<S: ConnectionSource>(
    source: &S,
    allowed: &BTreeSet<u32>,
    limits: &ConnectionLimits,
    retry_delay: std::time::Duration,
) -> AcceptedConnection<S::Stream> {
    loop {
        let stream = match source.accept_stream().await {
            Ok(stream) => stream,
            Err(error) => {
                warn!(%error, "transient unix accept failure");
                tokio::time::sleep(retry_delay).await;
                continue;
            }
        };
        let peer = match stream.peer_identity() {
            Ok(peer) => peer,
            Err(error) => {
                warn!(%error, "refused unix peer without credentials");
                continue;
            }
        };
        if !allowed.contains(&peer.uid) {
            warn!(uid = peer.uid, "refused unmapped unix peer");
            continue;
        }
        let Some(permit) = limits.acquire(peer.uid) else {
            warn!(uid = peer.uid, "refused unix peer at connection capacity");
            continue;
        };
        return AcceptedConnection {
            stream,
            peer,
            permit,
        };
    }
}

fn http1_builder(policy: UnixTransportPolicy) -> http1::Builder {
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(policy.header_read_timeout)
        .keep_alive(policy.keep_alive)
        .max_buf_size(policy.max_buffer_bytes)
        .max_headers(policy.max_headers);
    builder
}

/// One connection's service, carrying the transport admission that connection was accepted under.
///
/// Every request the connection makes arrives with a clone of the admission, which is how the task
/// an upgrade hands the socket to gets one to hand over. hyper resolves an upgradeable connection
/// future when it hands that socket over, so the upgraded task, not the connection, decides when
/// the slot returns: at the hand-over to its stream permit (`hand_over_transport`, ADR 0042). The
/// transport budget — 128 global and 32 per uid on unix, 128 and 16 per source address over TCP
/// and TLS — therefore counts every connection that has not upgraded.
///
/// Every `.serve_connection(` under `src/` that goes on to `.with_upgrades()` is served through
/// here — the three production listeners, and the three in-crate fixtures that stand in for one:
/// the unix fixture in this file's tests, the metrics stream harness, and the admitted TCP
/// listener in `app::tests::upgraded_transport_slot` — and
/// `app::metrics::tests::every_upgradeable_connection_publishes_its_transport_admission` reads
/// exactly that set. It is not every listener in the crate's test suite: three under `tests/`
/// serve the same production router with no transport budget at all, which that check names and
/// does not cover.
pub(crate) fn admitted_service<S>(
    permit: &TransportPermit,
    service: S,
) -> middleware::AddExtension<S, TransportPermit> {
    Extension(permit.clone()).layer(service)
}

/// Holds one connection's admission and bounds the connection future, and neither outlives it.
///
/// `pub(crate)` so the fixtures that stand in for a listener drive this rather than a copy of it:
/// a change to how the deadline and the admission meet must reach
/// `app::tests::upgraded_transport_slot`, which measures the production composition.
///
/// The deadline is on the future passed in. hyper resolves an upgradeable connection future when
/// it hands the socket to the upgrade, so a connection that upgrades leaves this bound behind at
/// the handshake and is bounded by its stream's own lifetime from there. Its transport admission
/// is released by the upgraded task's `hand_over_transport` once the stream permit holds the
/// socket (ADR 0042, `story:transport-admission-and-stream-lifetime-disagree`), which is what
/// `UnixTransportPolicy::production` records.
pub(crate) async fn enforce_connection_lifetime<P, F, T, E>(
    permit: P,
    lifetime: std::time::Duration,
    connection: F,
) -> Result<Result<T, E>, tokio::time::error::Elapsed>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    let _permit = permit;
    tokio::time::timeout(lifetime, connection).await
}

pub(crate) struct TcpConnectionLimits {
    global: Arc<Semaphore>,
    global_limit: usize,
    by_source: Mutex<BTreeMap<IpAddr, TcpSourceLimit>>,
    per_source: usize,
    max_sources: usize,
    sequence: AtomicU64,
    /// Admissions refused at capacity: global, per source, or a full source table (ADR 0045).
    rejected: Arc<AtomicU64>,
}

struct TcpSourceLimit {
    semaphore: Arc<Semaphore>,
    last_used: u64,
}

impl TcpConnectionLimits {
    pub(crate) fn production() -> Self {
        Self {
            global: Arc::new(Semaphore::new(128)),
            global_limit: 128,
            by_source: Mutex::new(BTreeMap::new()),
            per_source: 16,
            max_sources: 1_024,
            sequence: AtomicU64::new(1),
            rejected: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The operational listener's bound (ADR 0045 decision 1): eight connections at once, at most
    /// four of them from one source address, so one source cannot hold every slot.
    fn operational() -> Self {
        Self {
            global: Arc::new(Semaphore::new(OPS_MAX_CONNECTIONS)),
            global_limit: OPS_MAX_CONNECTIONS,
            by_source: Mutex::new(BTreeMap::new()),
            per_source: OPS_MAX_CONNECTIONS_PER_SOURCE,
            max_sources: OPS_MAX_CONNECTIONS,
            sequence: AtomicU64::new(1),
            rejected: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn gauge(&self) -> ConnectionGauge {
        ConnectionGauge {
            global: Arc::clone(&self.global),
            limit: self.global_limit,
            rejected: Arc::clone(&self.rejected),
        }
    }

    pub(crate) fn acquire(&self, source: IpAddr) -> Option<TransportPermit> {
        let permit = self.admit(source);
        if permit.is_none() {
            self.rejected.fetch_add(1, Ordering::Relaxed);
        }
        permit
    }

    fn admit(&self, source: IpAddr) -> Option<TransportPermit> {
        let source_limit = {
            let mut sources = self.by_source.lock();
            let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
            if let Some(limit) = sources.get_mut(&source) {
                limit.last_used = sequence;
                Arc::clone(&limit.semaphore)
            } else {
                if sources.len() >= self.max_sources {
                    let idle = sources
                        .iter()
                        .filter(|(_, entry)| {
                            entry.semaphore.available_permits() == self.per_source
                                && Arc::strong_count(&entry.semaphore) == 1
                        })
                        .min_by_key(|(_, entry)| entry.last_used)
                        .map(|(address, _)| *address);
                    sources.remove(&idle?);
                }
                let limit = Arc::new(Semaphore::new(self.per_source));
                sources.insert(
                    source,
                    TcpSourceLimit {
                        semaphore: Arc::clone(&limit),
                        last_used: sequence,
                    },
                );
                limit
            }
        };
        let global = Arc::clone(&self.global).try_acquire_owned().ok()?;
        let source = source_limit.try_acquire_owned().ok()?;
        Some(TransportPermit::new(global, source))
    }
}

struct InstanceLock {
    path: PathBuf,
    _file: nix::fcntl::Flock<std::fs::File>,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Complete local composition for one standalone Substrate daemon process.
///
/// Callers may construct this value and start a daemon, but all execution still crosses the
/// daemon's authenticated Unix-socket wire boundary. This API does not expose the host driver.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub socket: PathBuf,
    pub state: PathBuf,
    pub workspaces: PathBuf,
    pub deployment: String,
    pub allow_uids: Vec<u32>,
    pub allow_unrecorded_sessions: bool,
    pub cgroup_root: Option<PathBuf>,
    pub project_quota_ids: Option<(u32, u32)>,
    /// The ceiling on one exec's CPU bandwidth, in whole cores (`HostConfig::exec_cpu_cores`).
    ///
    /// Default 1. Opening the daemon refuses 0 or more cores than this process may use.
    pub exec_cpu_cores: u32,
    /// Operator-declared Git source URL prefixes. Credentials are never configuration.
    pub git_sources: Vec<GitSourceConfig>,
    pub bubblewrap: PathBuf,
    pub event_retention: u64,
    /// Operator-declared secret slots (ADR 0012), each a name and a bounded owner-private file.
    ///
    /// Never request data. An empty list means the `secrets.slots` capability is absent and a start
    /// naming a slot is `unserved` — there is no weaker delivery to fall back to.
    pub secret_slots: Vec<SecretSlot>,
    /// Operator-declared egress apertures (ADR 0013), each one destination tuple.
    ///
    /// Never request data. An empty list means the `exec.egress-apertures` capability is absent and
    /// a start naming an aperture is `unserved`; the sandbox keeps `--unshare-net` and there is no
    /// weaker reach to fall back to.
    pub egress_apertures: Vec<EgressAperture>,
    /// Operator-declared writable host roots (ADR 0038 kind 1), each a name and a host directory.
    ///
    /// Never request data: a start names a root and a mount, never a path. `serve` refuses, naming
    /// the root, any declaration that is not an existing owned directory clear of the workspace
    /// and state roots. An empty list means `exec.writable-roots.host` is absent and a start naming
    /// a host root is `unserved`.
    pub writable_roots: Vec<WritableRootConfig>,
    /// The certificate bundle a run with an aperture gets a private read-only snapshot of.
    ///
    /// `None` gives a sandbox no trust anchor. TLS still crosses the aperture byte for byte, and a
    /// child that verifies will refuse rather than trust something it cannot check.
    pub ca_bundle: Option<PathBuf>,
    /// Operator-declared trusted keys for delegated context (ADR 0011).
    ///
    /// Verifying material only, resolved once at startup. An empty list is a real posture, not a
    /// gap: no context can be verified, so presenting one is a named refusal and omitting one is
    /// the operation 0.6.0 already served.
    pub delegated_context_keys: Vec<DelegatedContextKey>,
    /// Whether an effectful operation must present a verified delegated context.
    ///
    /// Startup refuses `true` with no key: a deployment that requires what it can never verify
    /// refuses every mutation, and that is a configuration mistake, not a runtime one.
    pub require_delegated_context: bool,
    pub tcp: Option<TcpDaemonConfig>,
    /// Production HTTPS/WSS listener. It is distinct from development static-bearer TCP.
    pub tls: Option<TlsDaemonConfig>,
    /// Identity authority resolved for every production TLS request (ADR 0026).
    pub hosted_identity: Option<HostedIdentityConfig>,
    /// The operational listener (ADR 0045): liveness, readiness and metrics, off when `None`.
    pub ops: Option<OpsListenerConfig>,
}

/// The operational listener: plaintext, unauthenticated, `GET /livez`, `/readyz` and `/metrics`
/// only, on an address of its own (ADR 0045).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpsListenerConfig {
    pub listen: SocketAddr,
    /// Capability facts readiness requires the startup snapshot to publish (decision 5).
    pub require_facts: Vec<String>,
}

/// One operator-declared trusted key for delegated context: a `kid`, its issuer, public material.
///
/// The daemon's own configuration vocabulary. Which service signs is exactly this declaration and
/// nothing else in the codebase (ADR 0011), so identity-signed and connectors-signed differ here
/// and nowhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedContextKey {
    pub kid: String,
    pub issuer: String,
    /// A 32-byte Ed25519 verifying key. Never a seed and never a signing key: substrate mints no
    /// delegated context and holds nothing that could.
    pub public_key: [u8; 32],
}

/// One operator-declared egress aperture: a name and one destination tuple (ADR 0013).
///
/// The daemon's own configuration vocabulary, not the driver's — invariant 4 is why it is declared
/// here rather than re-exported from `substrate_host`. `serve` resolves the host once and converts
/// it at the composition root, and nowhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressAperture {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// The declared byte ceiling over both directions summed, per run (ADR 0014).
    ///
    /// The daemon's vocabulary again, and again not the driver's: `serve` carries it across at the
    /// composition root with the rest of the declaration. `None` is what every aperture declared
    /// before ADR 0014 is, and installs exactly what it installed then.
    pub max_bytes: Option<u64>,
}

/// One operator-declared writable host root: a name in the root-name set and the absolute host
/// directory behind it (ADR 0038 decision 2).
///
/// The daemon's own configuration vocabulary, converted at the composition root as a secret slot
/// is (invariant 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritableRootConfig {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSourceConfig {
    pub name: String,
    pub base_url: String,
}

/// One operator-declared secret slot: a name, and the file behind it (ADR 0012).
///
/// The daemon's own configuration vocabulary, not the driver's. Invariant 4 is why it is declared
/// here rather than re-exported from `substrate_host`: a second driver written against the port
/// would otherwise find the daemon already shaped around the first one's types. `serve` converts it
/// at the composition root, and nowhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretSlot {
    pub name: String,
    pub path: PathBuf,
}

/// Authenticated TCP listener for hosted/private-overlay composition.
#[derive(Debug, Clone)]
pub struct TcpDaemonConfig {
    pub listen: SocketAddr,
    pub path_prefix: String,
    pub bearer_file: PathBuf,
    pub subject: String,
    pub actor: String,
    pub private_overlay: bool,
    /// Explicit acknowledgement that the static-bearer transport is not the accepted hosted
    /// trust-envelope profile and must not be exposed as a production control surface.
    pub development_only: bool,
}

impl DaemonConfig {
    pub fn minimum(
        socket: impl Into<PathBuf>,
        state: impl Into<PathBuf>,
        workspaces: impl Into<PathBuf>,
        deployment: impl Into<String>,
        allow_uids: Vec<u32>,
    ) -> Self {
        Self {
            socket: socket.into(),
            state: state.into(),
            workspaces: workspaces.into(),
            deployment: deployment.into(),
            allow_uids,
            allow_unrecorded_sessions: false,
            cgroup_root: None,
            project_quota_ids: None,
            exec_cpu_cores: 1,
            git_sources: Vec::new(),
            bubblewrap: PathBuf::from("/usr/bin/bwrap"),
            event_retention: 10_000,
            secret_slots: Vec::new(),
            egress_apertures: Vec::new(),
            writable_roots: Vec::new(),
            ca_bundle: None,
            delegated_context_keys: Vec::new(),
            require_delegated_context: false,
            tcp: None,
            tls: None,
            hosted_identity: None,
            ops: None,
        }
    }
}

/// Serve one daemon until the process receives its shutdown signal.
///
/// # Errors
///
/// Returns an error before serving if the configuration or required host isolation is invalid,
/// and after serving if the shutdown signal cannot be installed.
#[allow(clippy::too_many_lines)] // Startup proof, ownership, and accept-loop cleanup stay adjacent.
pub async fn serve(config: DaemonConfig) -> anyhow::Result<()> {
    if config.tcp.is_some() && config.tls.is_some() {
        bail!(
            "tls.listener-config-invalid: development TCP and production TLS are mutually exclusive"
        );
    }
    if config.tls.is_some() != config.hosted_identity.is_some() {
        bail!(
            "auth.listener-config-invalid: production TLS and hosted Identity admission require each other"
        );
    }
    if config.tcp.is_none() && config.tls.is_none() && config.allow_uids.is_empty() {
        bail!("at least one explicit --allow-uid mapping is required");
    }
    if config.deployment.is_empty()
        || !config
            .deployment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        bail!("deployment must be a non-empty stable identifier");
    }
    prepare_private_state_path(&config.state)?;
    let _instance_lock = lock_state_identity(&config.state)?;
    if config.tcp.is_none() && config.tls.is_none() {
        prepare_socket(&config.socket)?;
    }
    if config.event_retention == 0 {
        bail!("event retention must be nonzero");
    }
    check_ops_listener(&config)?;
    check_secret_slots(&config.secret_slots)?;
    let git_sources = config
        .git_sources
        .iter()
        .map(|source| {
            substrate_host::GitSourceBinding::new(
                source.name.clone(),
                &source.base_url,
                config.ca_bundle.clone(),
            )
            .map_err(|_| anyhow!("workspace.git-source-invalid: Git source binding refused"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Resolved once, here, and pinned for the process's lifetime: the sandbox gets no resolver, so
    // a name that only resolves later resolves nowhere at all (ADR 0013).
    let egress_apertures = resolve_egress_apertures(&config.egress_apertures)?;
    check_writable_roots(
        &config.writable_roots,
        &config.workspaces,
        state_directory(&config.state)?,
        nix::unistd::Uid::effective().as_raw(),
    )?;
    check_writable_roots_clear_of(&config.writable_roots, &protected_host_paths(&config))?;
    check_ca_bundle(config.ca_bundle.as_deref())?;
    // Resolved before any listener exists: a trust anchor that only becomes usable later would let
    // the daemon serve mutations it could not attribute (ADR 0011).
    let delegated_context = resolve_delegated_context(
        &config.delegated_context_keys,
        config.require_delegated_context,
    )?;
    let tls_identity = config
        .tls
        .as_ref()
        .map(|tls| {
            load_server_config(tls).map_err(|error| {
                anyhow!(
                    "{}: production TLS identity material refused",
                    error.startup_code()
                )
            })
        })
        .transpose()?;
    let hosted_admission = config
        .hosted_identity
        .as_ref()
        .map(HostedAdmission::load)
        .transpose()?;
    let store = Arc::new(
        Store::open_with_event_retention(&config.state, config.event_retention)
            .context("open durable state")?,
    );
    std::fs::set_permissions(&config.state, std::fs::Permissions::from_mode(0o600))
        .context("restrict durable state database")?;
    let mut host_config = HostConfig::minimum(&config.workspaces);
    host_config.config_generation = configuration_generation(&config);
    host_config.cgroup_root = config.cgroup_root;
    host_config.project_quota_ids = config.project_quota_ids;
    host_config.exec_cpu_cores = config.exec_cpu_cores;
    host_config.git_sources = git_sources;
    host_config.bubblewrap = config.bubblewrap;
    host_config.event_retention = config.event_retention;
    host_config.secret_slots = config
        .secret_slots
        .into_iter()
        .map(|slot| substrate_host::SecretSlot {
            name: slot.name,
            path: slot.path,
        })
        .collect();
    host_config.egress_apertures = egress_apertures;
    host_config.writable_roots = config
        .writable_roots
        .into_iter()
        .map(|root| substrate_host::DeclaredWritableRoot {
            name: root.name,
            path: root.path,
        })
        .collect();
    host_config.aperture_root = aperture_state_root(&config.state);
    host_config.ca_bundle = config.ca_bundle;
    // ADR 0036 S2: host processes are served only by a daemon whose one listener is the personal
    // Unix socket, and only once the driver proved the delegated cgroup root usable for process
    // cgroups. Any other daemon opens no driver and publishes no `process.host`; every start it
    // receives is a named refusal, and nothing degrades into a weaker profile.
    let unix_only = config.tcp.is_none() && config.tls.is_none();
    let host_processes = if unix_only {
        match substrate_host::host_process::HostProcessSupervisor::open(
            host_config.cgroup_root.as_deref(),
            &process_state_root(&config.state),
        ) {
            Ok(supervisor) => Some(supervisor as Arc<dyn substrate_host::ProcessSupervisor>),
            Err(error) => {
                info!(code = error.code, "host processes are not served");
                None
            }
        }
    } else {
        None
    };
    host_config.host_processes = host_processes.is_some();
    let driver = HostDriver::open(host_config).context("open host driver")?;
    let mut app = App::with_delegated_context(
        store,
        driver,
        config.deployment,
        Arc::new(SystemAuthority),
        delegated_context,
    );
    Arc::get_mut(&mut app)
        .context("configure capture policy before sharing the application")?
        .allow_unrecorded_sessions = config.allow_unrecorded_sessions;
    // S7: without a driver, the logs an earlier run left are still read from the state
    // directory; nothing is created there and nothing runs.
    let host_process_logs = if host_processes.is_none() {
        Some(
            substrate_host::StoredProcessLogs::open(&process_state_root(&config.state))
                as Arc<dyn substrate_host::ProcessLogs>,
        )
    } else {
        None
    };
    let configured = Arc::get_mut(&mut app)
        .context("configure host processes before sharing the application")?;
    configured.host_processes = host_processes;
    configured.host_process_logs = host_process_logs;
    app.sweep_expired().await;
    let maintenance = MaintenanceTasks::start(&app);
    let _ops_listener = start_ops_listener(&app, &maintenance, config.ops.as_ref()).await?;
    if let Some(tcp) = config.tcp.as_ref() {
        let result = serve_tcp(Arc::clone(&app), tcp).await;
        maintenance.abort();
        return result;
    }
    if let (Some(tls), Some(identity)) = (config.tls.as_ref(), tls_identity) {
        let hosted_admission = hosted_admission.ok_or_else(|| {
            anyhow!(
                "auth.listener-config-invalid: production TLS requires hosted Identity admission"
            )
        })?;
        let result = serve_tls(Arc::clone(&app), tls, identity, hosted_admission).await;
        maintenance.abort();
        return result;
    }
    let listener = UnixListener::bind(&config.socket).context("bind unix socket")?;
    let socket_cleanup = SocketCleanup(config.socket.clone());
    std::fs::set_permissions(&config.socket, std::fs::Permissions::from_mode(0o600))
        .context("restrict unix socket")?;
    let allowed = config.allow_uids.into_iter().collect::<BTreeSet<_>>();
    let transport_policy = UnixTransportPolicy::production();
    let connection_limits = ConnectionLimits::new(&allowed, transport_policy);
    app.telemetry
        .register_listener(Listener::Unix, connection_limits.gauge());
    let request_metrics = RequestMetrics::new(&app, Listener::Unix);
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut connections = tokio::task::JoinSet::new();
    app.telemetry.start_accepting();
    info!(socket = %config.socket.display(), "substrate ready");

    loop {
        tokio::select! {
            accepted = accept_authorized(
                &listener,
                &allowed,
                &connection_limits,
                transport_policy.accept_retry_delay,
            ) => {
                let AcceptedConnection {
                    stream,
                    peer,
                    permit,
                } = accepted;
                let uid = peer.uid;
                let identity = Identity {
                    subject: format!("local:{uid}"),
                    actor: format!("unix-peer:{uid}"),
                    principal: peer.pid.map(|pid| format!("pid:{pid}")),
                };
                let service = router(Arc::clone(&app))
                    .layer(Extension(identity))
                    .layer(middleware::from_fn_with_state(
                        request_metrics.clone(),
                        record_request,
                    ));
                connections.spawn(async move {
                    let io = TokioIo::new(stream);
                    let builder = http1_builder(transport_policy);
                    let connection = builder
                        .serve_connection(
                            io,
                            TowerToHyperService::new(admitted_service(&permit, service)),
                        )
                        .with_upgrades();
                    match enforce_connection_lifetime(
                        permit,
                        transport_policy.connection_lifetime,
                        connection,
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => warn!(%error, "unix HTTP connection failed"),
                        Err(_) => warn!(uid, "unix HTTP connection lifetime expired"),
                    }
                });
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = joined {
                    warn!(%error, "unix connection task failed");
                }
            }
            signal = &mut shutdown => {
                // Readiness turns before any listener closes (ADR 0045 decision 6).
                app.telemetry.stop_accepting();
                signal?;
                break;
            }
        }
    }
    maintenance.abort();
    drop(listener);
    connections.abort_all();
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result
            && !error.is_cancelled()
        {
            warn!(%error, "unix connection task failed while stopping");
        }
    }
    app.driver
        .shutdown()
        .await
        .map_err(|error| anyhow!("driver shutdown refused: {}: {}", error.code, error.message))?;
    drop(socket_cleanup);
    Ok(())
}

/// Every rule a declared slot must satisfy before the daemon will serve at all.
///
/// Startup is the right place for all of it. A slot that only turns out to be undeclarable on the
/// first start that names it is a refusal the operator hears from a client instead of from the
/// process they configured.
///
/// # Errors
///
/// Returns the first rule broken, naming the slot and never reading its bytes.
#[allow(clippy::verbose_bit_mask)] // Permission masks read better than bit-position arithmetic.
fn check_secret_slots(slots: &[SecretSlot]) -> anyhow::Result<()> {
    let mut declared: BTreeSet<&str> = BTreeSet::new();
    for slot in slots {
        if !substrate_wire::valid_secret_slot_name(&slot.name) {
            bail!("secret slot names must match [a-z][a-z0-9_]{{0,63}}");
        }
        if !declared.insert(slot.name.as_str()) {
            bail!("secret slot {} is declared more than once", slot.name);
        }
        let metadata = std::fs::symlink_metadata(&slot.path)
            .with_context(|| format!("inspect the file declared for secret slot {}", slot.name))?;
        let mode = metadata.permissions().mode();
        if !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > substrate_wire::MAX_SECRET_SLOT_BYTES
            || metadata.uid() != nix::unistd::geteuid().as_raw()
            || mode & 0o077 != 0
        {
            bail!(
                "secret slot {} must name one bounded regular file with private workload ownership",
                slot.name
            );
        }
    }
    Ok(())
}

/// Resolves every declared aperture exactly once, at declaration, and pins what it resolved to.
///
/// DNS is outside the aperture. A run gets no resolver and performs no lookup, so a destination
/// that is not an address by the time the daemon is ready is a destination this daemon cannot
/// reach — said here, at startup, rather than by a run failing later for a reason nobody logged.
///
/// # Errors
///
/// Returns the first rule broken, naming the aperture.
fn resolve_egress_apertures(
    declared: &[EgressAperture],
) -> anyhow::Result<Vec<substrate_host::EgressAperture>> {
    if declared.len() > substrate_wire::MAX_EGRESS_APERTURES as usize {
        bail!(
            "at most {} egress apertures may be declared",
            substrate_wire::MAX_EGRESS_APERTURES
        );
    }
    let mut names: BTreeSet<&str> = BTreeSet::new();
    let mut resolved = Vec::with_capacity(declared.len());
    for aperture in declared {
        if !substrate_wire::valid_aperture_name(&aperture.name) {
            bail!("egress aperture names must match [a-z][a-z0-9_]{{0,63}}");
        }
        if !names.insert(aperture.name.as_str()) {
            bail!(
                "egress aperture {} is declared more than once",
                aperture.name
            );
        }
        if aperture.port == 0 {
            bail!("egress aperture {} declares no port", aperture.name);
        }
        // A ceiling of zero passes nothing, which no operator means and no run could use. Said
        // here rather than served, because an aperture that refuses its first byte is an outage
        // with a declaration behind it (ADR 0014).
        if aperture.max_bytes == Some(0) {
            bail!(
                "egress aperture {} declares a byte ceiling of zero",
                aperture.name
            );
        }
        let pinned =
            std::net::ToSocketAddrs::to_socket_addrs(&(aperture.host.as_str(), aperture.port))
                .with_context(|| {
                    format!(
                        "resolve the destination of egress aperture {}",
                        aperture.name
                    )
                })?
                .find(std::net::SocketAddr::is_ipv4)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "egress aperture {} resolved to no IPv4 destination",
                        aperture.name
                    )
                })?;
        resolved.push(substrate_host::EgressAperture {
            max_bytes: aperture.max_bytes,
            name: aperture.name.clone(),
            host: aperture.host.clone(),
            port: aperture.port,
            pinned,
        });
    }
    Ok(resolved)
}

/// The directory holding the durable state database and everything keyed beside it: the
/// aperture and host-process state roots and the instance lock.
fn state_directory(state: &Path) -> anyhow::Result<&Path> {
    state
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("durable state path requires a parent directory")
}

/// `path` with every symlink in its existing prefix resolved, and the part that does not exist yet
/// appended as written: what a later `create_dir_all` would make.
fn resolved_prefix(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute =
        std::path::absolute(path).with_context(|| format!("make {} absolute", path.display()))?;
    let mut existing = absolute.as_path();
    let mut rest = Vec::new();
    loop {
        match std::fs::canonicalize(existing) {
            Ok(resolved) => {
                return Ok(rest
                    .into_iter()
                    .rev()
                    .fold(resolved, |path: PathBuf, part| path.join(part)));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
                    return Err(error).with_context(|| format!("resolve {}", path.display()));
                };
                rest.push(name);
                existing = parent;
            }
            Err(error) => return Err(error).with_context(|| format!("resolve {}", path.display())),
        }
    }
}

/// Refuses, naming the root and the condition, any writable host root the driver could not bind
/// exactly as declared (ADR 0038 decision 2). Each must have a name in the root-name set declared
/// once, and a path that is absolute and in normal form, exists as a directory reached through no
/// symlink, is owned by `uid` (the daemon's own), and neither lies under nor contains the
/// workspace root or the state directory: a program could otherwise write another workspace, or
/// this daemon's own ledger.
///
/// The driver opens each root again, by descriptor, at every start that names it; this is the
/// startup half, so a declaration that could never be served stops the daemon rather than turning
/// into a refusal on every run.
///
/// # Errors
///
/// Returns the first rule broken, prefixed `config.writable-root-invalid`.
fn check_writable_roots(
    declared: &[WritableRootConfig],
    workspaces: &Path,
    state_directory: &Path,
    uid: u32,
) -> anyhow::Result<()> {
    let workspaces = resolved_prefix(workspaces)?;
    let state_directory = resolved_prefix(state_directory)?;
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for root in declared {
        let name = root.name.as_str();
        let path = root.path.as_path();
        let refuse = |condition: &str| {
            anyhow!("config.writable-root-invalid: writable root {name:?} {condition}")
        };
        if !substrate_wire::is_root_name(name) {
            return Err(refuse("is not a valid root name"));
        }
        if !names.insert(name) {
            return Err(refuse("is declared more than once"));
        }
        // ADR 0038 decision 2: the applied record states the bound host path as a string, so a
        // path with no exact string form can never be recorded as bound.
        if path.to_str().is_none() {
            return Err(refuse("does not name a UTF-8 path"));
        }
        if !path.is_absolute() {
            return Err(refuse("does not name an absolute path"));
        }
        let normal = path
            .components()
            .skip(1)
            .all(|component| matches!(component, std::path::Component::Normal(_)))
            && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str();
        if !normal {
            return Err(refuse("does not name a path in normal form"));
        }
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(refuse("does not exist"));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("inspect writable root {name:?}"));
            }
        };
        if !metadata.is_dir() {
            return Err(refuse("is not a directory"));
        }
        if std::fs::canonicalize(path).ok().as_deref() != Some(path) {
            return Err(refuse("is reached through a symlink"));
        }
        if metadata.uid() != uid {
            return Err(refuse(&format!("is not owned by the daemon's uid {uid}")));
        }
        for (reserved, what) in [
            (&workspaces, "the workspace root"),
            (&state_directory, "the state directory"),
        ] {
            if path.starts_with(reserved) {
                return Err(refuse(&format!("lies under {what}")));
            }
            if reserved.starts_with(path) {
                return Err(refuse(&format!("contains {what}")));
            }
        }
    }
    Ok(())
}

/// Every host path the daemon trusts outside the workspace root and the state directory, each
/// with what it is: the daemon executable, the API socket, the bubblewrap binary, every
/// secret-slot source, the CA bundle, the delegated cgroup root, the production TLS private key
/// and certificate chain, the hosted Identity CA bundle and the static-bearer file. A program
/// given a writable root containing one could replace the daemon or the confinement binary, read
/// or rewrite a secret, the daemon's identity key or the bearer it admits, swap a trust anchor,
/// talk to the API as the daemon's peer or move itself out of its cgroup.
fn protected_host_paths(config: &DaemonConfig) -> Vec<(PathBuf, String)> {
    let mut protected = vec![
        (config.socket.clone(), "the API socket".to_owned()),
        (
            config.bubblewrap.clone(),
            "the bubblewrap binary".to_owned(),
        ),
    ];
    if let Ok(executable) = std::env::current_exe() {
        protected.push((executable, "the daemon executable".to_owned()));
    }
    if let Some(tls) = &config.tls {
        protected.push((tls.private_key.clone(), "the TLS private key".to_owned()));
        protected.push((
            tls.certificate_chain.clone(),
            "the TLS certificate chain".to_owned(),
        ));
    }
    if let Some(identity) = &config.hosted_identity {
        protected.push((
            identity.ca_bundle.clone(),
            "the hosted Identity CA bundle".to_owned(),
        ));
    }
    if let Some(tcp) = &config.tcp {
        protected.push((tcp.bearer_file.clone(), "the static-bearer file".to_owned()));
    }
    protected.extend(config.secret_slots.iter().map(|slot| {
        (
            slot.path.clone(),
            format!("the source of secret slot {:?}", slot.name),
        )
    }));
    if let Some(ca_bundle) = &config.ca_bundle {
        protected.push((ca_bundle.clone(), "the CA bundle".to_owned()));
    }
    if let Some(cgroup_root) = &config.cgroup_root {
        protected.push((cgroup_root.clone(), "the delegated cgroup root".to_owned()));
    }
    protected
}

/// Refuses, naming the root, a writable root that equals or contains any of `protected`
/// ([`protected_host_paths`]). Each protected path is compared both as written and as resolved,
/// so a symlink to it inside the root and the root holding its target are both refused.
///
/// # Errors
///
/// Returns the first containment found, prefixed `config.writable-root-invalid`.
fn check_writable_roots_clear_of(
    declared: &[WritableRootConfig],
    protected: &[(PathBuf, String)],
) -> anyhow::Result<()> {
    let mut forms = Vec::new();
    for (path, what) in protected {
        if let Ok(absolute) = std::path::absolute(path) {
            forms.push((absolute, what));
        }
        forms.push((resolved_prefix(path)?, what));
    }
    for root in declared {
        let resolved = resolved_prefix(&root.path)?;
        for (path, what) in &forms {
            if path.starts_with(&root.path) || path.starts_with(&resolved) {
                return Err(anyhow!(
                    "config.writable-root-invalid: writable root {:?} contains {what}",
                    root.name
                ));
            }
        }
    }
    Ok(())
}

/// The trust anchor must exist and be readable now, not on the first run that needs it.
///
/// # Errors
///
/// Returns why the configured bundle cannot be snapshotted per run.
fn check_ca_bundle(path: Option<&std::path::Path>) -> anyhow::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let metadata = std::fs::symlink_metadata(path)
        .or_else(|_| std::fs::metadata(path))
        .with_context(|| "inspect the configured certificate bundle".to_owned())?;
    if !metadata.is_file() || metadata.len() == 0 {
        bail!("the certificate bundle must name one non-empty regular file");
    }
    Ok(())
}

/// Turns declared trusted keys into the policy the admission path uses.
///
/// Deliberately absent from [`configuration_generation`]: nothing about delegated context is a
/// published capability fact in `substrate-wire/0.7.0`, so rotating a key tells no client anything
/// and must not invalidate a capability snapshot or an admitted operation — the same reasoning that
/// keeps a secret slot's *file* out of the generation.
fn resolve_delegated_context(
    declared: &[DelegatedContextKey],
    required: bool,
) -> anyhow::Result<DelegatedContextPolicy> {
    let mut keys = Vec::with_capacity(declared.len());
    for key in declared {
        if key.kid.is_empty() || key.issuer.is_empty() {
            bail!("a delegated-context key needs a kid and an issuer");
        }
        let verifying_key = VerifyingKey::from_bytes(&key.public_key)
            .with_context(|| format!("delegated-context key {} is not an Ed25519 key", key.kid))?;
        keys.push(TrustedKey {
            kid: key.kid.clone(),
            issuer: key.issuer.clone(),
            verifying_key,
        });
    }
    DelegatedContextPolicy::new(keys, required).map_err(|error| anyhow!(error))
}

fn configuration_generation(config: &DaemonConfig) -> u64 {
    let mut material = Vec::new();
    for value in [
        config.deployment.as_bytes(),
        config.workspaces.as_os_str().as_encoded_bytes(),
        config.bubblewrap.as_os_str().as_encoded_bytes(),
    ] {
        material.extend_from_slice(&(value.len() as u64).to_be_bytes());
        material.extend_from_slice(value);
    }
    if let Some(root) = &config.cgroup_root {
        let value = root.as_os_str().as_encoded_bytes();
        material.extend_from_slice(&(value.len() as u64).to_be_bytes());
        material.extend_from_slice(value);
    } else {
        material.extend_from_slice(&0_u64.to_be_bytes());
    }
    material.extend_from_slice(&config.event_retention.to_be_bytes());
    // Slot **names** only. Which slots exist is configuration a client may notice change; what is
    // behind one is not, so a rotation must move nothing here (ADR 0012).
    let mut names: Vec<&str> = config
        .secret_slots
        .iter()
        .map(|slot| slot.name.as_str())
        .collect();
    names.sort_unstable();
    for name in names {
        material.extend_from_slice(&(name.len() as u64).to_be_bytes());
        material.extend_from_slice(name.as_bytes());
    }
    // Apertures whole — name *and* destination. Unlike a secret slot, what is behind an aperture is
    // exactly what a client is told (`exec.egress-apertures` publishes it), so changing it must
    // move the generation and invalidate every snapshot (design 02 V1 decision 5).
    let mut apertures: Vec<String> = config
        .egress_apertures
        .iter()
        .map(|aperture| {
            // The declaration as written, ceiling included: the fact publishes the ceiling, so a
            // ceiling that changed without moving the generation would leave a client holding a
            // snapshot that states a bound the daemon no longer enforces (ADR 0014).
            let ceiling = aperture
                .max_bytes
                .map(|max| format!("/max={max}"))
                .unwrap_or_default();
            format!(
                "{}={}:{}/tcp{ceiling}",
                aperture.name, aperture.host, aperture.port
            )
        })
        .collect();
    apertures.sort_unstable();
    for aperture in apertures {
        material.extend_from_slice(&(aperture.len() as u64).to_be_bytes());
        material.extend_from_slice(aperture.as_bytes());
    }
    // Writable roots whole, name and path: the fact publishes the names, and the path is what a
    // write lands in, so moving either must invalidate every snapshot. Nothing is added when none
    // is declared, so a daemon without one keeps the generation it had (ADR 0038).
    let mut writable_roots = config
        .writable_roots
        .iter()
        .map(|root| {
            let mut material = root.name.as_bytes().to_vec();
            material.push(b'=');
            material.extend_from_slice(root.path.as_os_str().as_encoded_bytes());
            material
        })
        .collect::<Vec<_>>();
    writable_roots.sort_unstable();
    for root in writable_roots {
        material.extend_from_slice(&(root.len() as u64).to_be_bytes());
        material.extend_from_slice(&root);
    }
    let mut git_sources = config
        .git_sources
        .iter()
        .map(|source| format!("{}={}", source.name, source.base_url))
        .collect::<Vec<_>>();
    git_sources.sort_unstable();
    for source in git_sources {
        material.extend_from_slice(&(source.len() as u64).to_be_bytes());
        material.extend_from_slice(source.as_bytes());
    }
    let digest = Sha256::digest(material);
    u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 prefix is eight bytes"),
    )
    .max(1)
}

#[derive(Clone)]
struct TcpAuthState {
    bearer_sha256: [u8; 32],
}

#[allow(clippy::too_many_lines)] // Admission, instrumentation and the accept loop stay adjacent.
async fn serve_tcp(app: Arc<App>, config: &TcpDaemonConfig) -> anyhow::Result<()> {
    if !config.development_only {
        bail!("static-bearer TCP is available only in the explicit development profile");
    }
    if !config.private_overlay {
        bail!("hosted TCP requires an explicitly configured private overlay");
    }
    if !config.listen.ip().is_loopback() {
        bail!("tls.listener-config-invalid: development static-bearer TCP must bind loopback");
    }
    if !valid_path_prefix(&config.path_prefix) {
        bail!("hosted TCP path prefix must be absolute, lowercase, and have no trailing slash");
    }
    if !valid_identity_ref(&config.subject) || !valid_identity_ref(&config.actor) {
        bail!("hosted TCP subject and actor must be bounded stable references");
    }
    let auth = TcpAuthState {
        bearer_sha256: read_bearer_digest(&config.bearer_file)?,
    };
    let identity = Identity {
        subject: config.subject.clone(),
        actor: config.actor.clone(),
        principal: None,
    };
    let service = development_router(Arc::clone(&app))
        .layer(Extension(identity))
        .layer(middleware::from_fn_with_state(auth, require_tcp_bearer))
        .layer(middleware::from_fn_with_state(
            RequestMetrics::new(&app, Listener::TcpDevelopment),
            record_request,
        ));
    let service = if config.path_prefix == "/" {
        service
    } else {
        Router::new().nest(&config.path_prefix, service)
    };
    let listener = TcpListener::bind(config.listen)
        .await
        .context("bind authenticated TCP listener")?;
    info!(listen = %config.listen, "substrate development-only authenticated TCP ready");
    let policy = UnixTransportPolicy::production();
    let limits = TcpConnectionLimits::production();
    app.telemetry
        .register_listener(Listener::TcpDevelopment, limits.gauge());
    app.telemetry.start_accepting();
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, address) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        warn!(%error, "transient TCP accept failure");
                        tokio::time::sleep(policy.accept_retry_delay).await;
                        continue;
                    }
                };
                let Some(permit) = limits.acquire(address.ip()) else {
                    warn!(source = %address.ip(), "refused TCP peer at connection capacity");
                    continue;
                };
                let service = service.clone();
                connections.spawn(async move {
                    let io = TokioIo::new(stream);
                    let builder = http1_builder(policy);
                    let connection = builder
                        .serve_connection(
                            io,
                            TowerToHyperService::new(admitted_service(&permit, service)),
                        )
                        .with_upgrades();
                    match enforce_connection_lifetime(
                        permit,
                        policy.connection_lifetime,
                        connection,
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => warn!(%error, "TCP HTTP connection failed"),
                        Err(_) => warn!(source = %address.ip(), "TCP connection lifetime expired"),
                    }
                });
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = joined {
                    warn!(%error, "TCP connection task failed");
                }
            }
            signal = &mut shutdown => {
                // Readiness turns before any listener closes (ADR 0045 decision 6).
                app.telemetry.stop_accepting();
                signal?;
                break;
            }
        }
    }
    drop(listener);
    drain_tcp_connections(&mut connections).await;
    app.driver
        .shutdown()
        .await
        .map_err(|error| anyhow!("driver shutdown refused: {}: {}", error.code, error.message))?;
    Ok(())
}

/// Serve the production transport while application identity remains fail-closed.
///
/// ADR 0024 deliberately does not turn a transport certificate into a caller identity. The next
/// Remote Foundation story replaces this pre-admission refusal with the hosted trust-envelope
/// verifier; until then no application handler can observe a fabricated remote subject.
async fn serve_tls(
    app: Arc<App>,
    config: &TlsDaemonConfig,
    mut identity: Arc<rustls::ServerConfig>,
    hosted_admission: HostedAdmission,
) -> anyhow::Result<()> {
    if config.listen.port() == 0 {
        bail!("tls.listener-config-invalid: production TLS requires a nonzero listen port");
    }
    let mut reload = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .context("tls.listener-config-invalid: install SIGHUP reload signal")?;
    let hosted_admission = Arc::new(hosted_admission.with_telemetry(Arc::clone(&app.telemetry)));
    let listener = TcpListener::bind(config.listen)
        .await
        .context("tls.listener-config-invalid: bind production TLS listener")?;
    info!(listen = %config.listen, "substrate production HTTPS/WSS transport ready");
    let policy = UnixTransportPolicy::production();
    let limits = TcpConnectionLimits::production();
    app.telemetry
        .register_listener(Listener::Tls, limits.gauge());
    app.telemetry.start_accepting();
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, address)) = accepted else {
                    warn!("transient production TLS accept failure");
                    tokio::time::sleep(policy.accept_retry_delay).await;
                    continue;
                };
                let Some(permit) = limits.acquire(address.ip()) else {
                    warn!(source = %address.ip(), "refused TLS peer at connection capacity");
                    continue;
                };
                let acceptor = TlsAcceptor::from(Arc::clone(&identity));
                connections.spawn(serve_tls_connection(
                    stream,
                    address,
                    permit,
                    acceptor,
                    Arc::clone(&app),
                    Arc::clone(&hosted_admission),
                    policy,
                ));
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = joined {
                    warn!(%error, "TLS connection task failed");
                }
            }
            signal = reload.recv() => {
                if signal.is_none() {
                    bail!("tls.listener-config-invalid: SIGHUP reload signal stream ended");
                }
                match load_server_config(config) {
                    Ok(replacement) => {
                        identity = replacement;
                        info!(code = "tls.identity-reloaded", "production TLS identity reloaded");
                    }
                    Err(_) => {
                        warn!(
                            code = "tls.reload-invalid",
                            certificate_chain = %config.certificate_chain.display(),
                            private_key = %config.private_key.display(),
                            "production TLS identity reload refused"
                        );
                    }
                }
            }
            signal = &mut shutdown => {
                // Readiness turns before any listener closes (ADR 0045 decision 6).
                app.telemetry.stop_accepting();
                signal?;
                break;
            }
        }
    }
    drop(listener);
    drain_tcp_connections(&mut connections).await;
    app.driver
        .shutdown()
        .await
        .map_err(|error| anyhow!("driver shutdown refused: {}: {}", error.code, error.message))?;
    Ok(())
}

async fn serve_tls_connection(
    stream: tokio::net::TcpStream,
    address: SocketAddr,
    permit: TransportPermit,
    acceptor: TlsAcceptor,
    app: Arc<App>,
    hosted_admission: Arc<HostedAdmission>,
    policy: UnixTransportPolicy,
) {
    let handshake = tokio::time::timeout(policy.header_read_timeout, acceptor.accept(stream)).await;
    let tls = match handshake {
        Ok(Ok(tls)) => tls,
        Ok(Err(_)) => {
            warn!(source = %address.ip(), "TLS handshake refused");
            return;
        }
        Err(_) => {
            warn!(source = %address.ip(), "TLS handshake timed out");
            return;
        }
    };
    let mut exporter = [0_u8; 32];
    if tls
        .get_ref()
        .1
        .export_keying_material(
            &mut exporter,
            substrate_wire::SESSION_AUTHORITY_EXPORTER_LABEL,
            None,
        )
        .is_err()
    {
        warn!(source = %address.ip(), "TLS session exporter unavailable");
        return;
    }
    let request_metrics = RequestMetrics::new(&app, Listener::Tls);
    let service = hosted_router(app, exporter)
        .layer(middleware::from_fn_with_state(
            hosted_admission,
            require_hosted_authority,
        ))
        .layer(middleware::from_fn_with_state(
            request_metrics,
            record_request,
        ));
    let io = TokioIo::new(tls);
    let builder = http1_builder(policy);
    let connection = builder
        .serve_connection(
            io,
            TowerToHyperService::new(admitted_service(&permit, service)),
        )
        .with_upgrades();
    match enforce_connection_lifetime(permit, policy.connection_lifetime, connection).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => warn!(source = %address.ip(), "TLS HTTP connection failed"),
        Err(_) => warn!(source = %address.ip(), "TLS connection lifetime expired"),
    }
}

async fn drain_tcp_connections(connections: &mut tokio::task::JoinSet<()>) {
    let drained = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(result) = connections.join_next().await {
            if let Err(error) = result {
                warn!(%error, "TCP connection task failed while draining");
            }
        }
    })
    .await;
    if drained.is_err() {
        connections.abort_all();
    }
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result
            && !error.is_cancelled()
        {
            warn!(%error, "TCP connection task failed while stopping");
        }
    }
}

async fn shutdown_signal() -> anyhow::Result<()> {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .context("install Ctrl-C shutdown signal")
    };
    #[cfg(unix)]
    let terminate = async {
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("install SIGTERM shutdown signal")?;
        signal.recv().await;
        Ok(())
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<anyhow::Result<()>>();
    tokio::select! {
        result = interrupt => result,
        result = terminate => result,
    }
}

async fn require_tcp_bearer(
    State(auth): State<TcpAuthState>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let admitted = presented.is_some_and(|value| {
        let digest = Sha256::digest(value.as_bytes());
        bool::from(auth.bearer_sha256.ct_eq(digest.as_ref()))
    });
    if admitted {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, "substrate bearer required\n").into_response()
    }
}

#[allow(clippy::verbose_bit_mask)] // Permission masks are clearer here than bit-position arithmetic.
fn read_bearer_digest(path: &Path) -> anyhow::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path).context("open TCP bearer file")?;
    let metadata = file.metadata().context("inspect TCP bearer file")?;
    let mode = metadata.mode();
    let effective_user = nix::unistd::geteuid().as_raw();
    let effective_group = nix::unistd::getegid().as_raw();
    let current_user_private =
        metadata.uid() == effective_user && mode & 0o400 != 0 && mode & 0o077 == 0;
    let root_projected_group = metadata.uid() == 0
        && metadata.gid() == effective_group
        && mode & 0o040 != 0
        && mode & 0o037 == 0;
    if !metadata.is_file()
        || metadata.len() > 512
        || !(current_user_private || root_projected_group)
    {
        bail!("TCP bearer file must be one bounded regular file with private workload ownership");
    }
    let mut bearer = Zeroizing::new(String::new());
    (&mut file)
        .take(513)
        .read_to_string(&mut bearer)
        .context("read TCP bearer file")?;
    if bearer.len() > 512 {
        bail!("TCP bearer file exceeds its admitted bound");
    }
    let bearer = bearer.trim();
    let valid = bearer
        .strip_prefix("dl_substrate_v1_")
        .is_some_and(|token| {
            token.len() == 43
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        });
    if !valid {
        bail!("TCP bearer file has an invalid credential shape");
    }
    Ok(Sha256::digest(bearer.as_bytes()).into())
}

fn valid_identity_ref(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn valid_path_prefix(value: &str) -> bool {
    value == "/"
        || (value.starts_with('/')
            && !value.ends_with('/')
            && value.len() <= 128
            && value.split('/').skip(1).all(|segment| {
                !segment.is_empty()
                    && segment.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            }))
}

fn prepare_private_state_path(state: &Path) -> anyhow::Result<()> {
    let parent = state
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("durable state path requires a parent directory")?;
    if !parent.exists() {
        std::fs::create_dir_all(parent).context("create state directory")?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .context("restrict state directory")?;
    }
    let parent_metadata = std::fs::symlink_metadata(parent).context("inspect state directory")?;
    let current_uid = nix::unistd::Uid::current().as_raw();
    if !parent_metadata.is_dir()
        || parent_metadata.uid() != current_uid
        || !owner_only(parent_metadata.permissions().mode())
    {
        bail!("state directory must be owner-controlled with mode 0700 or stricter");
    }
    match std::fs::symlink_metadata(state) {
        Ok(metadata)
            if metadata.is_file()
                && metadata.uid() == current_uid
                && owner_only(metadata.permissions().mode()) => {}
        Ok(_) => bail!("state database must be an owner-only regular file, never a symlink"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect durable state database"),
    }
    Ok(())
}

fn owner_only(mode: u32) -> bool {
    mode.trailing_zeros() >= 6
}

/// Where this daemon's egress-aperture run state lives: beside its durable state database, in
/// the owner-private state directory, keyed by the same state identity the instance lock holds.
/// Never in the workspace root, which may hold an embedder's own checkouts.
fn aperture_state_root(state: &Path) -> PathBuf {
    state.with_extension("apertures")
}

/// The owner-private directory holding one bounded log per host process (ADR 0036 S7): beside the
/// durable state, never in a caller's working directory and never in a temporary directory.
fn process_state_root(state: &Path) -> PathBuf {
    state.with_extension("processes")
}

fn lock_state_identity(state: &Path) -> anyhow::Result<InstanceLock> {
    let path = state.with_extension("instance.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create lock directory")?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .context("open instance lock")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .context("restrict instance lock")?;
    let file = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
        .map_err(|(_, error)| error)
        .context("another substrate daemon owns this durable state identity")?;
    Ok(InstanceLock { path, _file: file })
}

fn prepare_socket(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create socket directory")?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            std::fs::remove_file(path).context("remove stale unix socket")?;
        }
        Ok(_) => bail!("socket path exists and is not a unix socket"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect socket path"),
    }
    Ok(())
}

/// Connections the operational listener serves at once; a ninth is closed unanswered.
const OPS_MAX_CONNECTIONS: usize = 8;
/// Of those, the connections one source address may hold; a fifth from it is closed unanswered.
const OPS_MAX_CONNECTIONS_PER_SOURCE: usize = 4;
/// How long a connection to the operational listener has to send its request head.
const OPS_HEADER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
/// How long one operational connection may live, its one exchange included.
const OPS_CONNECTION_LIFETIME: std::time::Duration = std::time::Duration::from_secs(10);
/// The request head the operational listener buffers: twice hyper's 8 KiB floor.
const OPS_MAX_BUFFER_BYTES: usize = 16 * 1024;
const OPS_MAX_HEADERS: usize = 32;

/// Every rule the operational listener's declaration must satisfy before the daemon serves.
///
/// # Errors
///
/// `ops.listener-config-invalid` for a zero port, an address a control listener also binds, or a
/// required fact that is not a capability fact name.
fn check_ops_listener(config: &DaemonConfig) -> anyhow::Result<()> {
    let Some(ops) = config.ops.as_ref() else {
        return Ok(());
    };
    if ops.listen.port() == 0 {
        bail!("ops.listener-config-invalid: the operational listener requires a nonzero port");
    }
    let shares_control = config
        .tcp
        .iter()
        .map(|tcp| tcp.listen)
        .chain(config.tls.iter().map(|tls| tls.listen))
        .any(|control| {
            // `[::ffff:a.b.c.d]` is `a.b.c.d` in its IPv4-mapped spelling: compare canonically.
            let (control_ip, ops_ip) =
                (control.ip().to_canonical(), ops.listen.ip().to_canonical());
            control.port() == ops.listen.port()
                && (control_ip == ops_ip || control_ip.is_unspecified() || ops_ip.is_unspecified())
        });
    if shares_control {
        bail!(
            "ops.listener-config-invalid: the operational listener must not share a control listener's address"
        );
    }
    if let Some(unknown) = ops
        .require_facts
        .iter()
        .find(|fact| !is_capability_fact_name(fact))
    {
        bail!(
            "ops.listener-config-invalid: --ops-require-fact {unknown:?} is not a capability fact name"
        );
    }
    Ok(())
}

/// The running operational listener; dropping it stops the listener and every connection on it.
struct OpsListener(tokio::task::JoinHandle<()>);

impl Drop for OpsListener {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Binds and starts the operational listener when one is declared. It is bound before any
/// control listener, so the readiness a probe reads is `503` until a control listener is bound.
async fn start_ops_listener(
    app: &Arc<App>,
    maintenance: &MaintenanceTasks,
    config: Option<&OpsListenerConfig>,
) -> anyhow::Result<Option<OpsListener>> {
    let Some(config) = config else {
        return Ok(None);
    };
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|_| anyhow!("ops.listener-config-invalid: bind the operational listener"))?;
    let operations = Operations::new(
        Arc::clone(app),
        maintenance.handles(),
        config.require_facts.clone(),
        substrate_host::output_stall_terminations,
    );
    info!(listen = %config.listen, "substrate operational listener ready");
    Ok(Some(OpsListener(tokio::spawn(serve_ops(
        listener, operations,
    )))))
}

/// One exchange per connection, no keep-alive, no upgrade, no body read, no credential read.
async fn serve_ops(listener: TcpListener, operations: Arc<Operations>) {
    let slots = TcpConnectionLimits::operational();
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    continue;
                };
                let Some(slot) = slots.acquire(peer.ip().to_canonical()) else {
                    continue;
                };
                let operations = Arc::clone(&operations);
                connections.spawn(async move {
                    let _slot = slot;
                    let service = hyper::service::service_fn(
                        move |request: hyper::Request<hyper::body::Incoming>| {
                            let (parts, _body) = request.into_parts();
                            let answer = operations.answer(&hyper::Request::from_parts(parts, ()));
                            std::future::ready(Ok::<_, std::convert::Infallible>(answer))
                        },
                    );
                    let mut builder = http1::Builder::new();
                    builder
                        .timer(TokioTimer::new())
                        .header_read_timeout(OPS_HEADER_DEADLINE)
                        .keep_alive(false)
                        .max_buf_size(OPS_MAX_BUFFER_BYTES)
                        .max_headers(OPS_MAX_HEADERS);
                    let connection = builder.serve_connection(TokioIo::new(stream), service);
                    let _ = tokio::time::timeout(OPS_CONNECTION_LIFETIME, connection).await;
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

/// The daemon's two maintenance tasks: the lease sweeper and the containment step.
///
/// They are separate tasks so a driver that hangs on a containment kill stalls no other
/// maintenance duty, and they have one owner so every shutdown path that stops one stops the
/// other. Each of the three listener exits calls [`MaintenanceTasks::abort`]; dropping the value
/// aborts both on any other exit, an early `?` included. An attempt in flight when the daemon
/// stops is cancelled with its task, and `reconcile_after_restart` answers for its exec on the next
/// start, as it does for any exec live across a restart.
struct MaintenanceTasks {
    lease_sweeper: tokio::task::JoinHandle<()>,
    containment: tokio::task::JoinHandle<()>,
}

impl MaintenanceTasks {
    fn start(app: &Arc<App>) -> Self {
        let sweeper_app = Arc::clone(app);
        let lease_sweeper = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    () = sweeper_app.maintenance_notified() => {}
                }
                let started = std::time::Instant::now();
                sweeper_app.sweep_expired().await;
                sweeper_app
                    .telemetry
                    .maintenance_round(MaintenanceTask::LeaseSweep, started.elapsed());
            }
        });
        let containment_app = Arc::clone(app);
        let containment = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
            // A round held up by a kill the driver does not answer is followed by one tick, not
            // by a burst of the ticks it missed.
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    () = containment_app.containment_notified() => {}
                }
                let started = std::time::Instant::now();
                containment_app.contain_pending().await;
                containment_app
                    .telemetry
                    .maintenance_round(MaintenanceTask::Containment, started.elapsed());
            }
        });
        Self {
            lease_sweeper,
            containment,
        }
    }

    fn abort(&self) {
        self.lease_sweeper.abort();
        self.containment.abort();
    }

    /// What liveness watches: whether either task has ended.
    fn handles(&self) -> Vec<tokio::task::AbortHandle> {
        vec![
            self.lease_sweeper.abort_handle(),
            self.containment.abort_handle(),
        ]
    }
}

impl Drop for MaintenanceTasks {
    fn drop(&mut self) {
        self.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::convert::Infallible;

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    fn idle_maintenance_tasks() -> MaintenanceTasks {
        MaintenanceTasks {
            lease_sweeper: tokio::spawn(std::future::pending::<()>()),
            containment: tokio::spawn(std::future::pending::<()>()),
        }
    }

    async fn await_finished(handles: &[tokio::task::AbortHandle]) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !handles.iter().all(tokio::task::AbortHandle::is_finished) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both maintenance tasks end");
    }

    /// The call each of the three listener exits makes stops the containment step with the sweeper.
    #[tokio::test]
    async fn aborting_the_maintenance_tasks_stops_the_containment_step_with_the_sweeper() {
        let tasks = idle_maintenance_tasks();
        let handles = [
            tasks.lease_sweeper.abort_handle(),
            tasks.containment.abort_handle(),
        ];
        assert!(!handles.iter().any(tokio::task::AbortHandle::is_finished));
        tasks.abort();
        await_finished(&handles).await;
        drop(tasks);
    }

    /// Any other exit — an early `?` between starting the tasks and serving — drops the owner, and
    /// that stops both tasks too.
    #[tokio::test]
    async fn dropping_the_maintenance_tasks_stops_both() {
        let tasks = idle_maintenance_tasks();
        let handles = [
            tasks.lease_sweeper.abort_handle(),
            tasks.containment.abort_handle(),
        ];
        drop(tasks);
        await_finished(&handles).await;
    }

    /// The RFC 6455 example handshake key, spelled the way every other hand-written WebSocket
    /// client in this repository spells it (`tests/websocket.rs:23`, `tests/pipe_session.rs:40`,
    /// `tests/runtime_vectors.rs:93`). It is a constant rather than an inline literal because an
    /// inline value after `Sec-WebSocket-Key: ` gives Gitleaks' `generic-api-key` rule its keyword
    /// trigger, and `cargo xtask check-secrets` scans `--all` refs — so a branch that never merges
    /// still fails the gate.
    const HANDSHAKE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

    const fn test_policy(
        global_connections: usize,
        connections_per_uid: usize,
    ) -> UnixTransportPolicy {
        UnixTransportPolicy {
            global_connections,
            connections_per_uid,
            header_read_timeout: std::time::Duration::from_secs(3),
            connection_lifetime: std::time::Duration::from_secs(7),
            accept_retry_delay: std::time::Duration::ZERO,
            max_buffer_bytes: 8 * 1024,
            max_headers: 8,
            keep_alive: false,
        }
    }

    #[test]
    fn tcp_source_limit_evicts_only_idle_sources() {
        let limits = TcpConnectionLimits {
            global: Arc::new(Semaphore::new(8)),
            global_limit: 8,
            by_source: Mutex::new(BTreeMap::new()),
            per_source: 1,
            max_sources: 2,
            sequence: AtomicU64::new(1),
            rejected: Arc::new(AtomicU64::new(0)),
        };
        let active = limits.acquire("127.0.0.1".parse().unwrap()).unwrap();
        assert!(limits.acquire("127.0.0.2".parse().unwrap()).is_some());
        assert!(
            limits.acquire("127.0.0.3".parse().unwrap()).is_some(),
            "the idle second source is evicted"
        );
        assert!(limits.acquire("127.0.0.1".parse().unwrap()).is_none());
        drop(active);
        for octet in 4..=255 {
            assert!(
                limits
                    .acquire(format!("127.0.0.{octet}").parse().unwrap())
                    .is_some()
            );
        }
    }

    #[test]
    fn connection_limits_are_global_subject_scoped_and_recover_on_drop() {
        let allowed = BTreeSet::from([1000, 1001, 1002]);
        let policy = test_policy(2, 1);
        let limits = ConnectionLimits::new(&allowed, policy);

        let uid_1000 = limits.acquire(1000).expect("uid 1000 capacity");
        assert!(limits.acquire(1000).is_none());
        let uid_1001 = limits.acquire(1001).expect("uid 1001 capacity");
        assert!(limits.acquire(1002).is_none());
        assert!(limits.acquire(9999).is_none());
        // ADR 0045: the two refused at capacity are counted; the unmapped uid is not one.
        let gauge = limits.gauge();
        assert_eq!(gauge.rejected.load(Ordering::Relaxed), 2);
        assert_eq!((gauge.open(), gauge.limit), (2, 2));

        drop(uid_1000);
        assert!(limits.acquire(1000).is_some());
        drop(uid_1001);
    }

    /// For a connection that stays un-upgraded, which is the whole of what this bound covers:
    /// the future here never resolves, so the deadline is what ends it. An upgraded connection
    /// resolves its future at the handshake and is bounded by its stream instead —
    /// `app::tests::upgraded_transport_slot` pins that.
    #[tokio::test(start_paused = true)]
    async fn connection_lifetime_is_hard_and_recovers_capacity() {
        let allowed = BTreeSet::from([1000]);
        let policy = test_policy(1, 1);
        let limits = ConnectionLimits::new(&allowed, policy);
        let permit = limits.acquire(1000).expect("connection capacity");
        let task = tokio::spawn(enforce_connection_lifetime(
            permit,
            policy.connection_lifetime,
            std::future::pending::<Result<(), Infallible>>(),
        ));
        tokio::task::yield_now().await;

        // Held for as long as the connection runs, which is the half a recovery assertion alone
        // cannot see: a permit released early would recover capacity too, and sooner.
        assert!(
            limits.acquire(1000).is_none(),
            "the connection holds its admission while it is being served"
        );
        tokio::time::advance(policy.connection_lifetime).await;
        assert!(task.await.expect("lifetime task").is_err());
        assert!(limits.acquire(1000).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn incomplete_headers_hit_the_configured_deadline() {
        let policy = test_policy(1, 1);
        let (mut client, server) = tokio::io::duplex(1_024);
        let task = tokio::spawn(async move {
            let service = hyper::service::service_fn(|_request| async {
                Ok::<_, Infallible>(hyper::Response::new(http_body_util::Empty::<
                    hyper::body::Bytes,
                >::new()))
            });
            http1_builder(policy)
                .serve_connection(TokioIo::new(server), service)
                .await
        });
        client
            .write_all(b"GET / HTTP/1.1\r\nHost:")
            .await
            .expect("partial request");
        tokio::task::yield_now().await;

        tokio::time::advance(policy.header_read_timeout).await;
        assert!(task.await.expect("HTTP task").is_err());
    }

    #[tokio::test]
    async fn header_count_is_rejected_at_the_configured_bound() {
        let policy = test_policy(1, 1);
        let (mut client, server) = tokio::io::duplex(4_096);
        let task = tokio::spawn(async move {
            let service = hyper::service::service_fn(|_request| async {
                Ok::<_, Infallible>(hyper::Response::new(http_body_util::Empty::<
                    hyper::body::Bytes,
                >::new()))
            });
            http1_builder(policy)
                .serve_connection(TokioIo::new(server), service)
                .await
        });
        let mut request = b"GET / HTTP/1.1\r\n".to_vec();
        for index in 0..=policy.max_headers {
            request.extend_from_slice(format!("x-{index}: value\r\n").as_bytes());
        }
        request.extend_from_slice(b"\r\n");
        client.write_all(&request).await.expect("request headers");

        assert!(task.await.expect("HTTP task").is_err());
    }

    enum AcceptStep {
        AcceptError,
        PeerError,
        Peer(PeerIdentity),
    }

    struct FakeStream {
        peer: Result<PeerIdentity, io::ErrorKind>,
    }

    impl PeerCredentialSource for FakeStream {
        fn peer_identity(&self) -> io::Result<PeerIdentity> {
            self.peer.map_err(io::Error::from)
        }
    }

    struct FakeConnectionSource {
        steps: tokio::sync::Mutex<VecDeque<AcceptStep>>,
    }

    #[async_trait::async_trait]
    impl ConnectionSource for FakeConnectionSource {
        type Stream = FakeStream;

        async fn accept_stream(&self) -> io::Result<Self::Stream> {
            match self
                .steps
                .lock()
                .await
                .pop_front()
                .expect("scripted accept step")
            {
                AcceptStep::AcceptError => Err(io::Error::from(io::ErrorKind::ConnectionAborted)),
                AcceptStep::PeerError => Ok(FakeStream {
                    peer: Err(io::ErrorKind::PermissionDenied),
                }),
                AcceptStep::Peer(peer) => Ok(FakeStream { peer: Ok(peer) }),
            }
        }
    }

    #[tokio::test]
    async fn accept_and_peer_errors_continue_to_an_authorized_connection() {
        let source = FakeConnectionSource {
            steps: tokio::sync::Mutex::new(VecDeque::from([
                AcceptStep::AcceptError,
                AcceptStep::PeerError,
                AcceptStep::Peer(PeerIdentity {
                    uid: 2000,
                    pid: Some(20),
                }),
                AcceptStep::Peer(PeerIdentity {
                    uid: 1000,
                    pid: Some(10),
                }),
            ])),
        };
        let allowed = BTreeSet::from([1000]);
        let policy = test_policy(1, 1);
        let limits = ConnectionLimits::new(&allowed, policy);

        let accepted =
            accept_authorized(&source, &allowed, &limits, policy.accept_retry_delay).await;
        assert_eq!(
            accepted.peer,
            PeerIdentity {
                uid: 1000,
                pid: Some(10)
            }
        );
        assert!(source.steps.lock().await.is_empty());
        assert!(limits.acquire(1000).is_none());
        drop(accepted);
        assert!(limits.acquire(1000).is_some());
    }

    #[test]
    fn socket_cleanup_removes_bound_socket_on_early_drop() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("substrate.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind socket");
        let cleanup = SocketCleanup(path.clone());
        assert!(path.exists());

        drop(listener);
        drop(cleanup);
        assert!(!path.exists());
    }

    #[test]
    fn hosted_bearer_file_is_closed_and_hashed() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("bearer");
        let bearer = format!("dl_substrate_v1_{}", "a".repeat(43));
        std::fs::write(&path, format!("{bearer}\n")).expect("write bearer");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("secure bearer");
        assert_eq!(
            read_bearer_digest(&path).unwrap(),
            Sha256::digest(bearer).as_slice()
        );

        std::fs::write(&path, "invalid").expect("replace bearer");
        assert!(read_bearer_digest(&path).is_err());
    }

    #[test]
    fn hosted_bearer_file_refuses_widened_and_oversized_material() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("bearer");
        let bearer = format!("dl_substrate_v1_{}", "a".repeat(43));
        std::fs::write(&path, &bearer).expect("write bearer");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
            .expect("widen bearer");
        assert!(read_bearer_digest(&path).is_err());

        std::fs::write(&path, "a".repeat(513)).expect("write oversized bearer");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("secure oversized bearer");
        assert!(read_bearer_digest(&path).is_err());
    }

    #[test]
    fn hosted_bearer_file_admits_a_projected_symlink_to_a_safe_target() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("bearer-target");
        let link = directory.path().join("bearer");
        let bearer = format!("dl_substrate_v1_{}", "a".repeat(43));
        std::fs::write(&target, &bearer).expect("write bearer");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
            .expect("secure bearer");
        std::os::unix::fs::symlink(&target, &link).expect("project bearer");

        assert_eq!(
            read_bearer_digest(&link).unwrap(),
            Sha256::digest(bearer).as_slice()
        );
    }

    #[test]
    fn tcp_capacity_is_global_and_source_scoped() {
        let limits = TcpConnectionLimits::production();
        let source: IpAddr = "192.0.2.1".parse().unwrap();
        let permits = (0..16)
            .map(|_| limits.acquire(source).expect("per-source capacity"))
            .collect::<Vec<_>>();
        assert!(limits.acquire(source).is_none());
        assert!(limits.acquire("192.0.2.2".parse().unwrap()).is_some());
        let gauge = limits.gauge();
        assert_eq!(gauge.rejected.load(Ordering::Relaxed), 1);
        assert_eq!((gauge.open(), gauge.limit), (16, 128));
        drop(permits);
        assert!(limits.acquire(source).is_some());
    }

    #[test]
    fn durable_state_requires_an_owner_private_directory_and_regular_file() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("private");
        std::fs::create_dir(&private).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = private.join("state.db");
        prepare_private_state_path(&state).unwrap();

        let broad = directory.path().join("broad");
        std::fs::create_dir(&broad).unwrap();
        std::fs::set_permissions(&broad, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(prepare_private_state_path(&broad.join("state.db")).is_err());

        std::os::unix::fs::symlink(&state, private.join("linked.db")).unwrap();
        assert!(prepare_private_state_path(&private.join("linked.db")).is_err());
    }

    /// A host tree for the writable-root startup checks: `workspaces`, `state` and two candidate
    /// roots beside them, every path canonical.
    fn writable_root_tree() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(directory.path()).unwrap();
        for child in ["workspaces/inner", "state", "cache", "other"] {
            std::fs::create_dir_all(base.join(child)).unwrap();
        }
        (directory, base)
    }

    fn writable_root(name: &str, path: impl Into<PathBuf>) -> WritableRootConfig {
        WritableRootConfig {
            name: name.to_owned(),
            path: path.into(),
        }
    }

    fn writable_root_refusal(base: &Path, roots: &[WritableRootConfig], uid: u32) -> String {
        check_writable_roots(roots, &base.join("workspaces"), &base.join("state"), uid)
            .expect_err("a declaration the driver cannot bind stops startup")
            .to_string()
    }

    #[test]
    fn writable_roots_that_are_owned_directories_clear_of_both_roots_start() {
        let (_directory, base) = writable_root_tree();
        let uid = nix::unistd::Uid::effective().as_raw();
        check_writable_roots(&[], &base.join("workspaces"), &base.join("state"), uid).unwrap();
        check_writable_roots(
            &[
                writable_root("cache", base.join("cache")),
                writable_root("other", base.join("other")),
            ],
            &base.join("workspaces"),
            &base.join("state"),
            uid,
        )
        .unwrap();
        // A workspace root that does not exist yet is compared as it will be created.
        check_writable_roots(
            &[writable_root("cache", base.join("cache"))],
            &base.join("later/workspaces"),
            &base.join("state"),
            uid,
        )
        .unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One case per startup condition, each on its own fixture path.
    fn each_writable_root_startup_condition_is_refused_naming_the_root() {
        let (_directory, base) = writable_root_tree();
        let uid = nix::unistd::Uid::effective().as_raw();
        std::fs::write(base.join("file"), "x").unwrap();
        std::os::unix::fs::symlink(base.join("cache"), base.join("linked")).unwrap();
        std::os::unix::fs::symlink(&base, base.join("cache/up")).unwrap();
        let cases: Vec<(Vec<WritableRootConfig>, u32, &str, &str)> = vec![
            (
                vec![writable_root("-cache", base.join("cache"))],
                uid,
                "\"-cache\"",
                "is not a valid root name",
            ),
            (
                vec![
                    writable_root("cache", base.join("cache")),
                    writable_root("cache", base.join("other")),
                ],
                uid,
                "\"cache\"",
                "is declared more than once",
            ),
            (
                vec![writable_root("cache", "srv/cache")],
                uid,
                "\"cache\"",
                "does not name an absolute path",
            ),
            (
                vec![writable_root("cache", base.join("other/../cache"))],
                uid,
                "\"cache\"",
                "does not name a path in normal form",
            ),
            (
                vec![writable_root(
                    "cache",
                    format!("{}/./cache", base.display()),
                )],
                uid,
                "\"cache\"",
                "does not name a path in normal form",
            ),
            (
                vec![writable_root("cache", format!("{}/cache/", base.display()))],
                uid,
                "\"cache\"",
                "does not name a path in normal form",
            ),
            (
                vec![writable_root("cache", base.join("absent"))],
                uid,
                "\"cache\"",
                "does not exist",
            ),
            (
                vec![writable_root("cache", base.join("file"))],
                uid,
                "\"cache\"",
                "is not a directory",
            ),
            (
                vec![writable_root("cache", base.join("linked"))],
                uid,
                "\"cache\"",
                "is not a directory",
            ),
            (
                vec![writable_root("cache", base.join("cache/up/other"))],
                uid,
                "\"cache\"",
                "is reached through a symlink",
            ),
            (
                vec![writable_root("cache", base.join("cache"))],
                uid.wrapping_add(1),
                "\"cache\"",
                "is not owned by the daemon's uid",
            ),
            (
                vec![writable_root("cache", base.join("workspaces/inner"))],
                uid,
                "\"cache\"",
                "lies under the workspace root",
            ),
            (
                vec![writable_root("cache", base.clone())],
                uid,
                "\"cache\"",
                "contains the workspace root",
            ),
            (
                vec![writable_root("cache", base.join("workspaces"))],
                uid,
                "\"cache\"",
                "lies under the workspace root",
            ),
            (
                vec![writable_root("cache", base.join("state"))],
                uid,
                "\"cache\"",
                "lies under the state directory",
            ),
        ];
        for (roots, uid, name, condition) in cases {
            let refusal = writable_root_refusal(&base, &roots, uid);
            assert!(
                refusal.starts_with("config.writable-root-invalid: writable root "),
                "{refusal}"
            );
            assert!(refusal.contains(name), "{refusal} names {name}");
            assert!(refusal.contains(condition), "{refusal} states {condition}");
        }
        // The state directory contained by a root, apart from the workspace root.
        let state = base.join("cache/state");
        std::fs::create_dir_all(&state).unwrap();
        let refusal = check_writable_roots(
            &[writable_root("cache", base.join("cache"))],
            &base.join("workspaces"),
            &state,
            uid,
        )
        .unwrap_err()
        .to_string();
        assert!(
            refusal.contains("contains the state directory"),
            "{refusal}"
        );
    }

    /// A daemon configuration over the writable-root tree whose protected paths all lie in
    /// `other`, with `cache` declared writable; each case moves one protected path into `cache`.
    fn protected_config(base: &Path) -> DaemonConfig {
        let mut config = DaemonConfig::minimum(
            base.join("other/api.sock"),
            base.join("state/state.db"),
            base.join("workspaces"),
            "d",
            vec![1],
        );
        config.bubblewrap = base.join("other/bwrap");
        config.writable_roots = vec![writable_root("cache", base.join("cache"))];
        config
    }

    fn protected_refusal(config: &DaemonConfig) -> String {
        check_writable_roots_clear_of(&config.writable_roots, &protected_host_paths(config))
            .expect_err("a writable root holding a protected path stops startup")
            .to_string()
    }

    fn assert_protected_refusal(config: &DaemonConfig, what: &str) {
        let refusal = protected_refusal(config);
        assert!(
            refusal.starts_with("config.writable-root-invalid: writable root \"cache\" contains "),
            "{refusal}"
        );
        assert!(refusal.contains(what), "{refusal} names {what}");
    }

    #[test]
    fn a_writable_root_clear_of_every_protected_path_starts() {
        let (_directory, base) = writable_root_tree();
        let mut config = protected_config(&base);
        config.secret_slots = vec![SecretSlot {
            name: "token".to_owned(),
            path: base.join("other/token"),
        }];
        config.ca_bundle = Some(base.join("other/ca.pem"));
        config.cgroup_root = Some(base.join("other/cgroup"));
        check_writable_roots_clear_of(&config.writable_roots, &protected_host_paths(&config))
            .unwrap();
    }

    #[test]
    fn a_writable_root_containing_the_daemon_executable_is_refused() {
        let (_directory, base) = writable_root_tree();
        let mut config = protected_config(&base);
        let executable = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        config.writable_roots = vec![writable_root(
            "cache",
            executable.parent().expect("executable directory"),
        )];
        assert_protected_refusal(&config, "the daemon executable");
    }

    #[test]
    fn a_writable_root_path_that_is_not_utf8_is_refused_naming_the_root() {
        use std::os::unix::ffi::OsStrExt as _;
        let (_directory, base) = writable_root_tree();
        let uid = nix::unistd::Uid::effective().as_raw();
        let path = base.join(std::ffi::OsStr::from_bytes(b"cache-\xff"));
        std::fs::create_dir(&path).unwrap();
        let refusal = writable_root_refusal(&base, &[writable_root("cache", path)], uid);
        assert!(
            refusal.starts_with("config.writable-root-invalid: writable root \"cache\" "),
            "{refusal}"
        );
        assert!(refusal.contains("does not name a UTF-8 path"), "{refusal}");
    }

    #[test]
    fn a_writable_root_containing_the_api_socket_is_refused() {
        let (_directory, base) = writable_root_tree();
        let mut config = protected_config(&base);
        config.socket = base.join("cache/api.sock");
        assert_protected_refusal(&config, "the API socket");
    }

    #[test]
    fn a_writable_root_containing_the_bubblewrap_binary_is_refused() {
        let (_directory, base) = writable_root_tree();
        let mut config = protected_config(&base);
        config.bubblewrap = base.join("cache/bin/bwrap");
        assert_protected_refusal(&config, "the bubblewrap binary");
        // Reached through a symlink outside the root: the resolved target is what runs.
        std::fs::write(base.join("cache/bwrap"), "x").unwrap();
        std::os::unix::fs::symlink(base.join("cache/bwrap"), base.join("other/bwrap")).unwrap();
        config.bubblewrap = base.join("other/bwrap");
        assert_protected_refusal(&config, "the bubblewrap binary");
    }

    #[test]
    fn a_writable_root_containing_a_secret_slot_source_is_refused() {
        let (_directory, base) = writable_root_tree();
        let mut config = protected_config(&base);
        config.secret_slots = vec![
            SecretSlot {
                name: "clear".to_owned(),
                path: base.join("other/clear"),
            },
            SecretSlot {
                name: "token".to_owned(),
                path: base.join("cache/token"),
            },
        ];
        assert_protected_refusal(&config, "the source of secret slot \"token\"");
    }

    #[test]
    fn a_writable_root_containing_the_ca_bundle_is_refused() {
        let (_directory, base) = writable_root_tree();
        let mut config = protected_config(&base);
        config.ca_bundle = Some(base.join("cache/ca.pem"));
        assert_protected_refusal(&config, "the CA bundle");
    }

    #[test]
    fn a_writable_root_containing_or_equal_to_the_delegated_cgroup_root_is_refused() {
        let (_directory, base) = writable_root_tree();
        let mut config = protected_config(&base);
        config.cgroup_root = Some(base.join("cache/delegated"));
        assert_protected_refusal(&config, "the delegated cgroup root");
        config.cgroup_root = Some(base.join("cache"));
        assert_protected_refusal(&config, "the delegated cgroup root");
    }

    #[test]
    fn writable_roots_move_the_configuration_generation_only_when_declared() {
        let base = DaemonConfig::minimum("/srv/s.sock", "/srv/state/s.db", "/srv/ws", "d", vec![1]);
        let mut declared = base.clone();
        declared.writable_roots = vec![writable_root("cache", "/srv/cache")];
        let mut moved = base.clone();
        moved.writable_roots = vec![writable_root("cache", "/srv/elsewhere")];
        let mut renamed = base.clone();
        renamed.writable_roots = vec![writable_root("tools", "/srv/cache")];
        let generations = [&base, &declared, &moved, &renamed].map(configuration_generation);
        assert_eq!(
            generations.iter().collect::<BTreeSet<_>>().len(),
            4,
            "{generations:?}"
        );
    }

    /// `story:aperture-state-outside-workspace-root`: the daemon keeps aperture run state in its
    /// own owner-private state directory, keyed by its state identity, not in the workspace root.
    #[test]
    fn aperture_state_lives_beside_the_durable_state_not_in_the_workspace_root() {
        let state = Path::new("/srv/substrate/state/state.db");
        let root = aperture_state_root(state);
        assert_eq!(root, Path::new("/srv/substrate/state/state.apertures"));
        assert_eq!(root.parent(), state.parent());
        assert_ne!(
            aperture_state_root(Path::new("/srv/substrate/state/other.db")),
            root,
            "two state identities in one directory share aperture state"
        );
    }

    /// The production unix accept-and-serve loop, over a route that upgrades to a WebSocket and
    /// keeps its socket open the way a session attachment does.
    ///
    /// The route takes a slot from `stream_slots` before it answers `101` and refuses `429` when
    /// none is free, as every served stream family does with its own cap (ADR 0042), so a case can
    /// observe both budgets: the transport's and the stream's.
    async fn serve_attachable_unix_listener(
        listener: UnixListener,
        allowed: BTreeSet<u32>,
        limits: Arc<ConnectionLimits>,
        policy: UnixTransportPolicy,
        stream_slots: Arc<Semaphore>,
    ) {
        loop {
            let AcceptedConnection { stream, permit, .. } =
                accept_authorized(&listener, &allowed, &limits, policy.accept_retry_delay).await;
            let stream_slots = Arc::clone(&stream_slots);
            let service = Router::new().route(
                "/attach",
                axum::routing::get(
                    |transport: Option<Extension<TransportPermit>>,
                     upgrade: axum::extract::ws::WebSocketUpgrade| async move {
                        // The stream's own admission, taken before the `101` as every served
                        // stream takes its permit in the handler.
                        let Ok(stream_slot) = stream_slots.try_acquire_owned() else {
                            return StatusCode::TOO_MANY_REQUESTS.into_response();
                        };
                        let stream_slot = FixtureStreamSlot(stream_slot);
                        // The hand-over the three production routes make, made here too: the
                        // admission this connection was accepted under is taken out of the request,
                        // moved into the upgraded task and handed to the stream slot there, so what
                        // this fixture measures is the production mechanism rather than one written
                        // for the test.
                        let transport_admission = transport.map(|Extension(permit)| permit);
                        // Bounded like every served upgrade, so the crate-wide check in
                        // `app::metrics::tests::every_websocket_upgrade_declares_its_frame_message_and_lifetime_bounds`
                        // holds for fixtures too. It reads `src/` recursively and does not know a
                        // test harness from a route, which is the strict reading and the right
                        // one: a fixture that may run unbounded is a fixture that does not
                        // resemble the thing it stands in for.
                        upgrade
                            .max_frame_size(1_024)
                            .max_message_size(1_024)
                            .on_upgrade(move |mut socket| async move {
                                // A fixture carries a lifetime for the same reason a route does:
                                // the crate-wide check reads `src/` recursively and cannot tell a
                                // harness from a served route. A fixture allowed to run unbounded
                                // does not resemble the thing it stands in for.
                                struct FixturePolicy {
                                    lifetime: std::time::Duration,
                                }
                                hand_over_transport(transport_admission, &stream_slot);
                                let _stream_slot = stream_slot;
                                let policy = FixturePolicy {
                                    lifetime: std::time::Duration::from_secs(30),
                                };
                                let _ = tokio::time::timeout(policy.lifetime, async move {
                                    // One frame proves the upgraded socket is live; it then stays
                                    // open.
                                    let _ =
                                        socket.send(axum::extract::ws::Message::from("live")).await;
                                    while socket.recv().await.is_some() {}
                                })
                                .await;
                            })
                            .into_response()
                    },
                ),
            );
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let connection = http1_builder(policy)
                    .serve_connection(
                        io,
                        TowerToHyperService::new(admitted_service(&permit, service)),
                    )
                    .with_upgrades();
                let _ = enforce_connection_lifetime(permit, policy.connection_lifetime, connection)
                    .await;
            });
        }
    }

    /// The fixture route's stream permit: a slot of the stream cap the listener was given.
    struct FixtureStreamSlot(#[allow(dead_code)] OwnedSemaphorePermit);

    impl StreamSlot for FixtureStreamSlot {}

    /// What one plain HTTP request on a brand new connection is answered with, or `None` when the
    /// daemon served nothing at all.
    ///
    /// A connection refused at capacity is dropped by `accept_authorized` before anything reads
    /// it, which the kernel reports to the client as an orderly end of file or as a reset,
    /// depending on whether the client's request bytes were still queued. Both are the same
    /// observation — nothing was served — and neither is confusable with the bytes of a response.
    async fn ask_over_a_new_connection(
        socket_path: &Path,
        deadline: std::time::Duration,
    ) -> Option<String> {
        let mut stream = UnixStream::connect(socket_path)
            .await
            .expect("connect a client");
        stream
            .write_all(b"GET /attach HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("send a plain request");
        let mut served = [0_u8; 128];
        match tokio::time::timeout(deadline, stream.read(&mut served))
            .await
            .expect("the connection is refused or served within the deadline")
        {
            Ok(0) => None,
            Ok(read) => Some(String::from_utf8_lossy(&served[..read]).into_owned()),
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => None,
            Err(error) => panic!("read the connection: {error}"),
        }
    }

    /// The response head an upgrade request on a brand new connection is answered with, and the
    /// connection, or `None` when the daemon served nothing at all (refused at accept, as
    /// `ask_over_a_new_connection` describes).
    async fn upgrade_over_a_new_connection(
        socket_path: &Path,
        deadline: std::time::Duration,
    ) -> Option<(String, UnixStream)> {
        let mut stream = UnixStream::connect(socket_path)
            .await
            .expect("connect the upgrading client");
        stream
            .write_all(
                format!(
                    "GET /attach HTTP/1.1\r\n\
                     Host: localhost\r\n\
                     Connection: Upgrade\r\n\
                     Upgrade: websocket\r\n\
                     Sec-WebSocket-Version: 13\r\n\
                     Sec-WebSocket-Key: {HANDSHAKE_KEY}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .ok()?;
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 4_096, "bounded response head");
            match tokio::time::timeout(deadline, stream.read_exact(&mut byte))
                .await
                .expect("the connection is refused or served within the deadline")
            {
                Ok(_) => head.push(byte[0]),
                Err(error)
                    if head.is_empty()
                        && matches!(
                            error.kind(),
                            io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
                        ) =>
                {
                    return None;
                }
                Err(error) => panic!("read the response head: {error}"),
            }
        }
        Some((
            String::from_utf8(head).expect("utf-8 response head"),
            stream,
        ))
    }

    /// The first upgrade answer on a new connection, polled past refusals at accept: a plain
    /// connection that just ended may still hold its transport slot until the server observes the
    /// closed peer.
    async fn upgrade_when_admitted(
        socket_path: &Path,
        deadline: std::time::Duration,
    ) -> (String, UnixStream) {
        for _ in 0..100 {
            if let Some(answer) = upgrade_over_a_new_connection(socket_path, deadline).await {
                return answer;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("no upgrade request was admitted at the transport within two seconds");
    }

    /// **An upgraded connection hands its transport slot to its stream** (ADR 0042, decisions 1
    /// and 2). hyper resolves an upgradeable connection future when it hands the socket to the
    /// upgrade, and from then on the socket is counted by its stream's cap, not the transport's.
    ///
    /// Measured here rather than inferred, in three steps, with the per-uid transport budget and
    /// the fixture route's stream cap both set to one:
    ///
    /// 1. with an upgraded socket provably still serving, the next plain connection from that uid
    ///    is served — the transport slot came back at the upgrade;
    /// 2. a second upgrade from that uid is admitted at the transport and refused `429` by the
    ///    stream cap — the upgraded socket is still counted, by the stream's budget;
    /// 3. once the upgraded socket ends, a new upgrade from the same uid is `101` again — the
    ///    stream slot is held for the socket's life rather than leaked for the process's.
    ///
    /// This case was `an_upgraded_websocket_keeps_its_per_uid_connection_permit`, which asserted
    /// that finding 4's socket kept the transport slot for the stream's lifetime
    /// (`story:upgraded-connections-keep-their-permit`). ADR 0042 moves what counts it; step 2 is
    /// finding 4's guarantee in its new form, so the socket is never counted by neither budget.
    #[tokio::test]
    async fn an_upgraded_websocket_hands_its_per_uid_connection_permit_to_its_stream() {
        const DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

        let directory = tempfile::tempdir().expect("temporary directory");
        let socket_path = directory.path().join("substrate.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind unix socket");
        let uid = std::fs::metadata(&socket_path)
            .expect("socket metadata")
            .uid();
        let allowed = BTreeSet::from([uid]);
        // An upgrade needs HTTP/1.1 keep-alive semantics, exactly as the production policy states.
        let policy = UnixTransportPolicy {
            keep_alive: true,
            ..test_policy(4, 1)
        };
        let limits = Arc::new(ConnectionLimits::new(&allowed, policy));

        let _server = tokio::spawn(serve_attachable_unix_listener(
            listener,
            allowed,
            Arc::clone(&limits),
            policy,
            Arc::new(Semaphore::new(1)),
        ));

        let (head, mut upgrading) = upgrade_over_a_new_connection(&socket_path, DEADLINE)
            .await
            .expect("the first connection is admitted");
        assert!(
            head.starts_with("HTTP/1.1 101 "),
            "the websocket upgrade succeeded: {head}"
        );
        let mut frame = [0_u8; 6];
        tokio::time::timeout(DEADLINE, upgrading.read_exact(&mut frame))
            .await
            .expect("the upgraded socket sends its first frame")
            .expect("read the first frame");
        assert_eq!(
            &frame, b"\x81\x04live",
            "the upgraded socket is serving after the handshake"
        );

        // 1. The transport slot came back at the upgrade: the hand-over runs before the first
        // frame, so the only per-uid slot is free while the upgraded socket still serves.
        let served = ask_over_a_new_connection(&socket_path, DEADLINE).await;
        assert!(
            served
                .as_deref()
                .is_some_and(|answer| answer.starts_with("HTTP/1.1 ")),
            "the upgraded socket still holds the only per-uid transport slot, so a second \
             connection from that uid was refused at accept; ADR 0042 hands it to the stream at \
             the upgrade. It answered: {served:?}"
        );

        // 2. Still counted, by the stream's cap.
        let (refused, refused_stream) = upgrade_when_admitted(&socket_path, DEADLINE).await;
        assert!(
            refused.starts_with("HTTP/1.1 429 "),
            "the upgraded socket holds the only stream slot, so a second upgrade must be refused \
             by the stream cap: {refused}"
        );
        drop(refused_stream);

        // 3. Held for the socket's life, not leaked: when the upgraded socket ends, a new upgrade
        // from the same uid is served again. Polled, because the upgraded task returns its stream
        // slot when it observes the closed peer rather than when the peer closes.
        drop(upgrading);
        let mut answer = String::new();
        for _ in 0..100 {
            let (head, again) = upgrade_when_admitted(&socket_path, DEADLINE).await;
            answer = head;
            if answer.starts_with("HTTP/1.1 101 ") {
                drop(again);
                break;
            }
            drop(again);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            answer.starts_with("HTTP/1.1 101 "),
            "the ended upgraded socket returns its stream slot, so the same uid upgrades again; \
             it answered: {answer}"
        );
    }
}
