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
use crate::{EventPage, SdkError};

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
        let (socket, response) = tokio_tungstenite::client_async(request, stream)
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))?;
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
