#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![allow(
    clippy::missing_errors_doc,
    reason = "all fallible public calls return the documented SdkError variants"
)]

mod http_pool;
mod managed;
mod model;
#[cfg(test)]
#[path = "../../../test-support/unix_socket.rs"]
mod test_unix_socket;
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
    Refusal, RefusalClass, RunOutput, Signal, WorkspaceObservation, WorkspaceOrigin,
    WorkspaceState,
};
pub use substrate_wire::{
    AppliedConfinement, AppliedWritableRoot, ConfinementFamily, ConfinementProperty,
    ConfinementRequires, DigestedFileSlice, DirectoryEntry, DirectoryEntryKind, DirectoryPage,
    ExecMeasurement, ExecUsage, ExecutionCapsuleInput, ExpectedFileState, FileEditInput,
    FileMutationResult, FilePatchInput, GitBaselineFile, GitChange, GitChangeSet, GitChangeSide,
    GitChangeStatus, GitChangesQuery, GitSource, GitSourceEnvelope, LinePatchEdit,
    MAX_EVENT_PAGE_ITEMS, MAX_FILE_BYTES, MAX_IO_BYTES, MAX_LIST_ITEMS, MAX_PTY_WINDOW_COLUMNS,
    MAX_PTY_WINDOW_ROWS, MAX_SNAPSHOT_PAGE_ITEMS, MAX_WRITABLE_ROOTS, MetricsResourceKind,
    PipeSessionCapabilities, ProcessStreamEnd, ProcessStreamGap, ProcessStreamItem,
    ProcessStreamItemKind, PtyWindow, RESOURCE_USAGE_SAMPLE_INTERVAL_MS, ReadOnlyFile,
    ReadOnlyRoot, SecretSlotRequest, SessionAttachmentState, SessionMode, SnapshotMetadata,
    SnapshotPage, StorageLimit, TextMatchPolicy, UnifiedDiff, WorkspaceAccess, WorkspaceTree,
    WorkspaceTreeEntry, WritableGitRoot, WritableHostRoot, WritableRoot,
};
pub use transport::{EventStream, MetricsStream, ProcessEventStream, RemoteEndpoint};

use transport::{IdKind, Transport, decode_result, encode_path, id_segment};

/// Largest input budget accepted by one session.
pub const MAX_SESSION_INPUT_BYTES: u64 = 16 * 1024 * 1024;
/// Largest individual client frame accepted by one session.
pub const MAX_SESSION_FRAME_BYTES: u64 = 64 * 1024;
/// Capture selection for interactive session builders.
pub use substrate_wire::CaptureMode;

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

    /// Adopt a directory the operator placed under the daemon's workspace root (ADR 0035,
    /// `workspace.adopt`). The workspace gets a minted `ws_…` id and has no lease and no storage
    /// limit; destroying it forgets the record and leaves the directory and its contents.
    ///
    /// # Errors
    ///
    /// A root name outside the published set is refused here without a request; every other
    /// refusal is the daemon's, recorded under the operation id.
    pub async fn adopt_workspace(
        &self,
        root_name: impl AsRef<str>,
        labels: BTreeMap<String, String>,
        operation_id: Option<String>,
    ) -> Result<Workspace, SdkError> {
        let root_name = root_name.as_ref();
        if !substrate_wire::is_root_name(root_name) {
            return Err(SdkError::Protocol(
                "workspace root name is outside the published root-name set".to_owned(),
            ));
        }
        let input = substrate_wire::WorkspaceAdoptInput {
            root_name: root_name.into(),
            labels,
        };
        let (_, observed): (_, substrate_wire::Workspace) = self
            .mutation("POST", "/v1/workspaces/adopt", &input, operation_id)
            .await?;
        Ok(Workspace {
            client: self.clone(),
            observed: observed.into(),
        })
    }

    pub async fn get_workspace(&self, id: impl AsRef<str>) -> Result<Workspace, SdkError> {
        let path = format!(
            "/v1/workspaces/{}",
            id_segment(IdKind::Workspace, id.as_ref())?
        );
        let observed: substrate_wire::Workspace = self.get(&path).await?;
        Ok(Workspace {
            client: self.clone(),
            observed: observed.into(),
        })
    }

    /// Destroy a workspace by id, without reading it first.
    ///
    /// A retry under the same operation id is answered with the stored outcome, also once the
    /// workspace is gone and [`Client::get_workspace`] answers `resource.not-found`.
    ///
    /// The id is one path segment, as at every call that names a resource by id: an id that is
    /// empty, `.` or `..`, or holds `/`, `?`, `#`, `%` or a control character, is refused with
    /// [`SdkError::Protocol`] before anything is sent, so this call addresses a workspace and
    /// never a route beneath one.
    pub async fn destroy_workspace(
        &self,
        id: impl AsRef<str>,
        operation_id: Option<String>,
    ) -> Result<bool, SdkError> {
        let target = format!(
            "/v1/workspaces/{}",
            id_segment(IdKind::Workspace, id.as_ref())?
        );
        let (_, absent): (_, substrate_wire::WorkspaceAbsence) = self
            .mutation(
                "DELETE",
                &target,
                &substrate_wire::EmptyInput {},
                operation_id,
            )
            .await?;
        Ok(absent.absent)
    }

    pub async fn get_exec(&self, id: impl AsRef<str>) -> Result<Exec, SdkError> {
        let path = format!("/v1/execs/{}", id_segment(IdKind::Exec, id.as_ref())?);
        let observed: substrate_wire::Exec = self.get(&path).await?;
        Ok(Exec {
            client: self.clone(),
            observed: observed.into(),
        })
    }

    pub async fn get_pipe_session(&self, id: impl AsRef<str>) -> Result<PipeSession, SdkError> {
        let path = format!("/v1/sessions/{}", id_segment(IdKind::Session, id.as_ref())?);
        let observed: substrate_wire::PipeSession = self.get(&path).await?;
        Ok(PipeSession {
            client: self.clone(),
            observed: observed.into(),
        })
    }

    pub async fn operation(&self, id: impl AsRef<str>) -> Result<Operation, SdkError> {
        let path = format!("/v1/ops/{}", id_segment(IdKind::Operation, id.as_ref())?);
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
            id_segment(IdKind::Snapshot, snapshot_id.as_ref())?
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

    /// Opens one host process's event stream (ADR 0036 S12), from its start or after `cursor`,
    /// the cursor of an item an earlier stream delivered. Served only on the personal Unix
    /// socket to the daemon's own uid; any other caller meets the route's named refusal.
    pub async fn process_events(
        &self,
        process_id: impl AsRef<str>,
        cursor: Option<&str>,
    ) -> Result<ProcessEventStream, SdkError> {
        self.inner
            .transport
            .process_events(process_id.as_ref(), cursor)
            .await
    }

    async fn read_machine(&self) -> Result<Machine, SdkError> {
        let snapshot: substrate_wire::CapabilitySnapshot = self.get("/v1/machine").await?;
        Ok(snapshot.into())
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, SdkError> {
        let response = self.inner.transport.request("GET", path, None).await?;
        decode_result(&response)
    }

    async fn get_with_omissions<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<WithOmissions<T>, SdkError> {
        let response = self.inner.transport.request("GET", path, None).await?;
        let listing = decode_result(&response)?;
        Ok(WithOmissions {
            listing,
            omitted_entries: omitted_entries(&response)?,
        })
    }

    async fn mutation<I: Serialize, O: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        input: &I,
        operation_id: Option<String>,
    ) -> Result<(String, O), SdkError> {
        self.recovered_mutation(method, path, input, operation_id, None)
            .await
    }

    async fn mutation_with_source_authority<I: Serialize, O: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        input: &I,
        operation_id: Option<String>,
        source_authority: &str,
    ) -> Result<(String, O), SdkError> {
        self.recovered_mutation(method, path, input, operation_id, Some(source_authority))
            .await
    }

    /// One mutation under one operation id (design 18). A transport failure without an answer is
    /// reconciled against the operation ledger, with or without a source authority.
    async fn recovered_mutation<I: Serialize, O: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        input: &I,
        operation_id: Option<String>,
        source_authority: Option<&str>,
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
            .send_mutation(method, path, &body, source_authority)
            .await
        {
            Ok(response) => decode_result(&response).map(|result| (operation_id, result)),
            Err(SdkError::Transport(_)) => {
                self.recover_mutation(method, path, &body, &operation_id, source_authority)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    /// Sends one serialized mutation, with the workspace source authority when it has one.
    async fn send_mutation(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        source_authority: Option<&str>,
    ) -> Result<transport::HttpResponse, SdkError> {
        let transport = &self.inner.transport;
        match source_authority {
            Some(source_authority) => {
                transport
                    .request_with_source_authority(method, path, Some(body), source_authority)
                    .await
            }
            None => transport.request(method, path, Some(body)).await,
        }
    }

    async fn recover_mutation<O: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        operation_id: &str,
        source_authority: Option<&str>,
    ) -> Result<(String, O), SdkError> {
        match self.operation(operation_id).await {
            Ok(operation) => outcome::<O>(operation, operation_id),
            Err(SdkError::Refusal(refusal)) if refusal.code == "resource.not-found" => {
                match self
                    .send_mutation(method, path, body, source_authority)
                    .await
                {
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
    /// Selects the empty source, replacing any earlier selection: a Git source selected before
    /// is dropped together with its source authority.
    pub fn empty(mut self) -> Self {
        self.source = substrate_wire::WorkspaceSource::Empty(substrate_wire::EmptySource::Empty);
        self.source_authorization = None;
        self
    }

    /// Selects a Git source and its source authority, replacing any earlier selection.
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

/// A listing together with how many entries it omitted (ADR 0043): entries the wire cannot name,
/// a FIFO, socket, device or non-UTF-8 name, read from the `x-b10x-omitted-entries` response
/// header. Zero when the header is absent. On a tree that reached its limit the count is a lower
/// bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithOmissions<T> {
    pub listing: T,
    pub omitted_entries: u64,
}

/// The omitted-entry count a response states: absent is zero; present is a decimal integer of at
/// least 1, and anything else is a protocol error rather than a guess (invariant 3).
fn omitted_entries(response: &transport::HttpResponse) -> Result<u64, SdkError> {
    let Some(value) = response.headers.get(substrate_wire::OMITTED_ENTRIES_HEADER) else {
        return Ok(0);
    };
    match value.parse::<u64>() {
        Ok(count)
            if count >= 1
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && !value.starts_with('0') =>
        {
            Ok(count)
        }
        _ => Err(SdkError::Protocol(format!(
            "{} is not a positive decimal count",
            substrate_wire::OMITTED_ENTRIES_HEADER
        ))),
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

    /// This workspace's id as the path segment of a request target ([`id_segment`]).
    fn id_segment(&self) -> Result<String, SdkError> {
        id_segment(IdKind::Workspace, &self.observed.id)
    }

    pub fn command(&self, program: impl Into<String>) -> CommandBuilder {
        CommandBuilder {
            workspace: self.clone(),
            argv: vec![program.into()],
            allowed_environment: Vec::new(),
            environment: BTreeMap::new(),
            workspace_access: WorkspaceAccess::default(),
            network: substrate_wire::NetworkMode::None,
            apertures: Vec::new(),
            scratch: None,
            measurements: BTreeSet::new(),
            read_only_roots: Vec::new(),
            writable_roots: Vec::new(),
            read_only_files: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            workdir: None,
            policy: None,
            lease_ttl: None,
            operation_id: None,
        }
    }

    pub fn pipe_session(&self, program: impl Into<String>) -> PipeSessionBuilder {
        PipeSessionBuilder {
            capture: substrate_wire::CaptureMode::Recorded,
            workspace: self.clone(),
            argv: vec![program.into()],
            allowed_environment: Vec::new(),
            environment: BTreeMap::new(),
            workspace_access: WorkspaceAccess::default(),
            network: substrate_wire::NetworkMode::None,
            apertures: Vec::new(),
            scratch: None,
            measurements: BTreeSet::new(),
            read_only_roots: Vec::new(),
            writable_roots: Vec::new(),
            read_only_files: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            workdir: None,
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
            self.id_segment()?,
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

    /// One page of a directory's immediate children, in name order; `cursor` is the page
    /// before's `next_cursor`. The path `"."` is the workspace's root (ADR 0052). It is admitted
    /// here and by [`Workspace::read_directory_with_omissions`] only: every other file call
    /// refuses it. A daemon older than the root page refuses it `workspace.path-escape`.
    pub async fn read_directory(
        &self,
        path: impl AsRef<str>,
        cursor: Option<&str>,
        limit_items: u32,
    ) -> Result<DirectoryPage, SdkError> {
        self.read_directory_with_omissions(path, cursor, limit_items)
            .await
            .map(|page| page.listing)
    }

    /// [`Workspace::read_directory`] together with how many entries the directory omits from
    /// every page (ADR 0043). The count is the directory's, so every page carries the same one.
    pub async fn read_directory_with_omissions(
        &self,
        path: impl AsRef<str>,
        cursor: Option<&str>,
        limit_items: u32,
    ) -> Result<WithOmissions<DirectoryPage>, SdkError> {
        if limit_items == 0 || limit_items > MAX_LIST_ITEMS {
            return Err(SdkError::Protocol(
                "directory limit is outside its contract bound".to_owned(),
            ));
        }
        substrate_wire::validate_directory_address(path.as_ref())
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
            self.id_segment()?,
            encode_path(path.as_ref())
        );
        let result: WithOmissions<substrate_wire::FileReadResult> =
            self.client.get_with_omissions(&target).await?;
        match result.listing {
            substrate_wire::FileReadResult::Directory(page) => Ok(WithOmissions {
                listing: page,
                omitted_entries: result.omitted_entries,
            }),
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
            self.id_segment()?,
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
            self.id_segment()?,
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
        let target = format!("/v1/workspaces/{}/lease/renew", self.id_segment()?);
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
        self.client
            .destroy_workspace(&self.observed.id, operation_id.into())
            .await
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
            self.id_segment()?,
            encode_path(path.as_ref())
        );
        let slice: substrate_wire::DigestedFileSlice = self.client.get(&target).await?;
        slice.try_into()
    }

    /// A bounded tree. `truncated` is true when the walk reached `limit_items` with entries left
    /// or omitted an entry it cannot name; [`Workspace::tree_with_omissions`] tells the two apart.
    pub async fn tree(
        &self,
        limit_items: u32,
        include_hidden: bool,
    ) -> Result<WorkspaceTree, SdkError> {
        self.tree_with_omissions(limit_items, include_hidden)
            .await
            .map(|tree| tree.listing)
    }

    /// [`Workspace::tree`] together with how many entries the walked part of the tree omitted
    /// (ADR 0043). A tree with `truncated` true and a count of zero reached its limit only.
    pub async fn tree_with_omissions(
        &self,
        limit_items: u32,
        include_hidden: bool,
    ) -> Result<WithOmissions<WorkspaceTree>, SdkError> {
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
            .get_with_omissions(&format!(
                "/v2/workspaces/{}/tree?{query}",
                self.id_segment()?
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
                self.id_segment()?,
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
                self.id_segment()?
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
            self.id_segment()?,
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
    apertures: Vec<String>,
    scratch: Option<StorageLimit>,
    measurements: BTreeSet<ExecMeasurement>,
    read_only_roots: Vec<ReadOnlyRoot>,
    writable_roots: Vec<WritableRoot>,
    read_only_files: Vec<ReadOnlyFile>,
    secret_slots: Vec<SecretSlotRequest>,
    capsule: Option<ExecutionCapsuleInput>,
    workdir: Option<String>,
    policy: Option<ExecutionPolicy>,
    lease_ttl: Option<Duration>,
    operation_id: Option<String>,
}

#[must_use]
pub struct PipeSessionBuilder {
    capture: substrate_wire::CaptureMode,
    workspace: Workspace,
    argv: Vec<String>,
    allowed_environment: Vec<BaselineEnvironment>,
    environment: BTreeMap<String, String>,
    workspace_access: WorkspaceAccess,
    network: substrate_wire::NetworkMode,
    apertures: Vec<String>,
    scratch: Option<StorageLimit>,
    measurements: BTreeSet<ExecMeasurement>,
    read_only_roots: Vec<ReadOnlyRoot>,
    writable_roots: Vec<WritableRoot>,
    read_only_files: Vec<ReadOnlyFile>,
    secret_slots: Vec<SecretSlotRequest>,
    capsule: Option<ExecutionCapsuleInput>,
    workdir: Option<String>,
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
    /// Select terminal capture explicitly. Unrecorded requires daemon policy and driver support.
    /// No retry downgrades this choice; output remains live-only with no reconnect replay.
    pub fn capture(mut self, capture: substrate_wire::CaptureMode) -> Self {
        self.capture = capture;
        self
    }

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

    /// Selects one operator-declared egress aperture by name (ADR 0013), replacing any earlier
    /// selection.
    pub fn aperture(mut self, name: impl Into<String>) -> Self {
        self.network = substrate_wire::NetworkMode::Aperture;
        self.apertures = vec![name.into()];
        self
    }

    /// Selects several operator-declared egress apertures by name, in order (design 26 § 2),
    /// replacing any earlier selection.
    ///
    /// The n-th name is reached in the run at `127.0.0.n`. One name is sent as `aperture`, as
    /// [`Self::aperture`] sends it; two to [`substrate_wire::MAX_EGRESS_APERTURES`] are sent as
    /// `apertures`. The list is sent as given: an empty, over-long or repeated list is the
    /// daemon's named refusal, never adjusted here.
    pub fn apertures<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.network = substrate_wire::NetworkMode::Aperture;
        self.apertures = names.into_iter().map(Into::into).collect();
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

    /// Mounts one host directory read-only (ADR 0010). Repeatable. More than
    /// [`substrate_wire::MAX_READ_ONLY_ROOTS`] are refused with [`SdkError::Protocol`] before
    /// anything is sent.
    pub fn read_only_root(mut self, root: ReadOnlyRoot) -> Self {
        self.read_only_roots.push(root);
        self
    }

    /// Mounts one writable root (ADR 0038): a host directory the daemon operator declared with
    /// `--writable-root`, by name, or a Git checkout workspace the caller holds. Repeatable; the
    /// roots reach the start request in the order given. More than [`MAX_WRITABLE_ROOTS`] are
    /// refused with [`SdkError::Protocol`] before anything is sent.
    pub fn writable_root(mut self, root: WritableRoot) -> Self {
        self.writable_roots.push(root);
        self
    }

    /// Declares one host file read-only at a mount (ADR 0033).
    pub fn read_only_file(mut self, file: ReadOnlyFile) -> Self {
        self.read_only_files.push(file);
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

    /// Selects the directory the program starts in (design 26 § 1), replacing any earlier
    /// selection. Without one it starts in `/workspace`.
    ///
    /// The path is absolute and lexically normal, at most [`substrate_wire::MAX_WORKDIR_BYTES`]
    /// bytes, and at or beneath `/workspace` or another mount this start places. Any other path
    /// is refused with [`SdkError::Protocol`] before anything is sent.
    ///
    /// A session start is answered `session.confinement-unavailable` first on a host that cannot
    /// confine a session, before its working directory is read. On a host that can, one that
    /// publishes no `exec.workdir` fact answers `exec.workdir-unserved`, and a directory that is
    /// absent, or reached through a symlink, `exec.workdir-missing`.
    pub fn workdir(mut self, path: impl Into<String>) -> Self {
        self.workdir = Some(path.into());
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
        validate_read_only_root_bound(&self.read_only_roots)?;
        validate_execution_storage(&self.workspace_access, self.scratch)?;
        validate_writable_root_bound(&self.writable_roots)?;
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
        let (aperture, apertures) = aperture_selection(self.apertures);
        let exec = substrate_wire::ExecStartInput {
            writable_roots: self.writable_roots,
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
                requires: None,
                capability_snapshot: self.workspace.client.machine().capability_snapshot,
                network: self.network,
                aperture,
                apertures,
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
            read_only_files: self.read_only_files,
            secret_slots: self.secret_slots,
            capsule: self.capsule,
            lease_ttl_ms: Some(required_duration_millis(lease_ttl)?),
            workdir: self.workdir,
        };
        validate_workdir(&exec)?;
        let input = substrate_wire::PipeSessionStartInput {
            capture: self.capture,
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

    /// This session's id as the path segment of a request target ([`id_segment`]).
    fn id_segment(&self) -> Result<String, SdkError> {
        id_segment(IdKind::Session, &self.observed.id)
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
        let target = format!("/v1/sessions/{}/signal", self.id_segment()?);
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
        let target = format!("/v1/sessions/{}/lease/renew", self.id_segment()?);
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
        let target = format!("/v1/sessions/{}", self.id_segment()?);
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

    /// Selects one operator-declared egress aperture by name (ADR 0013), replacing any earlier
    /// selection.
    pub fn aperture(mut self, name: impl Into<String>) -> Self {
        self.network = substrate_wire::NetworkMode::Aperture;
        self.apertures = vec![name.into()];
        self
    }

    /// Selects several operator-declared egress apertures by name, in order (design 26 § 2),
    /// replacing any earlier selection.
    ///
    /// The n-th name is reached in the run at `127.0.0.n`. One name is sent as `aperture`, as
    /// [`Self::aperture`] sends it; two to [`substrate_wire::MAX_EGRESS_APERTURES`] are sent as
    /// `apertures`. The list is sent as given: an empty, over-long or repeated list is the
    /// daemon's named refusal, never adjusted here.
    pub fn apertures<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.network = substrate_wire::NetworkMode::Aperture;
        self.apertures = names.into_iter().map(Into::into).collect();
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

    /// Mounts one host directory read-only (ADR 0010). Repeatable. More than
    /// [`substrate_wire::MAX_READ_ONLY_ROOTS`] are refused with [`SdkError::Protocol`] before
    /// anything is sent.
    pub fn read_only_root(mut self, root: ReadOnlyRoot) -> Self {
        self.read_only_roots.push(root);
        self
    }

    /// Mounts one writable root (ADR 0038): a host directory the daemon operator declared with
    /// `--writable-root`, by name, or a Git checkout workspace the caller holds. Repeatable; the
    /// roots reach the start request in the order given. More than [`MAX_WRITABLE_ROOTS`] are
    /// refused with [`SdkError::Protocol`] before anything is sent.
    pub fn writable_root(mut self, root: WritableRoot) -> Self {
        self.writable_roots.push(root);
        self
    }

    /// Declares one host file read-only at a mount (ADR 0033).
    pub fn read_only_file(mut self, file: ReadOnlyFile) -> Self {
        self.read_only_files.push(file);
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

    /// Selects the directory the program starts in (design 26 § 1), replacing any earlier
    /// selection. Without one it starts in `/workspace`.
    ///
    /// The path is absolute and lexically normal, at most [`substrate_wire::MAX_WORKDIR_BYTES`]
    /// bytes, and at or beneath `/workspace` or another mount this start places. Any other path
    /// is refused with [`SdkError::Protocol`] before anything is sent. An exec start on a host that
    /// publishes no `exec.workdir` fact is answered `exec.workdir-unserved`, and one in a
    /// directory that is absent, or reached through a symlink, `exec.workdir-missing`.
    pub fn workdir(mut self, path: impl Into<String>) -> Self {
        self.workdir = Some(path.into());
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
        validate_read_only_root_bound(&self.read_only_roots)?;
        validate_execution_storage(&self.workspace_access, self.scratch)?;
        validate_writable_root_bound(&self.writable_roots)?;
        let policy = self.policy.ok_or(SdkError::Builder { field: "policy" })?;
        let capability_snapshot = self.workspace.client.machine().capability_snapshot;
        let (aperture, apertures) = aperture_selection(self.apertures);
        let input = substrate_wire::ExecStartInput {
            writable_roots: self.writable_roots,
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
                requires: None,
                capability_snapshot,
                network: self.network,
                aperture,
                apertures,
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
            read_only_files: self.read_only_files,
            secret_slots: self.secret_slots,
            capsule: self.capsule,
            lease_ttl_ms: duration_millis(self.lease_ttl)?,
            workdir: self.workdir,
        };
        validate_workdir(&input)?;
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

    /// This exec's id as the path segment of a request target ([`id_segment`]).
    fn id_segment(&self) -> Result<String, SdkError> {
        id_segment(IdKind::Exec, &self.observed.id)
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
        let target = format!("/v1/execs/{}/signal", self.id_segment()?);
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
        let target = format!("/v1/execs/{}/lease/renew", self.id_segment()?);
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
        let target = format!("/v1/execs/{}", self.id_segment()?);
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
            let target = format!("/v1/execs/{}/output?{query}", self.id_segment()?);
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
        let target = format!("/v1/execs/{}/output?{query}", self.id_segment()?);
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

/// Refuses more read-only roots than [`substrate_wire::MAX_READ_ONLY_ROOTS`] before a request is
/// sent (ADR 0010). Every other rule on a root is the daemon's, answered with its named refusal.
fn validate_read_only_root_bound(roots: &[ReadOnlyRoot]) -> Result<(), SdkError> {
    if roots.len() > substrate_wire::MAX_READ_ONLY_ROOTS as usize {
        return Err(SdkError::Protocol(
            substrate_wire::WireValidationError::InvalidReadOnlyRootBounds.to_string(),
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

/// Refuses more writable roots than [`MAX_WRITABLE_ROOTS`] before a request is sent (ADR 0038).
/// Every other rule on a root is the daemon's, answered with its named refusal.
fn validate_writable_root_bound(roots: &[WritableRoot]) -> Result<(), SdkError> {
    if roots.len() > MAX_WRITABLE_ROOTS as usize {
        return Err(SdkError::Protocol(
            substrate_wire::WireValidationError::WritableRootsExceeded.to_string(),
        ));
    }
    Ok(())
}

/// Refuses a working directory the wire's lexical rule refuses (design 26 § 1) before a request is
/// sent, with [`substrate_wire::validate_workdir`]: a path that is not absolute and normal within
/// its bound, or one outside every mount this start places. Whether the host serves a working
/// directory and whether the directory exists are the daemon's answers, `exec.workdir-unserved`
/// and `exec.workdir-missing`.
fn validate_workdir(input: &substrate_wire::ExecStartInput) -> Result<(), SdkError> {
    substrate_wire::validate_workdir(input).map_err(|error| SdkError::Protocol(error.to_string()))
}

/// Writes a builder's aperture selection in the wire's two forms (design 26 § 2): one name as
/// `aperture`, any other count as `apertures`.
fn aperture_selection(mut names: Vec<String>) -> (Option<String>, Vec<String>) {
    if names.len() == 1 {
        (names.pop(), Vec::new())
    } else {
        (None, names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{UnixListener, UnixStream};

    fn with_omitted_header(value: Option<&str>) -> transport::HttpResponse {
        let mut headers = BTreeMap::new();
        if let Some(value) = value {
            headers.insert(
                substrate_wire::OMITTED_ENTRIES_HEADER.to_owned(),
                value.to_owned(),
            );
        }
        transport::HttpResponse {
            status: 200,
            headers,
            body: Vec::new(),
        }
    }

    /// ADR 0043: absent is zero, a positive decimal is the count, and nothing else is guessed.
    #[test]
    fn the_omitted_entries_header_is_read_strictly() {
        assert_eq!(omitted_entries(&with_omitted_header(None)).ok(), Some(0));
        assert_eq!(
            omitted_entries(&with_omitted_header(Some("1"))).ok(),
            Some(1)
        );
        assert_eq!(
            omitted_entries(&with_omitted_header(Some("1000"))).ok(),
            Some(1000)
        );
        for malformed in ["0", "01", "+1", "-1", "", "one", "1 "] {
            assert!(
                matches!(
                    omitted_entries(&with_omitted_header(Some(malformed))),
                    Err(SdkError::Protocol(_))
                ),
                "{malformed:?} is refused"
            );
        }
    }

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

    /// Issue #236 (ADR 0010, amended 2026-11-20): the SDK sends eight read-only roots and
    /// refuses a ninth before any request.
    #[test]
    fn eight_read_only_roots_are_sent_and_a_ninth_is_refused() {
        let roots: Vec<ReadOnlyRoot> = (1..=9)
            .map(|index| ReadOnlyRoot {
                host_path: format!("/srv/r{index}"),
                mount: format!("/mnt/r{index}"),
            })
            .collect();
        assert!(validate_read_only_root_bound(&roots[..8]).is_ok());
        let Err(SdkError::Protocol(message)) = validate_read_only_root_bound(&roots) else {
            panic!("nine read-only roots are refused by the SDK")
        };
        assert_eq!(
            message,
            substrate_wire::WireValidationError::InvalidReadOnlyRootBounds.to_string()
        );
    }

    /// `story:sdk-read-only-root-bound`: more read-only roots than `MAX_READ_ONLY_ROOTS` are
    /// refused by the exec and session builders before any request is sent: the fake daemon's next
    /// request after both over-bound builds is the sentinel lookup.
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One fake daemon answers the whole journey, in order.
    async fn read_only_roots_over_the_wire_bound_are_refused_before_sending() {
        const SENTINEL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
        async fn answer(listener: &UnixListener, head: &str, status: &str, body: &[u8]) {
            let (mut stream, _) = listener.accept().await.expect("accept SDK request");
            let request = read_request(&mut stream).await;
            assert!(
                request.starts_with(head.as_bytes()),
                "expected {head:?}, got {:?}",
                String::from_utf8_lossy(&request[..request.len().min(80)])
            );
            stream
                .write_all(&response(status, body))
                .await
                .expect("write fake response");
        }
        let temporary = crate::test_unix_socket::tempdir("daemon.sock");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let vector: Value = serde_json::from_str(include_str!(
            "../../../contracts/substrate-wire/0.27.0/vectors/http/machine-probe.json"
        ))
        .expect("machine vector");
        let machine = serde_json::to_vec(
            vector
                .pointer("/expected/response/body")
                .expect("machine response body"),
        )
        .expect("machine response JSON");
        let server = tokio::spawn(async move {
            answer(
                &listener,
                "GET /v1/machine HTTP/1.1\r\n",
                "200 OK",
                &machine,
            )
            .await;
            let workspace = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_workspace",
                "operation": "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
                "result": {
                    "id": "ws_example",
                    "kind": "workspace",
                    "labels": {},
                    "observed_at": "2026-11-08T00:00:00Z",
                    "state": "ready",
                    "origin": "created",
                    "root_name": "ws_example"
                }
            }))
            .expect("workspace response JSON");
            answer(
                &listener,
                "POST /v1/workspaces HTTP/1.1\r\n",
                "201 Created",
                &workspace,
            )
            .await;
            let missing = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_sentinel",
                "error": {
                    "class": "refused",
                    "code": "resource.not-found",
                    "message": "The operation does not exist.",
                    "retriable": false
                }
            }))
            .expect("missing operation JSON");
            answer(
                &listener,
                &format!("GET /v1/ops/{SENTINEL} HTTP/1.1\r\n"),
                "404 Not Found",
                &missing,
            )
            .await;
        });

        let client = Client::builder()
            .unix_socket(&socket)
            .connect()
            .await
            .expect("connect fake daemon");
        let workspace = client
            .workspace()
            .empty()
            .create()
            .await
            .expect("workspace create");
        let policy = || {
            ExecutionPolicy::builder()
                .timeout(Duration::from_secs(5))
                .cpu_time(Duration::from_secs(1))
                .memory_bytes(64 * 1024 * 1024)
                .processes(16)
                .output_bytes(64 * 1024)
                .build()
                .expect("complete execution policy")
        };
        let too_many = || {
            (0..=substrate_wire::MAX_READ_ONLY_ROOTS).map(|index| ReadOnlyRoot {
                host_path: format!("/srv/r{index}"),
                mount: format!("/mnt/r{index}"),
            })
        };
        let expected = substrate_wire::WireValidationError::InvalidReadOnlyRootBounds.to_string();

        let mut command = workspace.command("/usr/bin/true").policy(policy());
        for root in too_many() {
            command = command.read_only_root(root);
        }
        let Err(SdkError::Protocol(message)) = command.start().await else {
            panic!("an exec over the read-only-root bound is refused by the SDK")
        };
        assert_eq!(message, expected);

        let mut session = workspace
            .pty_session(
                "/usr/bin/true",
                PtyWindow {
                    columns: 80,
                    rows: 24,
                },
            )
            .policy(policy())
            .lease(Duration::from_secs(5))
            .input_limit_bytes(1024)
            .frame_limit_bytes(1024)
            .queued_frames(4);
        for root in too_many() {
            session = session.read_only_root(root);
        }
        let Err(SdkError::Protocol(message)) = session.start().await else {
            panic!("a session over the read-only-root bound is refused by the SDK")
        };
        assert_eq!(message, expected);

        let Err(SdkError::Refusal(refusal)) = client.operation(SENTINEL).await else {
            panic!("the sentinel operation is absent")
        };
        assert_eq!(refusal.code, "resource.not-found");
        server.await.expect("fake daemon task");
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

    /// The source selected last decides what a workspace builder creates: `empty()` after
    /// `git(..)` sends the empty source and no source authority, and `git(..)` after `empty()`
    /// sends the Git source with its authority.
    #[tokio::test]
    async fn the_workspace_builder_takes_the_source_selected_last() {
        const AUTHORITY: &str = "source-authority-of-the-case";
        const HEADER: &str = "x-b10x-workspace-source-authorization";
        let temporary = crate::test_unix_socket::tempdir("daemon.sock");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let vector: Value = serde_json::from_str(include_str!(
            "../../../contracts/substrate-wire/0.27.0/vectors/http/machine-probe.json"
        ))
        .expect("machine vector");
        let machine = serde_json::to_vec(
            vector
                .pointer("/expected/response/body")
                .expect("machine response body"),
        )
        .expect("machine response JSON");
        let server = tokio::spawn(async move {
            answer_one(
                &listener,
                "GET /v1/machine HTTP/1.1\r\n",
                "200 OK",
                &machine,
            )
            .await;
            let workspace = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_workspace",
                "operation": "01ARZ3NDEKTSV4RRFFQ69G5FB4",
                "result": {
                    "id": "ws_example",
                    "kind": "workspace",
                    "labels": {},
                    "observed_at": "2026-11-08T00:00:00Z",
                    "state": "ready",
                    "origin": "created",
                    "root_name": "ws_example"
                }
            }))
            .expect("workspace response JSON");
            let create = "POST /v1/workspaces HTTP/1.1\r\n";
            [
                answer_one(&listener, create, "201 Created", &workspace).await,
                answer_one(&listener, create, "201 Created", &workspace).await,
            ]
        });

        let client = Client::builder()
            .unix_socket(&socket)
            .connect()
            .await
            .expect("connect fake daemon");
        let git = |builder: WorkspaceBuilder| {
            builder.git(
                "origin",
                "example/repository",
                "refs/heads/main",
                "0123456789abcdef0123456789abcdef01234567",
                1,
                AUTHORITY,
            )
        };
        git(client.workspace())
            .empty()
            .create()
            .await
            .expect("an empty workspace after git(..).empty()");
        git(client.workspace().empty())
            .create()
            .await
            .expect("a Git workspace after empty().git(..)");

        let requests = server.await.expect("fake daemon task");
        let source = |request: &[u8]| {
            serde_json::from_slice::<Value>(request_body(request)).expect("create JSON")["input"]
                ["source"]
                .clone()
        };
        let names_authority =
            |request: &[u8]| String::from_utf8_lossy(request).contains(&format!("{HEADER}: "));
        assert_eq!(
            source(&requests[0]),
            "empty",
            "empty() selected last sends the empty source"
        );
        assert!(
            !names_authority(&requests[0]),
            "an empty source carries no source authority"
        );
        assert_eq!(
            source(&requests[1])["git"]["commit"],
            "0123456789abcdef0123456789abcdef01234567",
            "git(..) selected last sends the Git source"
        );
        assert!(
            String::from_utf8_lossy(&requests[1]).contains(&format!("{HEADER}: {AUTHORITY}\r\n")),
            "a Git source carries its authority"
        );
    }

    #[test]
    fn command_bytes_do_not_claim_unadvertised_additions() {
        let input = substrate_wire::ExecStartInput {
            writable_roots: Vec::new(),
            workspace: "ws_example".to_owned(),
            argv: vec!["/usr/bin/true".to_owned()],
            env: substrate_wire::ExecEnvironment {
                allow: Vec::new(),
                set: BTreeMap::new(),
            },
            sandbox: substrate_wire::ConfinementRequest {
                requires: None,
                capability_snapshot: format!("sha256:{}", "7".repeat(64)),
                network: substrate_wire::NetworkMode::None,
                aperture: None,
                apertures: Vec::new(),
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
            read_only_files: Vec::new(),
            secret_slots: Vec::new(),
            capsule: None,
            lease_ttl_ms: None,
            workdir: None,
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
        for later in [
            "scratch",
            "measurements",
            "read_only_roots",
            "read_only_files",
            "secret_slots",
        ] {
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
        let temporary = crate::test_unix_socket::tempdir("daemon.sock");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let vector: Value = serde_json::from_str(include_str!(
            "../../../contracts/substrate-wire/0.27.0/vectors/http/machine-probe.json"
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
                    "state": "ready",
                    "origin": "created",
                    "root_name": "ws_recovered"
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

    /// Design 18: a Git workspace create whose reply is lost is recovered like every other
    /// create. The ledger's stored answer is returned; when the ledger has no row, the identical
    /// request is sent once more with its source authority; and when the ledger cannot be asked
    /// either, the error carries the operation id the request was sent under.
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One fake daemon answers the three recoveries, in order.
    async fn a_lost_git_create_response_is_recovered_from_the_ledger() {
        const STORED: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
        const REPLAYED: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
        const AUTHORITY: &str = "source-authority-of-the-case";
        /// Reads one request whose line starts with `head`, then drops the connection unanswered.
        async fn lose_reply(listener: &UnixListener, head: &str) -> Vec<u8> {
            let (mut stream, _) = listener.accept().await.expect("accept SDK request");
            let request = read_request(&mut stream).await;
            assert!(
                request.starts_with(head.as_bytes()),
                "expected {head:?}, got {:?}",
                String::from_utf8_lossy(&request[..request.len().min(80)])
            );
            request
        }
        fn created(operation: &str, id: &str) -> Vec<u8> {
            serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_create",
                "operation": operation,
                "result": {
                    "id": id,
                    "kind": "workspace",
                    "labels": {},
                    "observed_at": "2026-11-08T00:00:00Z",
                    "state": "ready",
                    "origin": "created",
                    "root_name": id
                }
            }))
            .expect("workspace response JSON")
        }
        fn operation_of(request: &[u8]) -> String {
            serde_json::from_slice::<Value>(request_body(request)).expect("mutation JSON")["op"]
                .as_str()
                .expect("operation id")
                .to_owned()
        }
        fn carries_authority(request: &[u8]) -> bool {
            String::from_utf8_lossy(request).contains(&format!(
                "x-b10x-workspace-source-authorization: {AUTHORITY}\r\n"
            ))
        }
        const CREATE: &str = "POST /v1/workspaces HTTP/1.1\r\n";

        let temporary = crate::test_unix_socket::tempdir("daemon.sock");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let vector: Value = serde_json::from_str(include_str!(
            "../../../contracts/substrate-wire/0.27.0/vectors/http/machine-probe.json"
        ))
        .expect("machine vector");
        let machine = serde_json::to_vec(
            vector
                .pointer("/expected/response/body")
                .expect("machine response body"),
        )
        .expect("machine response JSON");
        let server = tokio::spawn(async move {
            answer_one(
                &listener,
                "GET /v1/machine HTTP/1.1\r\n",
                "200 OK",
                &machine,
            )
            .await;

            // The daemon accepted the create and its reply is lost: the ledger holds the answer.
            let first = lose_reply(&listener, CREATE).await;
            assert_eq!(operation_of(&first), STORED);
            assert!(carries_authority(&first));
            let record = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_ledger",
                "result": {
                    "accepted_at": "2026-11-08T00:00:00Z",
                    "actor": "local:1000",
                    "capability_snapshot": null,
                    "operation": STORED,
                    "operation_kind": "workspace.create",
                    "outcome": {
                        "kind": "success",
                        "result": {
                            "id": "ws_stored",
                            "kind": "workspace",
                            "labels": {},
                            "observed_at": "2026-11-08T00:00:00Z",
                            "state": "ready",
                            "origin": "created",
                            "root_name": "ws_stored"
                        }
                    },
                    "principal": null,
                    "request_hash": "7".repeat(64),
                    "resource": "ws_stored",
                    "state": "terminal",
                    "terminal_at": "2026-11-08T00:00:00Z"
                }
            }))
            .expect("operation record JSON");
            answer_one(
                &listener,
                &format!("GET /v1/ops/{STORED} HTTP/1.1\r\n"),
                "200 OK",
                &record,
            )
            .await;

            // The ledger has no row: the identical request is sent once more, authority included.
            let first = lose_reply(&listener, CREATE).await;
            let missing = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_ledger",
                "error": {
                    "class": "refused",
                    "code": "resource.not-found",
                    "message": "The operation does not exist.",
                    "retriable": false
                }
            }))
            .expect("missing operation JSON");
            answer_one(
                &listener,
                &format!("GET /v1/ops/{REPLAYED} HTTP/1.1\r\n"),
                "404 Not Found",
                &missing,
            )
            .await;
            let replay = answer_one(
                &listener,
                CREATE,
                "201 Created",
                &created(REPLAYED, "ws_replayed"),
            )
            .await;
            assert_eq!(request_body(&replay), request_body(&first));
            assert!(carries_authority(&replay));

            // Neither the create nor the ledger answers: the outcome stays open.
            let lost = operation_of(&lose_reply(&listener, CREATE).await);
            lose_reply(&listener, &format!("GET /v1/ops/{lost} HTTP/1.1\r\n")).await;
            lost
        });

        let client = Client::builder()
            .unix_socket(&socket)
            .connect()
            .await
            .expect("connect fake daemon");
        let git = || {
            client.workspace().git(
                "origin",
                "example/repository",
                "refs/heads/main",
                "0123456789abcdef0123456789abcdef01234567",
                1,
                AUTHORITY,
            )
        };
        let workspace = git()
            .operation_id(STORED)
            .create()
            .await
            .expect("the ledger's stored answer recovers the lost reply");
        assert_eq!(workspace.id(), "ws_stored");
        let workspace = git()
            .operation_id(REPLAYED)
            .create()
            .await
            .expect("an absent ledger row replays the identical request");
        assert_eq!(workspace.id(), "ws_replayed");
        let open = git()
            .create()
            .await
            .map(|workspace| workspace.id().to_owned());
        let Err(SdkError::UnknownOperation { operation_id }) = open else {
            panic!("an open outcome carries its operation id, got {open:?}")
        };
        assert_eq!(operation_id, server.await.expect("fake daemon task"));
    }

    /// Accepts one connection, checks its request line starts with `head`, answers `status` with
    /// `body`, and returns the request.
    async fn answer_one(listener: &UnixListener, head: &str, status: &str, body: &[u8]) -> Vec<u8> {
        let (mut stream, _) = listener.accept().await.expect("accept SDK request");
        let request = read_request(&mut stream).await;
        assert!(
            request.starts_with(head.as_bytes()),
            "expected {head:?}, got {:?}",
            String::from_utf8_lossy(&request[..request.len().min(80)])
        );
        stream
            .write_all(&response(status, body))
            .await
            .expect("write fake response");
        request
    }

    /// `story:sdk-aperture-list`: the exec and session builders send several apertures as the
    /// wire's `apertures` list in the order given, and one aperture as `aperture`, whether it was
    /// named by `aperture` or as a one-name list.
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One fake daemon answers the whole journey, in order.
    async fn an_aperture_list_reaches_the_start_request_in_the_wires_two_forms() {
        let temporary = crate::test_unix_socket::tempdir("daemon.sock");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let vector: Value = serde_json::from_str(include_str!(
            "../../../contracts/substrate-wire/0.27.0/vectors/http/machine-probe.json"
        ))
        .expect("machine vector");
        let machine = serde_json::to_vec(
            vector
                .pointer("/expected/response/body")
                .expect("machine response body"),
        )
        .expect("machine response JSON");
        let server = tokio::spawn(async move {
            answer_one(
                &listener,
                "GET /v1/machine HTTP/1.1\r\n",
                "200 OK",
                &machine,
            )
            .await;
            let workspace = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_workspace",
                "operation": "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                "result": {
                    "id": "ws_example",
                    "kind": "workspace",
                    "labels": {},
                    "observed_at": "2026-11-08T00:00:00Z",
                    "state": "ready",
                    "origin": "created",
                    "root_name": "ws_example"
                }
            }))
            .expect("workspace response JSON");
            answer_one(
                &listener,
                "POST /v1/workspaces HTTP/1.1\r\n",
                "201 Created",
                &workspace,
            )
            .await;
            let unserved = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_unserved",
                "error": {
                    "class": "unserved",
                    "code": "exec.egress-apertures-unserved",
                    "message": "The host declares no aperture.",
                    "retriable": false
                }
            }))
            .expect("refusal JSON");
            let mut sandboxes = Vec::new();
            for (head, pointer) in [
                ("POST /v1/execs HTTP/1.1\r\n", "/input/sandbox"),
                ("POST /v1/sessions HTTP/1.1\r\n", "/input/exec/sandbox"),
                ("POST /v1/execs HTTP/1.1\r\n", "/input/sandbox"),
                ("POST /v1/execs HTTP/1.1\r\n", "/input/sandbox"),
                ("POST /v1/sessions HTTP/1.1\r\n", "/input/exec/sandbox"),
            ] {
                let request = answer_one(&listener, head, "501 Not Implemented", &unserved).await;
                let body: Value = serde_json::from_slice(request_body(&request)).expect("JSON");
                sandboxes.push(body.pointer(pointer).expect("sandbox").clone());
            }
            sandboxes
        });

        let client = Client::builder()
            .unix_socket(&socket)
            .connect()
            .await
            .expect("connect fake daemon");
        let workspace = client
            .workspace()
            .empty()
            .create()
            .await
            .expect("workspace create");
        let policy = || {
            ExecutionPolicy::builder()
                .timeout(Duration::from_secs(5))
                .cpu_time(Duration::from_secs(1))
                .memory_bytes(64 * 1024 * 1024)
                .processes(16)
                .output_bytes(64 * 1024)
                .build()
                .expect("complete execution policy")
        };
        let session = || {
            workspace
                .pipe_session("/usr/bin/true")
                .policy(policy())
                .lease(Duration::from_secs(5))
                .input_limit_bytes(1024)
                .frame_limit_bytes(1024)
                .queued_frames(4)
        };
        let refused = |result: Result<(), SdkError>| {
            let Err(SdkError::Refusal(refusal)) = result else {
                panic!("the fake daemon refuses the start")
            };
            assert_eq!(refusal.code, "exec.egress-apertures-unserved");
        };
        refused(
            workspace
                .command("/usr/bin/true")
                .policy(policy())
                .apertures(["dial", "model"])
                .start()
                .await
                .map(drop),
        );
        refused(
            session()
                .apertures(vec!["dial".to_owned(), "model".to_owned()])
                .start()
                .await
                .map(drop),
        );
        refused(
            workspace
                .command("/usr/bin/true")
                .policy(policy())
                .aperture("model")
                .start()
                .await
                .map(drop),
        );
        refused(
            workspace
                .command("/usr/bin/true")
                .policy(policy())
                .apertures(["model"])
                .start()
                .await
                .map(drop),
        );
        refused(
            session()
                .apertures(["dial", "model"])
                .aperture("model")
                .start()
                .await
                .map(drop),
        );

        let sandboxes = server.await.expect("fake daemon task");
        for listed in &sandboxes[..2] {
            assert_eq!(listed["network"], "aperture", "{listed}");
            assert_eq!(listed["apertures"], serde_json::json!(["dial", "model"]));
            assert!(listed.get("aperture").is_none(), "{listed}");
        }
        for single in &sandboxes[2..] {
            assert_eq!(single["network"], "aperture", "{single}");
            assert_eq!(single["aperture"], "model", "{single}");
            assert!(single.get("apertures").is_none(), "{single}");
        }
    }

    fn host_root(name: &str, mount: &str) -> WritableRoot {
        WritableRoot::Host(WritableHostRoot {
            host: name.to_owned(),
            mount: mount.to_owned(),
        })
    }

    /// `story:sdk-writable-roots`: the exec and session builders send their writable roots in the
    /// order given, and more than `MAX_WRITABLE_ROOTS` are refused before any request is sent: the
    /// fake daemon's next request after both over-bound builds is the sentinel lookup.
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One fake daemon answers the whole journey, in order.
    async fn writable_roots_reach_the_start_request_in_order_and_are_bounded_before_sending() {
        const SENTINEL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
        let temporary = crate::test_unix_socket::tempdir("daemon.sock");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let vector: Value = serde_json::from_str(include_str!(
            "../../../contracts/substrate-wire/0.27.0/vectors/http/machine-probe.json"
        ))
        .expect("machine vector");
        let machine = serde_json::to_vec(
            vector
                .pointer("/expected/response/body")
                .expect("machine response body"),
        )
        .expect("machine response JSON");
        let ordered = serde_json::json!([
            {"host": "logs", "mount": "/srv/logs"},
            {"workspace": "ws_second", "mount": "/srv/checkout"},
            {"host": "envelopes", "mount": "/srv/envelopes"},
        ]);
        let expected = ordered.clone();
        let server = tokio::spawn(async move {
            answer_one(
                &listener,
                "GET /v1/machine HTTP/1.1\r\n",
                "200 OK",
                &machine,
            )
            .await;
            let workspace = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_workspace",
                "operation": "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                "result": {
                    "id": "ws_example",
                    "kind": "workspace",
                    "labels": {},
                    "observed_at": "2026-11-08T00:00:00Z",
                    "state": "ready",
                    "origin": "created",
                    "root_name": "ws_example"
                }
            }))
            .expect("workspace response JSON");
            answer_one(
                &listener,
                "POST /v1/workspaces HTTP/1.1\r\n",
                "201 Created",
                &workspace,
            )
            .await;
            let unserved = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_unserved",
                "error": {
                    "class": "unserved",
                    "code": "exec.sandbox-unavailable",
                    "message": "The host cannot confine this run.",
                    "retriable": false
                }
            }))
            .expect("refusal JSON");
            let exec = answer_one(
                &listener,
                "POST /v1/execs HTTP/1.1\r\n",
                "501 Not Implemented",
                &unserved,
            )
            .await;
            let exec: Value = serde_json::from_slice(request_body(&exec)).expect("exec JSON");
            assert_eq!(exec["input"]["writable_roots"], expected, "{exec}");
            let session = answer_one(
                &listener,
                "POST /v1/sessions HTTP/1.1\r\n",
                "501 Not Implemented",
                &unserved,
            )
            .await;
            let session: Value =
                serde_json::from_slice(request_body(&session)).expect("session JSON");
            assert_eq!(
                session["input"]["exec"]["writable_roots"], expected,
                "{session}"
            );
            let missing = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_sentinel",
                "error": {
                    "class": "refused",
                    "code": "resource.not-found",
                    "message": "The operation does not exist.",
                    "retriable": false
                }
            }))
            .expect("missing operation JSON");
            answer_one(
                &listener,
                &format!("GET /v1/ops/{SENTINEL} HTTP/1.1\r\n"),
                "404 Not Found",
                &missing,
            )
            .await;
        });

        let client = Client::builder()
            .unix_socket(&socket)
            .connect()
            .await
            .expect("connect fake daemon");
        let workspace = client
            .workspace()
            .empty()
            .create()
            .await
            .expect("workspace create");
        let policy = || {
            ExecutionPolicy::builder()
                .timeout(Duration::from_secs(5))
                .cpu_time(Duration::from_secs(1))
                .memory_bytes(64 * 1024 * 1024)
                .processes(16)
                .output_bytes(64 * 1024)
                .build()
                .expect("complete execution policy")
        };
        let roots: Vec<WritableRoot> =
            serde_json::from_value(ordered).expect("three writable roots");

        let mut command = workspace.command("/usr/bin/true").policy(policy());
        for root in roots.clone() {
            command = command.writable_root(root);
        }
        let Err(SdkError::Refusal(refusal)) = command.start().await else {
            panic!("the fake daemon refuses the exec")
        };
        assert_eq!(refusal.code, "exec.sandbox-unavailable");

        let mut session = workspace
            .pipe_session("/usr/bin/true")
            .policy(policy())
            .lease(Duration::from_secs(5))
            .input_limit_bytes(1024)
            .frame_limit_bytes(1024)
            .queued_frames(4);
        for root in roots {
            session = session.writable_root(root);
        }
        let Err(SdkError::Refusal(refusal)) = session.start().await else {
            panic!("the fake daemon refuses the session")
        };
        assert_eq!(refusal.code, "exec.sandbox-unavailable");

        let too_many = || {
            (0..=MAX_WRITABLE_ROOTS)
                .map(|index| host_root(&format!("r{index}"), &format!("/srv/r{index}")))
        };
        let mut command = workspace.command("/usr/bin/true").policy(policy());
        for root in too_many() {
            command = command.writable_root(root);
        }
        let Err(SdkError::Protocol(message)) = command.start().await else {
            panic!("an exec over the writable-root bound is refused by the SDK")
        };
        assert!(message.contains("writable roots"), "{message}");
        let mut session = workspace
            .pty_session(
                "/usr/bin/true",
                PtyWindow {
                    columns: 80,
                    rows: 24,
                },
            )
            .policy(policy())
            .lease(Duration::from_secs(5))
            .input_limit_bytes(1024)
            .frame_limit_bytes(1024)
            .queued_frames(4);
        for root in too_many() {
            session = session.writable_root(root);
        }
        let Err(SdkError::Protocol(message)) = session.start().await else {
            panic!("a session over the writable-root bound is refused by the SDK")
        };
        assert!(message.contains("writable roots"), "{message}");

        let Err(SdkError::Refusal(refusal)) = client.operation(SENTINEL).await else {
            panic!("the sentinel operation is absent")
        };
        assert_eq!(refusal.code, "resource.not-found");
        server.await.expect("fake daemon task");
    }

    /// Design 26 § 1: `workdir(..)` on the exec and session builders reaches the start request as
    /// the wire's `workdir` member, a start that names none carries no such member, and the
    /// daemon's refusal keeps its name. A path the wire's lexical rule refuses is refused with
    /// that rule's text before any request is sent: the fake daemon's next request after the
    /// refused builds is the sentinel lookup.
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One fake daemon answers the whole journey, in order.
    async fn a_working_directory_reaches_the_start_request_and_is_checked_before_sending() {
        const SENTINEL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB3";
        let temporary = crate::test_unix_socket::tempdir("daemon.sock");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let vector: Value = serde_json::from_str(include_str!(
            "../../../contracts/substrate-wire/0.27.0/vectors/http/machine-probe.json"
        ))
        .expect("machine vector");
        let machine = serde_json::to_vec(
            vector
                .pointer("/expected/response/body")
                .expect("machine response body"),
        )
        .expect("machine response JSON");
        let server = tokio::spawn(async move {
            answer_one(
                &listener,
                "GET /v1/machine HTTP/1.1\r\n",
                "200 OK",
                &machine,
            )
            .await;
            let workspace = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_workspace",
                "operation": "01ARZ3NDEKTSV4RRFFQ69G5FB5",
                "result": {
                    "id": "ws_example",
                    "kind": "workspace",
                    "labels": {},
                    "observed_at": "2026-11-08T00:00:00Z",
                    "state": "ready",
                    "origin": "created",
                    "root_name": "ws_example"
                }
            }))
            .expect("workspace response JSON");
            answer_one(
                &listener,
                "POST /v1/workspaces HTTP/1.1\r\n",
                "201 Created",
                &workspace,
            )
            .await;
            let unserved = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_unserved",
                "error": {
                    "class": "unserved",
                    "code": "exec.workdir-unserved",
                    "message": "A working directory is not served by this host.",
                    "retriable": false,
                    "address": "workdir"
                }
            }))
            .expect("refusal JSON");
            let mut inputs = Vec::new();
            for (head, pointer) in [
                ("POST /v1/execs HTTP/1.1\r\n", "/input"),
                ("POST /v1/sessions HTTP/1.1\r\n", "/input/exec"),
                ("POST /v1/execs HTTP/1.1\r\n", "/input"),
            ] {
                let request = answer_one(&listener, head, "501 Not Implemented", &unserved).await;
                let body: Value = serde_json::from_slice(request_body(&request)).expect("JSON");
                inputs.push(body.pointer(pointer).expect("exec start input").clone());
            }
            let missing = serde_json::to_vec(&serde_json::json!({
                "api_version": "v1",
                "request_id": "req_sentinel",
                "error": {
                    "class": "refused",
                    "code": "resource.not-found",
                    "message": "The operation does not exist.",
                    "retriable": false
                }
            }))
            .expect("missing operation JSON");
            answer_one(
                &listener,
                &format!("GET /v1/ops/{SENTINEL} HTTP/1.1\r\n"),
                "404 Not Found",
                &missing,
            )
            .await;
            inputs
        });

        let client = Client::builder()
            .unix_socket(&socket)
            .connect()
            .await
            .expect("connect fake daemon");
        let workspace = client
            .workspace()
            .empty()
            .create()
            .await
            .expect("workspace create");
        let policy = || {
            ExecutionPolicy::builder()
                .timeout(Duration::from_secs(5))
                .cpu_time(Duration::from_secs(1))
                .memory_bytes(64 * 1024 * 1024)
                .processes(16)
                .output_bytes(64 * 1024)
                .build()
                .expect("complete execution policy")
        };
        let command = || workspace.command("/usr/bin/true").policy(policy());
        let session = || {
            workspace
                .pipe_session("/usr/bin/true")
                .policy(policy())
                .lease(Duration::from_secs(5))
                .input_limit_bytes(1024)
                .frame_limit_bytes(1024)
                .queued_frames(4)
        };
        let unserved = |result: Result<(), SdkError>| {
            let Err(SdkError::Refusal(refusal)) = result else {
                panic!("the fake daemon refuses the start")
            };
            assert_eq!(refusal.code, "exec.workdir-unserved");
            assert_eq!(refusal.class, RefusalClass::Unserved);
        };
        unserved(
            command()
                .workdir("/workspace/crates/gate")
                .start()
                .await
                .map(drop),
        );
        // Beneath a mount this start places itself.
        unserved(
            session()
                .read_only_root(ReadOnlyRoot {
                    host_path: "/srv/tool".to_owned(),
                    mount: "/opt/tool".to_owned(),
                })
                .workdir("/opt/tool/bin")
                .start()
                .await
                .map(drop),
        );
        unserved(command().start().await.map(drop));

        let invalid = substrate_wire::WireValidationError::WorkdirInvalid.to_string();
        let outside = substrate_wire::WireValidationError::WorkdirOutsideMounts.to_string();
        for (workdir, expected) in [
            ("workspace/crates", &invalid),
            ("/workspace/../etc", &invalid),
            ("/workspace/", &invalid),
            ("/tmp", &outside),
            ("/opt/tool/bin", &outside),
        ] {
            let Err(SdkError::Protocol(message)) = command().workdir(workdir).start().await else {
                panic!("an exec in {workdir:?} is refused by the SDK")
            };
            assert_eq!(&message, expected, "{workdir:?}");
            let Err(SdkError::Protocol(message)) = session().workdir(workdir).start().await else {
                panic!("a session in {workdir:?} is refused by the SDK")
            };
            assert_eq!(&message, expected, "{workdir:?}");
        }

        let Err(SdkError::Refusal(refusal)) = client.operation(SENTINEL).await else {
            panic!("the sentinel operation is absent")
        };
        assert_eq!(refusal.code, "resource.not-found");
        let inputs = server.await.expect("fake daemon task");
        assert_eq!(
            inputs[0]["workdir"], "/workspace/crates/gate",
            "{}",
            inputs[0]
        );
        assert_eq!(inputs[1]["workdir"], "/opt/tool/bin", "{}", inputs[1]);
        assert!(inputs[2].get("workdir").is_none(), "{}", inputs[2]);
    }

    /// An identifier is one path segment, at every entry point that names a resource by id. A
    /// word that is not one segment is refused naming the kind of identifier, whether a caller
    /// passed it or a handle holds it as the member of a daemon's answer, and nothing is sent:
    /// the fake daemon keeps the line of every request after the machine probe and holds none.
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One table: every by-id entry point against every word.
    async fn an_identifier_that_is_not_one_path_segment_sends_no_request() {
        fn refused<T>(named: &str, entry: &str, word: &str, result: Result<T, SdkError>) {
            match result {
                Err(SdkError::Protocol(message)) => {
                    assert!(
                        message.starts_with(named),
                        "{entry} with {word:?}: {message}"
                    );
                }
                Err(error) => panic!("{entry} with {word:?} answered {error:?}"),
                Ok(_) => panic!("{entry} with {word:?} was sent and answered"),
            }
        }
        fn result_of(vector: &str) -> Value {
            serde_json::from_str::<Value>(vector)
                .expect("vector")
                .pointer("/expected/response/body/result")
                .expect("response result")
                .clone()
        }

        let temporary = crate::test_unix_socket::tempdir("daemon.sock");
        let socket = temporary.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let machine = serde_json::to_vec(
            serde_json::from_str::<Value>(include_str!(
                "../../../contracts/substrate-wire/0.27.0/vectors/http/machine-probe.json"
            ))
            .expect("machine vector")
            .pointer("/expected/response/body")
            .expect("machine response body"),
        )
        .expect("machine response JSON");
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let kept = Arc::clone(&seen);
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.expect("accept SDK request");
                let request = read_request(&mut stream).await;
                if request.starts_with(b"GET /v1/machine HTTP/1.1\r\n") {
                    stream
                        .write_all(&response("200 OK", &machine))
                        .await
                        .expect("write machine response");
                } else {
                    let text = String::from_utf8_lossy(&request);
                    let line = text.lines().next().unwrap_or_default().to_owned();
                    kept.lock().expect("kept request lines").push(line);
                }
            }
        });

        let client = Client::builder()
            .unix_socket(&socket)
            .connect()
            .await
            .expect("connect fake daemon");
        let workspace_of = |word: &str| Workspace {
            client: client.clone(),
            observed: serde_json::from_value::<substrate_wire::Workspace>(serde_json::json!({
                "id": word,
                "kind": "workspace",
                "labels": {},
                "observed_at": "2026-11-08T00:00:00Z",
                "state": "ready",
                "origin": "created",
                "root_name": "ws_example"
            }))
            .expect("workspace observation")
            .into(),
        };
        let exec_vector = result_of(include_str!(
            "../../../contracts/substrate-wire/0.27.0/vectors/http/exec-get.json"
        ));
        let exec_of = |word: &str| {
            let mut observed = exec_vector.clone();
            observed["id"] = word.into();
            Exec {
                client: client.clone(),
                observed: serde_json::from_value::<substrate_wire::Exec>(observed)
                    .expect("exec observation")
                    .into(),
            }
        };
        let session_vector = result_of(include_str!(
            "../../../contracts/substrate-wire/0.27.0/vectors/http/pipe-session-start.json"
        ));
        let session_of = |word: &str| {
            let mut observed = session_vector.clone();
            observed["id"] = word.into();
            observed["exec"] = word.into();
            PipeSession {
                client: client.clone(),
                observed: serde_json::from_value::<substrate_wire::PipeSession>(observed)
                    .expect("session observation")
                    .into(),
            }
        };
        let second = Duration::from_secs(1);
        let digest = "0".repeat(64);

        for word in [
            "a/b",
            "ws/files/x",
            "..",
            "%2F",
            "",
            ".",
            "a?b",
            "a#b",
            "a\nb",
        ] {
            let named = "workspace id";
            refused(
                named,
                "Client::get_workspace",
                word,
                client.get_workspace(word).await,
            );
            refused(
                named,
                "Client::destroy_workspace",
                word,
                client.destroy_workspace(word, None).await,
            );
            refused(
                "exec id",
                "Client::get_exec",
                word,
                client.get_exec(word).await,
            );
            refused(
                "session id",
                "Client::get_pipe_session",
                word,
                client.get_pipe_session(word).await,
            );
            refused(
                "operation id",
                "Client::operation",
                word,
                client.operation(word).await,
            );
            refused(
                "snapshot id",
                "Client::reconciliation_snapshot_page",
                word,
                client.reconciliation_snapshot_page(word, None, 10).await,
            );
            refused(
                "process id",
                "Client::process_events",
                word,
                client.process_events(word, None).await,
            );

            let mut workspace = workspace_of(word);
            let refresh = workspace.refresh().await.map(drop);
            refused(named, "Workspace::refresh", word, refresh);
            let read = workspace.read_file("a.txt", 0, 16).await;
            refused(named, "Workspace::read_file", word, read);
            let listed = workspace.read_directory("dir", None, 10).await;
            refused(named, "Workspace::read_directory", word, listed);
            let written = workspace.write_file("a.txt", b"x").await;
            refused(named, "Workspace::write_file", word, written);
            let deleted = workspace.delete_file("a.txt").await;
            refused(named, "Workspace::delete_file", word, deleted);
            let renewed = workspace.renew_lease(second).await.map(drop);
            refused(named, "Workspace::renew_lease", word, renewed);
            let read = workspace.read_file_v2("a.txt", 0, 16).await;
            refused(named, "Workspace::read_file_v2", word, read);
            let tree = workspace.tree(10, false).await;
            refused(named, "Workspace::tree", word, tree);
            let baseline = workspace.read_git_file("a.txt", 16).await;
            refused(named, "Workspace::read_git_file", word, baseline);
            let changes = workspace.git_changes(1, 1024, 1024).await;
            refused(named, "Workspace::git_changes", word, changes);
            let replaced = workspace
                .replace_file(
                    "a.txt",
                    b"x",
                    ExpectedFileState::Absent,
                    false,
                    None::<String>,
                )
                .await;
            refused(named, "Workspace::replace_file", word, replaced);
            let edit = FileEditInput {
                expected_sha256: digest.clone(),
                old_text: "a".to_owned(),
                new_text: "b".to_owned(),
                match_policy: TextMatchPolicy::Exact,
            };
            let edited = workspace.edit_file("a.txt", edit, None::<String>).await;
            refused(named, "Workspace::edit_file", word, edited);
            let patch = FilePatchInput {
                expected_sha256: digest.clone(),
                edits: vec![LinePatchEdit::InsertBefore {
                    line: 1,
                    text: "x".to_owned(),
                }],
            };
            let patched = workspace.patch_file("a.txt", patch, None::<String>).await;
            refused(named, "Workspace::patch_file", word, patched);
            refused(named, "Workspace::destroy", word, workspace.destroy().await);

            let named = "exec id";
            let mut exec = exec_of(word);
            let refresh = exec.refresh().await.map(drop);
            refused(named, "Exec::refresh", word, refresh);
            let waited = exec.wait_for(Duration::from_millis(1)).await.map(drop);
            refused(named, "Exec::wait_for", word, waited);
            let signalled = exec.signal(Signal::Terminate, second).await.map(drop);
            refused(named, "Exec::signal", word, signalled);
            let renewed = exec.renew_lease(second).await.map(drop);
            refused(named, "Exec::renew_lease", word, renewed);
            let page = exec.output_page(OutputStream::Stdout, 0, 16).await;
            refused(named, "Exec::output_page", word, page);
            let output = exec.output(substrate_wire::OutputStream::Stdout).await;
            refused(named, "Exec::output", word, output);
            refused(named, "Exec::retire", word, exec.retire().await);

            let named = "session id";
            let mut session = session_of(word);
            let refresh = session.refresh().await.map(drop);
            refused(named, "PipeSession::refresh", word, refresh);
            let signalled = session.signal(Signal::Terminate, second).await.map(drop);
            refused(named, "PipeSession::signal", word, signalled);
            let renewed = session.renew_lease(second).await.map(drop);
            refused(named, "PipeSession::renew_lease", word, renewed);
            let attached = session.attach().await;
            refused(named, "PipeSession::attach", word, attached);
            let exec = session.exec_observation().await;
            refused("exec id", "PipeSession::exec_observation", word, exec);
            refused(named, "PipeSession::retire", word, session.retire().await);
        }

        let sent = seen.lock().expect("kept request lines").clone();
        assert!(sent.is_empty(), "requests were sent: {sent:?}");
        // The control: an id that is one segment reaches the fake daemon, which keeps its line.
        assert!(matches!(
            client.get_workspace("ws_example").await,
            Err(SdkError::Transport(_))
        ));
        server.abort();
        let sent = seen.lock().expect("kept request lines").clone();
        assert_eq!(sent, ["GET /v1/workspaces/ws_example HTTP/1.1"]);
    }
}
