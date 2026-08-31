#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read as _};
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::delegation::{DelegatedContextPolicy, TrustedKey};
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
            // WebSocket session attachment requires HTTP/1.1 upgrade/keep-alive semantics. The
            // outer connection lifetime remains the finite bound for idle and upgraded peers.
            keep_alive: true,
        }
    }
}

struct ConnectionPermit {
    _global: OwnedSemaphorePermit,
    _subject: OwnedSemaphorePermit,
}

struct ConnectionLimits {
    global: Arc<Semaphore>,
    by_uid: BTreeMap<u32, Arc<Semaphore>>,
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
            by_uid,
        }
    }

    fn acquire(&self, uid: u32) -> Option<ConnectionPermit> {
        let subject_limit = Arc::clone(self.by_uid.get(&uid)?);
        let global = Arc::clone(&self.global).try_acquire_owned().ok()?;
        let subject = subject_limit.try_acquire_owned().ok()?;
        Some(ConnectionPermit {
            _global: global,
            _subject: subject,
        })
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
    permit: ConnectionPermit,
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

async fn enforce_connection_lifetime<P, F, T, E>(
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

struct TcpConnectionPermit {
    _global: OwnedSemaphorePermit,
    _source: OwnedSemaphorePermit,
}

struct TcpConnectionLimits {
    global: Arc<Semaphore>,
    by_source: Mutex<BTreeMap<IpAddr, TcpSourceLimit>>,
    per_source: usize,
    max_sources: usize,
    sequence: AtomicU64,
}

struct TcpSourceLimit {
    semaphore: Arc<Semaphore>,
    last_used: u64,
}

impl TcpConnectionLimits {
    fn production() -> Self {
        Self {
            global: Arc::new(Semaphore::new(128)),
            by_source: Mutex::new(BTreeMap::new()),
            per_source: 16,
            max_sources: 1_024,
            sequence: AtomicU64::new(1),
        }
    }

    fn acquire(&self, source: IpAddr) -> Option<TcpConnectionPermit> {
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
        Some(TcpConnectionPermit {
            _global: global,
            _source: source,
        })
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
    pub cgroup_root: Option<PathBuf>,
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
            cgroup_root: None,
            bubblewrap: PathBuf::from("/usr/bin/bwrap"),
            event_retention: 10_000,
            secret_slots: Vec::new(),
            egress_apertures: Vec::new(),
            ca_bundle: None,
            delegated_context_keys: Vec::new(),
            require_delegated_context: false,
            tcp: None,
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
    if config.tcp.is_none() && config.allow_uids.is_empty() {
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
    if config.tcp.is_none() {
        prepare_socket(&config.socket)?;
    }
    if config.event_retention == 0 {
        bail!("event retention must be nonzero");
    }
    check_secret_slots(&config.secret_slots)?;
    // Resolved once, here, and pinned for the process's lifetime: the sandbox gets no resolver, so
    // a name that only resolves later resolves nowhere at all (ADR 0013).
    let egress_apertures = resolve_egress_apertures(&config.egress_apertures)?;
    check_ca_bundle(config.ca_bundle.as_deref())?;
    // Resolved before any listener exists: a trust anchor that only becomes usable later would let
    // the daemon serve mutations it could not attribute (ADR 0011).
    let delegated_context = resolve_delegated_context(
        &config.delegated_context_keys,
        config.require_delegated_context,
    )?;
    let store = Arc::new(
        Store::open_with_event_retention(&config.state, config.event_retention)
            .context("open durable state")?,
    );
    std::fs::set_permissions(&config.state, std::fs::Permissions::from_mode(0o600))
        .context("restrict durable state database")?;
    let mut host_config = HostConfig::minimum(&config.workspaces);
    host_config.config_generation = configuration_generation(&config);
    host_config.cgroup_root = config.cgroup_root;
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
    host_config.ca_bundle = config.ca_bundle;
    let driver = HostDriver::open(host_config).context("open host driver")?;
    let app = App::with_delegated_context(
        store,
        driver,
        config.deployment,
        Arc::new(SystemAuthority),
        delegated_context,
    );
    app.sweep_expired().await;
    let sweeper_app = Arc::clone(&app);
    let lease_sweeper = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                () = sweeper_app.maintenance_notified() => {}
            }
            sweeper_app.sweep_expired().await;
        }
    });
    if let Some(tcp) = config.tcp.as_ref() {
        let result = serve_tcp(Arc::clone(&app), tcp).await;
        lease_sweeper.abort();
        return result;
    }
    let listener = UnixListener::bind(&config.socket).context("bind unix socket")?;
    let socket_cleanup = SocketCleanup(config.socket.clone());
    std::fs::set_permissions(&config.socket, std::fs::Permissions::from_mode(0o600))
        .context("restrict unix socket")?;
    let allowed = config.allow_uids.into_iter().collect::<BTreeSet<_>>();
    let transport_policy = UnixTransportPolicy::production();
    let connection_limits = ConnectionLimits::new(&allowed, transport_policy);
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
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
                let service = router(Arc::clone(&app)).layer(Extension(identity));
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let builder = http1_builder(transport_policy);
                    let connection = builder
                        .serve_connection(io, TowerToHyperService::new(service))
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
            signal = &mut shutdown => {
                signal?;
                break;
            }
        }
    }
    lease_sweeper.abort();
    drop(listener);
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

async fn serve_tcp(app: Arc<App>, config: &TcpDaemonConfig) -> anyhow::Result<()> {
    if !config.development_only {
        bail!("static-bearer TCP is available only in the explicit development profile");
    }
    if !config.private_overlay {
        bail!("hosted TCP requires an explicitly configured private overlay");
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
    let service = router(app)
        .layer(Extension(identity))
        .layer(middleware::from_fn_with_state(auth, require_tcp_bearer));
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
                        .serve_connection(io, TowerToHyperService::new(service))
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
                signal?;
                break;
            }
        }
    }
    drop(listener);
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
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::convert::Infallible;

    use tokio::io::AsyncWriteExt as _;

    use super::*;

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
            by_source: Mutex::new(BTreeMap::new()),
            per_source: 1,
            max_sources: 2,
            sequence: AtomicU64::new(1),
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

        drop(uid_1000);
        assert!(limits.acquire(1000).is_some());
        drop(uid_1001);
    }

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
}
