//! Hosted Identity admission for the production TLS listener (ADR 0026).

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::BufReader;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use http_body_util::{BodyExt as _, Limited};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use substrate_wire::{
    AUTH_AUTHORITY_INVALID, AUTH_AUTHORITY_UNAVAILABLE, AUTH_CREDENTIAL_ABSENT, AUTH_SCOPE_DENIED,
};
use substrate_wire::{ErrorClass, ErrorDetail, Failure};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use ulid::Ulid;

use crate::app::{Admission, Telemetry};
use crate::{CONTRACT_BUNDLE, CONTRACT_BUNDLE_SHA256, Identity};

const AUDIENCE: &str = "urn:b10x:substrate";
const AUTHORITY_PATH: &str = "/v1/access-authority";
const MAX_CA_BUNDLE_BYTES: u64 = 1024 * 1024;
const MAX_AUTHORITY_BYTES: usize = 64 * 1024;
const RESOLUTION_TIMEOUT: Duration = Duration::from_secs(5);
/// Idle Identity connections held for reuse (ADR 0040).
const MAX_IDLE_IDENTITY_CONNECTIONS: usize = 8;
/// An idle Identity connection older than this is closed, never used (ADR 0040).
const IDENTITY_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_AUTHORITY_LIFETIME_SECONDS: i64 = 5 * 60;
const CLOCK_SKEW_SECONDS: i64 = 30;
const ALLOWED_SCOPES: [&str; 3] = ["exec", "observe", "workspaces"];

/// Identity relying-party configuration for the production listener (ADR 0026).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedIdentityConfig {
    /// Exact HTTPS public origin Identity writes into `iss`.
    pub origin: String,
    /// PEM roots used only to authenticate that Identity origin.
    pub ca_bundle: PathBuf,
}

pub(crate) struct HostedAdmission {
    origin: String,
    authority: String,
    host: String,
    port: u16,
    server_name: ServerName<'static>,
    connector: TlsConnector,
    /// Connections that completed a clean exchange, most recent last (ADR 0040).
    idle: Arc<Mutex<IdlePool>>,
    /// Where admission results and resolution times are counted (ADR 0045); absent, nothing is.
    telemetry: Option<Arc<Telemetry>>,
}

/// The idle half of the Identity connection pool and whether its reaper is running.
#[derive(Default)]
struct IdlePool {
    entries: Vec<IdleConnection>,
    reaper: bool,
}

fn lock_pool(pool: &Mutex<IdlePool>) -> std::sync::MutexGuard<'_, IdlePool> {
    pool.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Close each idle connection once it has been idle for `IDENTITY_IDLE_TIMEOUT`, whether or not
/// another resolution arrives. One reaper runs while the pool holds a connection; it holds the
/// pool only weakly and exits when the pool is empty or gone.
fn spawn_reaper(pool: Weak<Mutex<IdlePool>>) {
    tokio::spawn(async move {
        loop {
            let deadline = {
                let Some(pool) = pool.upgrade() else {
                    return;
                };
                let mut state = lock_pool(&pool);
                state.entries.retain(IdleConnection::usable);
                let Some(oldest) = state.entries.iter().map(|entry| entry.since).min() else {
                    state.reaper = false;
                    return;
                };
                oldest + IDENTITY_IDLE_TIMEOUT
            };
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        }
    });
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolvedAuthority {
    iss: String,
    sub: String,
    aud: String,
    iat: i64,
    nbf: i64,
    exp: i64,
    jti: String,
    act: ResolvedActor,
    scope: String,
    principal_kind: String,
    tenant_id: String,
    #[serde(rename = "email")]
    _email: Option<String>,
    #[serde(rename = "groups")]
    _groups: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolvedActor {
    sub: String,
}

struct ConnectionTask(tokio::task::JoinHandle<()>);

impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One HTTP/1.1 connection to Identity. Dropping it closes the connection.
struct IdentityConnection {
    sender: http1::SendRequest<Body>,
    _task: ConnectionTask,
}

struct IdleConnection {
    connection: IdentityConnection,
    since: Instant,
}

impl IdleConnection {
    fn usable(&self) -> bool {
        self.since.elapsed() < IDENTITY_IDLE_TIMEOUT && !self.connection.sender.is_closed()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmissionFailure {
    Absent,
    Invalid,
    Scope,
    Unavailable,
}

impl HostedAdmission {
    /// Resolve and validate the exact Identity origin and its explicit trust roots before bind.
    pub(crate) fn load(config: &HostedIdentityConfig) -> anyhow::Result<Self> {
        let uri = Uri::from_str(&config.origin)
            .map_err(|_| anyhow!("auth.listener-config-invalid: Identity origin is invalid"))?;
        if uri.scheme_str() != Some("https")
            || uri.authority().is_none()
            || uri
                .authority()
                .is_some_and(|value| value.as_str().contains('@'))
            || uri.query().is_some()
            || uri.path() != "/"
        {
            bail!("auth.listener-config-invalid: Identity origin must be one exact HTTPS origin");
        }
        let authority = uri
            .authority()
            .expect("checked authority")
            .as_str()
            .to_owned();
        let host = uri
            .host()
            .ok_or_else(|| anyhow!("auth.listener-config-invalid: Identity origin has no host"))?
            .to_owned();
        let port = uri.port_u16().unwrap_or(443);
        if port == 0 {
            bail!("auth.listener-config-invalid: Identity origin has no usable port");
        }
        let server_name = ServerName::try_from(host.clone()).map_err(|_| {
            anyhow!("auth.listener-config-invalid: Identity origin has no valid TLS server name")
        })?;
        let connector = load_connector(&config.ca_bundle)?;
        Ok(Self {
            origin: config.origin.trim_end_matches('/').to_owned(),
            authority,
            host,
            port,
            server_name,
            connector,
            idle: Arc::new(Mutex::new(IdlePool::default())),
            telemetry: None,
        })
    }

    /// Counts every admission this listener decides into the daemon's telemetry.
    pub(crate) fn with_telemetry(mut self, telemetry: Arc<Telemetry>) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    async fn admit(
        &self,
        credential: &str,
        required: Option<&str>,
    ) -> Result<Identity, AdmissionFailure> {
        let authority = tokio::time::timeout(RESOLUTION_TIMEOUT, self.resolve(credential))
            .await
            .map_err(|_| AdmissionFailure::Unavailable)??;
        let scopes = validate_authority(&authority, &self.origin)?;
        if required.is_some_and(|required| !scopes.contains(required)) {
            return Err(AdmissionFailure::Scope);
        }
        let subject = hosted_reference(&self.origin, &authority.tenant_id, &authority.sub);
        let actor = hosted_reference(&self.origin, &authority.tenant_id, &authority.act.sub);
        Ok(Identity {
            principal: Some(subject.clone()),
            subject,
            actor,
        })
    }

    /// Resolve one credential online. Every call reaches Identity; only the connection is reused
    /// (ADR 0040).
    async fn resolve(&self, credential: &str) -> Result<ResolvedAuthority, AdmissionFailure> {
        let mut authorization = HeaderValue::from_str(&format!("Bearer {credential}"))
            .map_err(|_| AdmissionFailure::Invalid)?;
        authorization.set_sensitive(true);
        if let Some(connection) = self.checkout()
            && let Some(result) = self.exchange(connection, &authorization).await
        {
            return result;
        }
        let connection = self.connect().await?;
        self.exchange(connection, &authorization)
            .await
            .unwrap_or(Err(AdmissionFailure::Unavailable))
    }

    async fn connect(&self) -> Result<IdentityConnection, AdmissionFailure> {
        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|_| AdmissionFailure::Unavailable)?;
        let tls = self
            .connector
            .connect(self.server_name.clone(), tcp)
            .await
            .map_err(|_| AdmissionFailure::Unavailable)?;
        let (sender, connection) = http1::handshake(TokioIo::new(tls))
            .await
            .map_err(|_| AdmissionFailure::Unavailable)?;
        let task = ConnectionTask(tokio::spawn(async move {
            let _ = connection.await;
        }));
        Ok(IdentityConnection {
            sender,
            _task: task,
        })
    }

    /// One request on one connection, `None` when the connection failed before Identity answered.
    /// The connection returns to the pool only after a clean exchange; on every other path it is
    /// dropped here, which closes it.
    async fn exchange(
        &self,
        mut connection: IdentityConnection,
        authorization: &HeaderValue,
    ) -> Option<Result<ResolvedAuthority, AdmissionFailure>> {
        if connection.sender.ready().await.is_err() {
            return None;
        }
        let Ok(request) = Request::builder()
            .method(Method::GET)
            .uri(AUTHORITY_PATH)
            .header(header::HOST, &self.authority)
            .header(header::AUTHORIZATION, authorization.clone())
            .header("x-b10x-audience", AUDIENCE)
            .body(Body::empty())
        else {
            return Some(Err(AdmissionFailure::Unavailable));
        };
        match connection.sender.try_send_request(request).await {
            Ok(response) => Some(self.read_authority(connection, response).await),
            // hyper hands the request back only when it was never written to the connection, so
            // Identity cannot have received it and a retry cannot resolve it twice (ADR 0040).
            Err(error) if error.message().is_some() => None,
            Err(_) => Some(Err(AdmissionFailure::Unavailable)),
        }
    }

    async fn read_authority(
        &self,
        connection: IdentityConnection,
        response: hyper::Response<hyper::body::Incoming>,
    ) -> Result<ResolvedAuthority, AdmissionFailure> {
        if response.status() != StatusCode::OK {
            return if response.status().is_server_error()
                || matches!(
                    response.status(),
                    StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
                ) {
                Err(AdmissionFailure::Unavailable)
            } else {
                Err(AdmissionFailure::Invalid)
            };
        }
        if response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| !value.eq_ignore_ascii_case("application/json"))
        {
            return Err(AdmissionFailure::Invalid);
        }
        let keep_alive = response_keeps_alive(&response);
        let bytes = Limited::new(response.into_body(), MAX_AUTHORITY_BYTES)
            .collect()
            .await
            .map_err(|_| AdmissionFailure::Invalid)?
            .to_bytes();
        let authority = serde_json::from_slice(&bytes).map_err(|_| AdmissionFailure::Invalid)?;
        if keep_alive {
            self.checkin(connection);
        }
        Ok(authority)
    }

    /// The most recently returned usable idle connection; expired or closed ones are dropped.
    fn checkout(&self) -> Option<IdentityConnection> {
        let mut idle = lock_pool(&self.idle);
        idle.entries.retain(IdleConnection::usable);
        idle.entries.pop().map(|entry| entry.connection)
    }

    fn checkin(&self, connection: IdentityConnection) {
        if connection.sender.is_closed() {
            return;
        }
        let mut idle = lock_pool(&self.idle);
        idle.entries.retain(IdleConnection::usable);
        if idle.entries.len() < MAX_IDLE_IDENTITY_CONNECTIONS {
            idle.entries.push(IdleConnection {
                connection,
                since: Instant::now(),
            });
            if !idle.reaper {
                idle.reaper = true;
                spawn_reaper(Arc::downgrade(&self.idle));
            }
        }
    }
}

/// An HTTP/1.1 response that does not ask to close its connection.
fn response_keeps_alive<B>(response: &hyper::Response<B>) -> bool {
    response.version() == hyper::Version::HTTP_11
        && !response
            .headers()
            .get_all(header::CONNECTION)
            .iter()
            .any(|value| {
                value.to_str().map_or(true, |value| {
                    value
                        .split(',')
                        .any(|token| token.trim().eq_ignore_ascii_case("close"))
                })
            })
}

fn load_connector(path: &std::path::Path) -> anyhow::Result<TlsConnector> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| anyhow!("auth.trust-roots-invalid: cannot open Identity CA bundle"))?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("auth.trust-roots-invalid: cannot inspect Identity CA bundle"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_CA_BUNDLE_BYTES {
        bail!("auth.trust-roots-invalid: Identity CA bundle must be one bounded regular file");
    }
    let mut roots = RootCertStore::empty();
    let mut reader = BufReader::new(file);
    let mut certificate_count = 0_usize;
    loop {
        let item = rustls_pemfile::read_one(&mut reader)
            .map_err(|_| anyhow!("auth.trust-roots-invalid: Identity CA bundle is invalid"))?;
        match item {
            Some(rustls_pemfile::Item::X509Certificate(certificate)) => {
                roots.add(certificate).map_err(|_| {
                    anyhow!("auth.trust-roots-invalid: Identity CA certificate is invalid")
                })?;
                certificate_count += 1;
            }
            Some(_) => {
                bail!(
                    "auth.trust-roots-invalid: Identity CA bundle contains non-certificate material"
                );
            }
            None => break,
        }
    }
    if certificate_count == 0 {
        bail!("auth.trust-roots-invalid: Identity CA bundle contains no certificate");
    }
    let mut client = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(roots)
        .with_no_client_auth();
    client.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsConnector::from(Arc::new(client)))
}

fn validate_authority(
    authority: &ResolvedAuthority,
    origin: &str,
) -> Result<BTreeSet<String>, AdmissionFailure> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AdmissionFailure::Invalid)?;
    let now = i64::try_from(now.as_secs()).map_err(|_| AdmissionFailure::Invalid)?;
    let lifetime_is_valid = authority
        .exp
        .checked_sub(authority.iat)
        .is_some_and(|lifetime| lifetime <= MAX_AUTHORITY_LIFETIME_SECONDS);
    if authority.iss != origin
        || authority.aud != AUDIENCE
        || authority.iat > now + CLOCK_SKEW_SECONDS
        || authority.nbf > now + CLOCK_SKEW_SECONDS
        || authority.nbf < authority.iat
        || authority.exp <= now
        || authority.exp <= authority.nbf
        || !lifetime_is_valid
        || authority.principal_kind != "human"
        || !valid_claim(&authority.sub, 512)
        || !valid_claim(&authority.act.sub, 512)
        || !valid_claim(&authority.tenant_id, 256)
        || !valid_claim(&authority.jti, 256)
    {
        return Err(AdmissionFailure::Invalid);
    }
    let split = authority.scope.split_ascii_whitespace().collect::<Vec<_>>();
    if split.is_empty()
        || split.join(" ") != authority.scope
        || split.iter().any(|scope| !ALLOWED_SCOPES.contains(scope))
    {
        return Err(AdmissionFailure::Invalid);
    }
    let scopes = split
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if scopes.len() != authority.scope.split_ascii_whitespace().count() {
        return Err(AdmissionFailure::Invalid);
    }
    Ok(scopes)
}

fn valid_claim(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value.chars().any(char::is_control)
        && value.trim() == value
}

fn hosted_reference(issuer: &str, tenant: &str, subject: &str) -> String {
    let mut digest = Sha256::new();
    for field in [issuer, tenant, subject] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field.as_bytes());
    }
    format!("hosted:{}", BASE64URL.encode(digest.finalize()))
}

pub(crate) fn required_scope(path: &str) -> Option<&'static str> {
    if path == "/v1/machine"
        || path.starts_with("/v1/metrics")
        || path.starts_with("/v1/events")
        || path.starts_with("/v1/reconciliation-snapshots")
        || path.starts_with("/v1/ops/")
    {
        Some("observe")
    } else if path.starts_with("/v1/workspaces") || path.starts_with("/v2/workspaces") {
        Some("workspaces")
    } else if path.starts_with("/v1/execs") || path.starts_with("/v1/sessions") {
        Some("exec")
    } else if path.starts_with("/v1/processes") {
        Some("process")
    } else {
        None
    }
}

fn access_credential(request: &Request) -> Result<&str, AdmissionFailure> {
    let Some(value) = request.headers().get(header::AUTHORIZATION) else {
        return Err(AdmissionFailure::Absent);
    };
    value
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| {
            value
                .strip_prefix("identity_access_v1_")
                .is_some_and(|token| {
                    token.len() == 43
                        && token
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                })
        })
        .ok_or(AdmissionFailure::Invalid)
}

pub(crate) async fn require_hosted_authority(
    State(admission): State<Arc<HostedAdmission>>,
    mut request: Request,
    next: Next,
) -> Response {
    let required = required_scope(request.uri().path());
    let (admitted, resolution) = match access_credential(&request) {
        Ok(credential) => {
            let started = Instant::now();
            let admitted = admission.admit(credential, required).await;
            (admitted, Some(started.elapsed()))
        }
        Err(failure) => (Err(failure), None),
    };
    if let Some(telemetry) = &admission.telemetry {
        let result = match &admitted {
            Ok(_) => Admission::Admitted,
            Err(AdmissionFailure::Absent) => Admission::CredentialAbsent,
            Err(AdmissionFailure::Invalid) => Admission::AuthorityInvalid,
            Err(AdmissionFailure::Scope) => Admission::ScopeDenied,
            Err(AdmissionFailure::Unavailable) => Admission::AuthorityUnavailable,
        };
        telemetry.admission(result, resolution);
    }
    let identity = match admitted {
        Ok(identity) => identity,
        Err(failure) => return refusal(request.uri().path(), failure),
    };
    request.headers_mut().remove(header::AUTHORIZATION);
    request.extensions_mut().insert(identity);
    next.run(request).await
}

fn refusal(path: &str, failure: AdmissionFailure) -> Response {
    let (status, class, code, message, retriable) = match failure {
        AdmissionFailure::Absent => (
            StatusCode::UNAUTHORIZED,
            ErrorClass::Refused,
            AUTH_CREDENTIAL_ABSENT,
            "A hosted Identity credential is required.",
            false,
        ),
        AdmissionFailure::Invalid => (
            StatusCode::UNAUTHORIZED,
            ErrorClass::Refused,
            AUTH_AUTHORITY_INVALID,
            "The hosted Identity authority is not admitted.",
            false,
        ),
        AdmissionFailure::Scope => (
            StatusCode::FORBIDDEN,
            ErrorClass::Refused,
            AUTH_SCOPE_DENIED,
            "The hosted Identity authority does not carry the route scope.",
            false,
        ),
        AdmissionFailure::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorClass::Unserved,
            AUTH_AUTHORITY_UNAVAILABLE,
            "Hosted Identity authority cannot be resolved.",
            true,
        ),
    };
    let api_version = if path.starts_with("/v2/") { "v2" } else { "v1" };
    let error = ErrorDetail {
        class,
        code: code.to_owned(),
        message: message.to_owned(),
        retriable,
        address: None,
        operation: None,
    };
    let mut response = (
        status,
        axum::Json(Failure {
            api_version: api_version.to_owned(),
            request_id: format!("req_{}", Ulid::generate()),
            error: error.clone(),
        }),
    )
        .into_response();
    // The code the request-outcome layer counts, as every other failure answer carries it.
    response.extensions_mut().insert(error);
    response.headers_mut().insert(
        "x-b10x-contract",
        CONTRACT_BUNDLE.parse().expect("static contract header"),
    );
    response.headers_mut().insert(
        "x-b10x-contract-bundle-sha256",
        CONTRACT_BUNDLE_SHA256
            .parse()
            .expect("static contract digest header"),
    );
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"substrate\", error=\"invalid_token\""),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::{
        AdmissionFailure, HostedIdentityConfig, ResolvedActor, ResolvedAuthority,
        access_credential, hosted_reference, required_scope, validate_authority,
    };
    use tempfile::TempDir;

    #[test]
    fn route_families_map_to_the_existing_scope_vocabulary() {
        for (path, expected) in [
            ("/v1/machine", Some("observe")),
            ("/v1/events/stream", Some("observe")),
            ("/v1/workspaces/ws_test", Some("workspaces")),
            ("/v2/workspaces/ws_test/tree", Some("workspaces")),
            ("/v1/execs", Some("exec")),
            ("/v1/sessions/ses_test/attach", Some("exec")),
            ("/v1/processes", Some("process")),
            ("/v1/processes/stop", Some("process")),
            ("/v1/processes/proc_test/logs", Some("process")),
            ("/v1/processes/proc_test/lease/renew", Some("process")),
            ("/v1/not-a-route", None),
        ] {
            assert_eq!(required_scope(path), expected, "{path}");
        }
    }

    #[test]
    fn identity_origin_is_exact_https_and_trust_roots_fail_closed() {
        let root = TempDir::new().expect("tempdir");
        let empty = root.path().join("empty.pem");
        std::fs::write(&empty, []).expect("empty roots");
        for origin in [
            "http://identity.test/",
            "https://user@identity.test/",
            "https://identity.test/path",
            "https://identity.test/?query=yes",
        ] {
            let result = super::HostedAdmission::load(&HostedIdentityConfig {
                origin: origin.to_owned(),
                ca_bundle: empty.clone(),
            });
            let Err(error) = result else {
                panic!("{origin} was admitted");
            };
            assert!(error.to_string().contains("auth.listener-config-invalid"));
        }
        let result = super::HostedAdmission::load(&HostedIdentityConfig {
            origin: "https://identity.test/".to_owned(),
            ca_bundle: empty,
        });
        let Err(error) = result else {
            panic!("empty roots were admitted");
        };
        assert!(error.to_string().contains("auth.trust-roots-invalid"));
    }

    fn authority() -> ResolvedAuthority {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("wall time")
            .as_secs();
        let now = i64::try_from(now).expect("wall time fits");
        ResolvedAuthority {
            iss: "https://identity.test".to_owned(),
            sub: "subject".to_owned(),
            aud: "urn:b10x:substrate".to_owned(),
            iat: now,
            nbf: now,
            exp: now + 300,
            jti: "token-id".to_owned(),
            act: ResolvedActor {
                sub: "actor".to_owned(),
            },
            scope: "exec observe workspaces".to_owned(),
            principal_kind: "human".to_owned(),
            tenant_id: "tenant".to_owned(),
            _email: None,
            _groups: Vec::new(),
        }
    }

    #[test]
    fn authority_validation_is_exact_bounded_and_closed() {
        let valid = authority();
        assert_eq!(
            validate_authority(&valid, "https://identity.test")
                .expect("valid authority")
                .into_iter()
                .collect::<Vec<_>>(),
            ["exec", "observe", "workspaces"]
        );

        let mut invalid = Vec::new();
        let mut issuer = valid.clone();
        issuer.iss = "https://other.test".to_owned();
        invalid.push(issuer);
        let mut audience = valid.clone();
        audience.aud = "urn:b10x:other".to_owned();
        invalid.push(audience);
        let mut future = valid.clone();
        future.iat += 31;
        future.nbf += 31;
        future.exp += 31;
        invalid.push(future);
        let mut inverted = valid.clone();
        inverted.nbf -= 1;
        invalid.push(inverted);
        let mut long = valid.clone();
        long.exp += 1;
        invalid.push(long);
        let mut kind = valid.clone();
        kind.principal_kind = "service".to_owned();
        invalid.push(kind);
        let mut subject = valid.clone();
        subject.sub.clear();
        invalid.push(subject);
        let mut tenant = valid.clone();
        tenant.tenant_id = " tenant".to_owned();
        invalid.push(tenant);
        for scope in ["observe  exec", "observe observe", "observe admin", ""] {
            let mut authority = valid.clone();
            authority.scope = scope.to_owned();
            invalid.push(authority);
        }
        for authority in invalid {
            assert_eq!(
                validate_authority(&authority, "https://identity.test"),
                Err(AdmissionFailure::Invalid)
            );
        }
    }

    #[test]
    fn only_the_identity_access_credential_shape_reaches_resolution() {
        let valid = format!("identity_{}_v1_{}", "access", "a".repeat(43));
        let request = axum::http::Request::builder()
            .header("authorization", format!("Bearer {valid}"))
            .body(axum::body::Body::empty())
            .expect("request");
        assert_eq!(access_credential(&request), Ok(valid.as_str()));
        for value in [
            format!("Bearer identity_{}_v1_short", "access"),
            format!("bearer identity_{}_v1_{}", "access", "a".repeat(43)),
            format!("Bearer identity_{}_v1_{}", "session", "a".repeat(43)),
        ] {
            let request = axum::http::Request::builder()
                .header("authorization", &value)
                .body(axum::body::Body::empty())
                .expect("request");
            assert_eq!(access_credential(&request), Err(AdmissionFailure::Invalid));
        }
    }

    #[test]
    fn hosted_subjects_are_stable_and_tenant_bound_without_exposing_claims() {
        let reference = hosted_reference("https://identity.test", "tenant-a", "subject-a");
        assert_eq!(
            reference,
            hosted_reference("https://identity.test", "tenant-a", "subject-a")
        );
        assert_ne!(
            reference,
            hosted_reference("https://identity.test", "tenant-b", "subject-a")
        );
        assert_ne!(
            reference,
            hosted_reference("https://identity.test", "tenant-a", "subject-b")
        );
        assert!(reference.starts_with("hosted:"));
        assert!(!reference.contains("tenant-a"));
        assert!(!reference.contains("subject-a"));
    }
}
