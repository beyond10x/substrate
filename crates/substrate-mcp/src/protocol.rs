use std::fs::{File, OpenOptions};
use std::io::Read as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _, ReadBuf};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use crate::{State, surface};

pub const MAX_FRAME_BYTES: usize = 2_097_152;
pub const MAX_REQUEST_ID_BYTES: usize = 128;
pub const MAX_CALLS: usize = 16;
pub const WAIT_LIMIT: Duration = Duration::from_secs(30);

#[allow(
    clippy::too_many_lines,
    reason = "the bounded session lifecycle and its shutdown order stay visible together"
)]
pub async fn serve(state: Arc<State>) -> Result<(), String> {
    let mut input = BoundedLines::new(NonblockingStdin::open()?);
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| "could not install interrupt handler".to_owned())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| "could not install termination handler".to_owned())?;
    let (outbound, mut responses) = mpsc::channel::<Value>(MAX_CALLS);
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(response) = responses.recv().await {
            let mut bytes = serde_json::to_vec(&response)
                .map_err(|_| "could not encode MCP response".to_owned())?;
            if bytes.len() > MAX_FRAME_BYTES {
                return Err("outgoing MCP frame exceeded its bound".to_owned());
            }
            bytes.push(b'\n');
            stdout
                .write_all(&bytes)
                .await
                .map_err(|_| "could not write MCP response".to_owned())?;
            stdout
                .flush()
                .await
                .map_err(|_| "could not flush MCP response".to_owned())?;
        }
        Ok(())
    });
    let initialized = Arc::new(AtomicBool::new(false));
    let permits = Arc::new(Semaphore::new(MAX_CALLS));
    let mut calls = JoinSet::new();

    loop {
        let next = tokio::select! {
            frame = input.next() => frame?,
            _ = interrupt.recv() => None,
            _ = terminate.recv() => None,
        };
        let Some(frame) = next else {
            break;
        };
        let message: Value = serde_json::from_slice(&frame)
            .map_err(|_| "incoming MCP frame is not one valid UTF-8 JSON document".to_owned())?;
        let Some(request) = Request::parse(&message) else {
            outbound
                .send(json_rpc_error(Value::Null, -32600, "invalid request", None))
                .await
                .map_err(|_| "MCP response writer stopped".to_owned())?;
            continue;
        };
        if request.id.is_none() {
            if request.method == "notifications/initialized" {
                initialized.store(true, Ordering::Release);
            }
            continue;
        }
        let id = request.id.expect("checked as present");
        if request.method == "initialize" {
            initialized.store(true, Ordering::Release);
            outbound
                .send(json_rpc_result(id, initialize_result(&request.params)))
                .await
                .map_err(|_| "MCP response writer stopped".to_owned())?;
            continue;
        }
        if !initialized.load(Ordering::Acquire) {
            outbound
                .send(json_rpc_error(
                    id,
                    -32002,
                    "server is not initialized",
                    None,
                ))
                .await
                .map_err(|_| "MCP response writer stopped".to_owned())?;
            continue;
        }
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            outbound
                .send(json_rpc_error(
                    id,
                    -32001,
                    "concurrent call bound reached",
                    None,
                ))
                .await
                .map_err(|_| "MCP response writer stopped".to_owned())?;
            continue;
        };
        let state = Arc::clone(&state);
        let outbound = outbound.clone();
        calls.spawn(async move {
            let _permit = permit;
            let response = match tokio::time::timeout(
                WAIT_LIMIT,
                dispatch(&request.method, request.params, &state),
            )
            .await
            {
                Ok(Ok(value)) => json_rpc_result(id, value),
                Ok(Err((code, message, data))) => json_rpc_error(id, code, message, data),
                Err(_) => json_rpc_error(id, -32003, "adapter call deadline elapsed", None),
            };
            let _ = outbound.send(response).await;
        });
        while calls.try_join_next().is_some() {}
    }

    calls.abort_all();
    while calls.join_next().await.is_some() {}
    drop(outbound);
    writer
        .await
        .map_err(|_| "MCP response writer task failed".to_owned())?
}

struct NonblockingStdin {
    input: AsyncFd<File>,
}

impl NonblockingStdin {
    fn open() -> Result<Self, String> {
        let input = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK)
            .open("/dev/stdin")
            .map_err(|_| "could not open nonblocking MCP input".to_owned())?;
        AsyncFd::new(input)
            .map(|input| Self { input })
            .map_err(|_| "could not register nonblocking MCP input".to_owned())
    }
}

impl AsyncRead for NonblockingStdin {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            let mut ready = ready!(self.input.poll_read_ready(context))?;
            let result = ready.try_io(|input| {
                let mut file = input.get_ref();
                file.read(buffer.initialize_unfilled())
            });
            match result {
                Ok(Ok(read)) => {
                    buffer.advance(read);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_) => {}
            }
        }
    }
}

async fn dispatch(
    method: &str,
    params: Value,
    state: &State,
) -> Result<Value, (i64, &'static str, Option<Value>)> {
    match method {
        "ping" => Ok(json!({})),
        "tools/list" => Ok(surface::tools()),
        "tools/call" => Ok(surface::call(params, state).await),
        "resources/list" => Ok(surface::resources()),
        "resources/templates/list" => Ok(surface::resource_templates()),
        "resources/read" => surface::read_resource(params, state)
            .await
            .map_err(|error| {
                (
                    -32602,
                    "invalid or refused resource request",
                    Some(surface::error_detail(error)),
                )
            }),
        _ => Err((-32601, "method not found", None)),
    }
}

fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_else(|| rmcp::model::ProtocolVersion::LATEST.as_str());
    let negotiated = rmcp::model::ProtocolVersion::KNOWN_VERSIONS
        .iter()
        .find(|version| version.as_str() == requested)
        .unwrap_or(&rmcp::model::ProtocolVersion::LATEST)
        .as_str();
    json!({
        "protocolVersion": negotiated,
        "capabilities": {
            "tools": {"listChanged": false},
            "resources": {"subscribe": false, "listChanged": false}
        },
        "serverInfo": {
            "name": "b10x-substrate-mcp",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Disposable development-only Substrate test surface. Every mutation requires an operation_id."
    })
}

struct Request {
    id: Option<Value>,
    method: String,
    params: Value,
}

impl Request {
    fn parse(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if object.get("jsonrpc")?.as_str()? != "2.0" {
            return None;
        }
        let method = object.get("method")?.as_str()?.to_owned();
        let id = object.get("id").cloned();
        if let Some(id) = &id {
            match id {
                Value::String(value) if value.len() <= MAX_REQUEST_ID_BYTES => {}
                Value::Number(_) => {}
                _ => return None,
            }
        }
        Some(Self {
            id,
            method,
            params: object.get("params").cloned().unwrap_or_else(|| json!({})),
        })
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the helper closes ownership of one response"
)]
fn json_rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the helper closes ownership of one response"
)]
fn json_rpc_error(id: Value, code: i64, message: &'static str, data: Option<Value>) -> Value {
    let mut error = json!({"code": code, "message": message});
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

struct BoundedLines<R> {
    input: R,
    buffered: Vec<u8>,
}

impl<R: AsyncRead + Unpin> BoundedLines<R> {
    fn new(input: R) -> Self {
        Self {
            input,
            buffered: Vec::new(),
        }
    }

    async fn next(&mut self) -> Result<Option<Vec<u8>>, String> {
        loop {
            if let Some(newline) = self.buffered.iter().position(|byte| *byte == b'\n') {
                if newline > MAX_FRAME_BYTES {
                    return Err("incoming MCP frame exceeded its bound".to_owned());
                }
                let mut rest = self.buffered.split_off(newline + 1);
                std::mem::swap(&mut rest, &mut self.buffered);
                rest.truncate(newline);
                if rest.last() == Some(&b'\r') {
                    rest.pop();
                }
                return Ok(Some(rest));
            }
            if self.buffered.len() > MAX_FRAME_BYTES {
                return Err("incoming MCP frame exceeded its bound".to_owned());
            }
            let mut chunk = [0_u8; 8 * 1024];
            let read = self
                .input
                .read(&mut chunk)
                .await
                .map_err(|_| "could not read MCP input".to_owned())?;
            if read == 0 {
                if self.buffered.is_empty() {
                    return Ok(None);
                }
                if self.buffered.len() > MAX_FRAME_BYTES {
                    return Err("incoming MCP frame exceeded its bound".to_owned());
                }
                return Ok(Some(std::mem::take(&mut self.buffered)));
            }
            self.buffered.extend_from_slice(&chunk[..read]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BoundedLines, MAX_CALLS, MAX_FRAME_BYTES, MAX_REQUEST_ID_BYTES, Request, initialize_result,
    };
    use serde_json::json;

    #[tokio::test]
    async fn the_reader_refuses_one_unbounded_frame() {
        let bytes = vec![b'x'; MAX_FRAME_BYTES + 1];
        let mut lines = BoundedLines::new(bytes.as_slice());
        assert!(lines.next().await.is_err());
    }

    #[tokio::test]
    async fn the_reader_accepts_the_exact_frame_bound() {
        let mut bytes = vec![b'x'; MAX_FRAME_BYTES];
        bytes.push(b'\n');
        let mut lines = BoundedLines::new(bytes.as_slice());
        assert_eq!(
            lines
                .next()
                .await
                .expect("bounded frame")
                .expect("frame")
                .len(),
            MAX_FRAME_BYTES
        );
    }

    #[tokio::test]
    async fn the_call_semaphore_admits_exactly_the_declared_bound() {
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CALLS));
        let mut held = Vec::new();
        for _ in 0..MAX_CALLS {
            held.push(
                std::sync::Arc::clone(&permits)
                    .try_acquire_owned()
                    .expect("declared permit"),
            );
        }
        assert!(std::sync::Arc::clone(&permits).try_acquire_owned().is_err());
    }

    #[test]
    fn batches_and_long_string_ids_are_not_requests() {
        assert!(Request::parse(&json!([])).is_none());
        assert!(
            Request::parse(&json!({
                "jsonrpc": "2.0",
                "id": "x".repeat(129),
                "method": "ping"
            }))
            .is_none()
        );
        assert!(
            Request::parse(&json!({
                "jsonrpc": "2.0",
                "id": "x".repeat(MAX_REQUEST_ID_BYTES),
                "method": "ping"
            }))
            .is_some()
        );
    }

    #[test]
    fn initialization_negotiates_only_known_protocol_versions() {
        let known = initialize_result(&json!({"protocolVersion": "2025-06-18"}));
        assert_eq!(known["protocolVersion"], "2025-06-18");

        let unknown = initialize_result(&json!({"protocolVersion": "2099-01-01"}));
        assert_eq!(
            unknown["protocolVersion"],
            rmcp::model::ProtocolVersion::LATEST.as_str()
        );
    }
}
