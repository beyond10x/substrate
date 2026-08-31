use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;
#[cfg(feature = "linked-daemon")]
use serde::Serialize;
use serde_json::Value;

/// The daemon contract this SDK release understands.
pub const CONTRACT: &str = "substrate-wire/0.4.0";
/// Digest of the immutable contract bundle this SDK release understands.
pub const CONTRACT_SHA256: &str =
    "002337bd011a0b68f8680cc157ee4d0424d49392c36a0f85e5fa0449ea4ea0da";

/// Verified facts needed by the high-level SDK.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[allow(
    clippy::struct_excessive_bools,
    reason = "these independent booleans are verified daemon capability facts"
)]
pub struct Machine {
    pub capability_snapshot: String,
    pub driver_version: String,
    pub configuration_generation: u64,
    pub probed_at: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub guarded_workspace_io: bool,
    pub exec_argv_only: bool,
    pub exec_no_egress: bool,
    pub exec_cgroup_limits: bool,
    pub exec_cgroup_kill: bool,
    pub events_pull: bool,
    pub events_stream: bool,
}

impl From<substrate_wire::CapabilitySnapshot> for Machine {
    fn from(value: substrate_wire::CapabilitySnapshot) -> Self {
        let facts = value.facts;
        let cgroup = facts.exec_cgroup_limits.as_ref();
        Self {
            capability_snapshot: value.snapshot,
            driver_version: value.driver_version,
            configuration_generation: value.config_generation,
            probed_at: value.probed_at,
            valid_until: value.valid_until,
            guarded_workspace_io: facts.workspace_guarded_io == Some(true),
            exec_argv_only: facts.exec_argv_only == Some(true),
            exec_no_egress: facts.exec_no_egress == Some(true),
            exec_cgroup_limits: cgroup
                .is_some_and(|value| value.cpu && value.memory && value.processes),
            exec_cgroup_kill: facts.exec_cgroup_kill == Some(true),
            events_pull: facts.events_pull == Some(true),
            events_stream: facts.events_stream == Some(true),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspaceState {
    Ready,
    Destroying,
    Destroyed,
    Expired,
    Unknown,
}

impl From<substrate_wire::WorkspaceState> for WorkspaceState {
    fn from(value: substrate_wire::WorkspaceState) -> Self {
        match value {
            substrate_wire::WorkspaceState::Ready => Self::Ready,
            substrate_wire::WorkspaceState::Destroying => Self::Destroying,
            substrate_wire::WorkspaceState::Destroyed => Self::Destroyed,
            substrate_wire::WorkspaceState::Expired => Self::Expired,
            substrate_wire::WorkspaceState::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Lease {
    pub ttl: Duration,
    pub renew_by: DateTime<Utc>,
    pub authorizing_operation: String,
}

impl From<substrate_wire::LeaseObservation> for Lease {
    fn from(value: substrate_wire::LeaseObservation) -> Self {
        Self {
            ttl: Duration::from_millis(value.ttl_ms),
            renew_by: value.renew_by,
            authorizing_operation: value.authorizing_operation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkspaceObservation {
    pub id: String,
    pub labels: BTreeMap<String, String>,
    pub observed_at: DateTime<Utc>,
    pub state: WorkspaceState,
    pub lease: Option<Lease>,
}

impl From<substrate_wire::Workspace> for WorkspaceObservation {
    fn from(value: substrate_wire::Workspace) -> Self {
        Self {
            id: value.id,
            labels: value.labels,
            observed_at: value.observed_at,
            state: value.state.into(),
            lease: value.lease.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileObservation {
    pub workspace: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub atomic_replacement: bool,
    pub observed_at: DateTime<Utc>,
}

impl From<substrate_wire::FileObservation> for FileObservation {
    fn from(value: substrate_wire::FileObservation) -> Self {
        Self {
            workspace: value.workspace,
            path: value.path,
            size: value.size,
            sha256: value.sha256,
            atomic_replacement: value.atomic_replacement,
            observed_at: value.observed_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileContents {
    pub workspace: String,
    pub path: String,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub eof: bool,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExecState {
    Accepted,
    Running,
    Exited,
    Cancelled,
    Expired,
    Unknown,
}

impl ExecState {
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Exited | Self::Cancelled | Self::Expired)
    }
}

impl From<substrate_wire::ExecState> for ExecState {
    fn from(value: substrate_wire::ExecState) -> Self {
        match value {
            substrate_wire::ExecState::Accepted => Self::Accepted,
            substrate_wire::ExecState::Running => Self::Running,
            substrate_wire::ExecState::Exited => Self::Exited,
            substrate_wire::ExecState::Cancelled => Self::Cancelled,
            substrate_wire::ExecState::Expired => Self::Expired,
            substrate_wire::ExecState::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExecExit {
    pub code: Option<u8>,
    pub signal: Option<Signal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExecObservation {
    pub id: String,
    pub workspace: String,
    pub state: ExecState,
    pub observed_at: DateTime<Utc>,
    pub requested: substrate_wire::ConfinementRequest,
    pub applied: Option<substrate_wire::AppliedConfinement>,
    pub usage: Option<substrate_wire::ExecUsage>,
    pub exit: Option<ExecExit>,
    pub lease: Option<Lease>,
    pub refusal: Option<ObservedRefusal>,
}

impl From<substrate_wire::Exec> for ExecObservation {
    fn from(value: substrate_wire::Exec) -> Self {
        Self {
            id: value.id,
            workspace: value.workspace,
            state: value.state.into(),
            observed_at: value.observed_at,
            requested: value.requested,
            applied: value.applied,
            usage: value.usage,
            exit: value.exit.map(|exit| ExecExit {
                code: exit.code,
                signal: exit.signal.map(Into::into),
            }),
            lease: value.lease.map(Into::into),
            refusal: value.refusal.map(|refusal| ObservedRefusal {
                class: refusal.class.into(),
                code: refusal.code,
                message: refusal.message,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunOutput {
    pub exec: ExecObservation,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PipeSessionState {
    Accepted,
    Ready,
    Attached,
    Exited,
    Cancelled,
    Expired,
    Unknown,
}

impl From<substrate_wire::SessionState> for PipeSessionState {
    fn from(value: substrate_wire::SessionState) -> Self {
        match value {
            substrate_wire::SessionState::Accepted => Self::Accepted,
            substrate_wire::SessionState::Ready => Self::Ready,
            substrate_wire::SessionState::Attached => Self::Attached,
            substrate_wire::SessionState::Exited => Self::Exited,
            substrate_wire::SessionState::Cancelled => Self::Cancelled,
            substrate_wire::SessionState::Expired => Self::Expired,
            substrate_wire::SessionState::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PipeSessionObservation {
    pub id: String,
    pub exec_id: String,
    pub workspace: String,
    pub state: PipeSessionState,
    pub observed_at: DateTime<Utc>,
    pub lease: Lease,
    pub input_limit_bytes: u64,
    pub frame_limit_bytes: u64,
    pub queued_frames: u32,
}

impl From<substrate_wire::PipeSession> for PipeSessionObservation {
    fn from(value: substrate_wire::PipeSession) -> Self {
        Self {
            id: value.id,
            exec_id: value.exec,
            workspace: value.workspace,
            state: value.state.into(),
            observed_at: value.observed_at,
            lease: value.lease.into(),
            input_limit_bytes: value.limits.input_bytes,
            frame_limit_bytes: value.limits.frame_bytes,
            queued_frames: value.limits.queued_frames,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PipeFrame {
    Output {
        sequence: u64,
        stream: OutputStream,
        bytes: Vec<u8>,
    },
    Truncated {
        sequence: u64,
        stream: OutputStream,
    },
    Exit {
        sequence: u64,
        state: ExecState,
        exit: Option<ExecExit>,
    },
    ProtocolError {
        sequence: u64,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl From<substrate_wire::OutputStream> for OutputStream {
    fn from(value: substrate_wire::OutputStream) -> Self {
        match value {
            substrate_wire::OutputStream::Stdout => Self::Stdout,
            substrate_wire::OutputStream::Stderr => Self::Stderr,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Interrupt,
    Terminate,
    Kill,
}

impl From<Signal> for substrate_wire::Signal {
    fn from(value: Signal) -> Self {
        match value {
            Signal::Interrupt => Self::Int,
            Signal::Terminate => Self::Term,
            Signal::Kill => Self::Kill,
        }
    }
}

impl From<substrate_wire::Signal> for Signal {
    fn from(value: substrate_wire::Signal) -> Self {
        match value {
            substrate_wire::Signal::Int => Self::Interrupt,
            substrate_wire::Signal::Term => Self::Terminate,
            substrate_wire::Signal::Kill => Self::Kill,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalClass {
    Refused,
    Conflict,
    Unserved,
    Exhausted,
    Failed,
}

impl From<substrate_wire::ErrorClass> for RefusalClass {
    fn from(value: substrate_wire::ErrorClass) -> Self {
        match value {
            substrate_wire::ErrorClass::Refused => Self::Refused,
            substrate_wire::ErrorClass::Conflict => Self::Conflict,
            substrate_wire::ErrorClass::Unserved => Self::Unserved,
            substrate_wire::ErrorClass::Exhausted => Self::Exhausted,
            substrate_wire::ErrorClass::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Refusal {
    pub class: RefusalClass,
    pub code: String,
    pub message: String,
    pub retriable: bool,
    pub address: Option<String>,
    pub operation_id: Option<String>,
}

impl From<substrate_wire::ErrorDetail> for Refusal {
    fn from(value: substrate_wire::ErrorDetail) -> Self {
        Self {
            class: value.class.into(),
            code: value.code,
            message: value.message,
            retriable: value.retriable,
            address: value.address,
            operation_id: value.operation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ObservedRefusal {
    pub class: RefusalClass,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Event {
    pub generation: u64,
    pub sequence: u64,
    pub resource: String,
    pub resource_kind: String,
    pub transition: String,
    pub observed_at: DateTime<Utc>,
    pub principal: Option<String>,
    pub operation_id: Option<String>,
    pub observation: Value,
}

impl From<substrate_wire::Event> for Event {
    fn from(value: substrate_wire::Event) -> Self {
        let operation_id = match value.cause {
            substrate_wire::EventCause::Operation { operation } => Some(operation),
            substrate_wire::EventCause::Control { .. } => None,
        };
        Self {
            generation: value.generation,
            sequence: value.seq,
            resource: value.resource,
            resource_kind: value.resource_kind,
            transition: value.transition,
            observed_at: value.observed_at,
            principal: value.principal,
            operation_id,
            observation: value.observation,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct EventPage {
    pub source_scope: String,
    pub generation: u64,
    pub events: Vec<Event>,
    pub next_cursor: String,
    pub through_sequence: u64,
    pub first_retained_sequence: Option<u64>,
}

impl From<substrate_wire::EventPage> for EventPage {
    fn from(value: substrate_wire::EventPage) -> Self {
        Self {
            source_scope: value.source_scope,
            generation: value.generation,
            events: value.items.into_iter().map(Into::into).collect(),
            next_cursor: value.next_cursor,
            through_sequence: value.through_seq,
            first_retained_sequence: value.first_retained_seq,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OperationState {
    Refused,
    Accepted,
    Unknown,
    Terminal,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Operation {
    pub id: String,
    pub kind: String,
    pub state: OperationState,
    pub resource: Option<String>,
    pub result: Option<Value>,
    pub refusal: Option<Refusal>,
}

impl From<substrate_wire::OperationRecord> for Operation {
    fn from(value: substrate_wire::OperationRecord) -> Self {
        let (result, refusal) = match value.outcome {
            Some(substrate_wire::OperationOutcome::Success { result }) => (Some(result), None),
            Some(substrate_wire::OperationOutcome::Error { error }) => (None, Some(error.into())),
            None => (None, None),
        };
        let state = match value.state {
            substrate_wire::OperationState::Refused => OperationState::Refused,
            substrate_wire::OperationState::Accepted => OperationState::Accepted,
            substrate_wire::OperationState::Unknown => OperationState::Unknown,
            substrate_wire::OperationState::Terminal => OperationState::Terminal,
        };
        Self {
            id: value.operation,
            kind: value.operation_kind,
            state,
            resource: value.resource,
            result,
            refusal,
        }
    }
}

/// Required bounds for one command or raw-pipe process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPolicy {
    pub(crate) timeout: Duration,
    pub(crate) cpu_time: Duration,
    pub(crate) memory_bytes: u64,
    pub(crate) processes: u32,
    pub(crate) output_bytes: u64,
}

impl ExecutionPolicy {
    pub fn builder() -> ExecutionPolicyBuilder {
        ExecutionPolicyBuilder::default()
    }

    pub(crate) fn wire(&self) -> Result<substrate_wire::ExecLimits, &'static str> {
        Ok(substrate_wire::ExecLimits {
            timeout_ms: u64::try_from(self.timeout.as_millis())
                .map_err(|_| "timeout exceeds the wire range")?,
            output_bytes: self.output_bytes,
            processes: self.processes,
            memory_bytes: self.memory_bytes,
            cpu_millis: u64::try_from(self.cpu_time.as_millis())
                .map_err(|_| "CPU time exceeds the wire range")?,
        })
    }
}

#[derive(Debug, Default)]
#[must_use]
pub struct ExecutionPolicyBuilder {
    timeout: Option<Duration>,
    cpu_time: Option<Duration>,
    memory_bytes: Option<u64>,
    processes: Option<u32>,
    output_bytes: Option<u64>,
}

impl ExecutionPolicyBuilder {
    pub fn timeout(mut self, value: Duration) -> Self {
        self.timeout = Some(value);
        self
    }

    pub fn cpu_time(mut self, value: Duration) -> Self {
        self.cpu_time = Some(value);
        self
    }

    pub fn memory_bytes(mut self, value: u64) -> Self {
        self.memory_bytes = Some(value);
        self
    }

    pub fn processes(mut self, value: u32) -> Self {
        self.processes = Some(value);
        self
    }

    pub fn output_bytes(mut self, value: u64) -> Self {
        self.output_bytes = Some(value);
        self
    }

    pub fn build(self) -> Result<ExecutionPolicy, crate::SdkError> {
        let policy = ExecutionPolicy {
            timeout: self
                .timeout
                .ok_or(crate::SdkError::Builder { field: "timeout" })?,
            cpu_time: self
                .cpu_time
                .ok_or(crate::SdkError::Builder { field: "cpu_time" })?,
            memory_bytes: self.memory_bytes.ok_or(crate::SdkError::Builder {
                field: "memory_bytes",
            })?,
            processes: self
                .processes
                .ok_or(crate::SdkError::Builder { field: "processes" })?,
            output_bytes: self.output_bytes.ok_or(crate::SdkError::Builder {
                field: "output_bytes",
            })?,
        };
        if policy.timeout.is_zero()
            || policy.cpu_time.is_zero()
            || policy.memory_bytes == 0
            || policy.processes == 0
            || policy.output_bytes == 0
        {
            return Err(crate::SdkError::Protocol(
                "execution-policy bounds must be nonzero".to_owned(),
            ));
        }
        policy
            .wire()
            .map_err(|message| crate::SdkError::Protocol(message.to_owned()))?;
        Ok(policy)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineEnvironment {
    Lang,
    Locale,
    Path,
    Terminal,
    TimeZone,
}

impl From<BaselineEnvironment> for substrate_wire::BaselineEnvironment {
    fn from(value: BaselineEnvironment) -> Self {
        match value {
            BaselineEnvironment::Lang => Self::Lang,
            BaselineEnvironment::Locale => Self::LcAll,
            BaselineEnvironment::Path => Self::Path,
            BaselineEnvironment::Terminal => Self::Term,
            BaselineEnvironment::TimeZone => Self::Tz,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct EventStreamFrame {
    pub kind: String,
    pub page: Option<substrate_wire::EventPage>,
    pub code: Option<String>,
    pub cursor: Option<String>,
}

#[cfg(feature = "linked-daemon")]
#[cfg(feature = "linked-daemon")]
#[derive(Debug, Serialize)]
pub(crate) struct LinkedChildConfig {
    pub socket: String,
    pub state: String,
    pub workspaces: String,
    pub deployment: String,
    pub uid: u32,
    pub cgroup_root: Option<String>,
    pub bubblewrap: String,
    pub event_retention: u64,
}

#[cfg(feature = "linked-daemon")]
impl<'de> Deserialize<'de> for LinkedChildConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Fields {
            socket: String,
            state: String,
            workspaces: String,
            deployment: String,
            uid: u32,
            cgroup_root: Option<String>,
            bubblewrap: String,
            event_retention: u64,
        }
        let fields = Fields::deserialize(deserializer)?;
        Ok(Self {
            socket: fields.socket,
            state: fields.state,
            workspaces: fields.workspaces,
            deployment: fields.deployment,
            uid: fields.uid,
            cgroup_root: fields.cgroup_root,
            bubblewrap: fields.bubblewrap,
            event_retention: fields.event_retention,
        })
    }
}
