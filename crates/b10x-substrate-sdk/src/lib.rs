#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![allow(
    clippy::missing_errors_doc,
    reason = "all fallible public calls return the documented SdkError variants"
)]

mod http_pool;
mod managed;
mod model;
mod transport;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
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
use zeroize::Zeroizing;

pub use managed::{ManagedDaemon, ManagedDaemonBuilder, run_daemon_child_if_requested};
pub use model::{
    BaselineEnvironment, CONTRACT, CONTRACT_SHA256, DigestedFileContents, Event, EventPage,
    ExecExit, ExecObservation, ExecState, ExecutionPolicy, ExecutionPolicyBuilder, FileContents,
    FileObservation, Lease, Machine, MetricsObservation, MetricsSample, ObservedRefusal, Operation,
    OperationState, OutputPage, OutputStream, PipeFrame, PipeSessionObservation, PipeSessionState,
    Refusal, RefusalClass, RunOutput, Signal, WorkspaceObservation, WorkspaceState,
};
pub use substrate_wire::{
    AppliedConfinement, DigestedFileSlice, DirectoryEntry, DirectoryEntryKind, DirectoryPage,
    ExecMeasurement, ExecUsage, ExecutionCapsuleInput, ExpectedFileState, FileEditInput,
    FileMutationResult, FilePatchInput, GitBaselineFile, GitChange, GitChangeSet, GitChangeSide,
    GitChangeStatus, GitChangesQuery, GitSource, GitSourceEnvelope, LinePatchEdit,
    MAX_EVENT_PAGE_ITEMS, MAX_FILE_BYTES, MAX_IO_BYTES, MAX_LIST_ITEMS, MAX_PTY_WINDOW_COLUMNS,
    MAX_PTY_WINDOW_ROWS, MAX_SNAPSHOT_PAGE_ITEMS, MetricsResourceKind, PipeSessionCapabilities,
    PtyWindow, RESOURCE_USAGE_SAMPLE_INTERVAL_MS, ReadOnlyRoot, SecretSlotRequest,
    SessionAttachmentState, SessionMode, SnapshotMetadata, SnapshotPage, StorageLimit,
    TextMatchPolicy, UnifiedDiff, WorkspaceAccess, WorkspaceTree, WorkspaceTreeEntry,
};
pub use transport::{EventStream, MetricsStream, RemoteEndpoint};

use transport::{Transport, decode_result, encode_path};

/// Largest input budget accepted by one session.
pub const MAX_SESSION_INPUT_BYTES: u64 = 16 * 1024 * 1024;
/// Largest individual client frame accepted by one session.
pub const MAX_SESSION_FRAME_BYTES: u64 = 64 * 1024;
/// Largest declared live-output queue accepted by one session.
pub const MAX_SESSION_QUEUED_FRAMES: u32 = 16;
/// Largest process count accepted by an execution policy.
pub const MAX_EXEC_PROCESSES: u32 = 4_096;
/// Smallest memory ceiling accepted by an execution policy.
pub const MIN_EXEC_MEMORY_BYTES: u64 = 1024 * 1024;
/// Largest memory ceiling accepted by an execution policy.
pub const MAX_EXEC_MEMORY_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
/// Largest wall-clock or cumulative CPU duration accepted by an execution policy.
pub const MAX_EXEC_DURATION: Duration = Duration::from_hours(24);

#[derive(Serialize)]
struct SnapshotPageQuery<'a> {
    cursor: Option<&'a str>,
    limit: u32,
}

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
    #[error("the remote access-token provider is unavailable")]
    TokenUnavailable,
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

/// Why a remote request needs a hosted Identity access credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessTokenReason {
    /// A credential for a new HTTP or WebSocket request.
    Request,
    /// One replacement credential after a named hosted-authentication refusal.
    RefreshAfterAuthorizationFailure,
}

/// One opaque hosted Identity access credential.
///
/// Its allocation is zeroed on drop and neither `Debug` nor `Display` exposes its bytes.
pub struct AccessToken(Zeroizing<String>);

impl AccessToken {
    pub fn new(value: impl Into<String>) -> Result<Self, SdkError> {
        let value = value.into();
        let valid = value
            .strip_prefix("identity_access_v1_")
            .is_some_and(|token| {
                token.len() == 43
                    && token
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            });
        if !valid {
            return Err(SdkError::TokenUnavailable);
        }
        Ok(Self(Zeroizing::new(value)))
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

/// Boxed future returned by an [`AccessTokenProvider`].
pub type AccessTokenFuture<'a> =
    Pin<Box<dyn Future<Output = Result<AccessToken, SdkError>> + Send + 'a>>;

/// Supplies short-lived hosted Identity authority without giving the SDK credential storage.
pub trait AccessTokenProvider: Send + Sync {
    fn access_token(&self, reason: AccessTokenReason) -> AccessTokenFuture<'_>;
}

impl<F, Fut> AccessTokenProvider for F
where
    F: Fn(AccessTokenReason) -> Fut + Send + Sync,
    Fut: Future<Output = Result<AccessToken, SdkError>> + Send + 'static,
{
    fn access_token(&self, reason: AccessTokenReason) -> AccessTokenFuture<'_> {
        Box::pin(self(reason))
    }
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
            source: substrate_wire::WorkspaceSource::Empty(substrate_wire::EmptySource::Empty),
            labels: BTreeMap::new(),
            storage: None,
            lease_ttl: None,
            operation_id: None,
            source_authorization: None,
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
        let path = format!("/v1/sessions/{}", encode_path(id.as_ref()));
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

    pub async fn session_capabilities(&self) -> Result<PipeSessionCapabilities, SdkError> {
        self.get("/v1/sessions").await
    }

    pub async fn metrics(
        &self,
        resource_kind: MetricsResourceKind,
        resource_id: impl AsRef<str>,
    ) -> Result<MetricsObservation, SdkError> {
        let query = serde_urlencoded::to_string(substrate_wire::MetricsQuery {
            resource_kind,
            resource_id: resource_id.as_ref().to_owned(),
        })
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let observed: substrate_wire::MetricsObservation =
            self.get(&format!("/v1/metrics?{query}")).await?;
        Ok(observed.into())
    }

    pub async fn metrics_stream(
        &self,
        exec_id: impl AsRef<str>,
    ) -> Result<MetricsStream, SdkError> {
        self.inner.transport.metrics_stream(exec_id.as_ref()).await
    }

    pub async fn create_reconciliation_snapshot(&self) -> Result<SnapshotMetadata, SdkError> {
        let body = serde_json::to_vec(&substrate_wire::EmptyInput {})
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let response = self
            .inner
            .transport
            .request("POST", "/v1/reconciliation-snapshots", Some(&body))
            .await?;
        decode_result(&response)
    }

    pub async fn reconciliation_snapshot_page(
        &self,
        snapshot_id: impl AsRef<str>,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<SnapshotPage, SdkError> {
        if limit == 0 || limit > MAX_SNAPSHOT_PAGE_ITEMS {
            return Err(SdkError::Protocol(
                "snapshot limit is outside 1..=1000".to_owned(),
            ));
        }
        let query = serde_urlencoded::to_string(SnapshotPageQuery { cursor, limit })
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        self.get(&format!(
            "/v1/reconciliation-snapshots/{}?{query}",
            encode_path(snapshot_id.as_ref())
        ))
        .await
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

    async fn mutation_with_source_authority<I: Serialize, O: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        input: &I,
        operation_id: Option<String>,
        source_authority: &str,
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
        let response = self
            .inner
            .transport
            .request_with_source_authority(method, path, Some(&body), source_authority)
            .await?;
        decode_result(&response).map(|result| (operation_id, result))
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

impl RemoteEndpoint {
    /// Bind an actor's current token provider to shared, credential-free transport resources.
    /// The machine and contract are checked for this caller; no authority is cached.
    pub async fn connect(
        &self,
        provider: impl AccessTokenProvider + 'static,
    ) -> Result<Client, SdkError> {
        let transport = self.bind(Arc::new(provider));
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
    https_endpoint: Option<String>,
    trust_roots: Option<PathBuf>,
    server_identity: Option<String>,
    token_provider: Option<Arc<dyn AccessTokenProvider>>,
}

impl ClientBuilder {
    pub fn unix_socket(mut self, socket: impl Into<PathBuf>) -> Self {
        self.socket = Some(socket.into());
        self
    }

    /// Select a production remote endpoint. The value must be one exact HTTPS origin.
    pub fn https_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.https_endpoint = Some(endpoint.into());
        self
    }

    /// Supply the bounded PEM roots used only for this Substrate endpoint.
    pub fn trust_roots(mut self, path: impl Into<PathBuf>) -> Self {
        self.trust_roots = Some(path.into());
        self
    }

    /// Set the DNS identity rustls must verify in the endpoint certificate.
    pub fn server_identity(mut self, identity: impl Into<String>) -> Self {
        self.server_identity = Some(identity.into());
        self
    }

    /// Supply short-lived hosted Identity credentials asynchronously, once per request.
    pub fn token_provider(mut self, provider: impl AccessTokenProvider + 'static) -> Self {
        self.token_provider = Some(Arc::new(provider));
        self
    }

    pub async fn connect(self) -> Result<Client, SdkError> {
        let remote_field_is_set = self.https_endpoint.is_some()
            || self.trust_roots.is_some()
            || self.server_identity.is_some()
            || self.token_provider.is_some();
        let transport = match (self.socket, remote_field_is_set) {
            (Some(socket), false) => Transport::new(socket),
            (None, true) => {
                let endpoint = self.https_endpoint.ok_or(SdkError::Builder {
                    field: "https_endpoint",
                })?;
                let trust_roots = self.trust_roots.ok_or(SdkError::Builder {
                    field: "trust_roots",
                })?;
                Transport::remote(
                    &endpoint,
                    &trust_roots,
                    self.server_identity.ok_or(SdkError::Builder {
                        field: "server_identity",
                    })?,
                    self.token_provider.ok_or(SdkError::Builder {
                        field: "token_provider",
                    })?,
                )?
            }
            (Some(_), true) => {
                return Err(SdkError::Protocol(
                    "unix_socket and https_endpoint are mutually exclusive".to_owned(),
                ));
            }
            (None, false) => return Err(SdkError::Builder { field: "socket" }),
        };
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
    source: substrate_wire::WorkspaceSource,
    labels: BTreeMap<String, String>,
    storage: Option<StorageLimit>,
    lease_ttl: Option<Duration>,
    operation_id: Option<String>,
    source_authorization: Option<Zeroizing<String>>,
}

impl WorkspaceBuilder {
    pub fn empty(self) -> Self {
        self
    }

    pub fn git(
        mut self,
        source: impl Into<String>,
        locator: impl Into<String>,
        reference: impl Into<String>,
        commit: impl Into<String>,
        depth: u16,
        source_authorization: impl Into<String>,
    ) -> Self {
        self.source = substrate_wire::WorkspaceSource::Git(GitSourceEnvelope {
            git: GitSource {
                source: source.into(),
                locator: locator.into(),
                reference: reference.into(),
                commit: commit.into(),
                depth,
            },
        });
        self.source_authorization = Some(Zeroizing::new(source_authorization.into()));
        self
    }

    pub fn label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    pub fn storage(mut self, limit: StorageLimit) -> Self {
        self.storage = Some(limit);
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
        if self
            .storage
            .is_some_and(|limit| !limit.within_contract_bounds())
        {
            return Err(SdkError::Protocol(
                "workspace storage quota is outside the contract bound".to_owned(),
            ));
        }
        let input = substrate_wire::WorkspaceCreateInput {
            source: self.source,
            labels: self.labels,
            storage: self.storage,
            lease_ttl_ms: duration_millis(self.lease_ttl)?,
        };
        let (_, observed): (_, substrate_wire::Workspace) = match self.source_authorization {
            Some(authority) => {
                self.client
                    .mutation_with_source_authority(
                        "POST",
                        "/v1/workspaces",
                        &input,
                        self.operation_id,
                        authority.as_str(),
                    )
                    .await?
            }
            None => {
                self.client
                    .mutation("POST", "/v1/workspaces", &input, self.operation_id)
                    .await?
            }
        };
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
            workspace_access: WorkspaceAccess::default(),
            network: substrate_wire::NetworkMode::None,
            aperture: None,
            scratch: None,
            measurements: BTreeSet::new(),
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
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
            workspace_access: WorkspaceAccess::default(),
            network: substrate_wire::NetworkMode::None,
            aperture: None,
            scratch: None,
            measurements: BTreeSet::new(),
            read_only_roots: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            policy: None,
            lease_ttl: None,
            input_limit_bytes: None,
            frame_limit_bytes: None,
            queued_frames: None,
            mode: SessionMode::Pipes,
            window: None,
            operation_id: None,
        }
    }

    pub fn pty_session(&self, program: impl Into<String>, window: PtyWindow) -> PipeSessionBuilder {
        let mut builder = self.pipe_session(program);
        builder.mode = SessionMode::Pty;
        builder.window = Some(window);
        builder
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

    pub async fn read_directory(
        &self,
        path: impl AsRef<str>,
        cursor: Option<&str>,
        limit_items: u32,
    ) -> Result<DirectoryPage, SdkError> {
        if limit_items == 0 || limit_items > MAX_LIST_ITEMS {
            return Err(SdkError::Protocol(
                "directory limit is outside its contract bound".to_owned(),
            ));
        }
        substrate_wire::validate_relative_path(path.as_ref())
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let query = serde_urlencoded::to_string(substrate_wire::FileReadQuery {
            mode: substrate_wire::FileMode::Directory,
            offset: None,
            limit_bytes: None,
            cursor: cursor.map(ToOwned::to_owned),
            limit_items: Some(limit_items),
        })
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let target = format!(
            "/v1/workspaces/{}/files/{}?{query}",
            encode_path(&self.observed.id),
            encode_path(path.as_ref())
        );
        let result: substrate_wire::FileReadResult = self.client.get(&target).await?;
        match result {
            substrate_wire::FileReadResult::Directory(page) => Ok(page),
            substrate_wire::FileReadResult::File(_) => Err(SdkError::Protocol(
                "daemon returned a file for a directory read".to_owned(),
            )),
        }
    }

    pub async fn write_file(
        &self,
        path: impl AsRef<str>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<FileObservation, SdkError> {
        self.write_file_with_operation_id(path, bytes, None::<String>)
            .await
    }

    pub async fn write_file_with_operation_id(
        &self,
        path: impl AsRef<str>,
        bytes: impl AsRef<[u8]>,
        operation_id: impl Into<Option<String>>,
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
            content: base64_content(bytes.as_ref()),
        };
        let target = format!(
            "/v1/workspaces/{}/files/{}",
            encode_path(&self.observed.id),
            encode_path(path.as_ref())
        );
        let (_, observed): (_, substrate_wire::FileObservation) = self
            .client
            .mutation("PUT", &target, &input, operation_id.into())
            .await?;
        Ok(observed.into())
    }

    pub async fn delete_file(&self, path: impl AsRef<str>) -> Result<bool, SdkError> {
        self.delete_file_with_operation_id(path, None::<String>)
            .await
    }

    pub async fn delete_file_with_operation_id(
        &self,
        path: impl AsRef<str>,
        operation_id: impl Into<Option<String>>,
    ) -> Result<bool, SdkError> {
        substrate_wire::validate_relative_path(path.as_ref())
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let target = format!(
            "/v1/workspaces/{}/files/{}",
            encode_path(&self.observed.id),
            encode_path(path.as_ref())
        );
        let (_, absent): (_, substrate_wire::FileAbsence) = self
            .client
            .mutation(
                "DELETE",
                &target,
                &substrate_wire::EmptyInput {},
                operation_id.into(),
            )
            .await?;
        Ok(absent.absent)
    }

    pub async fn renew_lease(&mut self, ttl: Duration) -> Result<&WorkspaceObservation, SdkError> {
        self.renew_lease_with_operation_id(ttl, None::<String>)
            .await
    }

    pub async fn renew_lease_with_operation_id(
        &mut self,
        ttl: Duration,
        operation_id: impl Into<Option<String>>,
    ) -> Result<&WorkspaceObservation, SdkError> {
        let input = substrate_wire::LeaseRenewInput {
            ttl_ms: required_duration_millis(ttl)?,
        };
        let target = format!(
            "/v1/workspaces/{}/lease/renew",
            encode_path(&self.observed.id)
        );
        let (_, observed): (_, substrate_wire::Workspace) = self
            .client
            .mutation("POST", &target, &input, operation_id.into())
            .await?;
        self.observed = observed.into();
        Ok(&self.observed)
    }

    pub async fn destroy(self) -> Result<bool, SdkError> {
        self.destroy_with_operation_id(None::<String>).await
    }

    pub async fn destroy_with_operation_id(
        self,
        operation_id: impl Into<Option<String>>,
    ) -> Result<bool, SdkError> {
        let target = format!("/v1/workspaces/{}", encode_path(&self.observed.id));
        let (_, absent): (_, substrate_wire::WorkspaceAbsence) = self
            .client
            .mutation(
                "DELETE",
                &target,
                &substrate_wire::EmptyInput {},
                operation_id.into(),
            )
            .await?;
        Ok(absent.absent)
    }

    pub async fn read_file_v2(
        &self,
        path: impl AsRef<str>,
        offset: u64,
        limit_bytes: u64,
    ) -> Result<DigestedFileContents, SdkError> {
        validate_file_read(path.as_ref(), limit_bytes)?;
        let query = serde_urlencoded::to_string(substrate_wire::FileReadQuery {
            mode: substrate_wire::FileMode::File,
            offset: Some(offset),
            limit_bytes: Some(limit_bytes),
            cursor: None,
            limit_items: None,
        })
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let target = format!(
            "/v2/workspaces/{}/files/{}?{query}",
            encode_path(&self.observed.id),
            encode_path(path.as_ref())
        );
        let slice: substrate_wire::DigestedFileSlice = self.client.get(&target).await?;
        slice.try_into()
    }

    pub async fn tree(
        &self,
        limit_items: u32,
        include_hidden: bool,
    ) -> Result<WorkspaceTree, SdkError> {
        let query = substrate_wire::WorkspaceTreeQuery {
            limit_items,
            include_hidden,
        };
        query
            .validate()
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let query = serde_urlencoded::to_string(query)
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        self.client
            .get(&format!(
                "/v2/workspaces/{}/tree?{query}",
                encode_path(&self.observed.id)
            ))
            .await
    }

    /// Read one complete file from the immutable commit installed for this Git workspace.
    ///
    /// `Ok(None)` means the path is absent in that commit. It says nothing about the current
    /// working tree, which remains available through [`Self::read_file_v2`].
    pub async fn read_git_file(
        &self,
        path: impl AsRef<str>,
        max_bytes: u64,
    ) -> Result<Option<GitBaselineFile>, SdkError> {
        validate_file_read(path.as_ref(), max_bytes)?;
        let query = serde_urlencoded::to_string(substrate_wire::GitBaselineFileQuery { max_bytes })
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let result: substrate_wire::GitBaselineFileResult = self
            .client
            .get(&format!(
                "/v2/workspaces/{}/git/baseline/{}?{query}",
                encode_path(&self.observed.id),
                encode_path(path.as_ref())
            ))
            .await?;
        Ok(result.file)
    }

    /// Compare the current index/worktree with the exact materialization commit.
    pub async fn git_changes(
        &self,
        max_files: u32,
        max_file_bytes: u64,
        max_total_bytes: u64,
    ) -> Result<GitChangeSet, SdkError> {
        let query = GitChangesQuery {
            max_files,
            max_file_bytes,
            max_total_bytes,
        };
        query
            .validate()
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let query = serde_urlencoded::to_string(query)
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        self.client
            .get(&format!(
                "/v2/workspaces/{}/git/changes?{query}",
                encode_path(&self.observed.id)
            ))
            .await
    }

    pub async fn replace_file(
        &self,
        path: impl AsRef<str>,
        bytes: impl AsRef<[u8]>,
        expected: ExpectedFileState,
        create_parents: bool,
        operation_id: impl Into<Option<String>>,
    ) -> Result<FileMutationResult, SdkError> {
        validate_file_mutation(path.as_ref(), bytes.as_ref())?;
        expected
            .validate()
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let input = substrate_wire::FileReplaceInput {
            content: base64_content(bytes.as_ref()),
            expected,
            create_parents,
        };
        self.file_mutation("PUT", "files", path.as_ref(), &input, operation_id.into())
            .await
    }

    pub async fn edit_file(
        &self,
        path: impl AsRef<str>,
        input: FileEditInput,
        operation_id: impl Into<Option<String>>,
    ) -> Result<FileMutationResult, SdkError> {
        substrate_wire::validate_relative_path(path.as_ref())
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        self.file_mutation(
            "POST",
            "file-edits",
            path.as_ref(),
            &input,
            operation_id.into(),
        )
        .await
    }

    pub async fn patch_file(
        &self,
        path: impl AsRef<str>,
        input: FilePatchInput,
        operation_id: impl Into<Option<String>>,
    ) -> Result<FileMutationResult, SdkError> {
        substrate_wire::validate_relative_path(path.as_ref())
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        self.file_mutation(
            "POST",
            "file-patches",
            path.as_ref(),
            &input,
            operation_id.into(),
        )
        .await
    }

    async fn file_mutation<I: Serialize>(
        &self,
        method: &str,
        route: &str,
        path: &str,
        input: &I,
        operation_id: Option<String>,
    ) -> Result<FileMutationResult, SdkError> {
        let target = format!(
            "/v2/workspaces/{}/{}/{}",
            encode_path(&self.observed.id),
            route,
            encode_path(path)
        );
        let (_, result) = self
            .client
            .mutation(method, &target, input, operation_id)
            .await?;
        Ok(result)
    }
}

#[must_use]
pub struct CommandBuilder {
    workspace: Workspace,
    argv: Vec<String>,
    allowed_environment: Vec<BaselineEnvironment>,
    environment: BTreeMap<String, String>,
    workspace_access: WorkspaceAccess,
    network: substrate_wire::NetworkMode,
    aperture: Option<String>,
    scratch: Option<StorageLimit>,
    measurements: BTreeSet<ExecMeasurement>,
    read_only_roots: Vec<ReadOnlyRoot>,
    secret_slots: Vec<SecretSlotRequest>,
    capsule: Option<ExecutionCapsuleInput>,
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
    workspace_access: WorkspaceAccess,
    network: substrate_wire::NetworkMode,
    aperture: Option<String>,
    scratch: Option<StorageLimit>,
    measurements: BTreeSet<ExecMeasurement>,
    read_only_roots: Vec<ReadOnlyRoot>,
    secret_slots: Vec<SecretSlotRequest>,
    capsule: Option<ExecutionCapsuleInput>,
    policy: Option<ExecutionPolicy>,
    lease_ttl: Option<Duration>,
    input_limit_bytes: Option<u64>,
    frame_limit_bytes: Option<u64>,
    queued_frames: Option<u32>,
    mode: SessionMode,
    window: Option<PtyWindow>,
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

    pub fn workspace_access(mut self, access: WorkspaceAccess) -> Self {
        self.workspace_access = access;
        self
    }

    pub fn aperture(mut self, name: impl Into<String>) -> Self {
        self.network = substrate_wire::NetworkMode::Aperture;
        self.aperture = Some(name.into());
        self
    }

    pub fn scratch(mut self, limit: StorageLimit) -> Self {
        self.scratch = Some(limit);
        self
    }

    pub fn measure(mut self, measurement: ExecMeasurement) -> Self {
        self.measurements.insert(measurement);
        self
    }

    pub fn read_only_root(mut self, root: ReadOnlyRoot) -> Self {
        self.read_only_roots.push(root);
        self
    }

    pub fn secret_slot(mut self, slot: SecretSlotRequest) -> Self {
        self.secret_slots.push(slot);
        self
    }

    pub fn capsule(mut self, capsule: ExecutionCapsuleInput) -> Self {
        self.capsule = Some(capsule);
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
        if self.argv.first().is_none_or(String::is_empty) {
            return Err(SdkError::Builder { field: "program" });
        }
        substrate_wire::validate_session_window(self.mode, self.window.as_ref())
            .map_err(|error| SdkError::Protocol(error.to_string()))?;
        validate_execution_storage(&self.workspace_access, self.scratch)?;
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
        if input_limit_bytes == 0 || input_limit_bytes > MAX_SESSION_INPUT_BYTES {
            return Err(SdkError::Protocol(
                "session input limit is outside its contract bound".to_owned(),
            ));
        }
        if frame_limit_bytes == 0 || frame_limit_bytes > MAX_SESSION_FRAME_BYTES {
            return Err(SdkError::Protocol(
                "session frame limit is outside its contract bound".to_owned(),
            ));
        }
        if queued_frames == 0 || queued_frames > MAX_SESSION_QUEUED_FRAMES {
            return Err(SdkError::Protocol(
                "session queue limit is outside its contract bound".to_owned(),
            ));
        }
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
                network: self.network,
                aperture: self.aperture,
                profile: substrate_wire::SandboxProfile::Workspace,
                required: true,
            },
            limits: policy
                .wire()
                .map_err(|error| SdkError::Protocol(error.to_owned()))?,
            wait: false,
            workspace_access: self.workspace_access,
            scratch: self.scratch,
            measurements: self.measurements,
            read_only_roots: self.read_only_roots,
            secret_slots: self.secret_slots,
            capsule: self.capsule,
            lease_ttl_ms: Some(required_duration_millis(lease_ttl)?),
        };
        let input = substrate_wire::PipeSessionStartInput {
            exec,
            input_limit_bytes,
            frame_limit_bytes,
            queued_frames,
            mode: self.mode,
            window: self.window,
        };
        let (_, observed): (_, substrate_wire::PipeSession) = self
            .workspace
            .client
            .mutation("POST", "/v1/sessions", &input, self.operation_id)
            .await?;
        Ok(PipeSession {
            client: self.workspace.client,
            observed: observed.into(),
        })
    }
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

    /// The execution observation behind this session, including applied confinement and usage.
    pub async fn exec_observation(&self) -> Result<ExecObservation, SdkError> {
        self.client
            .get_exec(&self.observed.exec_id)
            .await
            .map(|exec| exec.observed)
    }

    pub async fn attach(&self) -> Result<PipeChannel, SdkError> {
        let socket = self
            .client
            .inner
            .transport
            .session_websocket(&self.observed.id)
            .await?;
        Ok(PipeChannel {
            socket,
            next_sequence: 1,
            mode: self.observed.mode,
        })
    }

    pub async fn signal(
        &mut self,
        signal: Signal,
        grace: Duration,
    ) -> Result<&PipeSessionObservation, SdkError> {
        self.signal_with_operation_id(signal, grace, None::<String>)
            .await
    }

    pub async fn signal_with_operation_id(
        &mut self,
        signal: Signal,
        grace: Duration,
        operation_id: impl Into<Option<String>>,
    ) -> Result<&PipeSessionObservation, SdkError> {
        let input = substrate_wire::ExecSignalInput {
            signal: signal.into(),
            grace_ms: required_duration_millis(grace)?,
        };
        let target = format!("/v1/sessions/{}/signal", encode_path(&self.observed.id));
        let (_, observed): (_, substrate_wire::PipeSession) = self
            .client
            .mutation("POST", &target, &input, operation_id.into())
            .await?;
        self.observed = observed.into();
        Ok(&self.observed)
    }

    pub async fn renew_lease(
        &mut self,
        ttl: Duration,
    ) -> Result<&PipeSessionObservation, SdkError> {
        self.renew_lease_with_operation_id(ttl, None::<String>)
            .await
    }

    pub async fn renew_lease_with_operation_id(
        &mut self,
        ttl: Duration,
        operation_id: impl Into<Option<String>>,
    ) -> Result<&PipeSessionObservation, SdkError> {
        let input = substrate_wire::LeaseRenewInput {
            ttl_ms: required_duration_millis(ttl)?,
        };
        let target = format!(
            "/v1/sessions/{}/lease/renew",
            encode_path(&self.observed.id)
        );
        let (_, observed): (_, substrate_wire::PipeSession) = self
            .client
            .mutation("POST", &target, &input, operation_id.into())
            .await?;
        self.observed = observed.into();
        Ok(&self.observed)
    }

    pub async fn retire(self) -> Result<bool, SdkError> {
        self.retire_with_operation_id(None::<String>).await
    }

    pub async fn retire_with_operation_id(
        self,
        operation_id: impl Into<Option<String>>,
    ) -> Result<bool, SdkError> {
        let target = format!("/v1/sessions/{}", encode_path(&self.observed.id));
        let (_, absent): (_, substrate_wire::SessionAbsence) = self
            .client
            .mutation(
                "DELETE",
                &target,
                &substrate_wire::EmptyInput {},
                operation_id.into(),
            )
            .await?;
        Ok(absent.absent)
    }
}

pub struct PipeChannel {
    socket: transport::WebSocket,
    next_sequence: u64,
    mode: SessionMode,
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
        if self.mode == SessionMode::Pty {
            return Err(SdkError::Protocol(
                "a pty has no close-input frame; send the terminal EOF byte".to_owned(),
            ));
        }
        let frame = serde_json::json!({
            "kind": "close-input",
            "sequence": self.take_sequence()?,
        });
        self.send(frame).await
    }

    pub async fn resize(&mut self, window: PtyWindow) -> Result<(), SdkError> {
        if self.mode != SessionMode::Pty {
            return Err(SdkError::Protocol(
                "resize belongs to a pty session".to_owned(),
            ));
        }
        if !window.within_bounds() {
            return Err(SdkError::Protocol(
                "terminal window is outside the contract bound".to_owned(),
            ));
        }
        let frame = serde_json::json!({
            "kind": "resize",
            "sequence": self.take_sequence()?,
            "window": window,
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

    pub fn workspace_access(mut self, access: WorkspaceAccess) -> Self {
        self.workspace_access = access;
        self
    }

    pub fn aperture(mut self, name: impl Into<String>) -> Self {
        self.network = substrate_wire::NetworkMode::Aperture;
        self.aperture = Some(name.into());
        self
    }

    pub fn scratch(mut self, limit: StorageLimit) -> Self {
        self.scratch = Some(limit);
        self
    }

    pub fn measure(mut self, measurement: ExecMeasurement) -> Self {
        self.measurements.insert(measurement);
        self
    }

    pub fn read_only_root(mut self, root: ReadOnlyRoot) -> Self {
        self.read_only_roots.push(root);
        self
    }

    pub fn secret_slot(mut self, slot: SecretSlotRequest) -> Self {
        self.secret_slots.push(slot);
        self
    }

    pub fn capsule(mut self, capsule: ExecutionCapsuleInput) -> Self {
        self.capsule = Some(capsule);
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
        validate_execution_storage(&self.workspace_access, self.scratch)?;
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
                network: self.network,
                aperture: self.aperture,
                profile: substrate_wire::SandboxProfile::Workspace,
                required: true,
            },
            limits: policy
                .wire()
                .map_err(|error| SdkError::Protocol(error.to_owned()))?,
            wait,
            workspace_access: self.workspace_access,
            scratch: self.scratch,
            measurements: self.measurements,
            read_only_roots: self.read_only_roots,
            secret_slots: self.secret_slots,
            capsule: self.capsule,
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
        self.signal_with_operation_id(signal, grace, None::<String>)
            .await
    }

    pub async fn signal_with_operation_id(
        &mut self,
        signal: Signal,
        grace: Duration,
        operation_id: impl Into<Option<String>>,
    ) -> Result<&ExecObservation, SdkError> {
        let input = substrate_wire::ExecSignalInput {
            signal: signal.into(),
            grace_ms: required_duration_millis(grace)?,
        };
        let target = format!("/v1/execs/{}/signal", encode_path(&self.observed.id));
        let (_, observed): (_, substrate_wire::Exec) = self
            .client
            .mutation("POST", &target, &input, operation_id.into())
            .await?;
        self.observed = observed.into();
        Ok(&self.observed)
    }

    pub async fn renew_lease(&mut self, ttl: Duration) -> Result<&ExecObservation, SdkError> {
        self.renew_lease_with_operation_id(ttl, None::<String>)
            .await
    }

    pub async fn renew_lease_with_operation_id(
        &mut self,
        ttl: Duration,
        operation_id: impl Into<Option<String>>,
    ) -> Result<&ExecObservation, SdkError> {
        let input = substrate_wire::LeaseRenewInput {
            ttl_ms: required_duration_millis(ttl)?,
        };
        let target = format!("/v1/execs/{}/lease/renew", encode_path(&self.observed.id));
        let (_, observed): (_, substrate_wire::Exec) = self
            .client
            .mutation("POST", &target, &input, operation_id.into())
            .await?;
        self.observed = observed.into();
        Ok(&self.observed)
    }

    pub async fn retire(self) -> Result<bool, SdkError> {
        self.retire_with_operation_id(None::<String>).await
    }

    pub async fn retire_with_operation_id(
        self,
        operation_id: impl Into<Option<String>>,
    ) -> Result<bool, SdkError> {
        let target = format!("/v1/execs/{}", encode_path(&self.observed.id));
        let (_, absent): (_, substrate_wire::ExecAbsence) = self
            .client
            .mutation(
                "DELETE",
                &target,
                &substrate_wire::EmptyInput {},
                operation_id.into(),
            )
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

    pub async fn output_page(
        &self,
        stream: OutputStream,
        offset: u64,
        limit_bytes: u64,
    ) -> Result<OutputPage, SdkError> {
        if limit_bytes == 0 || limit_bytes > MAX_IO_BYTES {
            return Err(SdkError::Protocol(
                "output limit is outside its contract bound".to_owned(),
            ));
        }
        let query = serde_urlencoded::to_string(substrate_wire::ExecOutputQuery {
            stream: stream.into(),
            offset,
            limit_bytes,
        })
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
        let target = format!(
            "/v1/execs/{}/output?{query}",
            encode_path(&self.observed.id)
        );
        let slice: substrate_wire::OutputSlice = self.client.get(&target).await?;
        Ok(OutputPage {
            exec: slice.exec,
            stream: slice.stream.into(),
            offset: slice.offset,
            next_offset: slice.next_offset,
            bytes: slice
                .content
                .decode()
                .map_err(|error| SdkError::Protocol(error.to_string()))?,
            eof: slice.eof,
            truncated: slice.truncated,
            observed_at: slice.observed_at,
        })
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

fn base64_content(bytes: &[u8]) -> substrate_wire::Base64Content {
    substrate_wire::Base64Content {
        encoding: substrate_wire::Base64Encoding::Base64,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

fn validate_file_read(path: &str, limit_bytes: u64) -> Result<(), SdkError> {
    substrate_wire::validate_relative_path(path)
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
    if limit_bytes == 0 || limit_bytes > MAX_IO_BYTES {
        return Err(SdkError::Protocol(
            "file read limit is outside its contract bound".to_owned(),
        ));
    }
    Ok(())
}

fn validate_file_mutation(path: &str, bytes: &[u8]) -> Result<(), SdkError> {
    substrate_wire::validate_relative_path(path)
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
    let maximum = usize::try_from(MAX_FILE_BYTES).unwrap_or(usize::MAX);
    if bytes.len() > maximum {
        return Err(SdkError::Protocol(
            "file exceeds the write bound".to_owned(),
        ));
    }
    Ok(())
}

fn validate_execution_storage(
    access: &WorkspaceAccess,
    scratch: Option<StorageLimit>,
) -> Result<(), SdkError> {
    substrate_wire::validate_workspace_access(access)
        .map_err(|error| SdkError::Protocol(error.to_string()))?;
    if scratch.is_some_and(|limit| !limit.within_contract_bounds()) {
        return Err(SdkError::Protocol(
            "scratch storage quota is outside the contract bound".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{UnixListener, UnixStream};

    async fn read_request(stream: &mut UnixStream) -> Vec<u8> {
        let mut bytes = Vec::new();
        let boundary = loop {
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
            let mut block = [0_u8; 1024];
            let count = stream.read(&mut block).await.expect("read SDK request");
            assert!(count > 0, "SDK closed before its request head");
            bytes.extend_from_slice(&block[..count]);
        };
        let head = std::str::from_utf8(&bytes[..boundary]).expect("ASCII request head");
        let content_length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
            .unwrap_or(0);
        while bytes.len() < boundary + content_length {
            let mut block = [0_u8; 1024];
            let count = stream.read(&mut block).await.expect("read SDK body");
            assert!(count > 0, "SDK closed during its request body");
            bytes.extend_from_slice(&block[..count]);
        }
        bytes
    }

    fn response(status: &str, body: &[u8]) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nx-b10x-contract: {}\r\nx-b10x-contract-bundle-sha256: {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            CONTRACT,
            CONTRACT_SHA256,
            body.len()
        )
        .into_bytes()
        .into_iter()
        .chain(body.iter().copied())
        .collect()
    }

    fn request_body(request: &[u8]) -> &[u8] {
        let boundary = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("request boundary")
            + 4;
        &request[boundary..]
    }

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
            workspace_access: substrate_wire::WorkspaceAccess::ReadWrite,
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

    #[tokio::test]
    async fn a_lost_mutation_response_reconciles_and_replays_the_same_operation() {
        const OPERATION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let temporary = tempfile::tempdir().expect("temporary socket directory");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let vector: Value = serde_json::from_str(include_str!(
            "../../../contracts/substrate-wire/0.16.0/vectors/http/machine-probe.json"
        ))
        .expect("machine vector");
        let machine = serde_json::to_vec(
            vector
                .pointer("/expected/response/body")
                .expect("machine response body"),
        )
        .expect("machine response JSON");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept machine request");
            let request = read_request(&mut stream).await;
            assert!(request.starts_with(b"GET /v1/machine HTTP/1.1\r\n"));
            stream
                .write_all(&response("200 OK", &machine))
                .await
                .expect("write machine response");

            let (mut stream, _) = listener.accept().await.expect("accept mutation");
            let first = read_request(&mut stream).await;
            assert!(first.starts_with(b"POST /v1/workspaces HTTP/1.1\r\n"));
            assert_eq!(
                serde_json::from_slice::<Value>(request_body(&first)).expect("mutation JSON")["op"],
                OPERATION
            );
            drop(stream);

            let (mut stream, _) = listener.accept().await.expect("accept reconciliation");
            let request = read_request(&mut stream).await;
            assert!(
                request.starts_with(format!("GET /v1/ops/{OPERATION} HTTP/1.1\r\n").as_bytes())
            );
            let missing = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_recovery",
                "error": {
                    "class": "refused",
                    "code": "resource.not-found",
                    "message": "The operation does not exist.",
                    "retriable": false
                }
            }))
            .expect("missing operation JSON");
            stream
                .write_all(&response("404 Not Found", &missing))
                .await
                .expect("write missing operation response");

            let (mut stream, _) = listener.accept().await.expect("accept replay");
            let replay = read_request(&mut stream).await;
            assert!(replay.starts_with(b"POST /v1/workspaces HTTP/1.1\r\n"));
            assert_eq!(request_body(&replay), request_body(&first));
            let workspace = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_replay",
                "operation": OPERATION,
                "result": {
                    "id": "ws_recovered",
                    "kind": "workspace",
                    "labels": {},
                    "observed_at": "2026-09-01T00:00:00Z",
                    "state": "ready"
                }
            }))
            .expect("workspace response JSON");
            stream
                .write_all(&response("201 Created", &workspace))
                .await
                .expect("write replay response");
        });

        let client = Client::builder()
            .unix_socket(&socket)
            .connect()
            .await
            .expect("connect fake daemon");
        let workspace = client
            .workspace()
            .empty()
            .operation_id(OPERATION)
            .create()
            .await
            .expect("recover lost mutation response");
        assert_eq!(workspace.id(), "ws_recovered");
        server.await.expect("fake daemon task");
    }
}
