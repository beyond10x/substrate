//! The HTTPS client: one request per connection, each bounded in time and in body size.

use std::fmt;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::header::{self, HeaderValue};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use serde_json::{Value, json};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use zeroize::Zeroizing;

use crate::gone::PodTrace;
use crate::kubeconfig::{connector, refuse_members, text};
use crate::{API_TIMEOUT, JSON, MAX_API_BODY_BYTES, METADATA_LIST, USER_AGENT};

/// How a removal is paced: the pause between two tries, the tries in one round, and how often an
/// object whose removal is not confirmed is read again. A unit case shortens them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacing {
    /// The pause between two deletions of one round.
    pub poll: Duration,
    /// The deletions of one round.
    pub remove_attempts: u32,
    /// How soon a waiting removal notices the object is gone.
    pub notice: Duration,
}

impl Pacing {
    /// 500 ms, 60 tries and 2 s: a round of 30 s.
    pub const STANDARD: Self = Self {
        poll: Duration::from_millis(500),
        remove_attempts: 60,
        notice: Duration::from_secs(2),
    };
}

impl Default for Pacing {
    fn default() -> Self {
        Self::STANDARD
    }
}

/// Why a call has no usable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// No answer came: connect, TLS, HTTP, or the call's time bound. The API may have acted on the
    /// request all the same, so an unanswered create may have stored its object.
    Unanswered(String),
    /// The API answered `status`, which the call does not accept.
    Answered {
        /// The HTTP status.
        status: u16,
        /// The call, the status and the API's own bounded `Status.message`.
        detail: String,
    },
    /// The API answered something the call cannot read, or past a bound the call holds.
    Invalid(String),
}

impl ApiError {
    /// The status the API answered, if it answered one.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Answered { status, .. } => Some(*status),
            Self::Unanswered(_) | Self::Invalid(_) => None,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unanswered(detail) | Self::Answered { detail, .. } | Self::Invalid(detail) => {
                formatter.write_str(detail)
            }
        }
    }
}

impl std::error::Error for ApiError {}

impl From<ApiError> for String {
    fn from(error: ApiError) -> Self {
        error.to_string()
    }
}

/// How a list asks for its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListForm {
    /// Each item's `metadata` alone ([`METADATA_LIST`]).
    Metadata,
    /// Each item whole.
    Whole,
}

/// A list read whole across its pages.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    /// Every page's items, in order.
    pub items: Vec<Value>,
    /// The first page's `metadata.resourceVersion`: what a watch after this list starts from.
    pub resource_version: Option<String>,
    /// The first page's `Date` header: the API server's clock.
    pub date: Option<String>,
}

/// The cluster API, reached as the operator's configuration says.
pub struct Api {
    host: String,
    port: u16,
    authority: String,
    base_path: String,
    server_name: ServerName<'static>,
    connector: TlsConnector,
    bearer: Option<Zeroizing<String>>,
    namespace: String,
    /// How a removal is paced.
    pub pacing: Pacing,
    /// The bound on one call.
    pub timeout: Duration,
    /// The `User-Agent` every request sends.
    pub user_agent: &'static str,
}

impl fmt::Debug for Api {
    /// Names the server and the namespace, never the credential.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Api")
            .field("authority", &self.authority)
            .field("base_path", &self.base_path)
            .field("namespace", &self.namespace)
            .field("bearer", &self.bearer.as_ref().map(|_| "[redacted]"))
            .field("pacing", &self.pacing)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Api {
    pub(crate) fn new(cluster: &Value, user: &Value, namespace: &str) -> Result<Self, String> {
        refuse_members(cluster, user)?;
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
            pacing: Pacing::STANDARD,
            timeout: API_TIMEOUT,
            user_agent: USER_AGENT,
        })
    }

    /// The namespace the current context names: the only one the client addresses.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The namespace's Pods.
    pub fn pods(&self) -> String {
        format!("/api/v1/namespaces/{}/pods", self.namespace)
    }

    /// The namespace's `NetworkPolicy` objects.
    pub fn policies(&self) -> String {
        format!(
            "/apis/networking.k8s.io/v1/namespaces/{}/networkpolicies",
            self.namespace
        )
    }

    /// The namespace's `PersistentVolumeClaim` objects.
    pub fn claims(&self) -> String {
        format!(
            "/api/v1/namespaces/{}/persistentvolumeclaims",
            self.namespace
        )
    }

    /// The namespace's coordination `Lease` objects.
    pub fn leases(&self) -> String {
        format!(
            "/apis/coordination.k8s.io/v1/namespaces/{}/leases",
            self.namespace
        )
    }

    /// The API's own `Status.message` in `body`, bounded and cleared of the bearer token, or
    /// nothing.
    pub fn status_message(&self, body: &[u8]) -> String {
        self.redact(&status_message(body))
    }

    /// `text` with every occurrence of the bearer token replaced, so an API server that echoes the
    /// credential back cannot put it in an error.
    pub(crate) fn redact(&self, text: &str) -> String {
        match &self.bearer {
            Some(token) if text.contains(token.as_str()) => {
                text.replace(token.as_str(), "[redacted]")
            }
            _ => text.to_owned(),
        }
    }

    /// One request on its own connection, bounded in time and in body size. `Err` only when no
    /// answer came.
    ///
    /// # Errors
    ///
    /// Why no answer came.
    pub async fn call(
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
    ///
    /// # Errors
    ///
    /// Why no answer came.
    pub async fn list_metadata(&self, path: &str) -> Result<(u16, Option<String>, Bytes), String> {
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
            self.timeout,
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
        let (response, driver) = self.send(method, path, body, accept).await?;
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

    /// A TLS connection to the API server, verified against the cluster's own CA data.
    pub(crate) async fn connect(&self) -> Result<TlsStream<tokio::net::TcpStream>, String> {
        let tcp = tokio::net::TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|error| format!("connect: {error}"))?;
        self.connector
            .connect(self.server_name.clone(), tcp)
            .await
            .map_err(|error| format!("TLS: {error}"))
    }

    /// The `Authorization` header value, marked sensitive, when the user carries a token.
    pub(crate) fn authorization(&self) -> Result<Option<HeaderValue>, String> {
        let Some(token) = &self.bearer else {
            return Ok(None);
        };
        let line = Zeroizing::new(format!("Bearer {}", token.as_str()));
        let mut value = HeaderValue::from_str(&line)
            .map_err(|_| "the token is not a header value".to_owned())?;
        value.set_sensitive(true);
        Ok(Some(value))
    }

    /// The request URL's path and query for `path`, beneath the server's own base path.
    pub(crate) fn target(&self, path: &str) -> String {
        format!("{}{path}", self.base_path)
    }

    /// The server's `host[:port]`, as the configuration writes it.
    pub(crate) fn authority(&self) -> &str {
        &self.authority
    }

    /// Sends one request on its own connection; the answer's head and body, and the task that
    /// drives the connection.
    pub(crate) async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        accept: &'static str,
    ) -> Result<
        (
            hyper::Response<hyper::body::Incoming>,
            tokio::task::JoinHandle<Result<(), hyper::Error>>,
        ),
        String,
    > {
        let tls = self.connect().await?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(tls))
                .await
                .map_err(|error| format!("HTTP: {error}"))?;
        let driver = tokio::spawn(connection);
        let mut request = Request::builder()
            .method(method)
            .uri(self.target(path))
            .header(header::HOST, &self.authority)
            .header(header::ACCEPT, accept)
            .header(header::USER_AGENT, self.user_agent);
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        if let Some(value) = self.authorization()? {
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
        Ok((response, driver))
    }

    /// Creates `object` in `collection` and returns the API's answer, `Null` when it is not JSON.
    ///
    /// # Errors
    ///
    /// No answer, or any answer but 200 to 202.
    pub async fn create(&self, collection: &str, object: &Value) -> Result<Value, ApiError> {
        self.post(collection, object, "create").await
    }

    /// Asks the API to admit `object` into `collection` without storing it (`dryRun=All`): every
    /// admission step runs and the answer is the object as it would be stored.
    ///
    /// # Errors
    ///
    /// No answer, or any answer but 200 to 202: an admission refusal is the API's own status.
    pub async fn create_dry_run(
        &self,
        collection: &str,
        object: &Value,
    ) -> Result<Value, ApiError> {
        let separator = if collection.contains('?') { '&' } else { '?' };
        self.post(
            &format!("{collection}{separator}dryRun=All"),
            object,
            "dry-run create",
        )
        .await
    }

    async fn post(&self, path: &str, object: &Value, what: &str) -> Result<Value, ApiError> {
        match self
            .call(Method::POST, path, Some(object), MAX_API_BODY_BYTES)
            .await
        {
            Ok((200..=202, body)) => Ok(serde_json::from_slice(&body).unwrap_or(Value::Null)),
            Ok((status, body)) => Err(ApiError::Answered {
                status,
                detail: format!("{what} answered {status}: {}", self.status_message(&body)),
            }),
            Err(error) => Err(ApiError::Unanswered(error)),
        }
    }

    /// The object at `path`, or `None` once it is gone.
    ///
    /// # Errors
    ///
    /// No answer, an answer but 200 or 404, or a body that is not JSON.
    pub async fn get(&self, path: &str) -> Result<Option<Value>, ApiError> {
        match self
            .call(Method::GET, path, None, MAX_API_BODY_BYTES)
            .await
            .map_err(ApiError::Unanswered)?
        {
            (200, body) => serde_json::from_slice(&body).map(Some).map_err(|_| {
                ApiError::Invalid("the API answered something other than JSON".to_owned())
            }),
            (404, _) => Ok(None),
            (status, body) => Err(ApiError::Answered {
                status,
                detail: format!("get answered {status}: {}", self.status_message(&body)),
            }),
        }
    }

    /// Replaces the object at `path` with `object`, which carries the `metadata.resourceVersion`
    /// it was read at: the API refuses it 409 when the object changed since.
    ///
    /// # Errors
    ///
    /// No answer, or any answer but 200 or 201; a changed object is [`ApiError::Answered`] 409.
    pub async fn update(&self, path: &str, object: &Value) -> Result<Value, ApiError> {
        match self
            .call(Method::PUT, path, Some(object), MAX_API_BODY_BYTES)
            .await
        {
            Ok((200 | 201, body)) => Ok(serde_json::from_slice(&body).unwrap_or(Value::Null)),
            Ok((status, body)) => Err(ApiError::Answered {
                status,
                detail: format!("update answered {status}: {}", self.status_message(&body)),
            }),
            Err(error) => Err(ApiError::Unanswered(error)),
        }
    }

    /// Asks the API to delete the object at `path`; `true` once it answers the object is gone.
    ///
    /// # Errors
    ///
    /// No answer, or any answer but 200, 202 or 404.
    pub async fn delete(&self, path: &str, grace_seconds: u64) -> Result<bool, ApiError> {
        let options = json!({
            "apiVersion": "v1",
            "kind": "DeleteOptions",
            "gracePeriodSeconds": grace_seconds,
            "propagationPolicy": "Background",
        });
        self.delete_with(path, &options).await
    }

    /// [`Api::delete`] of the object at `path` only while its `uid` is `uid`
    /// (`preconditions.uid`): a same-named replacement is refused 409, never deleted.
    ///
    /// # Errors
    ///
    /// Those of [`Api::delete`]; another object under the name is [`ApiError::Answered`] 409.
    pub async fn delete_preconditioned(
        &self,
        path: &str,
        grace_seconds: u64,
        uid: &str,
    ) -> Result<bool, ApiError> {
        let options = json!({
            "apiVersion": "v1",
            "kind": "DeleteOptions",
            "gracePeriodSeconds": grace_seconds,
            "propagationPolicy": "Background",
            "preconditions": {"uid": uid},
        });
        self.delete_with(path, &options).await
    }

    async fn delete_with(&self, path: &str, options: &Value) -> Result<bool, ApiError> {
        match self
            .call(Method::DELETE, path, Some(options), MAX_API_BODY_BYTES)
            .await
            .map_err(ApiError::Unanswered)?
        {
            (404, _) => Ok(true),
            (200 | 202, _) => Ok(false),
            (status, body) => Err(ApiError::Answered {
                status,
                detail: format!("delete answered {status}: {}", self.status_message(&body)),
            }),
        }
    }

    /// Deletes the object at `path` with `grace_seconds` and polls until the API no longer has it.
    /// A Pod is deleted with a grace of at least one second, never forced; its disappearance alone
    /// is still no confirmation that its container stopped, because someone else may force-delete
    /// it meanwhile ([`crate::pod_end`] decides that).
    pub async fn remove(&self, path: &str, grace_seconds: u64) -> bool {
        self.remove_seen(path, grace_seconds, &mut PodTrace::default())
            .await
    }

    /// [`Api::remove`], noting every object it reads in `seen`.
    pub async fn remove_seen(&self, path: &str, grace_seconds: u64, seen: &mut PodTrace) -> bool {
        for _ in 0..self.pacing.remove_attempts {
            if self.remove_once(path, grace_seconds, seen).await {
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
    pub async fn remove_until_gone(&self, path: &str, grace_seconds: u64) {
        self.remove_until_gone_seen(path, grace_seconds, &mut PodTrace::default())
            .await;
    }

    /// [`Api::remove_until_gone`], noting every object it reads in `seen`.
    pub async fn remove_until_gone_seen(
        &self,
        path: &str,
        grace_seconds: u64,
        seen: &mut PodTrace,
    ) {
        let ceiling = self.pacing.poll * self.pacing.remove_attempts;
        let mut pause = self.pacing.poll;
        while !self.remove_once(path, grace_seconds, seen).await {
            let next = tokio::time::Instant::now() + pause;
            pause = (pause * 2).min(ceiling);
            loop {
                let wake = next.min(tokio::time::Instant::now() + self.pacing.notice);
                tokio::time::sleep_until(wake).await;
                if wake >= next {
                    break;
                }
                match self.get(path).await {
                    Ok(None) => return,
                    Ok(Some(object)) => {
                        seen.note(&object);
                    }
                    Err(_) => {}
                }
            }
        }
    }

    /// One deletion of the object at `path` and one read of it, noted in `seen`; `true` once it is
    /// gone.
    async fn remove_once(&self, path: &str, grace_seconds: u64, seen: &mut PodTrace) -> bool {
        if matches!(self.delete(path, grace_seconds).await, Ok(true)) {
            return true;
        }
        match self.get(path).await {
            Ok(None) => true,
            Ok(Some(object)) => {
                seen.note(&object);
                false
            }
            Err(_) => false,
        }
    }

    /// The API's answer to a read of `container`'s log in the Pod `name`, at most `limit` bytes
    /// plus one.
    ///
    /// # Errors
    ///
    /// Why no answer came.
    pub async fn log(
        &self,
        name: &str,
        container: &str,
        limit: usize,
    ) -> Result<(u16, Bytes), String> {
        let path = format!(
            "{}/{name}/log?container={container}&limitBytes={}",
            self.pods(),
            limit + 1
        );
        self.call(Method::GET, &path, None, limit.max(MAX_API_BODY_BYTES) + 1)
            .await
    }

    /// The API's `SelfSubjectRulesReview` of this client's credential in its namespace: the
    /// review's `status`, whose `resourceRules` state what the credential may do there. The review
    /// is not a stored object.
    ///
    /// # Errors
    ///
    /// No answer, an answer but 200 or 201, or an answer without a `status` object.
    pub async fn rules_review(&self) -> Result<Value, ApiError> {
        let review = json!({
            "apiVersion": "authorization.k8s.io/v1",
            "kind": "SelfSubjectRulesReview",
            "spec": {"namespace": self.namespace},
        });
        let answer = self
            .post(
                "/apis/authorization.k8s.io/v1/selfsubjectrulesreviews",
                &review,
                "rules review",
            )
            .await?;
        let status = &answer["status"];
        if status.is_object() {
            Ok(status.clone())
        } else {
            Err(ApiError::Invalid(
                "the rules review answered no status".to_owned(),
            ))
        }
    }

    /// Reads the list at `path` (which may carry a selector) whole, in pages of `page_items`, at
    /// most `max_pages` of them, each bounded at [`MAX_API_BODY_BYTES`]. `kind` is the list kind a
    /// server that ignores [`METADATA_LIST`] answers (`PodList`).
    ///
    /// # Errors
    ///
    /// No answer, an answer but 200, a body that is not the list, or a list that does not end
    /// within `max_pages`: never a short list read as whole.
    pub async fn list(
        &self,
        path: &str,
        kind: &str,
        form: ListForm,
        page_items: usize,
        max_pages: usize,
    ) -> Result<Listed, ApiError> {
        let what = kind.strip_suffix("List").unwrap_or(kind);
        let separator = if path.contains('?') { '&' } else { '?' };
        let first = format!("{path}{separator}limit={page_items}");
        let accept = match form {
            ListForm::Metadata => METADATA_LIST,
            ListForm::Whole => JSON,
        };
        let mut listed = Listed {
            items: Vec::new(),
            resource_version: None,
            date: None,
        };
        let mut page_path = first.clone();
        for page in 0..max_pages {
            let (status, date, body) = self
                .timed(Method::GET, &page_path, None, accept, MAX_API_BODY_BYTES)
                .await
                .map_err(ApiError::Unanswered)?;
            if status != 200 {
                return Err(ApiError::Answered {
                    status,
                    detail: format!(
                        "the {what} list answered {status}: {}",
                        self.status_message(&body)
                    ),
                });
            }
            let list: Value = serde_json::from_slice(&body).map_err(|_| {
                ApiError::Invalid("the API answered something other than JSON".to_owned())
            })?;
            if page == 0 {
                listed.date = date;
                listed.resource_version = list["metadata"]["resourceVersion"]
                    .as_str()
                    .map(str::to_owned);
            }
            listed
                .items
                .extend_from_slice(list_items(&list, kind, what).map_err(ApiError::Invalid)?);
            let Some(next) = continuation(&list) else {
                return Ok(listed);
            };
            let token: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
            page_path = format!("{first}&continue={token}");
        }
        Err(ApiError::Invalid(format!(
            "the {what} list did not end within {max_pages} pages of {page_items}"
        )))
    }

    /// Opens the watch at `path`, which carries `watch=1` and the `resourceVersion` to start from.
    ///
    /// # Errors
    ///
    /// [`WatchError::Expired`] for a 410: the history after that version is gone and the caller
    /// lists again; [`WatchError::Refused`] for 401 or 403; [`WatchError::Status`] for any other
    /// status but 200; [`WatchError::Unanswered`] when no answer came.
    pub async fn watch(&self, path: &str) -> Result<Watch, WatchError> {
        let (response, driver) = self
            .send(Method::GET, path, None, JSON)
            .await
            .map_err(WatchError::Unanswered)?;
        let driver = AbortOnDrop(driver);
        match response.status().as_u16() {
            200 => {}
            410 => return Err(WatchError::Expired),
            status @ (401 | 403) => return Err(WatchError::Refused(status)),
            status => return Err(WatchError::Status(status)),
        }
        Ok(Watch {
            body: response.into_body(),
            buffer: Vec::new(),
            read: 0,
            ended: false,
            bearer: self.bearer.clone(),
            _driver: driver,
        })
    }
}

/// Why a watch shows nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchError {
    /// No answer came.
    Unanswered(String),
    /// The history after the version asked for is gone (410, as a status or as an `ERROR` event):
    /// list again and watch from the list's version.
    Expired,
    /// The credential may not watch (401 or 403).
    Refused(u16),
    /// Any other status but 200.
    Status(u16),
    /// The stream failed mid-way.
    Stream,
    /// The watch passed [`MAX_API_BODY_BYTES`].
    Bound,
    /// A line that is not a JSON event.
    Malformed,
    /// An `ERROR` event other than 410, with its message cleared of the credential.
    Error(String),
}

impl fmt::Display for WatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unanswered(detail) => formatter.write_str(detail),
            Self::Expired => formatter
                .write_str("the watch answered 410: the history after the last read is gone"),
            Self::Refused(status) | Self::Status(status) => {
                write!(formatter, "the watch answered {status}")
            }
            Self::Stream => formatter.write_str("the watch stream failed"),
            Self::Bound => formatter.write_str("the watch passed its byte bound"),
            Self::Malformed => formatter.write_str("the watch sent something other than JSON"),
            Self::Error(message) => write!(formatter, "the watch answered an error: {message}"),
        }
    }
}

impl std::error::Error for WatchError {}

/// One event of a watch.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchEvent {
    /// `ADDED`, `MODIFIED`, `DELETED` or `BOOKMARK`.
    pub kind: String,
    /// The object the event carries.
    pub object: Value,
}

/// An open watch: its lines, read as they arrive, at most [`MAX_API_BODY_BYTES`] in all.
pub struct Watch {
    body: hyper::body::Incoming,
    buffer: Vec<u8>,
    read: usize,
    ended: bool,
    bearer: Option<Zeroizing<String>>,
    _driver: AbortOnDrop<Result<(), hyper::Error>>,
}

impl fmt::Debug for Watch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Watch")
            .field("read", &self.read)
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl Watch {
    /// The next line, its newline included; at the stream's end what is left without one, once;
    /// then `None`.
    ///
    /// # Errors
    ///
    /// [`WatchError::Stream`] when the stream fails, [`WatchError::Bound`] past the byte bound.
    pub async fn next_line(&mut self) -> Result<Option<Vec<u8>>, WatchError> {
        loop {
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                return Ok(Some(self.buffer.drain(..=end).collect()));
            }
            if self.ended {
                return Ok((!self.buffer.is_empty()).then(|| std::mem::take(&mut self.buffer)));
            }
            let Some(frame) = self.body.frame().await else {
                self.ended = true;
                continue;
            };
            let Ok(frame) = frame else {
                return Err(WatchError::Stream);
            };
            let Ok(data) = frame.into_data() else {
                continue;
            };
            self.read += data.len();
            if self.read > MAX_API_BODY_BYTES {
                return Err(WatchError::Bound);
            }
            self.buffer.extend_from_slice(&data);
        }
    }

    /// The next event, blank lines skipped; `None` once the stream ends.
    ///
    /// # Errors
    ///
    /// Those of [`Watch::next_line`]; [`WatchError::Malformed`] for a line that is not a JSON
    /// event; [`WatchError::Expired`] for an `ERROR` event with code 410, and
    /// [`WatchError::Error`] for any other.
    pub async fn next_event(&mut self) -> Result<Option<WatchEvent>, WatchError> {
        loop {
            let Some(line) = self.next_line().await? else {
                return Ok(None);
            };
            if line.trim_ascii().is_empty() {
                continue;
            }
            let event: Value = serde_json::from_slice(&line).map_err(|_| WatchError::Malformed)?;
            let Some(kind) = event["type"].as_str() else {
                return Err(WatchError::Malformed);
            };
            if kind == "ERROR" {
                if event["object"]["code"].as_u64() == Some(410) {
                    return Err(WatchError::Expired);
                }
                let message = event["object"]["message"].as_str().unwrap_or_default();
                let message = match &self.bearer {
                    Some(token) => message.replace(token.as_str(), "[redacted]"),
                    None => message.to_owned(),
                };
                return Err(WatchError::Error(message.chars().take(300).collect()));
            }
            return Ok(Some(WatchEvent {
                kind: kind.to_owned(),
                object: event["object"].clone(),
            }));
        }
    }
}

/// Aborts the task driving a connection when the read that uses it ends, however it ends.
pub(crate) struct AbortOnDrop<T>(pub(crate) tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The API's own `Status.message`, bounded, or nothing. Not cleared of the credential: quote it
/// through [`Api::status_message`].
pub(crate) fn status_message(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|status| status["message"].as_str().map(str::to_owned))
        .map(|message| message.chars().take(300).collect())
        .unwrap_or_default()
}

/// The items of a list answer: a `PartialObjectMetadataList`, or the `whole` list kind
/// (`NetworkPolicyList`, `PodList`) a server that ignored [`METADATA_LIST`] sends. An API server
/// answers an empty `PartialObjectMetadataList` without an items array, so `null` or absent items
/// are none. Any other kind, or items that are neither an array nor `null`, is an error, so a body
/// that is not that list never reads as an empty one.
///
/// # Errors
///
/// A body of another kind, or whose items are not a list.
pub fn list_items<'a>(list: &'a Value, whole: &str, what: &str) -> Result<&'a [Value], String> {
    let kind = list["kind"].as_str();
    if kind != Some("PartialObjectMetadataList") && kind != Some(whole) {
        return Err(format!("the {what} list answered no list kind it reads"));
    }
    match &list["items"] {
        Value::Array(items) => Ok(items),
        Value::Null => Ok(&[]),
        _ => Err(format!("the {what} list holds no items")),
    }
}

/// A list's `continue` token, or `None` on its last page.
pub fn continuation(list: &Value) -> Option<String> {
    list["metadata"]["continue"]
        .as_str()
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}
