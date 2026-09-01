use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use futures_util::StreamExt as _;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use ulid::Ulid;

use crate::model::{CONTRACT, CONTRACT_SHA256, EventStreamFrame};
use crate::{EventPage, MetricsSample, SdkError};

const MAX_RESPONSE_HEAD: usize = 16 * 1024;
const MAX_RESPONSE_BODY: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Transport {
    socket: PathBuf,
}

pub(crate) struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Transport {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, SdkError> {
        let mut stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))?;
        let request_id = format!("sdk_{}", Ulid::generate());
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nx-request-id: {request_id}\r\nConnection: close\r\n"
        );
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

    pub(crate) async fn websocket(
        &self,
        path: &str,
    ) -> Result<WebSocketStream<UnixStream>, SdkError> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))?;
        let request = format!("ws://localhost{path}")
            .into_client_request()
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let (socket, response) = match tokio_tungstenite::client_async(request, stream).await {
            Ok(upgraded) => upgraded,
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                let headers = response
                    .headers()
                    .iter()
                    .filter_map(|(name, value)| {
                        Some((
                            name.as_str().to_ascii_lowercase(),
                            value.to_str().ok()?.to_owned(),
                        ))
                    })
                    .collect();
                verify_contract_headers(&headers)?;
                let response = HttpResponse {
                    status: response.status().as_u16(),
                    body: response.body().clone().unwrap_or_default(),
                };
                return match decode_result::<serde_json::Value>(&response) {
                    Err(error) => Err(error),
                    Ok(_) => Err(SdkError::Protocol(
                        "WebSocket refusal carried a success envelope".to_owned(),
                    )),
                };
            }
            Err(error) => return Err(SdkError::Transport(error.to_string())),
        };
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().ok()?.to_owned(),
                ))
            })
            .collect();
        verify_contract_headers(&headers)?;
        Ok(socket)
    }
}

pub struct EventStream {
    socket: WebSocketStream<UnixStream>,
}

pub struct MetricsStream {
    socket: WebSocketStream<UnixStream>,
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

async fn read_response(stream: &mut UnixStream) -> Result<HttpResponse, SdkError> {
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
}
