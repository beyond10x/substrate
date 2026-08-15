#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{App, Identity, router};
use anyhow::{Context as _, bail};
use axum::Extension;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use substrate_host::{HostConfig, HostDriver};
use substrate_store::Store;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{info, warn};

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

async fn enforce_connection_lifetime<F, T, E>(
    permit: ConnectionPermit,
    lifetime: std::time::Duration,
    connection: F,
) -> Result<Result<T, E>, tokio::time::error::Elapsed>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    let _permit = permit;
    tokio::time::timeout(lifetime, connection).await
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
    if config.allow_uids.is_empty() {
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
    let _instance_lock = lock_instance(&config.socket)?;
    prepare_socket(&config.socket)?;
    if let Some(parent) = config.state.parent() {
        std::fs::create_dir_all(parent).context("create state directory")?;
    }
    if config.event_retention == 0 {
        bail!("event retention must be nonzero");
    }
    let store = Arc::new(
        Store::open_with_event_retention(&config.state, config.event_retention)
            .context("open durable state")?,
    );
    let mut host_config = HostConfig::minimum(&config.workspaces);
    host_config.cgroup_root = config.cgroup_root;
    host_config.bubblewrap = config.bubblewrap;
    host_config.event_retention = config.event_retention;
    let driver = HostDriver::open(host_config).context("open host driver")?;
    let app = App::new(store, driver, config.deployment);
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
    let listener = UnixListener::bind(&config.socket).context("bind unix socket")?;
    let socket_cleanup = SocketCleanup(config.socket.clone());
    std::fs::set_permissions(&config.socket, std::fs::Permissions::from_mode(0o600))
        .context("restrict unix socket")?;
    let allowed = config.allow_uids.into_iter().collect::<BTreeSet<_>>();
    let transport_policy = UnixTransportPolicy::production();
    let connection_limits = ConnectionLimits::new(&allowed, transport_policy);
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
            signal = tokio::signal::ctrl_c() => {
                signal.context("install shutdown signal")?;
                break;
            }
        }
    }
    lease_sweeper.abort();
    drop(listener);
    drop(socket_cleanup);
    Ok(())
}

fn lock_instance(socket: &Path) -> anyhow::Result<InstanceLock> {
    let path = socket.with_extension("lock");
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
    let file = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
        .map_err(|(_, error)| error)
        .context("another substrate daemon owns this socket identity")?;
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
}
