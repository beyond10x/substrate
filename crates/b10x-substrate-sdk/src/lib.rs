#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![allow(
    clippy::missing_errors_doc,
    reason = "all fallible public calls return the documented SdkError variants"
)]

mod managed;
mod model;
mod transport;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use base64::Engine as _;
use futures_util::{SinkExt as _, StreamExt as _};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use ulid::Ulid;

pub use managed::{ManagedDaemon, ManagedDaemonBuilder, run_daemon_child_if_requested};
pub use model::{
    BaselineEnvironment, Event, EventPage, ExecExit, ExecObservation, ExecState, ExecutionPolicy,
    ExecutionPolicyBuilder, FileContents, FileObservation, Lease, Machine, ObservedRefusal,
    Operation, OperationState, OutputStream, PipeFrame, PipeSessionObservation, PipeSessionState,
    Refusal, RefusalClass, RunOutput, Signal, WorkspaceObservation, WorkspaceState,
};
pub use transport::EventStream;

use transport::{Transport, decode_result, encode_path};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SdkError {
    #[error("substrate transport failed: {0}")]
    Transport(String),
    #[error("substrate refused the request: {0:?}")]
    Refusal(Refusal),
    #[error("substrate protocol error: {0}")]
    Protocol(String),
    #[error("builder field `{field}` is required")]
    Builder { field: &'static str },
    #[error("operation {operation_id} has no answered outcome")]
    UnknownOperation { operation_id: String },
    #[error("event history has a gap ({code})")]
    EventGap {
        code: String,
        cursor: Option<String>,
    },
    #[error("daemon contract does not match this SDK")]
    ContractMismatch {
        expected_contract: &'static str,
        expected_sha256: &'static str,
        observed_contract: Option<String>,
        observed_sha256: Option<String>,
    },
    #[error("managed daemon failed to start: {0}")]
    Startup(String),
    #[error("managed daemon shutdown failed: {0}")]
    Shutdown(String),
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    transport: Transport,
    machine: RwLock<Machine>,
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    pub fn machine(&self) -> Machine {
        self.inner
            .machine
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub async fn refresh_machine(&self) -> Result<Machine, SdkError> {
        let machine = self.read_machine().await?;
        *self
            .inner
            .machine
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = machine.clone();
        Ok(machine)
    }

    pub fn workspace(&self) -> WorkspaceBuilder {
        WorkspaceBuilder {
            client: self.clone(),
            labels: BTreeMap::new(),
            lease_ttl: None,
            operation_id: None,
        }
    }

    pub async fn get_workspace(&self, id: impl AsRef<str>) -> Result<Workspace, SdkError> {
        let path = format!("/v1/workspaces/{}", encode_path(id.as_ref()));
        let observed: substrate_wire::Workspace = self.get(&path).await?;
        Ok(Workspace {
            client: self.clone(),
            observed: observed.into(),
        })
    }

    pub async fn get_exec(&self, id: impl AsRef<str>) -> Result<Exec, SdkError> {
        let path = format!("/v1/execs/{}", encode_path(id.as_ref()));
        let observed: substrate_wire::Exec = self.get(&path).await?;
        Ok(Exec {
            client: self.clone(),
            observed: observed.into(),
        })
    }

    pub async fn get_pipe_session(&self, id: impl AsRef<str>) -> Result<PipeSession, SdkError> {
        let path = format!("/v1/pipe-sessions/{}", encode_path(id.as_ref()));
        let observed: substrate_wire::PipeSession = self.get(&path).await?;
        Ok(PipeSession {
            client: self.clone(),
            observed: observed.into(),
        })
    }

    pub async fn operation(&self, id: impl AsRef<str>) -> Result<Operation, SdkError> {
        let path = format!("/v1/ops/{}", encode_path(id.as_ref()));
        let record: substrate_wire::OperationRecord = self.get(&path).await?;
        Ok(record.into())
    }

    pub async fn events(&self, cursor: Option<&str>, limit: u32) -> Result<EventPage, SdkError> {
        if limit == 0 || limit > substrate_wire::MAX_EVENT_PAGE_ITEMS {
            return Err(SdkError::Protocol(
                "event limit is outside 1..=1000".to_owned(),
            ));
        }
        let query = serde_urlencoded::to_string(substrate_wire::EventQuery {
            cursor: cursor.map(ToOwned::to_owned),
            limit,
        })
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let page: substrate_wire::EventPage = self.get(&format!("/v1/events?{query}")).await?;
        Ok(page.into())
    }

    pub async fn event_stream(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<EventStream, SdkError> {
        if limit == 0 || limit > substrate_wire::MAX_EVENT_PAGE_ITEMS {
            return Err(SdkError::Protocol(
                "event limit is outside 1..=1000".to_owned(),
            ));
        }
        self.inner.transport.event_stream(cursor, limit).await
    }

    async fn read_machine(&self) -> Result<Machine, SdkError> {
        let snapshot: substrate_wire::CapabilitySnapshot = self.get("/v1/machine").await?;
        Ok(snapshot.into())
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, SdkError> {
        let response = self.inner.transport.request("GET", path, None).await?;
        decode_result(&response)
    }

    async fn mutation<I: Serialize, O: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        input: &I,
        operation_id: Option<String>,
    ) -> Result<(String, O), SdkError> {
        let operation_id = operation_id.unwrap_or_else(|| Ulid::generate().to_string());
        substrate_wire::validate_operation_id(&operation_id)
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let envelope = substrate_wire::Mutation {
            op: operation_id.clone(),
            input,
            delegated_context: None,
        };
        let body =
            serde_json::to_vec(&envelope).map_err(|error| SdkError::Protocol(error.to_string()))?;
        match self
            .inner
            .transport
            .request(method, path, Some(&body))
            .await
        {
            Ok(response) => decode_result(&response).map(|result| (operation_id, result)),
            Err(SdkError::Transport(_)) => {
                self.recover_mutation(method, path, &body, &operation_id)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    async fn recover_mutation<O: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        operation_id: &str,
    ) -> Result<(String, O), SdkError> {
        match self.operation(operation_id).await {
            Ok(operation) => outcome::<O>(operation, operation_id),
            Err(SdkError::Refusal(refusal)) if refusal.code == "resource.not-found" => {
                match self.inner.transport.request(method, path, Some(body)).await {
                    Ok(response) => {
                        decode_result(&response).map(|result| (operation_id.to_owned(), result))
                    }
                    Err(_) => Err(SdkError::UnknownOperation {
                        operation_id: operation_id.to_owned(),
                    }),
                }
            }
            Err(_) => Err(SdkError::UnknownOperation {
                operation_id: operation_id.to_owned(),
            }),
        }
    }
}

fn outcome<O: DeserializeOwned>(
    operation: Operation,
    operation_id: &str,
) -> Result<(String, O), SdkError> {
    if let Some(refusal) = operation.refusal {
        return Err(SdkError::Refusal(refusal));
    }
    let value = operation.result.ok_or_else(|| SdkError::UnknownOperation {
        operation_id: operation_id.to_owned(),
    })?;
    let result =
        serde_json::from_value(value).map_err(|error| SdkError::Protocol(error.to_string()))?;
    Ok((operation_id.to_owned(), result))
}

#[derive(Default)]
#[must_use]
pub struct ClientBuilder {
    socket: Option<PathBuf>,
}

impl ClientBuilder {
    pub fn unix_socket(mut self, socket: impl Into<PathBuf>) -> Self {
        self.socket = Some(socket.into());
        self
    }

    pub async fn connect(self) -> Result<Client, SdkError> {
        let socket = self.socket.ok_or(SdkError::Builder { field: "socket" })?;
        let transport = Transport::new(socket);
        let response = transport.request("GET", "/v1/machine", None).await?;
        let snapshot: substrate_wire::CapabilitySnapshot = decode_result(&response)?;
        Ok(Client {
            inner: Arc::new(ClientInner {
                transport,
                machine: RwLock::new(snapshot.into()),
            }),
        })
    }
}

#[must_use]
pub struct WorkspaceBuilder {
    client: Client,
    labels: BTreeMap<String, String>,
    lease_ttl: Option<Duration>,
    operation_id: Option<String>,
}

impl WorkspaceBuilder {
    pub fn empty(self) -> Self {
        self
    }

    pub fn label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    pub fn lease(mut self, ttl: Duration) -> Self {
        self.lease_ttl = Some(ttl);
        self
    }

    pub fn operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub async fn create(self) -> Result<Workspace, SdkError> {
        let input = substrate_wire::WorkspaceCreateInput {
            source: substrate_wire::WorkspaceSource::Empty(substrate_wire::EmptySource::Empty),
            labels: self.labels,
            storage: None,
            lease_ttl_ms: duration_millis(self.lease_ttl)?,
        };
        let (_, observed): (_, substrate_wire::Workspace) = self
            .client
            .mutation("POST", "/v1/workspaces", &input, self.operation_id)
            .await?;
        Ok(Workspace {
            client: self.client,
            observed: observed.into(),
        })
    }
}

#[derive(Clone)]
pub struct Workspace {
    client: Client,
    observed: WorkspaceObservation,
}

impl Workspace {
    pub fn observation(&self) -> &WorkspaceObservation {
        &self.observed
    }

    pub fn id(&self) -> &str {
        &self.observed.id
    }

    pub fn command(&self, program: impl Into<String>) -> CommandBuilder {
        CommandBuilder {
            workspace: self.clone(),
            argv: vec![program.into()],
            allowed_environment: Vec::new(),
            environment: BTreeMap::new(),
            policy: None,
            lease_ttl: None,
            operation_id: None,
        }
    }

    pub fn pipe_session(&self, program: impl Into<String>) -> PipeSessionBuilder {
        PipeSessionBuilder {
            workspace: self.clone(),
            argv: vec![program.into()],
            allowed_environment: Vec::new(),
            environment: BTreeMap::new(),
            policy: None,
            lease_ttl: None,
            input_limit_bytes: None,
            frame_limit_bytes: None,
            queued_frames: None,
            operation_id: None,
        }
    }

    pub async fn refresh(&mut self) -> Result<&WorkspaceObservation, SdkError> {
        let fresh = self.client.get_workspace(&self.observed.id).await?;
        self.observed = fresh.observed;
        Ok(&self.observed)
    }

    pub async fn read_file(
        &self,
        path: impl AsRef<str>,
        offset: u64,
        limit_bytes: u64,
    ) -> Result<FileContents, SdkError> {
        if limit_bytes == 0 || limit_bytes > substrate_wire::MAX_IO_BYTES {
            return Err(SdkError::Protocol(
                "file read limit is outside its contract bound".to_owned(),
            ));
        }
        substrate_wire::validate_relative_path(path.as_ref())
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let query = serde_urlencoded::to_string(substrate_wire::FileReadQuery {
            mode: substrate_wire::FileMode::File,
            offset: Some(offset),
            limit_bytes: Some(limit_bytes),
            cursor: None,
            limit_items: None,
        })
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let target = format!(
            "/v1/workspaces/{}/files/{}?{query}",
            encode_path(&self.observed.id),
            encode_path(path.as_ref())
        );
        let result: substrate_wire::FileReadResult = self.client.get(&target).await?;
        let substrate_wire::FileReadResult::File(file) = result else {
            return Err(SdkError::Protocol(
                "daemon returned a directory for a file read".to_owned(),
            ));
        };
        let bytes = file
            .content
            .decode()
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        Ok(FileContents {
            workspace: file.workspace,
            path: file.path,
            offset: file.offset,
            bytes,
            eof: file.eof,
            observed_at: file.observed_at,
        })
    }

    pub async fn write_file(
        &self,
        path: impl AsRef<str>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<FileObservation, SdkError> {
        substrate_wire::validate_relative_path(path.as_ref())
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let maximum = usize::try_from(substrate_wire::MAX_FILE_BYTES).unwrap_or(usize::MAX);
        if bytes.as_ref().len() > maximum {
            return Err(SdkError::Protocol(
                "file exceeds the write bound".to_owned(),
            ));
        }
        let input = substrate_wire::FileWriteInput {
            content: substrate_wire::Base64Content {
                encoding: substrate_wire::Base64Encoding::Base64,
                data: base64::engine::general_purpose::STANDARD.encode(bytes.as_ref()),
            },
        };
        let target = format!(
            "/v1/workspaces/{}/files/{}",
            encode_path(&self.observed.id),
            encode_path(path.as_ref())
        );
        let (_, observed): (_, substrate_wire::FileObservation) =
            self.client.mutation("PUT", &target, &input, None).await?;
        Ok(observed.into())
    }

    pub async fn delete_file(&self, path: impl AsRef<str>) -> Result<bool, SdkError> {
        substrate_wire::validate_relative_path(path.as_ref())
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let target = format!(
            "/v1/workspaces/{}/files/{}",
            encode_path(&self.observed.id),
            encode_path(path.as_ref())
        );
        let (_, absent): (_, substrate_wire::FileAbsence) = self
            .client
            .mutation("DELETE", &target, &substrate_wire::EmptyInput {}, None)
            .await?;
        Ok(absent.absent)
    }

    pub async fn renew_lease(&mut self, ttl: Duration) -> Result<&WorkspaceObservation, SdkError> {
        let input = substrate_wire::LeaseRenewInput {
            ttl_ms: required_duration_millis(ttl)?,
        };
        let target = format!(
            "/v1/workspaces/{}/lease/renew",
            encode_path(&self.observed.id)
        );
        let (_, observed): (_, substrate_wire::Workspace) =
            self.client.mutation("POST", &target, &input, None).await?;
        self.observed = observed.into();
        Ok(&self.observed)
    }

    pub async fn destroy(self) -> Result<bool, SdkError> {
        let target = format!("/v1/workspaces/{}", encode_path(&self.observed.id));
        let (_, absent): (_, substrate_wire::WorkspaceAbsence) = self
            .client
            .mutation("DELETE", &target, &substrate_wire::EmptyInput {}, None)
            .await?;
        Ok(absent.absent)
    }
}

#[must_use]
pub struct CommandBuilder {
    workspace: Workspace,
    argv: Vec<String>,
    allowed_environment: Vec<BaselineEnvironment>,
    environment: BTreeMap<String, String>,
    policy: Option<ExecutionPolicy>,
    lease_ttl: Option<Duration>,
    operation_id: Option<String>,
}

#[must_use]
pub struct PipeSessionBuilder {
    workspace: Workspace,
    argv: Vec<String>,
    allowed_environment: Vec<BaselineEnvironment>,
    environment: BTreeMap<String, String>,
    policy: Option<ExecutionPolicy>,
    lease_ttl: Option<Duration>,
    input_limit_bytes: Option<u64>,
    frame_limit_bytes: Option<u64>,
    queued_frames: Option<u32>,
    operation_id: Option<String>,
}

impl PipeSessionBuilder {
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.argv.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.argv.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn allow_environment(mut self, name: BaselineEnvironment) -> Self {
        self.allowed_environment.push(name);
        self
    }

    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }

    pub fn policy(mut self, policy: ExecutionPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    pub fn lease(mut self, ttl: Duration) -> Self {
        self.lease_ttl = Some(ttl);
        self
    }

    pub fn input_limit_bytes(mut self, bytes: u64) -> Self {
        self.input_limit_bytes = Some(bytes);
        self
    }

    pub fn frame_limit_bytes(mut self, bytes: u64) -> Self {
        self.frame_limit_bytes = Some(bytes);
        self
    }

    pub fn queued_frames(mut self, frames: u32) -> Self {
        self.queued_frames = Some(frames);
        self
    }

    pub fn operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub async fn start(self) -> Result<PipeSession, SdkError> {
        let policy = self.policy.ok_or(SdkError::Builder { field: "policy" })?;
        let lease_ttl = self.lease_ttl.ok_or(SdkError::Builder { field: "lease" })?;
        let input_limit_bytes = self.input_limit_bytes.ok_or(SdkError::Builder {
            field: "input_limit_bytes",
        })?;
        let frame_limit_bytes = self.frame_limit_bytes.ok_or(SdkError::Builder {
            field: "frame_limit_bytes",
        })?;
        let queued_frames = self.queued_frames.ok_or(SdkError::Builder {
            field: "queued_frames",
        })?;
        let exec = substrate_wire::ExecStartInput {
            workspace: self.workspace.observed.id.clone(),
            argv: self.argv,
            env: substrate_wire::ExecEnvironment {
                allow: self
                    .allowed_environment
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                set: self.environment,
            },
            sandbox: substrate_wire::ConfinementRequest {
                capability_snapshot: self.workspace.client.machine().capability_snapshot,
                network: substrate_wire::NetworkMode::None,
                aperture: None,
                profile: substrate_wire::SandboxProfile::Workspace,
                required: true,
            },
            limits: policy
                .wire()
                .map_err(|error| SdkError::Protocol(error.to_owned()))?,
            wait: false,
            scratch: None,
            measurements: BTreeSet::new(),
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            lease_ttl_ms: Some(required_duration_millis(lease_ttl)?),
        };
        let input = AdvertisedPipeSessionStart {
            exec,
            input_limit_bytes,
            frame_limit_bytes,
            queued_frames,
        };
        let (_, observed): (_, substrate_wire::PipeSession) = self
            .workspace
            .client
            .mutation("POST", "/v1/pipe-sessions", &input, self.operation_id)
            .await?;
        Ok(PipeSession {
            client: self.workspace.client,
            observed: observed.into(),
        })
    }
}

#[derive(Serialize)]
struct AdvertisedPipeSessionStart {
    exec: substrate_wire::ExecStartInput,
    input_limit_bytes: u64,
    frame_limit_bytes: u64,
    queued_frames: u32,
}

#[derive(Clone)]
pub struct PipeSession {
    client: Client,
    observed: PipeSessionObservation,
}

impl PipeSession {
    pub fn observation(&self) -> &PipeSessionObservation {
        &self.observed
    }

    pub fn id(&self) -> &str {
        &self.observed.id
    }

    pub async fn refresh(&mut self) -> Result<&PipeSessionObservation, SdkError> {
        let fresh = self.client.get_pipe_session(&self.observed.id).await?;
        self.observed = fresh.observed;
        Ok(&self.observed)
    }

    pub async fn attach(&self) -> Result<PipeChannel, SdkError> {
        let target = format!(
            "/v1/pipe-sessions/{}/attach",
            encode_path(&self.observed.id)
        );
        let socket = self.client.inner.transport.websocket(&target).await?;
        Ok(PipeChannel {
            socket,
            next_sequence: 1,
        })
    }

    pub async fn signal(
        &mut self,
        signal: Signal,
        grace: Duration,
    ) -> Result<&PipeSessionObservation, SdkError> {
        let input = substrate_wire::ExecSignalInput {
            signal: signal.into(),
            grace_ms: required_duration_millis(grace)?,
        };
        let target = format!(
            "/v1/pipe-sessions/{}/signal",
            encode_path(&self.observed.id)
        );
        let (_, observed): (_, substrate_wire::PipeSession) =
            self.client.mutation("POST", &target, &input, None).await?;
        self.observed = observed.into();
        Ok(&self.observed)
    }

    pub async fn renew_lease(
        &mut self,
        ttl: Duration,
    ) -> Result<&PipeSessionObservation, SdkError> {
        let input = substrate_wire::LeaseRenewInput {
            ttl_ms: required_duration_millis(ttl)?,
        };
        let target = format!(
            "/v1/pipe-sessions/{}/lease/renew",
            encode_path(&self.observed.id)
        );
        let (_, observed): (_, substrate_wire::PipeSession) =
            self.client.mutation("POST", &target, &input, None).await?;
        self.observed = observed.into();
        Ok(&self.observed)
    }

    pub async fn retire(self) -> Result<bool, SdkError> {
        let target = format!("/v1/pipe-sessions/{}", encode_path(&self.observed.id));
        let (_, absent): (_, substrate_wire::SessionAbsence) = self
            .client
            .mutation("DELETE", &target, &substrate_wire::EmptyInput {}, None)
            .await?;
        Ok(absent.absent)
    }
}

pub struct PipeChannel {
    socket: tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    next_sequence: u64,
}

impl PipeChannel {
    pub async fn write(&mut self, bytes: impl AsRef<[u8]>) -> Result<(), SdkError> {
        let frame = serde_json::json!({
            "kind": "stdin",
            "sequence": self.take_sequence()?,
            "content": {
                "encoding": "base64",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes.as_ref()),
            },
        });
        self.send(frame).await
    }

    pub async fn close_input(&mut self) -> Result<(), SdkError> {
        let frame = serde_json::json!({
            "kind": "close-input",
            "sequence": self.take_sequence()?,
        });
        self.send(frame).await
    }

    pub async fn signal(&mut self, signal: Signal, grace: Duration) -> Result<(), SdkError> {
        let signal = match signal {
            Signal::Interrupt => "INT",
            Signal::Terminate => "TERM",
            Signal::Kill => "KILL",
        };
        let frame = serde_json::json!({
            "kind": "signal",
            "sequence": self.take_sequence()?,
            "signal": signal,
            "grace_ms": required_duration_millis(grace)?,
        });
        self.send(frame).await
    }

    pub async fn next_frame(&mut self) -> Result<Option<PipeFrame>, SdkError> {
        while let Some(message) = self.socket.next().await {
            let message = message.map_err(|error| SdkError::Transport(error.to_string()))?;
            match message {
                Message::Text(text) => {
                    let frame: substrate_wire::PipeServerFrame = serde_json::from_str(&text)
                        .map_err(|error| SdkError::Protocol(error.to_string()))?;
                    return Ok(Some(pipe_frame(frame)?));
                }
                Message::Close(_) => return Ok(None),
                Message::Ping(bytes) => self
                    .socket
                    .send(Message::Pong(bytes))
                    .await
                    .map_err(|error| SdkError::Transport(error.to_string()))?,
                Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        Ok(None)
    }

    pub async fn close(mut self) -> Result<(), SdkError> {
        self.socket
            .close(None)
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))
    }

    async fn send(&mut self, frame: Value) -> Result<(), SdkError> {
        self.socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))
    }

    fn take_sequence(&mut self) -> Result<u64, SdkError> {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| SdkError::Protocol("pipe sequence is exhausted".to_owned()))?;
        Ok(sequence)
    }
}

fn pipe_frame(frame: substrate_wire::PipeServerFrame) -> Result<PipeFrame, SdkError> {
    match frame {
        substrate_wire::PipeServerFrame::Output {
            sequence,
            stream,
            content,
        } => Ok(PipeFrame::Output {
            sequence,
            stream: stream.into(),
            bytes: content
                .decode()
                .map_err(|error| SdkError::Protocol(error.to_string()))?,
        }),
        substrate_wire::PipeServerFrame::Truncated { sequence, stream } => {
            Ok(PipeFrame::Truncated {
                sequence,
                stream: stream.into(),
            })
        }
        substrate_wire::PipeServerFrame::Exit {
            sequence,
            state,
            exit,
        } => Ok(PipeFrame::Exit {
            sequence,
            state: state.into(),
            exit: exit.map(|exit| ExecExit {
                code: exit.code,
                signal: exit.signal.map(Into::into),
            }),
        }),
        substrate_wire::PipeServerFrame::ProtocolError {
            sequence,
            code,
            message,
        } => {
            let encoded = serde_json::to_value(code)
                .map_err(|error| SdkError::Protocol(error.to_string()))?;
            Ok(PipeFrame::ProtocolError {
                sequence,
                code: encoded
                    .as_str()
                    .unwrap_or("session.protocol-error")
                    .to_owned(),
                message,
            })
        }
    }
}

impl CommandBuilder {
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.argv.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.argv.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn allow_environment(mut self, name: BaselineEnvironment) -> Self {
        self.allowed_environment.push(name);
        self
    }

    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }

    pub fn policy(mut self, policy: ExecutionPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    pub fn lease(mut self, ttl: Duration) -> Self {
        self.lease_ttl = Some(ttl);
        self
    }

    pub fn operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub async fn start(self) -> Result<Exec, SdkError> {
        self.dispatch(false).await
    }

    pub async fn run(self) -> Result<RunOutput, SdkError> {
        let exec = self.dispatch(true).await?;
        let stdout = exec.output(substrate_wire::OutputStream::Stdout).await?;
        let stderr = exec.output(substrate_wire::OutputStream::Stderr).await?;
        Ok(RunOutput {
            exec: exec.observed,
            stdout: stdout.bytes,
            stderr: stderr.bytes,
            stdout_truncated: stdout.truncated,
            stderr_truncated: stderr.truncated,
        })
    }

    async fn dispatch(self, wait: bool) -> Result<Exec, SdkError> {
        if self.argv.first().is_none_or(String::is_empty) {
            return Err(SdkError::Builder { field: "program" });
        }
        let policy = self.policy.ok_or(SdkError::Builder { field: "policy" })?;
        let capability_snapshot = self.workspace.client.machine().capability_snapshot;
        let input = substrate_wire::ExecStartInput {
            workspace: self.workspace.observed.id.clone(),
            argv: self.argv,
            env: substrate_wire::ExecEnvironment {
                allow: self
                    .allowed_environment
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                set: self.environment,
            },
            sandbox: substrate_wire::ConfinementRequest {
                capability_snapshot,
                network: substrate_wire::NetworkMode::None,
                aperture: None,
                profile: substrate_wire::SandboxProfile::Workspace,
                required: true,
            },
            limits: policy
                .wire()
                .map_err(|error| SdkError::Protocol(error.to_owned()))?,
            wait,
            scratch: None,
            measurements: BTreeSet::new(),
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            lease_ttl_ms: duration_millis(self.lease_ttl)?,
        };
        let (_, observed): (_, substrate_wire::Exec) = self
            .workspace
            .client
            .mutation("POST", "/v1/execs", &input, self.operation_id)
            .await?;
        Ok(Exec {
            client: self.workspace.client,
            observed: observed.into(),
        })
    }
}

#[derive(Clone)]
pub struct Exec {
    client: Client,
    observed: ExecObservation,
}

impl Exec {
    pub fn observation(&self) -> &ExecObservation {
        &self.observed
    }

    pub fn id(&self) -> &str {
        &self.observed.id
    }

    pub async fn refresh(&mut self) -> Result<&ExecObservation, SdkError> {
        let fresh = self.client.get_exec(&self.observed.id).await?;
        self.observed = fresh.observed;
        Ok(&self.observed)
    }

    pub async fn wait_for(&mut self, timeout: Duration) -> Result<&ExecObservation, SdkError> {
        let deadline = Instant::now() + timeout;
        loop {
            self.refresh().await?;
            if self.observed.state.terminal() {
                return Ok(&self.observed);
            }
            if Instant::now() >= deadline {
                return Err(SdkError::Transport(
                    "deadline elapsed while waiting for the exec".to_owned(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn signal(
        &mut self,
        signal: Signal,
        grace: Duration,
    ) -> Result<&ExecObservation, SdkError> {
        let input = substrate_wire::ExecSignalInput {
            signal: signal.into(),
            grace_ms: required_duration_millis(grace)?,
        };
        let target = format!("/v1/execs/{}/signal", encode_path(&self.observed.id));
        let (_, observed): (_, substrate_wire::Exec) =
            self.client.mutation("POST", &target, &input, None).await?;
        self.observed = observed.into();
        Ok(&self.observed)
    }

    pub async fn renew_lease(&mut self, ttl: Duration) -> Result<&ExecObservation, SdkError> {
        let input = substrate_wire::LeaseRenewInput {
            ttl_ms: required_duration_millis(ttl)?,
        };
        let target = format!("/v1/execs/{}/lease/renew", encode_path(&self.observed.id));
        let (_, observed): (_, substrate_wire::Exec) =
            self.client.mutation("POST", &target, &input, None).await?;
        self.observed = observed.into();
        Ok(&self.observed)
    }

    pub async fn retire(self) -> Result<bool, SdkError> {
        let target = format!("/v1/execs/{}", encode_path(&self.observed.id));
        let (_, absent): (_, substrate_wire::ExecAbsence) = self
            .client
            .mutation("DELETE", &target, &substrate_wire::EmptyInput {}, None)
            .await?;
        Ok(absent.absent)
    }

    async fn output(&self, stream: substrate_wire::OutputStream) -> Result<Captured, SdkError> {
        let mut offset = 0_u64;
        let mut bytes = Vec::new();
        let mut truncated = false;
        loop {
            let query = serde_urlencoded::to_string(substrate_wire::ExecOutputQuery {
                stream,
                offset,
                limit_bytes: substrate_wire::MAX_IO_BYTES,
            })
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
            let target = format!(
                "/v1/execs/{}/output?{query}",
                encode_path(&self.observed.id)
            );
            let slice: substrate_wire::OutputSlice = self.client.get(&target).await?;
            bytes.extend(
                slice
                    .content
                    .decode()
                    .map_err(|error| SdkError::Protocol(error.to_string()))?,
            );
            truncated |= slice.truncated;
            offset = slice.next_offset;
            if slice.eof {
                return Ok(Captured { bytes, truncated });
            }
        }
    }
}

struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

fn duration_millis(value: Option<Duration>) -> Result<Option<u64>, SdkError> {
    value.map(required_duration_millis).transpose()
}

fn required_duration_millis(value: Duration) -> Result<u64, SdkError> {
    u64::try_from(value.as_millis())
        .map_err(|_| SdkError::Protocol("duration exceeds the wire range".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_policy_requires_every_bound() {
        let error = ExecutionPolicy::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .expect_err("missing policy fields");
        assert!(matches!(error, SdkError::Builder { field: "cpu_time" }));
    }

    #[test]
    fn path_encoding_preserves_separators_and_escapes_request_bytes() {
        assert_eq!(transport::encode_path("src/a file.rs"), "src/a%20file.rs");
    }

    #[test]
    fn command_bytes_do_not_claim_unadvertised_additions() {
        let input = substrate_wire::ExecStartInput {
            workspace: "ws_example".to_owned(),
            argv: vec!["/usr/bin/true".to_owned()],
            env: substrate_wire::ExecEnvironment {
                allow: Vec::new(),
                set: BTreeMap::new(),
            },
            sandbox: substrate_wire::ConfinementRequest {
                capability_snapshot: format!("sha256:{}", "7".repeat(64)),
                network: substrate_wire::NetworkMode::None,
                aperture: None,
                profile: substrate_wire::SandboxProfile::Workspace,
                required: true,
            },
            limits: substrate_wire::ExecLimits {
                timeout_ms: 1_000,
                output_bytes: 1_024,
                processes: 1,
                memory_bytes: 1_048_576,
                cpu_millis: 100,
            },
            wait: false,
            scratch: None,
            measurements: BTreeSet::new(),
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            lease_ttl_ms: None,
        };
        let mutation = substrate_wire::Mutation {
            op: ulid::Ulid::generate().to_string(),
            input,
            delegated_context: None,
        };
        let value = serde_json::to_value(mutation).expect("serialize advertised command");
        let object = value.as_object().expect("mutation object");
        assert!(!object.contains_key("delegated_context"));
        let input = object["input"].as_object().expect("input object");
        for later in ["scratch", "measurements", "read_only_roots", "secret_slots"] {
            assert!(!input.contains_key(later), "unexpected {later}");
        }
        assert!(
            !input["sandbox"]
                .as_object()
                .unwrap()
                .contains_key("aperture")
        );
    }
}
