//! The raw probe (design 31 § 2): HTTPS on `rustls` directly, one connection per request, never
//! retrying, for what the SDK deliberately hides. Every connect, handshake, read and write is
//! bounded by `--io-timeout`.

use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use zeroize::Zeroizing;

use crate::registry::{PINNED_CONTRACT, PINNED_SHA256};

/// The largest response the probe reads.
const MAX_RESPONSE: usize = 16 * 1024 * 1024;

/// One read's buffer, on the heap so that no case's future carries it.
const READ_BLOCK: usize = 16 * 1024;

/// Why an exchange produced no response. Closed codes only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    IoTimeout,
    Connect,
    Handshake,
    Transport,
    Malformed,
}

impl Fault {
    pub const fn code(self) -> &'static str {
        match self {
            Self::IoTimeout => "io-timeout",
            Self::Connect => "connect-failed",
            Self::Handshake => "handshake-failed",
            Self::Transport => "transport-failed",
            Self::Malformed => "response-malformed",
        }
    }
}

/// One raw response.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub contract: Option<String>,
    pub contract_sha256: Option<String>,
    pub body: Vec<u8>,
}

/// The error envelope of a reply, decoded into the wire crate's closed type.
pub struct ErrorView {
    pub class: substrate_wire::ErrorClass,
    pub code: String,
}

impl Reply {
    /// The reply carries the pinned contract pair.
    pub fn contract_pinned(&self) -> bool {
        self.contract.as_deref() == Some(PINNED_CONTRACT)
            && self.contract_sha256.as_deref() == Some(PINNED_SHA256)
    }

    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }

    pub fn error(&self) -> Option<ErrorView> {
        let failure: substrate_wire::Failure = serde_json::from_slice(&self.body).ok()?;
        Some(ErrorView {
            class: failure.error.class,
            code: failure.error.code,
        })
    }

    pub fn error_code(&self) -> Option<String> {
        self.error().map(|error| error.code)
    }

    /// The `result` member of a success envelope.
    pub fn result(&self) -> Option<Value> {
        let mut value = self.json()?;
        value.get_mut("result").map(Value::take)
    }
}

/// What the TLS handshake with the endpoint showed (§ 6, `endpoint`).
pub struct EndpointTls {
    pub tls_version: String,
    pub alpn: String,
    pub leaf_sha256: String,
    pub leaf_not_after: String,
}

/// One endpoint, its verified name and its trust roots.
#[derive(Clone)]
pub struct Probe {
    authority: String,
    server_name: ServerName<'static>,
    roots: Arc<RootCertStore>,
    io: Duration,
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

fn config(roots: &Arc<RootCertStore>, alpn: &[u8]) -> Option<Arc<ClientConfig>> {
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .ok()?
        .with_root_certificates(Arc::clone(roots))
        .with_no_client_auth();
    config.alpn_protocols = vec![alpn.to_vec()];
    Some(Arc::new(config))
}

/// Reads a PEM bundle into a root store; `None` when it holds no usable certificate.
pub fn trust_roots(pem: &[u8]) -> Option<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for certificate in rustls_pemfile::certs(&mut &*pem) {
        roots.add(certificate.ok()?).ok()?;
    }
    (!roots.is_empty()).then_some(roots)
}

fn certificate_error(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        .is_some_and(|inner| matches!(inner, rustls::Error::InvalidCertificate(_)))
}

impl Probe {
    pub fn new(
        authority: String,
        server_name: ServerName<'static>,
        roots: RootCertStore,
        io: Duration,
    ) -> Self {
        Self {
            authority,
            server_name,
            roots: Arc::new(roots),
            io,
        }
    }

    async fn bounded<T>(&self, future: impl Future<Output = Result<T, Fault>>) -> Result<T, Fault> {
        tokio::time::timeout(self.io, future)
            .await
            .map_err(|_| Fault::IoTimeout)?
    }

    async fn tcp(&self) -> Result<TcpStream, Fault> {
        self.bounded(async {
            TcpStream::connect(&self.authority)
                .await
                .map_err(|_| Fault::Connect)
        })
        .await
    }

    async fn tls(
        &self,
        name: ServerName<'static>,
        alpn: &[u8],
    ) -> Result<Result<TlsStream<TcpStream>, std::io::Error>, Fault> {
        let config = config(&self.roots, alpn).ok_or(Fault::Handshake)?;
        let tcp = self.tcp().await?;
        let connector = TlsConnector::from(config);
        tokio::time::timeout(self.io, connector.connect(name, tcp))
            .await
            .map_err(|_| Fault::IoTimeout)
    }

    async fn session(&self) -> Result<TlsStream<TcpStream>, Fault> {
        self.tls(self.server_name.clone(), b"http/1.1")
            .await?
            .map_err(|_| Fault::Handshake)
    }

    /// The negotiated version, ALPN and leaf certificate of one ordinary handshake.
    pub async fn endpoint_tls(&self) -> Result<EndpointTls, Fault> {
        let stream = self.session().await?;
        let (_, connection) = stream.get_ref();
        let leaf: &CertificateDer<'_> = connection
            .peer_certificates()
            .and_then(<[_]>::first)
            .ok_or(Fault::Handshake)?;
        let not_after = x509_parser::parse_x509_certificate(leaf.as_ref())
            .map_err(|_| Fault::Malformed)?
            .1
            .validity()
            .not_after
            .timestamp();
        let not_after = chrono::DateTime::from_timestamp(not_after, 0)
            .ok_or(Fault::Malformed)?
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let tls_version = match connection.protocol_version() {
            Some(rustls::ProtocolVersion::TLSv1_3) => "TLSv1.3",
            Some(rustls::ProtocolVersion::TLSv1_2) => "TLSv1.2",
            _ => "other",
        };
        let alpn = match connection.alpn_protocol() {
            Some(b"http/1.1") => "http/1.1",
            Some(_) => "other",
            None => "none",
        };
        Ok(EndpointTls {
            tls_version: tls_version.to_owned(),
            alpn: alpn.to_owned(),
            leaf_sha256: sha256_hex(leaf.as_ref()),
            leaf_not_after: not_after,
        })
    }

    /// One request on its own connection. `bearer` is the credential, read from a `Zeroizing`
    /// buffer and written only into a `Zeroizing` request buffer.
    pub async fn exchange(
        &self,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<Reply, Fault> {
        let request = self.request_bytes(method, path, bearer, body);
        let mut stream = self.session().await?;
        self.bounded(async {
            stream
                .write_all(&request)
                .await
                .map_err(|_| Fault::Transport)?;
            stream.flush().await.map_err(|_| Fault::Transport)
        })
        .await?;
        drop(request);
        let mut received = Vec::new();
        let mut block = vec![0_u8; READ_BLOCK];
        loop {
            let read = self
                .bounded(async {
                    match stream.read(&mut block).await {
                        Ok(count) => Ok(count),
                        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
                        Err(_) => Err(Fault::Transport),
                    }
                })
                .await?;
            received.extend_from_slice(&block[..read]);
            if let Some(reply) = parse_reply(&received)? {
                return Ok(reply);
            }
            if read == 0 || received.len() > MAX_RESPONSE {
                return Err(Fault::Malformed);
            }
        }
    }

    /// Sends one request, flushes it and closes the connection before reading anything.
    pub async fn send_and_drop(
        &self,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<(), Fault> {
        let request = self.request_bytes(method, path, bearer, body);
        let mut stream = self.session().await?;
        self.bounded(async {
            stream
                .write_all(&request)
                .await
                .map_err(|_| Fault::Transport)?;
            stream.flush().await.map_err(|_| Fault::Transport)?;
            let _ = stream.shutdown().await;
            Ok(())
        })
        .await
    }

    fn request_bytes(
        &self,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: Option<&[u8]>,
    ) -> Zeroizing<Vec<u8>> {
        let mut request = Zeroizing::new(Vec::with_capacity(512 + body.map_or(0, <[u8]>::len)));
        request.extend_from_slice(
            format!(
                "{method} {path} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\naccept: application/json\r\n",
                self.authority
            )
            .as_bytes(),
        );
        if let Some(bearer) = bearer {
            request.extend_from_slice(b"authorization: Bearer ");
            request.extend_from_slice(bearer.as_bytes());
            request.extend_from_slice(b"\r\n");
        }
        if let Some(body) = body {
            request.extend_from_slice(
                format!(
                    "content-type: application/json\r\ncontent-length: {}\r\n",
                    body.len()
                )
                .as_bytes(),
            );
        }
        request.extend_from_slice(b"\r\n");
        if let Some(body) = body {
            request.extend_from_slice(body);
        }
        request
    }

    /// Whether plaintext HTTP/1.1 on the TLS port got an HTTP answer.
    pub async fn plaintext_answered(&self) -> Result<bool, Fault> {
        let mut tcp = self.tcp().await?;
        let request = format!(
            "GET /v1/machine HTTP/1.1\r\nhost: {}\r\nconnection: close\r\n\r\n",
            self.authority
        );
        if self
            .bounded(async {
                tcp.write_all(request.as_bytes())
                    .await
                    .map_err(|_| Fault::Transport)
            })
            .await
            .is_err()
        {
            return Ok(false);
        }
        let mut answer = [0_u8; 64];
        let read = match tokio::time::timeout(self.io, tcp.read(&mut answer)).await {
            Ok(Ok(count)) => count,
            Ok(Err(_)) | Err(_) => 0,
        };
        Ok(answer[..read].starts_with(b"HTTP/"))
    }

    /// Whether a TLS 1.2-only `ClientHello` got a `ServerHello`.
    pub async fn tls12_accepted(&self) -> Result<bool, Fault> {
        let mut tcp = self.tcp().await?;
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| Fault::Transport)?;
        let host = match &self.server_name {
            ServerName::DnsName(name) => Some(name.as_ref().to_owned()),
            _ => None,
        };
        let hello = tls12_client_hello(&random, host.as_deref());
        if self
            .bounded(async { tcp.write_all(&hello).await.map_err(|_| Fault::Transport) })
            .await
            .is_err()
        {
            return Ok(false);
        }
        let mut answer = [0_u8; 6];
        let mut read = 0;
        while read < answer.len() {
            match tokio::time::timeout(self.io, tcp.read(&mut answer[read..])).await {
                Ok(Ok(count)) if count > 0 => read += count,
                _ => break,
            }
        }
        Ok(server_hello(&answer[..read]))
    }

    /// Whether a handshake offering only `h2` completed.
    pub async fn h2_only_accepted(&self) -> Result<bool, Fault> {
        Ok(self.tls(self.server_name.clone(), b"h2").await?.is_ok())
    }

    /// For a name the certificate must not carry: `Ok(true)` when verification failed on the
    /// certificate, `Ok(false)` when the handshake verified the name.
    pub async fn name_refused(&self, name: &str) -> Result<bool, Fault> {
        let name = ServerName::try_from(name.to_owned()).map_err(|_| Fault::Handshake)?;
        match self.tls(name, b"http/1.1").await? {
            Ok(_) => Ok(false),
            Err(error) if certificate_error(&error) => Ok(true),
            Err(_) => Err(Fault::Handshake),
        }
    }
}

/// A TLS 1.2 `ClientHello` without `supported_versions`, so a TLS 1.3-only server must refuse it.
pub fn tls12_client_hello(random: &[u8; 32], host: Option<&str>) -> Vec<u8> {
    fn u16_bytes(value: usize) -> [u8; 2] {
        u16::try_from(value).unwrap_or(u16::MAX).to_be_bytes()
    }
    let mut extensions = Vec::new();
    if let Some(host) = host {
        let mut entry = vec![0];
        entry.extend(u16_bytes(host.len()));
        entry.extend(host.as_bytes());
        let mut list = u16_bytes(entry.len()).to_vec();
        list.extend(entry);
        extensions.extend([0, 0]);
        extensions.extend(u16_bytes(list.len()));
        extensions.extend(list);
    }
    // supported_groups: x25519, secp256r1, secp384r1.
    extensions.extend([
        0x00, 0x0a, 0x00, 0x08, 0x00, 0x06, 0x00, 0x1d, 0x00, 0x17, 0x00, 0x18,
    ]);
    // ec_point_formats: uncompressed.
    extensions.extend([0x00, 0x0b, 0x00, 0x02, 0x01, 0x00]);
    // signature_algorithms.
    let algorithms: [u8; 16] = [
        0x04, 0x03, 0x08, 0x04, 0x04, 0x01, 0x05, 0x03, 0x08, 0x05, 0x05, 0x01, 0x08, 0x06, 0x06,
        0x01,
    ];
    extensions.extend([0x00, 0x0d]);
    extensions.extend(u16_bytes(algorithms.len() + 2));
    extensions.extend(u16_bytes(algorithms.len()));
    extensions.extend(algorithms);
    let suites: [u8; 12] = [
        0xc0, 0x2f, 0xc0, 0x30, 0xc0, 0x2b, 0xc0, 0x2c, 0xcc, 0xa8, 0xcc, 0xa9,
    ];
    let mut body = vec![0x03, 0x03];
    body.extend(random);
    body.push(0);
    body.extend(u16_bytes(suites.len()));
    body.extend(suites);
    body.extend([0x01, 0x00]);
    body.extend(u16_bytes(extensions.len()));
    body.extend(extensions);
    let length = u32::try_from(body.len()).unwrap_or(0).to_be_bytes();
    let mut handshake = vec![0x01, length[1], length[2], length[3]];
    handshake.extend(body);
    let mut record = vec![0x16, 0x03, 0x01];
    record.extend(u16_bytes(handshake.len()));
    record.extend(handshake);
    record
}

/// Whether the first bytes a server sent are a handshake record carrying a `ServerHello`.
pub fn server_hello(answer: &[u8]) -> bool {
    answer.len() >= 6 && answer[0] == 0x16 && answer[5] == 0x02
}

/// Parses one complete HTTP/1.1 response, `Ok(None)` while incomplete.
fn parse_reply(received: &[u8]) -> Result<Option<Reply>, Fault> {
    let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Ok(None);
    };
    let head = std::str::from_utf8(&received[..end]).map_err(|_| Fault::Malformed)?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.strip_prefix("HTTP/1.1 "))
        .and_then(|rest| rest.get(..3))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(Fault::Malformed)?;
    let mut contract = None;
    let mut contract_sha256 = None;
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(Fault::Malformed)?;
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "x-b10x-contract" => contract = Some(value.to_owned()),
            "x-b10x-contract-bundle-sha256" => contract_sha256 = Some(value.to_owned()),
            "content-length" => {
                length = Some(value.parse::<usize>().map_err(|_| Fault::Malformed)?);
            }
            "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
            _ => {}
        }
    }
    let rest = &received[end + 4..];
    let body = if chunked {
        match dechunk(rest)? {
            Some(body) => body,
            None => return Ok(None),
        }
    } else {
        let length = length.unwrap_or(0);
        if rest.len() < length {
            return Ok(None);
        }
        rest[..length].to_vec()
    };
    Ok(Some(Reply {
        status,
        contract,
        contract_sha256,
        body,
    }))
}

fn dechunk(mut rest: &[u8]) -> Result<Option<Vec<u8>>, Fault> {
    let mut body = Vec::new();
    loop {
        let Some(line_end) = rest.windows(2).position(|window| window == b"\r\n") else {
            return Ok(None);
        };
        let size = std::str::from_utf8(&rest[..line_end]).map_err(|_| Fault::Malformed)?;
        let size = size.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size, 16).map_err(|_| Fault::Malformed)?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(Some(body));
        }
        if rest.len() < size + 2 {
            return Ok(None);
        }
        body.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

/// One plaintext `GET` on the operational listener (ADR 0045), bounded by `io` per step.
pub async fn plain_get(authority: &str, path: &str, io: Duration) -> Result<Reply, Fault> {
    let mut tcp = tokio::time::timeout(io, TcpStream::connect(authority))
        .await
        .map_err(|_| Fault::IoTimeout)?
        .map_err(|_| Fault::Connect)?;
    let request = format!("GET {path} HTTP/1.1\r\nhost: {authority}\r\nconnection: close\r\n\r\n");
    tokio::time::timeout(io, tcp.write_all(request.as_bytes()))
        .await
        .map_err(|_| Fault::IoTimeout)?
        .map_err(|_| Fault::Transport)?;
    let mut received = Vec::new();
    let mut block = vec![0_u8; READ_BLOCK];
    loop {
        let read = match tokio::time::timeout(io, tcp.read(&mut block)).await {
            Err(_) => return Err(Fault::IoTimeout),
            Ok(Ok(count)) => count,
            Ok(Err(_)) => return Err(Fault::Transport),
        };
        received.extend_from_slice(&block[..read]);
        if let Some(reply) = parse_reply(&received)? {
            return Ok(reply);
        }
        if read == 0 || received.len() > MAX_RESPONSE {
            return Err(Fault::Malformed);
        }
    }
}

/// `host:port` of an `http://` origin; `None` for anything else.
pub fn http_authority(origin: &str) -> Option<String> {
    let rest = origin.strip_prefix("http://")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
        return None;
    }
    let (host, port) = rest.rsplit_once(':')?;
    port.parse::<u16>().ok().filter(|port| *port > 0)?;
    (!host.is_empty()).then(|| rest.to_owned())
}

/// `host:port` of an `https://` origin, and its host; `None` for anything else.
pub fn https_authority(endpoint: &str) -> Option<(String, String)> {
    let rest = endpoint.strip_prefix("https://")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
        return None;
    }
    let (host, port) = if let Some(stripped) = rest.strip_prefix('[') {
        let (host, port) = stripped.split_once("]:")?;
        (host.to_owned(), port)
    } else {
        let (host, port) = rest.rsplit_once(':')?;
        (host.to_owned(), port)
    };
    port.parse::<u16>().ok().filter(|port| *port > 0)?;
    (!host.is_empty()).then(|| (rest.to_owned(), host))
}

#[cfg(test)]
mod tests {
    use super::{
        dechunk, http_authority, https_authority, parse_reply, server_hello, tls12_client_hello,
    };

    #[test]
    fn a_tls12_hello_offers_no_supported_versions_extension() {
        let hello = tls12_client_hello(&[7; 32], Some("substrate.test"));
        assert_eq!(&hello[..3], &[0x16, 0x03, 0x01]);
        assert_eq!(hello[5], 0x01, "client hello");
        assert_eq!(&hello[9..11], &[0x03, 0x03], "legacy version TLS 1.2");
        let declared = usize::from(u16::from_be_bytes([hello[3], hello[4]]));
        assert_eq!(declared, hello.len() - 5);
        assert!(
            !hello.windows(2).any(|window| window == [0x00, 0x2b]),
            "no supported_versions extension"
        );
        assert!(hello.windows(14).any(|window| window == b"substrate.test"));
    }

    #[test]
    fn only_a_handshake_record_with_a_server_hello_counts_as_accepted() {
        assert!(server_hello(&[0x16, 0x03, 0x03, 0x00, 0x40, 0x02]));
        assert!(!server_hello(&[0x15, 0x03, 0x03, 0x00, 0x02, 0x02]));
        assert!(!server_hello(&[]));
    }

    #[test]
    fn replies_parse_by_length_and_by_chunks() {
        let reply = parse_reply(
            b"HTTP/1.1 404 Not Found\r\nx-b10x-contract: a\r\ncontent-length: 2\r\n\r\n{}",
        )
        .expect("parse")
        .expect("complete");
        assert_eq!((reply.status, reply.contract.as_deref()), (404, Some("a")));
        assert_eq!(reply.body, b"{}");
        assert!(
            parse_reply(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\n{}")
                .expect("parse")
                .is_none()
        );
        assert_eq!(
            dechunk(b"2\r\n{}\r\n0\r\n\r\n").expect("chunks"),
            Some(b"{}".to_vec())
        );
    }

    #[test]
    fn only_an_https_origin_with_a_port_is_an_endpoint() {
        assert_eq!(
            https_authority("https://127.0.0.1:8443/"),
            Some(("127.0.0.1:8443".to_owned(), "127.0.0.1".to_owned()))
        );
        assert_eq!(
            https_authority("https://[::1]:8443"),
            Some(("[::1]:8443".to_owned(), "::1".to_owned()))
        );
        assert_eq!(https_authority("http://a:1/"), None);
        assert_eq!(https_authority("https://a/"), None);
        assert_eq!(https_authority("https://a:1/v1"), None);
        assert_eq!(https_authority("https://u@a:1/"), None);
        assert_eq!(
            http_authority("http://127.0.0.1:9090"),
            Some("127.0.0.1:9090".to_owned())
        );
        assert_eq!(http_authority("https://127.0.0.1:9090"), None);
        assert_eq!(http_authority("http://a/metrics"), None);
    }
}
