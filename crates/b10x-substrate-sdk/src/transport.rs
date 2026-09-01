use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use futures_util::StreamExt as _;
use http::Uri;
use rustls::pki_types::{DnsName, ServerName};
use rustls::{ClientConfig, RootCertStore};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpStream, UnixStream};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use ulid::Ulid;

use crate::model::{CONTRACT, CONTRACT_SHA256, EventStreamFrame};
use crate::{
    AccessToken, AccessTokenProvider, AccessTokenReason, EventPage, MetricsSample, SdkError,
};

const MAX_RESPONSE_HEAD: usize = 16 * 1024;
const MAX_RESPONSE_BODY: usize = 4 * 1024 * 1024;
const MAX_CA_BUNDLE_BYTES: u64 = 1024 * 1024;
const SESSION_EXPORTER_BYTES: usize = 32;

pub(crate) trait IoStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> IoStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}
pub(crate) type BoxedIo = Box<dyn IoStream>;
pub(crate) type WebSocket = WebSocketStream<BoxedIo>;

#[derive(Clone)]
pub(crate) struct Transport {
    kind: TransportKind,
}

#[derive(Clone)]
enum TransportKind {
    Unix(PathBuf),
    Remote(RemoteTransport),
}

#[derive(Clone)]
struct RemoteTransport {
    authority: String,
    connect_host: String,
    port: u16,
    server_name: ServerName<'static>,
    connector: TlsConnector,
    token_provider: Arc<dyn AccessTokenProvider>,
}

pub(crate) struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Transport {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            kind: TransportKind::Unix(socket.into()),
        }
    }

    pub fn remote(
        endpoint: &str,
        trust_roots: &std::path::Path,
        server_identity: String,
        token_provider: Arc<dyn AccessTokenProvider>,
    ) -> Result<Self, SdkError> {
        let uri = Uri::from_str(endpoint)
            .map_err(|_| SdkError::Protocol("remote endpoint is invalid".to_owned()))?;
        if uri.scheme_str() != Some("https")
            || uri.authority().is_none()
            || uri
                .authority()
                .is_some_and(|authority| authority.as_str().contains('@'))
            || uri.query().is_some()
            || uri.path() != "/"
        {
            return Err(SdkError::Protocol(
                "remote endpoint must be one exact HTTPS origin".to_owned(),
            ));
        }
        let authority = uri
            .authority()
            .expect("checked authority")
            .as_str()
            .to_owned();
        let connect_host = uri
            .host()
            .ok_or_else(|| SdkError::Protocol("remote endpoint has no host".to_owned()))?
            .to_owned();
        let port = uri.port_u16().unwrap_or(443);
        if port == 0 {
            return Err(SdkError::Protocol(
                "remote endpoint has no usable port".to_owned(),
            ));
        }
        let dns_name = DnsName::try_from(server_identity)
            .map_err(|_| SdkError::Protocol("server identity is not a DNS name".to_owned()))?;
        let connector = load_connector(trust_roots)?;
        Ok(Self {
            kind: TransportKind::Remote(RemoteTransport {
                authority,
                connect_host,
                port,
                server_name: ServerName::DnsName(dns_name),
                connector,
                token_provider,
            }),
        })
    }

    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, SdkError> {
        match &self.kind {
            TransportKind::Unix(socket) => {
                let stream = UnixStream::connect(socket)
                    .await
                    .map_err(|error| SdkError::Transport(error.to_string()))?;
                request_on(stream, "localhost", None, method, path, body).await
            }
            TransportKind::Remote(remote) => remote.request(method, path, body).await,
        }
    }

    pub async fn event_stream(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<EventStream, SdkError> {
        let path = event_path(cursor, limit);
        let socket = self.websocket(&path).await?;
        Ok(EventStream { socket })
    }

    pub async fn metrics_stream(&self, exec_id: &str) -> Result<MetricsStream, SdkError> {
        let query = serde_urlencoded::to_string(substrate_wire::MetricsStreamQuery {
            exec_id: exec_id.to_owned(),
        })
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let socket = self
            .websocket(&format!("/v1/metrics/stream?{query}"))
            .await?;
        Ok(MetricsStream { socket })
    }

    pub(crate) async fn websocket(&self, path: &str) -> Result<WebSocket, SdkError> {
        match &self.kind {
            TransportKind::Unix(socket) => {
                let stream = UnixStream::connect(socket)
                    .await
                    .map_err(|error| SdkError::Transport(error.to_string()))?;
                websocket_on(Box::new(stream), &format!("ws://localhost{path}"), &[])
                    .await
                    .map_err(WebSocketAttemptError::into_sdk)
            }
            TransportKind::Remote(remote) => remote.websocket(path, &[]).await,
        }
    }

    pub(crate) async fn session_websocket(&self, session_id: &str) -> Result<WebSocket, SdkError> {
        let path = format!("/v1/sessions/{}/attach", encode_path(session_id));
        match &self.kind {
            TransportKind::Unix(_) => self.websocket(&path).await,
            TransportKind::Remote(remote) => remote.session_websocket(session_id, &path).await,
        }
    }
}

impl RemoteTransport {
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, SdkError> {
        let token = self.token(AccessTokenReason::Request).await?;
        let response = self.request_with_token(method, path, body, &token).await?;
        if !is_refreshable_auth_refusal(&response) {
            return Ok(response);
        }
        let token = self
            .token(AccessTokenReason::RefreshAfterAuthorizationFailure)
            .await?;
        self.request_with_token(method, path, body, &token).await
    }

    async fn request_with_token(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        token: &AccessToken,
    ) -> Result<HttpResponse, SdkError> {
        let stream = self.connect().await?;
        request_on(
            stream,
            &self.authority,
            Some(token.expose()),
            method,
            path,
            body,
        )
        .await
    }

    async fn websocket(
        &self,
        path: &str,
        extra_headers: &[(HeaderName, HeaderValue)],
    ) -> Result<WebSocket, SdkError> {
        let token = self.token(AccessTokenReason::Request).await?;
        match self.websocket_with_token(path, extra_headers, &token).await {
            Err(WebSocketAttemptError::Refusal(response))
                if is_refreshable_auth_refusal(&response) =>
            {
                let token = self
                    .token(AccessTokenReason::RefreshAfterAuthorizationFailure)
                    .await?;
                self.websocket_with_token(path, extra_headers, &token)
                    .await
                    .map_err(WebSocketAttemptError::into_sdk)
            }
            result => result.map_err(WebSocketAttemptError::into_sdk),
        }
    }

    async fn session_websocket(&self, session_id: &str, path: &str) -> Result<WebSocket, SdkError> {
        let mut seed = zeroize::Zeroizing::new([0_u8; 32]);
        getrandom::fill(seed.as_mut())
            .map_err(|_| SdkError::Transport("secure randomness is unavailable".to_owned()))?;
        let signing_key = SigningKey::from_bytes(&seed);
        let public_key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing_key.verifying_key().to_bytes());
        let mint_path = format!(
            "/v1/sessions/{}/attachment-authorities",
            encode_path(session_id)
        );
        let body = serde_json::to_vec(&substrate_wire::SessionAuthorityMintInput { public_key })
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let response = self.request("POST", &mint_path, Some(&body)).await?;
        let authority: substrate_wire::SessionAttachmentAuthority = decode_result(&response)?;
        self.session_websocket_with_authority(path, &authority, &signing_key)
            .await
    }

    async fn session_websocket_with_authority(
        &self,
        path: &str,
        authority: &substrate_wire::SessionAttachmentAuthority,
        signing_key: &SigningKey,
    ) -> Result<WebSocket, SdkError> {
        let token = self.token(AccessTokenReason::Request).await?;
        match self
            .session_websocket_attempt(path, authority, signing_key, &token)
            .await
        {
            Err(WebSocketAttemptError::Refusal(response))
                if is_refreshable_auth_refusal(&response) =>
            {
                let token = self
                    .token(AccessTokenReason::RefreshAfterAuthorizationFailure)
                    .await?;
                self.session_websocket_attempt(path, authority, signing_key, &token)
                    .await
                    .map_err(WebSocketAttemptError::into_sdk)
            }
            result => result.map_err(WebSocketAttemptError::into_sdk),
        }
    }

    async fn session_websocket_attempt(
        &self,
        path: &str,
        authority: &substrate_wire::SessionAttachmentAuthority,
        signing_key: &SigningKey,
        token: &AccessToken,
    ) -> Result<WebSocket, WebSocketAttemptError> {
        let (stream, exporter) = self
            .connect_with_exporter()
            .await
            .map_err(WebSocketAttemptError::Sdk)?;
        let timestamp = chrono::Utc::now().timestamp_millis();
        let proof = signing_key.sign(&substrate_wire::session_authority_transcript(
            &authority.authority_id,
            &exporter,
            timestamp,
        ));
        let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(proof.to_bytes());
        let headers = [
            bounded_header(
                substrate_wire::SESSION_AUTHORITY_ID_HEADER,
                &authority.authority_id,
            )?,
            bounded_header(
                substrate_wire::SESSION_AUTHORITY_BEARER_HEADER,
                &authority.authority,
            )?,
            bounded_header(
                substrate_wire::SESSION_AUTHORITY_TIMESTAMP_HEADER,
                &timestamp.to_string(),
            )?,
            bounded_header(substrate_wire::SESSION_AUTHORITY_PROOF_HEADER, &proof)?,
        ];
        websocket_on(
            stream,
            &format!("wss://{}{path}", self.authority),
            &with_authorization(&headers, token)?,
        )
        .await
    }

    async fn websocket_with_token(
        &self,
        path: &str,
        extra_headers: &[(HeaderName, HeaderValue)],
        token: &AccessToken,
    ) -> Result<WebSocket, WebSocketAttemptError> {
        let stream = self.connect().await.map_err(WebSocketAttemptError::Sdk)?;
        websocket_on(
            stream,
            &format!("wss://{}{path}", self.authority),
            &with_authorization(extra_headers, token)?,
        )
        .await
    }

    async fn token(&self, reason: AccessTokenReason) -> Result<AccessToken, SdkError> {
        self.token_provider
            .access_token(reason)
            .await
            .map_err(|_| SdkError::TokenUnavailable)
    }

    async fn connect(&self) -> Result<BoxedIo, SdkError> {
        let (stream, _) = self.connect_with_exporter().await?;
        Ok(stream)
    }

    async fn connect_with_exporter(&self) -> Result<(BoxedIo, [u8; 32]), SdkError> {
        let tcp = TcpStream::connect((self.connect_host.as_str(), self.port))
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))?;
        let tls = self
            .connector
            .connect(self.server_name.clone(), tcp)
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))?;
        let mut exporter = [0_u8; SESSION_EXPORTER_BYTES];
        tls.get_ref()
            .1
            .export_keying_material(
                &mut exporter,
                substrate_wire::SESSION_AUTHORITY_EXPORTER_LABEL,
                None,
            )
            .map_err(|_| SdkError::Transport("TLS exporter is unavailable".to_owned()))?;
        Ok((Box::new(tls), exporter))
    }
}

enum WebSocketAttemptError {
    Refusal(HttpResponse),
    Sdk(SdkError),
}

impl WebSocketAttemptError {
    fn into_sdk(self) -> SdkError {
        match self {
            Self::Refusal(response) => match decode_result::<serde_json::Value>(&response) {
                Err(error) => error,
                Ok(_) => {
                    SdkError::Protocol("WebSocket refusal carried a success envelope".to_owned())
                }
            },
            Self::Sdk(error) => error,
        }
    }
}

impl From<SdkError> for WebSocketAttemptError {
    fn from(error: SdkError) -> Self {
        Self::Sdk(error)
    }
}

pub struct EventStream {
    socket: WebSocket,
}

pub struct MetricsStream {
    socket: WebSocket,
}

impl MetricsStream {
    pub async fn next_sample(&mut self) -> Result<Option<MetricsSample>, SdkError> {
        while let Some(message) = self.socket.next().await {
            let message = message.map_err(|error| SdkError::Transport(error.to_string()))?;
            match message {
                Message::Text(text) => {
                    let frame: substrate_wire::MetricsStreamFrame = serde_json::from_str(&text)
                        .map_err(|error| SdkError::Protocol(error.to_string()))?;
                    let substrate_wire::MetricsStreamFrame::Usage { exec, usage } = frame;
                    return Ok(Some(MetricsSample { exec, usage }));
                }
                Message::Close(_) => return Ok(None),
                Message::Ping(bytes) => {
                    use futures_util::SinkExt as _;
                    self.socket
                        .send(Message::Pong(bytes))
                        .await
                        .map_err(|error| SdkError::Transport(error.to_string()))?;
                }
                Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        Ok(None)
    }
}

impl EventStream {
    pub async fn next_page(&mut self) -> Result<Option<EventPage>, SdkError> {
        while let Some(message) = self.socket.next().await {
            let message = message.map_err(|error| SdkError::Transport(error.to_string()))?;
            match message {
                Message::Text(text) => {
                    let frame: EventStreamFrame = serde_json::from_str(&text)
                        .map_err(|error| SdkError::Protocol(error.to_string()))?;
                    if frame.kind == "events" {
                        let page = frame.page.ok_or_else(|| {
                            SdkError::Protocol("event frame has no page".to_owned())
                        })?;
                        return Ok(Some(page.into()));
                    }
                    return Err(SdkError::EventGap {
                        code: frame
                            .code
                            .unwrap_or_else(|| "event.stream-ended".to_owned()),
                        cursor: frame.cursor,
                    });
                }
                Message::Close(_) => return Ok(None),
                Message::Ping(bytes) => {
                    use futures_util::SinkExt as _;
                    self.socket
                        .send(Message::Pong(bytes))
                        .await
                        .map_err(|error| SdkError::Transport(error.to_string()))?;
                }
                Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        Ok(None)
    }
}

async fn request_on<S>(
    mut stream: S,
    authority: &str,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<HttpResponse, SdkError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request_id = format!("sdk_{}", Ulid::generate());
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nx-request-id: {request_id}\r\nConnection: close\r\n"
    );
    if let Some(token) = token {
        write!(head, "authorization: Bearer {token}\r\n")
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
    }
    if let Some(body) = body {
        write!(
            head,
            "content-type: application/json\r\ncontent-length: {}\r\n",
            body.len()
        )
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|error| SdkError::Transport(error.to_string()))?;
    if let Some(body) = body {
        stream
            .write_all(body)
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))?;
    }
    read_response(&mut stream).await
}

async fn websocket_on(
    stream: BoxedIo,
    url: &str,
    headers: &[(HeaderName, HeaderValue)],
) -> Result<WebSocket, WebSocketAttemptError> {
    let mut request = url
        .into_client_request()
        .map_err(|error| WebSocketAttemptError::Sdk(SdkError::Protocol(error.to_string())))?;
    for (name, value) in headers {
        request.headers_mut().insert(name, value.clone());
    }
    let (socket, response) = match tokio_tungstenite::client_async(request, stream).await {
        Ok(upgraded) => upgraded,
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            let headers = response_headers(response.headers());
            verify_contract_headers(&headers).map_err(WebSocketAttemptError::Sdk)?;
            return Err(WebSocketAttemptError::Refusal(HttpResponse {
                status: response.status().as_u16(),
                body: response.body().clone().unwrap_or_default(),
            }));
        }
        Err(error) => {
            return Err(WebSocketAttemptError::Sdk(SdkError::Transport(
                error.to_string(),
            )));
        }
    };
    verify_contract_headers(&response_headers(response.headers()))
        .map_err(WebSocketAttemptError::Sdk)?;
    Ok(socket)
}

fn response_headers(headers: &http::HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            Some((
                name.as_str().to_ascii_lowercase(),
                value.to_str().ok()?.to_owned(),
            ))
        })
        .collect()
}

fn with_authorization(
    headers: &[(HeaderName, HeaderValue)],
    token: &AccessToken,
) -> Result<Vec<(HeaderName, HeaderValue)>, WebSocketAttemptError> {
    let mut result = headers.to_vec();
    let mut value = HeaderValue::from_str(&format!("Bearer {}", token.expose()))
        .map_err(|_| WebSocketAttemptError::Sdk(SdkError::TokenUnavailable))?;
    value.set_sensitive(true);
    result.push((http::header::AUTHORIZATION, value));
    Ok(result)
}

fn bounded_header(name: &'static str, value: &str) -> Result<(HeaderName, HeaderValue), SdkError> {
    let name = HeaderName::from_static(name);
    let mut value = HeaderValue::from_str(value)
        .map_err(|_| SdkError::Protocol("session authority header is invalid".to_owned()))?;
    value.set_sensitive(true);
    Ok((name, value))
}

fn is_refreshable_auth_refusal(response: &HttpResponse) -> bool {
    if response.status != 401 {
        return false;
    }
    serde_json::from_slice::<substrate_wire::Failure>(&response.body)
        .ok()
        .is_some_and(|failure| {
            matches!(
                failure.error.code.as_str(),
                substrate_wire::AUTH_CREDENTIAL_ABSENT | substrate_wire::AUTH_AUTHORITY_INVALID
            )
        })
}

fn load_connector(path: &std::path::Path) -> Result<TlsConnector, SdkError> {
    let file = File::open(path)
        .map_err(|_| SdkError::Protocol("remote trust roots cannot be opened".to_owned()))?;
    let metadata = file
        .metadata()
        .map_err(|_| SdkError::Protocol("remote trust roots cannot be inspected".to_owned()))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_CA_BUNDLE_BYTES {
        return Err(SdkError::Protocol(
            "remote trust roots must be one bounded regular file".to_owned(),
        ));
    }
    let mut roots = RootCertStore::empty();
    let mut reader = BufReader::new(file);
    let mut count = 0_usize;
    loop {
        match rustls_pemfile::read_one(&mut reader)
            .map_err(|_| SdkError::Protocol("remote trust roots are invalid".to_owned()))?
        {
            Some(rustls_pemfile::Item::X509Certificate(certificate)) => {
                roots.add(certificate).map_err(|_| {
                    SdkError::Protocol(
                        "remote trust roots contain an invalid certificate".to_owned(),
                    )
                })?;
                count += 1;
            }
            Some(_) => {
                return Err(SdkError::Protocol(
                    "remote trust roots contain non-certificate material".to_owned(),
                ));
            }
            None => break,
        }
    }
    if count == 0 {
        return Err(SdkError::Protocol(
            "remote trust roots contain no certificate".to_owned(),
        ));
    }
    let mut config = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsConnector::from(Arc::new(config)))
}

async fn read_response<S>(stream: &mut S) -> Result<HttpResponse, SdkError>
where
    S: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let boundary = loop {
        if let Some(index) = find(&bytes, b"\r\n\r\n") {
            break index + 4;
        }
        if bytes.len() >= MAX_RESPONSE_HEAD {
            return Err(SdkError::Protocol(
                "response head exceeds its bound".to_owned(),
            ));
        }
        let mut chunk = [0_u8; 4096];
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))?;
        if read == 0 {
            return Err(SdkError::Transport(
                "daemon closed before the response head".to_owned(),
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    };
    let head = std::str::from_utf8(&bytes[..boundary])
        .map_err(|_| SdkError::Protocol("response head is not ASCII".to_owned()))?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| SdkError::Protocol("response has no valid status".to_owned()))?;
    let mut headers = BTreeMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    verify_contract_headers(&headers)?;
    if headers.contains_key("transfer-encoding") {
        return Err(SdkError::Protocol(
            "chunked responses are outside the SDK transport".to_owned(),
        ));
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| SdkError::Protocol("response has no content-length".to_owned()))?;
    if length > MAX_RESPONSE_BODY {
        return Err(SdkError::Protocol(
            "response body exceeds its bound".to_owned(),
        ));
    }
    while bytes.len() < boundary + length {
        let remaining = boundary + length - bytes.len();
        let mut chunk = vec![0_u8; remaining.min(8192)];
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))?;
        if read == 0 {
            return Err(SdkError::Transport(
                "daemon closed during the response body".to_owned(),
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(HttpResponse {
        status,
        body: bytes[boundary..boundary + length].to_vec(),
    })
}

fn verify_contract_headers(headers: &BTreeMap<String, String>) -> Result<(), SdkError> {
    let observed_contract = headers.get("x-b10x-contract").cloned();
    let observed_sha256 = headers.get("x-b10x-contract-bundle-sha256").cloned();
    if observed_contract.as_deref() != Some(CONTRACT)
        || observed_sha256.as_deref() != Some(CONTRACT_SHA256)
    {
        return Err(SdkError::ContractMismatch {
            expected_contract: CONTRACT,
            expected_sha256: CONTRACT_SHA256,
            observed_contract,
            observed_sha256,
        });
    }
    Ok(())
}

pub(crate) fn decode_result<T: DeserializeOwned>(response: &HttpResponse) -> Result<T, SdkError> {
    if (200..300).contains(&response.status) {
        let envelope: substrate_wire::Success<T> = serde_json::from_slice(&response.body)
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        return Ok(envelope.result);
    }
    let failure: substrate_wire::Failure = serde_json::from_slice(&response.body)
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
    Err(SdkError::Refusal(failure.error.into()))
}

pub(crate) fn encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            write!(&mut encoded, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    encoded
}

fn event_path(cursor: Option<&str>, limit: u32) -> String {
    let mut path = format!("/v1/events/stream?limit={limit}");
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&encode_path(cursor));
    }
    path
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::{CONTRACT, CONTRACT_SHA256, Transport, verify_contract_headers};
    use crate::SdkError;
    use futures_util::SinkExt as _;
    use std::collections::BTreeMap;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::UnixListener;

    fn headers(contract: &str, digest: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("x-b10x-contract".to_owned(), contract.to_owned()),
            (
                "x-b10x-contract-bundle-sha256".to_owned(),
                digest.to_owned(),
            ),
        ])
    }

    #[allow(
        clippy::result_large_err,
        clippy::unnecessary_wraps,
        reason = "tungstenite's handshake callback requires this exact result type"
    )]
    fn add_contract_headers(
        _: &tokio_tungstenite::tungstenite::handshake::server::Request,
        mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
    ) -> Result<
        tokio_tungstenite::tungstenite::handshake::server::Response,
        tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
    > {
        response.headers_mut().insert(
            "x-b10x-contract",
            CONTRACT.parse().expect("contract header"),
        );
        response.headers_mut().insert(
            "x-b10x-contract-bundle-sha256",
            CONTRACT_SHA256.parse().expect("digest header"),
        );
        Ok(response)
    }

    fn assert_mismatch(
        headers: &BTreeMap<String, String>,
        contract: Option<&str>,
        digest: Option<&str>,
    ) {
        let error = verify_contract_headers(headers).expect_err("claim must be refused");
        match error {
            SdkError::ContractMismatch {
                expected_contract,
                expected_sha256,
                observed_contract,
                observed_sha256,
            } => {
                assert_eq!(expected_contract, CONTRACT);
                assert_eq!(expected_sha256, CONTRACT_SHA256);
                assert_eq!(observed_contract.as_deref(), contract);
                assert_eq!(observed_sha256.as_deref(), digest);
            }
            other => panic!("contract claim produced another error: {other}"),
        }
    }

    #[test]
    fn the_promoted_contract_claim_is_accepted() {
        verify_contract_headers(&headers(CONTRACT, CONTRACT_SHA256))
            .expect("the exact promoted pair is accepted");
    }

    #[test]
    fn missing_older_unknown_and_wrong_digest_claims_are_refused() {
        assert_mismatch(&BTreeMap::new(), None, None);

        let older_contract = "substrate-wire/0.4.0";
        let older_digest = "002337bd011a0b68f8680cc157ee4d0424d49392c36a0f85e5fa0449ea4ea0da";
        assert_mismatch(
            &headers(older_contract, older_digest),
            Some(older_contract),
            Some(older_digest),
        );

        let unknown = "substrate-wire/99.0.0";
        assert_mismatch(
            &headers(unknown, CONTRACT_SHA256),
            Some(unknown),
            Some(CONTRACT_SHA256),
        );

        let wrong_digest = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        assert_mismatch(
            &headers(CONTRACT, wrong_digest),
            Some(CONTRACT),
            Some(wrong_digest),
        );
    }

    #[tokio::test]
    async fn an_older_claim_is_refused_before_an_operation_body_is_read() {
        let temporary = tempfile::tempdir().expect("temporary socket directory");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept SDK connection");
            let mut request = vec![0_u8; 4096];
            let read = stream.read(&mut request).await.expect("read SDK request");
            assert!(read > 0, "SDK sent no request");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
x-b10x-contract: substrate-wire/0.4.0\r\n\
x-b10x-contract-bundle-sha256: 002337bd011a0b68f8680cc157ee4d0424d49392c36a0f85e5fa0449ea4ea0da\r\n\
content-length: not-a-number\r\n\r\n",
                )
                .await
                .expect("write mismatched response");
        });

        let Err(error) = Transport::new(socket)
            .request("GET", "/v1/machine", None)
            .await
        else {
            panic!("older claim was accepted");
        };
        assert!(
            matches!(error, SdkError::ContractMismatch { .. }),
            "{error}"
        );
        server.await.expect("fake daemon task");
    }

    #[tokio::test]
    async fn an_event_stream_gap_preserves_its_code_and_cursor() {
        let temporary = tempfile::tempdir().expect("temporary socket directory");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept SDK connection");
            let mut websocket = tokio_tungstenite::accept_hdr_async(stream, add_contract_headers)
                .await
                .expect("upgrade event stream");
            websocket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::json!({
                        "kind": "gap",
                        "page": null,
                        "code": "event.retention-gap",
                        "cursor": "ev2.scope_test.41.1"
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("send gap frame");
        });

        let mut events = Transport::new(socket)
            .event_stream(None, 10)
            .await
            .expect("open event stream");
        let error = events.next_page().await.expect_err("gap must be surfaced");
        match error {
            SdkError::EventGap { code, cursor } => {
                assert_eq!(code, "event.retention-gap");
                assert_eq!(cursor.as_deref(), Some("ev2.scope_test.41.1"));
            }
            other => panic!("gap produced another error: {other}"),
        }
        server.await.expect("fake daemon task");
    }
}
