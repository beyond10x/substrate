//! `pods/exec` over WebSocket `v5.channel.k8s.io`: one bounded call that sends its whole input and
//! closes it while it reads both output streams and the API's `Status` until the server closes.
//!
//! Every binary message carries its channel in its first byte: 0 stdin, 1 stdout, 2 stderr, 3 the
//! final `Status`. Version 5 adds 255, whose one-byte payload names the channel it closes, so the
//! input ends without closing the connection.

use futures_util::{SinkExt as _, StreamExt as _};
use hyper::header;
use serde_json::Value;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Message};

use crate::MAX_API_BODY_BYTES;
use crate::api::{Api, ApiError, container_name, pod_name};

/// The one subprotocol the client offers. A server that does not choose it is refused.
pub const EXEC_PROTOCOL: &str = "v5.channel.k8s.io";

const STDIN: u8 = 0;
const STDOUT: u8 = 1;
const STDERR: u8 = 2;
const STATUS: u8 = 3;
const CLOSE: u8 = 255;
/// The input bytes one message carries.
const STDIN_CHUNK: usize = 64 * 1024;
/// The bytes of a refused upgrade's body the client reads: a `Status` is far smaller.
const MAX_REFUSAL_BODY_BYTES: usize = 64 * 1024;
/// How long the client waits for the rest of a refused upgrade's body after its head.
const REFUSAL_BODY_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// What one `pods/exec` call read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExecOutput {
    /// The first `limit` bytes of the command's stdout.
    pub stdout: Vec<u8>,
    /// The first `limit` bytes of its stderr.
    pub stderr: Vec<u8>,
    /// Whether stdout passed `limit`.
    pub stdout_truncated: bool,
    /// Whether stderr passed `limit`.
    pub stderr_truncated: bool,
    /// The `Status` the API sent on channel 3, if it sent one.
    pub status: Option<Value>,
}

impl ExecOutput {
    /// The command's exit code as the `Status` states it: 0 for `Success`, the `ExitCode` cause of
    /// a `NonZeroExitCode` failure, and `None` for any other or no `Status`.
    pub fn exit_code(&self) -> Option<i32> {
        let status = self.status.as_ref()?;
        if status["status"] == "Success" {
            return Some(0);
        }
        if status["reason"] != "NonZeroExitCode" {
            return None;
        }
        status["details"]["causes"]
            .as_array()?
            .iter()
            .find(|cause| cause["reason"] == "ExitCode")?["message"]
            .as_str()?
            .parse()
            .ok()
    }
}

impl Api {
    /// Runs `command` in `container` of the Pod `pod` through `pods/exec`, with `stdin` as its
    /// whole input, and reads at most `limit` bytes of each output stream, within the client's
    /// call bound.
    ///
    /// The input is written while the output is read, so a command that writes while it reads
    /// completes, and a command that ends before it read all of its input keeps its `Status`.
    ///
    /// # Errors
    ///
    /// [`ApiError::Invalid`], before any request, for a `pod` that is not a DNS-1123 subdomain
    /// or a `container` that is not a DNS-1123 label; [`ApiError::Answered`] when the API refuses
    /// the upgrade; [`ApiError::Unanswered`] when no answer came, the server chose no
    /// `v5.channel.k8s.io`, or the stream failed or outlasted the call bound before the server
    /// sent its `Status`: the command may have run.
    pub async fn exec(
        &self,
        pod: &str,
        container: &str,
        command: &[&str],
        stdin: &[u8],
        limit: usize,
    ) -> Result<ExecOutput, ApiError> {
        tokio::time::timeout(
            self.timeout,
            self.exec_unbounded(pod, container, command, stdin, limit),
        )
        .await
        .map_err(|_| ApiError::Unanswered("the exec did not end in time".to_owned()))?
    }

    /// The upgrade request: the exec address, the one offered subprotocol and the credential.
    fn exec_request(
        &self,
        pod: &str,
        container: &str,
        command: &[&str],
    ) -> Result<tungstenite::handshake::client::Request, ApiError> {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("container", container);
        for part in command {
            query.append_pair("command", part);
        }
        query.append_pair("stdin", "true");
        query.append_pair("stdout", "true");
        query.append_pair("stderr", "true");
        let path = format!("{}/{pod}/exec?{}", self.pods(), query.finish());
        let mut request = format!("wss://{}{}", self.authority(), self.target(&path))
            .into_client_request()
            .map_err(|_| ApiError::Unanswered("exec: the request is not a URL".to_owned()))?;
        let headers = request.headers_mut();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            header::HeaderValue::from_static(EXEC_PROTOCOL),
        );
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static(self.user_agent),
        );
        if let Some(value) = self.authorization().map_err(ApiError::Unanswered)? {
            headers.insert(header::AUTHORIZATION, value);
        }
        Ok(request)
    }

    /// The API's refusal of the upgrade, `response`, with the `Status` its body carries. The
    /// handshake only borrowed `connection`, so the body can still be read from it: tungstenite
    /// keeps only the bytes that arrived with the head.
    async fn refused_upgrade<S>(
        &self,
        response: &tungstenite::http::Response<Option<Vec<u8>>>,
        connection: &mut S,
    ) -> ApiError
    where
        S: tokio::io::AsyncRead + Unpin,
    {
        let status = response.status().as_u16();
        let tail = response.body().as_deref().unwrap_or_default();
        let body = refusal_body(response.headers(), tail, connection).await;
        ApiError::Answered {
            status,
            reason: self.status_reason(&body),
            detail: format!("exec answered {status}: {}", self.status_message(&body)),
        }
    }

    async fn exec_unbounded(
        &self,
        pod: &str,
        container: &str,
        command: &[&str],
        stdin: &[u8],
        limit: usize,
    ) -> Result<ExecOutput, ApiError> {
        pod_name(pod)?;
        container_name(container)?;
        let request = self.exec_request(pod, container, command)?;
        let mut tls = self.connect().await.map_err(ApiError::Unanswered)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_API_BODY_BYTES))
            .max_frame_size(Some(MAX_API_BODY_BYTES));
        let (socket, _) = match tokio_tungstenite::client_async_with_config(
            request,
            &mut tls,
            Some(config),
        )
        .await
        {
            Ok(connected) => connected,
            Err(tungstenite::Error::Http(response)) => {
                return Err(self.refused_upgrade(&response, &mut tls).await);
            }
            Err(error) => {
                return Err(ApiError::Unanswered(self.redact(&format!("exec: {error}"))));
            }
        };
        let unanswered = |error: tungstenite::Error| {
            ApiError::Unanswered(self.redact(&format!("exec: {error}")))
        };
        // The input is written while the output is read, never before it: a command that writes
        // while it reads would otherwise stall both sides on full socket buffers.
        let (mut sink, mut stream) = socket.split();
        let write = async {
            for chunk in stdin.chunks(STDIN_CHUNK) {
                let mut message = Vec::with_capacity(chunk.len() + 1);
                message.push(STDIN);
                message.extend_from_slice(chunk);
                sink.feed(Message::binary(message)).await?;
            }
            sink.send(Message::binary(vec![CLOSE, STDIN])).await
        };
        let read = async {
            let mut output = ExecOutput::default();
            while let Some(message) = stream.next().await {
                let data = match message {
                    Ok(Message::Binary(data)) => data,
                    Ok(Message::Close(_))
                    | Err(
                        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed,
                    ) => break,
                    Ok(_) => continue,
                    // The status is the server's last message: a stream that fails after it has
                    // nothing more to say.
                    Err(error) if output.status.is_none() => return Err(unanswered(error)),
                    Err(_) => break,
                };
                let Some((channel, payload)) = data.split_first() else {
                    continue;
                };
                match *channel {
                    STDOUT => keep(
                        &mut output.stdout,
                        &mut output.stdout_truncated,
                        payload,
                        limit,
                    ),
                    STDERR => keep(
                        &mut output.stderr,
                        &mut output.stderr_truncated,
                        payload,
                        limit,
                    ),
                    STATUS => {
                        output.status = Some(serde_json::from_slice(payload).map_err(|_| {
                            ApiError::Invalid("exec sent a status that is not JSON".to_owned())
                        })?);
                    }
                    _ => {}
                }
            }
            Ok(output)
        };
        tokio::pin!(write, read);
        let mut writing = true;
        let mut write_failure = None;
        let output = loop {
            tokio::select! {
                written = &mut write, if writing => {
                    writing = false;
                    write_failure = written.err();
                }
                output = &mut read => break output?,
            }
        };
        // A command that ended without reading all of its input still sent its status; only
        // without one does a failed write explain anything.
        if output.status.is_none() {
            return Err(write_failure.map_or_else(
                || ApiError::Unanswered("exec ended without a status".to_owned()),
                unanswered,
            ));
        }
        Ok(output)
    }
}

/// The body of a refused upgrade whose head announced `headers`: `tail`, what arrived with the
/// head, and what follows it on `connection`, framed as the head states (`Content-Length`,
/// chunked, or until the server closes), at most [`MAX_REFUSAL_BODY_BYTES`] and within
/// [`REFUSAL_BODY_WAIT`]. What did not arrive in time is not waited for: the refusal stands
/// without it.
async fn refusal_body<S>(headers: &header::HeaderMap, tail: &[u8], connection: &mut S) -> Vec<u8>
where
    S: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt as _;
    let chunked = headers
        .get(header::TRANSFER_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|_| !chunked);
    let mut raw = tail.to_vec();
    let complete = |raw: &[u8]| match length {
        Some(length) => raw.len() >= length,
        None if chunked => dechunk(raw).1,
        None => false,
    };
    let _ = tokio::time::timeout(REFUSAL_BODY_WAIT, async {
        let mut buffer = [0_u8; 4096];
        while !complete(&raw) && raw.len() < MAX_REFUSAL_BODY_BYTES {
            match connection.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => raw.extend_from_slice(&buffer[..read]),
            }
        }
    })
    .await;
    raw.truncate(MAX_REFUSAL_BODY_BYTES);
    match length {
        Some(length) => {
            raw.truncate(length);
            raw
        }
        None if chunked => dechunk(&raw).0,
        None => raw,
    }
}

/// The data of a chunked body read so far, and whether its last chunk arrived. A chunk not yet
/// whole, or a size line that does not parse, ends the data there.
fn dechunk(raw: &[u8]) -> (Vec<u8>, bool) {
    let mut data = Vec::new();
    let mut rest = raw;
    loop {
        let Some(end) = rest.windows(2).position(|pair| pair == b"\r\n") else {
            return (data, false);
        };
        let size = std::str::from_utf8(&rest[..end])
            .ok()
            .and_then(|line| line.split(';').next())
            .and_then(|size| usize::from_str_radix(size.trim(), 16).ok());
        let Some(size) = size else {
            return (data, false);
        };
        if size == 0 {
            return (data, true);
        }
        let start = end + 2;
        let after = rest.get(start..).unwrap_or_default();
        let Some(chunk) = after.get(..size) else {
            data.extend_from_slice(after);
            return (data, false);
        };
        data.extend_from_slice(chunk);
        rest = after
            .get(size..)
            .and_then(|tail| tail.get(2..))
            .unwrap_or_default();
    }
}

/// Appends `payload` to `stream` up to `limit` bytes; past it, marks the stream truncated.
fn keep(stream: &mut Vec<u8>, truncated: &mut bool, payload: &[u8], limit: usize) {
    let room = limit.saturating_sub(stream.len());
    if payload.len() > room {
        *truncated = true;
    }
    stream.extend_from_slice(&payload[..payload.len().min(room)]);
}
