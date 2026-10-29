//! `pods/exec` over WebSocket `v5.channel.k8s.io`: one bounded call that sends its whole input,
//! closes it, and reads both output streams and the API's `Status` until the server closes.
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
use crate::api::{Api, ApiError};

/// The one subprotocol the client offers. A server that does not choose it is refused.
pub const EXEC_PROTOCOL: &str = "v5.channel.k8s.io";

const STDIN: u8 = 0;
const STDOUT: u8 = 1;
const STDERR: u8 = 2;
const STATUS: u8 = 3;
const CLOSE: u8 = 255;
/// The input bytes one message carries.
const STDIN_CHUNK: usize = 64 * 1024;

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
    /// # Errors
    ///
    /// [`ApiError::Answered`] when the API refuses the upgrade; [`ApiError::Unanswered`] when no
    /// answer came, the server chose no `v5.channel.k8s.io`, or the stream failed or outlasted
    /// the call bound before the server closed it: the command may have run.
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

    async fn exec_unbounded(
        &self,
        pod: &str,
        container: &str,
        command: &[&str],
        stdin: &[u8],
        limit: usize,
    ) -> Result<ExecOutput, ApiError> {
        let request = self.exec_request(pod, container, command)?;
        let tls = self.connect().await.map_err(ApiError::Unanswered)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_API_BODY_BYTES))
            .max_frame_size(Some(MAX_API_BODY_BYTES));
        let (mut socket, _) =
            match tokio_tungstenite::client_async_with_config(request, tls, Some(config)).await {
                Ok(connected) => connected,
                Err(tungstenite::Error::Http(response)) => {
                    let status = response.status().as_u16();
                    let body = response.body().as_deref().unwrap_or_default();
                    return Err(ApiError::Answered {
                        status,
                        detail: format!("exec answered {status}: {}", self.status_message(body)),
                    });
                }
                Err(error) => {
                    return Err(ApiError::Unanswered(self.redact(&format!("exec: {error}"))));
                }
            };
        let unanswered = |error: tungstenite::Error| {
            ApiError::Unanswered(self.redact(&format!("exec: {error}")))
        };
        for chunk in stdin.chunks(STDIN_CHUNK) {
            let mut message = Vec::with_capacity(chunk.len() + 1);
            message.push(STDIN);
            message.extend_from_slice(chunk);
            socket
                .feed(Message::binary(message))
                .await
                .map_err(unanswered)?;
        }
        socket
            .send(Message::binary(vec![CLOSE, STDIN]))
            .await
            .map_err(unanswered)?;
        let mut output = ExecOutput::default();
        while let Some(message) = socket.next().await {
            let data = match message {
                Ok(Message::Binary(data)) => data,
                Ok(Message::Close(_)) => break,
                Ok(_) => continue,
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    break;
                }
                Err(error) => return Err(unanswered(error)),
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
        if output.status.is_none() {
            return Err(ApiError::Unanswered(
                "exec ended without a status".to_owned(),
            ));
        }
        Ok(output)
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
