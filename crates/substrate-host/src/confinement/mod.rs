//! Confinement as one contract served by declared adapters (atlas ADR 0109, ADR 0044).
//!
//! A client opens an adapter by name ([`open`]), reads what it proved ([`AdapterFacts`]: the
//! contract families it serves and the facts it proved at open), and has a
//! [`ConfinementProfile`] admitted against that proof. A profile that needs anything the adapter
//! lacks is refused by name, in the order of the domain `substrate.confinement`
//! (`spec/confinement/confinement.yaml`):
//!
//! 1. a reserved field (`confinement.reserved-field-unserved`, 501), then a deny-list beside an
//!    empty allow-list (`confinement.exec-denied-paths-without-subpaths`, 422) or over its bound
//!    (`confinement.exec-denied-paths-exceeded`, 422): both read the profile alone (design 35
//!    D13, D10);
//! 2. an adapter that did not open, an `inner` it does not wrap, or no `inner` on an adapter opened
//!    around one (`confinement.adapter-unserved`, 501);
//! 3. on a declared-insecure adapter, a missing or blank reason (`confinement.local-reason-missing`,
//!    422);
//! 4. on a digest-pinning adapter, an image not named by digest (`confinement.image-unpinned`, 422);
//! 5. on a declared-insecure adapter the host's policy forbids, anything
//!    (`confinement.local-refused`, 403);
//! 6. the first needed family not served, in [`FAMILY_ORDER`] (`confinement.family-unserved`, 501);
//! 7. the first required fact not proved, in [`FACT_ORDER`] (`confinement.fact-unserved`, 501).
//!
//! A start or render decides the profile again against the opened adapter's own proof, so a
//! profile admitted against a copied or altered proof is refused there by the same names.
//!
//! Every admission and every refusal, at admission or at start, writes one record to the
//! profile's [`AuditSink`]. Nothing
//! falls back to another adapter, and nothing degrades (invariant 3). This is substrate-host's
//! API, not the wire: `HostDriverKind` and every contract bundle are unchanged (ADR 0044
//! decision 7).
//!
//! The registry names all six adapters ([`ADAPTERS`]). Each lives in its own file with its own
//! request type. Each opens only where its mechanism is present and otherwise answers
//! `confinement.adapter-unserved`: `macos` on macOS with Seatbelt, `systemd` on Linux through the
//! user's systemd manager around `local` or `linux`, `oci` with a Docker or podman runtime that
//! answers, and `pod` with an operator's cluster configuration whose cluster answers.

pub mod facts;
pub mod linux;
pub(crate) mod live;
pub mod local;
pub mod macos;
pub mod oci;
pub mod pod;
pub mod profile;
mod stdin;
pub mod systemd;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;

pub use crate::confinement_types::{AdapterName, ContractFamily, Fact};
// The types the module's own public signatures name, so a consumer builds every value it passes
// from this module alone: `LinuxRequest::new` and `OciRequest` take the host's configuration and
// declared roots, `ConfinementProfile` holds the wire's root, file and slot types, and
// `ConfinementAdapter::signal` takes a `Signal`.
pub use crate::{DeclaredWritableRoot, HostConfig};
// Which bytes of each output stream a run keeps (design 29 § 5).
pub use crate::output_retention::{KeptOutput, OutputRetention, Retention};
pub use facts::{
    AdapterFacts, FACT_ORDER, FAMILY_ORDER, FactWithheld, InnerAdapter, ReservedField,
};
pub use profile::{
    ConfinementProfile, IoBounds, Ipc, Network, OutputDelivery, ProcessBounds, StdinMode,
    image_is_pinned,
};
// A run's standard input (design 32 § 2).
pub use stdin::{MAX_STDIN_BYTES, STDIN_CHUNK_BYTES, run_with_stdin};
// A run's output delivered while it runs (design 32 § 3). `PipeStream` is the host's own type;
// the wire's `OutputStream` is a different one.
pub use crate::observation::PipeStream;
pub use live::{LIVE_OUTPUT_RETENTION_INVALID, LIVE_OUTPUT_UNREAD, MAX_QUEUED_CHUNKS, OutputEvent};
pub use substrate_wire::{ReadOnlyFile, ReadOnlyRoot, SecretSlotRequest, Signal, WritableHostRoot};
// What a run may write inside its workspace: the wire's own type, so one validator answers it
// (design 29 § 1).
pub use substrate_wire::WorkspaceAccess;

use crate::{DriverError, DriverErrorClass};

pub const ADAPTER_UNSERVED: &str = "confinement.adapter-unserved";
pub const FAMILY_UNSERVED: &str = "confinement.family-unserved";
pub const FACT_UNSERVED: &str = "confinement.fact-unserved";
pub const LOCAL_REFUSED: &str = "confinement.local-refused";
pub const LOCAL_REASON_MISSING: &str = "confinement.local-reason-missing";
pub const IMAGE_UNPINNED: &str = "confinement.image-unpinned";
pub const RESERVED_FIELD_UNSERVED: &str = "confinement.reserved-field-unserved";
/// A profile sets `exec_denied_paths` beside an empty `exec_subpaths` (design 35 D13).
pub const EXEC_DENIED_PATHS_WITHOUT_SUBPATHS: &str =
    "confinement.exec-denied-paths-without-subpaths";
/// A profile sets more than [`MAX_EXEC_DENIED_PATHS`] deny entries (design 35 D10).
pub const EXEC_DENIED_PATHS_EXCEEDED: &str = "confinement.exec-denied-paths-exceeded";
/// The most entries `ConfinementProfile.exec_denied_paths` holds (design 35 D10). It bounds
/// the size of a profile and of the helper's argument list, not a start's cost, which
/// follows the size of the directories that hold a denied path.
pub const MAX_EXEC_DENIED_PATHS: usize = 64;
/// A start whose profile names no workspace: a run needs a directory to start in.
pub const WORKSPACE_REQUIRED: &str = "confinement.workspace-required";
/// A `render` the adapter cannot reproduce faithfully without acquiring run-time material.
pub const RENDER_UNSERVED: &str = "confinement.render-unserved";
/// A writable workspace subtree that is `.git` or lies beneath one, compared ASCII
/// case-insensitively (design 29 § 1): 422, class `Refused`.
pub const WRITABLE_SUBTREE_GIT: &str = "confinement.writable-subtree-git";
/// A tail retention on an adapter that bounds a read of its output only from the start (design
/// 29 § 6): 501, class `Unserved`.
pub const OUTPUT_RETENTION_UNSERVED: &str = "confinement.output-retention-unserved";

/// The registry: every adapter ADR 0109 decision 3 names, in that order.
pub const ADAPTERS: [AdapterName; 6] = [
    AdapterName::Local,
    AdapterName::Linux,
    AdapterName::Macos,
    AdapterName::Systemd,
    AdapterName::Oci,
    AdapterName::Pod,
];

impl AdapterName {
    /// The adapter's name as `spec/confinement` spells it. The client wire never carries it
    /// (design 30 § 3, ADR 0048 D1); the operational listener's `substrate_adapter_info` label
    /// does (D7).
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Local => "local",
            Self::Macos => "macos",
            Self::Oci => "oci",
            Self::Pod => "pod",
            Self::Systemd => "systemd",
        }
    }
}

/// What `open` is asked for. Each request type lives beside its adapter.
#[derive(Debug, Clone)]
pub enum AdapterRequest {
    Local(local::LocalRequest),
    Linux(Box<linux::LinuxRequest>),
    Macos(macos::MacosRequest),
    Systemd(systemd::SystemdRequest),
    Oci(oci::OciRequest),
    Pod(pod::PodRequest),
}

impl AdapterRequest {
    pub const fn adapter(&self) -> AdapterName {
        match self {
            Self::Local(_) => AdapterName::Local,
            Self::Linux(_) => AdapterName::Linux,
            Self::Macos(_) => AdapterName::Macos,
            Self::Systemd(_) => AdapterName::Systemd,
            Self::Oci(_) => AdapterName::Oci,
            Self::Pod(_) => AdapterName::Pod,
        }
    }
}

/// Opens the adapter named by `request`, which proves its families and facts now.
///
/// # Errors
///
/// `confinement.adapter-unserved` when the adapter cannot open on this host or platform;
/// `confinement.fact-unserved` when its proof lacks a fact the adapter itself must prove
/// ([`check_open`]); a host failure the adapter met while opening, as
/// [`ConfinementError::Driver`].
pub fn open(request: AdapterRequest) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    let adapter = match request {
        AdapterRequest::Local(request) => Ok(local::open(request)),
        AdapterRequest::Linux(request) => linux::open(*request),
        AdapterRequest::Macos(request) => macos::open(request),
        AdapterRequest::Systemd(request) => systemd::open(request),
        AdapterRequest::Oci(request) => oci::open(request),
        AdapterRequest::Pod(request) => pod::open(request),
    }?;
    check_open(adapter.facts())?;
    Ok(adapter)
}

/// The facts an adapter must prove to open at all, checked on every proof [`open`] returns.
///
/// `local`, and `systemd` around it, is always declared insecure (ADR 0044 decision 3, atlas ADR
/// 0109 decision 4), so no `local` adapter exists that admission could take for a confined one.
/// `oci` and `pod` run only an image named by digest, so admission's digest check holds on every
/// one of them. A proof without its fact is refused by that fact's name, never opened.
///
/// # Errors
///
/// `confinement.fact-unserved` naming `declared_insecure` or `image_digest_pinned`.
pub fn check_open(facts: &AdapterFacts) -> Result<(), ConfinementError> {
    let required = match (&facts.adapter, facts.inner) {
        (AdapterName::Local, _) | (AdapterName::Systemd, Some(InnerAdapter::Local)) => {
            Some(Fact::DeclaredInsecure)
        }
        (AdapterName::Oci | AdapterName::Pod, _) => Some(Fact::ImageDigestPinned),
        _ => None,
    };
    match required {
        Some(fact) if !facts.proves(&fact) => Err(ConfinementError::FactUnserved {
            adapter: facts.adapter.clone(),
            fact,
            withheld: None,
        }),
        _ => Ok(()),
    }
}

/// The contract's answer to a profile addressed to an adapter that did not open: a reserved field
/// first, then the shape of the deny-list, then `confinement.adapter-unserved`. It is audited
/// like every refusal.
pub fn admit_unopened(adapter: &AdapterName, profile: &ConfinementProfile) -> ConfinementError {
    let error = reserved(profile)
        .and_then(|()| denied_paths_shape(adapter, profile))
        .err()
        .unwrap_or(ConfinementError::AdapterUnserved {
            adapter: adapter.clone(),
            detail: "the adapter did not open on this host".to_owned(),
        });
    audit(profile, &AuditRecord::refused(adapter, None, &error));
    error
}

/// What the text of `confinement.fact-unserved` adds for a fact the open withheld (design 35
/// D27): the cause, the path, the configuration field it belongs to and the file-system type,
/// so an operator learns why the lists are not served from the refusal itself. Empty otherwise.
fn withheld_reason(withheld: Option<&FactWithheld>) -> String {
    let Some(entry) = withheld else {
        return String::new();
    };
    let FactWithheld {
        cause,
        address,
        path,
        ..
    } = entry;
    match &entry.file_system {
        Some(file_system) => {
            let (last, listed) = substrate_exec_gate::place::LISTED_FILE_SYSTEMS
                .split_last()
                .expect("the list of file-system types is not empty");
            format!(
                ": {cause}: {path} ({address}) is on {file_system}; the lists are served on {} \
                 and {last}",
                listed.join(", ")
            )
        }
        None => format!(": {cause}: the kernel reports no mount ID for {path} ({address})"),
    }
}

/// A named refusal of the confinement contract, or the host's own refusal met while running.
#[derive(Debug, thiserror::Error)]
pub enum ConfinementError {
    #[error("confinement.adapter-unserved: {adapter:?}: {detail}")]
    AdapterUnserved {
        adapter: AdapterName,
        detail: String,
    },
    #[error("confinement.family-unserved: {adapter:?} does not serve {family:?}")]
    FamilyUnserved {
        adapter: AdapterName,
        family: ContractFamily,
    },
    #[error(
        "confinement.fact-unserved: {adapter:?} did not prove {fact:?}{}",
        withheld_reason(withheld.as_ref())
    )]
    FactUnserved {
        adapter: AdapterName,
        fact: Fact,
        /// Why the adapter withheld the fact at its open, where it did (design 35 D27): the
        /// proof's first entry for the fact, which the text repeats. `None` for a fact that
        /// is absent for any other reason.
        withheld: Option<FactWithheld>,
    },
    #[error("confinement.local-refused: the host's policy forbids {adapter:?}")]
    LocalRefused { adapter: AdapterName },
    #[error(
        "confinement.local-reason-missing: {adapter:?} runs declared insecure and needs a reason"
    )]
    LocalReasonMissing { adapter: AdapterName },
    #[error("confinement.image-unpinned: {adapter:?} runs an image by sha256 digest only")]
    ImageUnpinned { adapter: AdapterName },
    #[error("confinement.reserved-field-unserved: {field:?} is reserved")]
    ReservedFieldUnserved { field: ReservedField },
    #[error(
        "confinement.exec-denied-paths-without-subpaths: the profile for {adapter:?} denies \
         executables and lists none; a deny-list holds only beside a non-empty exec_subpaths"
    )]
    ExecDeniedPathsWithoutSubpaths { adapter: AdapterName },
    #[error(
        "confinement.exec-denied-paths-exceeded: the profile for {adapter:?} denies more than \
         {MAX_EXEC_DENIED_PATHS} paths"
    )]
    ExecDeniedPathsExceeded { adapter: AdapterName },
    #[error(transparent)]
    Driver(#[from] DriverError),
}

impl ConfinementError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::AdapterUnserved { .. } => ADAPTER_UNSERVED,
            Self::FamilyUnserved { .. } => FAMILY_UNSERVED,
            Self::FactUnserved { .. } => FACT_UNSERVED,
            Self::LocalRefused { .. } => LOCAL_REFUSED,
            Self::LocalReasonMissing { .. } => LOCAL_REASON_MISSING,
            Self::ImageUnpinned { .. } => IMAGE_UNPINNED,
            Self::ReservedFieldUnserved { .. } => RESERVED_FIELD_UNSERVED,
            Self::ExecDeniedPathsWithoutSubpaths { .. } => EXEC_DENIED_PATHS_WITHOUT_SUBPATHS,
            Self::ExecDeniedPathsExceeded { .. } => EXEC_DENIED_PATHS_EXCEEDED,
            Self::Driver(error) => error.code,
        }
    }

    /// The HTTP-style status the model gives the code.
    pub fn status(&self) -> u16 {
        match self {
            Self::LocalRefused { .. } => 403,
            Self::LocalReasonMissing { .. }
            | Self::ImageUnpinned { .. }
            | Self::ExecDeniedPathsWithoutSubpaths { .. }
            | Self::ExecDeniedPathsExceeded { .. } => 422,
            Self::AdapterUnserved { .. }
            | Self::FamilyUnserved { .. }
            | Self::FactUnserved { .. }
            | Self::ReservedFieldUnserved { .. } => 501,
            Self::Driver(error) => match error.class {
                DriverErrorClass::Refused => 422,
                DriverErrorClass::Conflict => 409,
                DriverErrorClass::Unserved => 501,
                DriverErrorClass::Exhausted => 429,
                DriverErrorClass::NotFound => 404,
                DriverErrorClass::Failed => 500,
            },
        }
    }
}

/// One audit record (ADR 0109 decision 2): an admission, a refusal, a start or an end, or a
/// private `TMPDIR` a run left behind.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuditRecord {
    ProfileAdmitted {
        profile: ProfileId,
        adapter: AdapterName,
        #[serde(skip_serializing_if = "Option::is_none")]
        inner: Option<InnerAdapter>,
        families: Vec<ContractFamily>,
        facts: Vec<Fact>,
        declared_insecure: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        local_reason: Option<String>,
    },
    ProfileRefused {
        adapter: AdapterName,
        #[serde(skip_serializing_if = "Option::is_none")]
        inner: Option<InnerAdapter>,
        code: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        family: Option<ContractFamily>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fact: Option<Fact>,
        #[serde(skip_serializing_if = "Option::is_none")]
        field: Option<ReservedField>,
    },
    RunStarted {
        run: RunId,
        profile: ProfileId,
        adapter: AdapterName,
        #[serde(skip_serializing_if = "Option::is_none")]
        inner: Option<InnerAdapter>,
        /// The reason, when the run is declared insecure.
        #[serde(skip_serializing_if = "Option::is_none")]
        declared_insecure: Option<String>,
        program: String,
    },
    /// A run's private `TMPDIR` that could not be removed after the run ended: it is still on the
    /// host at `path`.
    PrivateTmpdirLeft {
        run: RunId,
        profile: ProfileId,
        adapter: AdapterName,
        path: String,
        error: String,
    },
    /// A run's end (design 36 D15): what the adapter's observation of the ended run says, with
    /// the profile's id. Written once for every run that received `run_started`, also for one
    /// that ended `unknown`, and never while the run runs (D16, D18, D24).
    #[non_exhaustive]
    RunEnded {
        run: RunId,
        profile: ProfileId,
        adapter: AdapterName,
        #[serde(skip_serializing_if = "Option::is_none")]
        inner: Option<InnerAdapter>,
        /// The reason of a declared-insecure run, as `run_started` carries it.
        #[serde(skip_serializing_if = "Option::is_none")]
        declared_insecure: Option<String>,
        /// `ended` or `unknown`. Never `running`.
        state: RunState,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit: Option<RunExit>,
        /// The limit that ended the run, where the adapter observed one; absent is "none named".
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<RunLimit>,
        /// Absent where the adapter does not observe it, as on the observation.
        #[serde(skip_serializing_if = "Option::is_none")]
        memory_oom_kills: Option<u64>,
        /// Process creations the run's process bound refused; absent where the adapter does not
        /// read it, as on the observation (design 36 § 3).
        #[serde(skip_serializing_if = "Option::is_none")]
        process_limit_hits: Option<u64>,
    },
}

impl AuditRecord {
    fn refused(
        adapter: &AdapterName,
        inner: Option<InnerAdapter>,
        error: &ConfinementError,
    ) -> Self {
        let (family, fact, field) = match error {
            ConfinementError::FamilyUnserved { family, .. } => (Some(family.clone()), None, None),
            ConfinementError::FactUnserved { fact, .. } => (None, Some(fact.clone()), None),
            ConfinementError::ReservedFieldUnserved { field } => (None, None, Some(*field)),
            _ => (None, None, None),
        };
        Self::ProfileRefused {
            adapter: adapter.clone(),
            inner,
            code: error.code(),
            family,
            fact,
            field,
        }
    }
}

/// Where a profile's audit records go. Called synchronously, once per record, on the thread that
/// admitted or started (for `private_tmpdir_left`, on the task that ended the run); a sink that
/// blocks blocks that call.
///
/// `run_ended` is called on a thread of its own, named `substrate-run-ended`, after the run's end
/// was published, so it may arrive after `wait` returned, and a sink that blocks or panics on it
/// holds that thread only, never `wait`, another run or another record (design 36 D20). A panic
/// is contained where the consumer is built with `panic = "unwind"`; with `panic = "abort"` a
/// sink that panics ends the process, on this record as on every other. The thread enters the
/// runtime that was current when the run's end was handed to it, so a sink that reaches the
/// runtime on `run_started` reaches it on `run_ended`; a sink that spawns onto that runtime once
/// it shut down gets Tokio's behaviour for that, a task dropped unrun, and the record itself is
/// still written. Where the system refuses the thread, the call goes to the runtime's blocking
/// pool, or, where no runtime is current or its pool is shutting down, to the thread that hands
/// the record over. The run's end task ends once the call returned; a runtime that drops the task
/// before that waits for the call, so `run_ended` is in the sink when the runtime's drop
/// returned, and a sink that never returns holds that drop. Where the run's end task is dropped
/// before the run ended, as when its runtime shuts down, the record is written with
/// `state: unknown` on the thread that drops the task, and not from a thread that is panicking
/// (D22).
///
/// A sink is a synchronous call. It may hand work to the runtime it finds current (a spawn, a
/// channel send) and must not wait on that runtime (`Handle::block_on`, a blocking wait for a
/// task, a timer or I/O of it): on `run_ended`, once nothing drives that runtime (on a
/// current-thread runtime once its `block_on` returned, or once its drop began) nothing drives
/// that wait either, so the sink does not return and the runtime's drop does not return. The
/// other records are called on the caller's thread, which is inside the runtime where the caller
/// is async; there Tokio panics on `Handle::block_on`, and on a current-thread runtime only
/// `Runtime::block_on` drives its timers and I/O.
#[derive(Clone)]
pub struct AuditSink(Arc<dyn Fn(&AuditRecord) + Send + Sync>);

impl AuditSink {
    pub fn new(sink: impl Fn(&AuditRecord) + Send + Sync + 'static) -> Self {
        Self(Arc::new(sink))
    }

    pub fn record(&self, record: &AuditRecord) {
        (self.0)(record);
    }
}

impl fmt::Debug for AuditSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuditSink")
    }
}

fn audit(profile: &ConfinementProfile, record: &AuditRecord) {
    if let Some(sink) = &profile.audit {
        sink.record(record);
    }
}

/// substrate-host mints one per admitted profile: a UUID (version 7, from a ULID's time and
/// randomness). It is not a wire identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ProfileId(String);

impl ProfileId {
    fn mint() -> Self {
        let mut bytes = ulid::Ulid::generate().to_bytes();
        bytes[6] = (bytes[6] & 0x0f) | 0x70;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = hex::encode(bytes);
        Self(format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A profile one opened adapter admitted. Only admission makes one, and only the adapter that
/// admitted it starts it.
#[derive(Debug, Clone)]
pub struct AdmittedProfile {
    id: ProfileId,
    instance: u64,
    adapter: AdapterName,
    inner: Option<InnerAdapter>,
    families: Vec<ContractFamily>,
    facts: Vec<Fact>,
    declared_insecure: bool,
    profile: ConfinementProfile,
}

impl AdmittedProfile {
    pub fn id(&self) -> &ProfileId {
        &self.id
    }

    pub fn adapter(&self) -> &AdapterName {
        &self.adapter
    }

    pub fn inner(&self) -> Option<InnerAdapter> {
        self.inner
    }

    pub fn declared_insecure(&self) -> bool {
        self.declared_insecure
    }

    /// The reason a declared-insecure run carries in every observation.
    pub fn insecure_reason(&self) -> Option<&str> {
        self.declared_insecure
            .then(|| self.profile.reason())
            .flatten()
    }

    pub fn profile(&self) -> &ConfinementProfile {
        &self.profile
    }

    /// The admission's audit record.
    pub fn record(&self) -> AuditRecord {
        AuditRecord::ProfileAdmitted {
            profile: self.id.clone(),
            adapter: self.adapter.clone(),
            inner: self.inner,
            families: self.families.clone(),
            facts: self.facts.clone(),
            declared_insecure: self.declared_insecure,
            local_reason: self.profile.local_reason.clone(),
        }
    }

    /// Refuses a start or render on an adapter other than the one that admitted this profile, a
    /// profile the contract order refuses against this adapter's own proof, and a start on an
    /// adapter that does not serve `process_run`.
    ///
    /// The instance alone does not prove the admission was made against this proof: an
    /// [`AdapterFacts`] is `Clone` with public members, so a copy carrying the same instance can
    /// claim a fact, such as `local_permitted`, the opened adapter never proved. The profile is
    /// therefore decided again against `facts`, the adapter's own, and refused by that decision's
    /// name.
    pub(crate) fn check_on(
        &self,
        facts: &AdapterFacts,
        running: bool,
    ) -> Result<(), ConfinementError> {
        if self.instance != facts.instance() {
            return Err(ConfinementError::AdapterUnserved {
                adapter: facts.adapter.clone(),
                detail: "the profile was admitted by another adapter".to_owned(),
            });
        }
        decide(facts, &self.profile)?;
        if running && !facts.serves(&ContractFamily::ProcessRun) {
            return Err(ConfinementError::FamilyUnserved {
                adapter: facts.adapter.clone(),
                family: ContractFamily::ProcessRun,
            });
        }
        Ok(())
    }

    /// Writes the refusal record of a start `facts`' adapter refused after admission, whatever
    /// refused it: the contract, the adapter or the host. Every adapter's `start` passes its
    /// outcome through here, so no refusal leaves without a record (ADR 0109 decision 2).
    pub(crate) fn audit_refusal<T>(
        &self,
        facts: &AdapterFacts,
        outcome: Result<T, ConfinementError>,
    ) -> Result<T, ConfinementError> {
        if let Err(error) = &outcome {
            audit(
                &self.profile,
                &AuditRecord::refused(&facts.adapter, facts.inner, error),
            );
        }
        outcome
    }

    /// Writes a start's audit record, and returns the one value that writes the run's
    /// `run_ended` (design 36 D16): inert where the profile has no sink (D19).
    pub(crate) fn audit_start(&self, run: &RunId, program: &str) -> RunEndRecord {
        let declared_insecure = self.insecure_reason().map(str::to_owned);
        audit(
            &self.profile,
            &AuditRecord::RunStarted {
                run: run.clone(),
                profile: self.id.clone(),
                adapter: self.adapter.clone(),
                inner: self.inner,
                declared_insecure: declared_insecure.clone(),
                program: program.to_owned(),
            },
        );
        RunEndRecord(self.profile.audit.clone().map(|sink| PendingEnd {
            sink,
            run: run.clone(),
            profile: self.id.clone(),
            adapter: self.adapter.clone(),
            inner: self.inner,
            declared_insecure,
        }))
    }

    /// Writes the record of a private `TMPDIR` a run left behind because its removal failed.
    pub(crate) fn audit_private_tmpdir_left(
        &self,
        run: &RunId,
        path: &std::path::Path,
        error: &std::io::Error,
    ) {
        audit(
            &self.profile,
            &AuditRecord::PrivateTmpdirLeft {
                run: run.clone(),
                profile: self.id.clone(),
                adapter: self.adapter.clone(),
                path: path.display().to_string(),
                error: error.to_string(),
            },
        );
    }
}

/// The one `run_ended` record of a started run (design 36 D16). Only
/// [`AdmittedProfile::audit_start`] makes one, it is not `Clone`, and [`Self::write`] consumes
/// it, so a run that received no `run_started` gets no `run_ended` and no run gets two.
///
/// It owns what the record needs, the sink among it, so the run's end task writes it whether or
/// not the adapter still exists (D21). Dropped unwritten, it writes `run_ended` with
/// `state: unknown` and nothing else known, synchronously on the dropping thread, unless that
/// thread is panicking (D22). Without a sink it is inert (D19).
#[must_use = "the run's end task writes its run_ended (design 36 D17)"]
pub(crate) struct RunEndRecord(Option<PendingEnd>);

/// What a [`RunEndRecord`] with a sink holds until it is written.
struct PendingEnd {
    sink: AuditSink,
    run: RunId,
    profile: ProfileId,
    adapter: AdapterName,
    inner: Option<InnerAdapter>,
    declared_insecure: Option<String>,
}

impl PendingEnd {
    /// The record of the ended `observation`; a state of `running`, which no end task passes,
    /// is written `unknown` (D15: never `running`).
    fn record(&self, observation: RunObservation) -> AuditRecord {
        let RunObservation {
            adapter,
            inner,
            declared_insecure,
            state,
            exit,
            limit,
            memory_oom_kills,
            process_limit_hits,
            ..
        } = observation;
        AuditRecord::RunEnded {
            run: self.run.clone(),
            profile: self.profile.clone(),
            adapter,
            inner,
            declared_insecure,
            state: if state == RunState::Running {
                RunState::Unknown
            } else {
                state
            },
            exit,
            limit,
            memory_oom_kills,
            process_limit_hits,
        }
    }

    /// D22: the adapter lost sight of the run.
    fn unknown(&self) -> AuditRecord {
        AuditRecord::RunEnded {
            run: self.run.clone(),
            profile: self.profile.clone(),
            adapter: self.adapter.clone(),
            inner: self.inner,
            declared_insecure: self.declared_insecure.clone(),
            state: RunState::Unknown,
            exit: None,
            limit: None,
            memory_oom_kills: None,
            process_limit_hits: None,
        }
    }
}

/// Calls `sink` with `record`, and a sink that panics does not unwind into the caller.
fn record_contained(sink: &AuditSink, record: &AuditRecord) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink.record(record)));
}

/// A built `run_ended` on its way to the sink: written once, when it is dropped, on whichever
/// thread drops it, so a job dropped unrun, as by a runtime's shutdown, still writes it there. A
/// sink that panics is contained, and nothing is written from a thread that is panicking (D22).
/// Dropping `written` afterwards tells the run's end task that the write is over.
struct EndWrite {
    sink: AuditSink,
    record: AuditRecord,
    written: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for EndWrite {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            record_contained(&self.sink, &self.record);
        }
    }
}

/// What the run's end task holds while its `run_ended` is written (D20, amended 2026-11-24).
/// [`Self::finished`] waits on a channel until the write is over, holding no thread. Dropped
/// before that, as when the runtime drops the task at its shutdown, it joins the record's thread
/// on the dropping thread, so the runtime's drop returns once the record is in the sink. From a
/// thread that is panicking it does not join: the record's thread still writes while the process
/// lives, and a panic is never held by a sink.
#[must_use = "the end task awaits the write (design 36 D20)"]
pub(crate) struct EndWriting {
    thread: Option<std::thread::JoinHandle<()>>,
    done: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl EndWriting {
    const NONE: Self = Self {
        thread: None,
        done: None,
    };

    /// Ends once the record's write is over: written, contained or abandoned by a panicking
    /// thread.
    pub(crate) async fn finished(mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.await;
        }
        // The write is over; the thread ends by itself.
        self.thread = None;
    }
}

impl Drop for EndWriting {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take()
            && !std::thread::panicking()
        {
            let _ = thread.join();
        }
    }
}

/// The name of the thread that calls a sink with one `run_ended` (D20).
const RUN_ENDED_THREAD: &str = "substrate-run-ended";

/// A job that writes one `run_ended`, as handed to a thread.
type EndJob = Box<dyn FnOnce() + Send>;

/// Calls the sink with `write` on a thread of its own, so a sink that blocks or panics shares no
/// thread with any run's exit detection, `wait` or other record (D20, amended 2026-11-24).
fn deliver(write: EndWrite) -> EndWriting {
    deliver_with(write, |job| {
        std::thread::Builder::new()
            .name(RUN_ENDED_THREAD.to_owned())
            .spawn(job)
    })
}

/// [`deliver`] with the thread's creation passed in. The thread enters the runtime current at
/// the call, where there is one, so a sink reaches the runtime on `run_ended` as it does on
/// `run_started`. Where the system refuses the thread, the record is not lost: it goes to the
/// current runtime's blocking pool, and where no runtime is current, or that pool is shutting
/// down and drops the job, it is written on the calling thread before the call returns. On every
/// path it is written once and a sink that panics is contained.
fn deliver_with(
    mut write: EndWrite,
    spawn_thread: impl FnOnce(EndJob) -> std::io::Result<std::thread::JoinHandle<()>>,
) -> EndWriting {
    let (written, done) = tokio::sync::oneshot::channel();
    write.written = Some(written);
    let runtime = tokio::runtime::Handle::try_current().ok();
    let slot = Arc::new(parking_lot::Mutex::new(Some(write)));
    let job = take_and_write(&slot);
    let entered = runtime.clone();
    let error = match spawn_thread(Box::new(move || {
        let _entered = entered.as_ref().map(tokio::runtime::Handle::enter);
        job();
    })) {
        Ok(thread) => {
            return EndWriting {
                thread: Some(thread),
                done: Some(done),
            };
        }
        Err(error) => error,
    };
    tracing::warn!(
        %error,
        "the system refused a thread for an audit run_ended record; it is written on the \
         runtime's blocking pool, or on this thread where that pool does not take it"
    );
    if let Some(runtime) = runtime {
        let job = take_and_write(&slot);
        // A pool that is shutting down drops the job unrun, and the drop below writes. A pool
        // that cannot start a thread keeps the job queued and panics; the panic is contained,
        // and a thread of the pool writes the record once one is free, or the runtime's
        // shutdown drops the job and writes it on that thread.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(runtime.spawn_blocking(job));
        }));
    }
    // Written here only where no job holds the slot any more.
    drop(slot);
    EndWriting {
        thread: None,
        done: Some(done),
    }
}

/// A job that takes the write out of `slot` and writes it, outside the lock; a job that never
/// runs leaves it in `slot` for the next place.
fn take_and_write(slot: &Arc<parking_lot::Mutex<Option<EndWrite>>>) -> EndJob {
    let slot = Arc::clone(slot);
    Box::new(move || {
        let write = slot.lock().take();
        drop(write);
    })
}

impl RunEndRecord {
    /// Whether the profile had no sink, so nothing will be written (D19).
    pub(crate) fn is_inert(&self) -> bool {
        self.0.is_none()
    }

    /// Writes the run's `run_ended` from `observation`, the adapter's own observation of the
    /// ended run, on a thread of its own ([`deliver`], D20), with or without a current runtime,
    /// and returns what the end task awaits until the write is over. Without a sink it starts no
    /// thread (D19).
    pub(crate) fn write(mut self, observation: RunObservation) -> EndWriting {
        let Some(pending) = self.0.take() else {
            return EndWriting::NONE;
        };
        deliver(EndWrite {
            record: pending.record(observation),
            sink: pending.sink,
            written: None,
        })
    }
}

impl Drop for RunEndRecord {
    fn drop(&mut self) {
        if let Some(pending) = self.0.take()
            && !std::thread::panicking()
        {
            record_contained(&pending.sink, &pending.unknown());
        }
    }
}

/// The run's end task's half of design 36 D17: once the end is published, it takes the value
/// `audit_start` made from `receiver`, writes the record of the ended `observation` and ends
/// once the write is over (D20). A start that failed after the task was spawned dropped the
/// sender, and nothing is written.
pub(crate) async fn write_run_end(
    receiver: tokio::sync::oneshot::Receiver<RunEndRecord>,
    observation: RunObservation,
) {
    if let Ok(record) = receiver.await {
        record.write(observation).finished().await;
    }
}

static INSTANCES: AtomicU64 = AtomicU64::new(1);

/// A fresh identity for one opened adapter, carried by its [`AdapterFacts`].
pub(crate) fn next_instance() -> u64 {
    INSTANCES.fetch_add(1, Ordering::Relaxed)
}

fn reserved(profile: &ConfinementProfile) -> Result<(), ConfinementError> {
    if profile.ingress_routes.is_some() {
        return Err(ConfinementError::ReservedFieldUnserved {
            field: ReservedField::IngressRoutes,
        });
    }
    if profile.rate_bounds.is_some() {
        return Err(ConfinementError::ReservedFieldUnserved {
            field: ReservedField::RateBounds,
        });
    }
    Ok(())
}

/// Design 35 D13 and D10: a deny-list beside an empty allow-list, and one over its bound. Both
/// read the profile's two lists and no adapter fact, so every adapter answers them alike, before
/// `confinement.fact-unserved`.
fn denied_paths_shape(
    adapter: &AdapterName,
    profile: &ConfinementProfile,
) -> Result<(), ConfinementError> {
    if !profile.exec_denied_paths.is_empty() && profile.exec_subpaths.is_empty() {
        return Err(ConfinementError::ExecDeniedPathsWithoutSubpaths {
            adapter: adapter.clone(),
        });
    }
    if profile.exec_denied_paths.len() > MAX_EXEC_DENIED_PATHS {
        return Err(ConfinementError::ExecDeniedPathsExceeded {
            adapter: adapter.clone(),
        });
    }
    Ok(())
}

/// The contract's admission order against an opened adapter's proof, then the adapter's own
/// `check`, with exactly one audit record either way.
fn admission(
    facts: &AdapterFacts,
    profile: &ConfinementProfile,
    check: impl FnOnce(&ConfinementProfile) -> Result<(), ConfinementError>,
) -> Result<AdmittedProfile, ConfinementError> {
    let decided = decide(facts, profile).and_then(|admitted| {
        check(profile)?;
        Ok(admitted)
    });
    match &decided {
        Ok(admitted) => audit(profile, &admitted.record()),
        Err(error) => audit(
            profile,
            &AuditRecord::refused(&facts.adapter, facts.inner, error),
        ),
    }
    decided
}

fn decide(
    facts: &AdapterFacts,
    profile: &ConfinementProfile,
) -> Result<AdmittedProfile, ConfinementError> {
    let adapter = || facts.adapter.clone();
    reserved(profile)?;
    denied_paths_shape(&facts.adapter, profile)?;
    // Both ways: an `inner` this adapter does not wrap, and an adapter opened around an `inner`
    // the profile does not name (`confinement.yaml`, `AdmitProfile` outcome `adapter-unserved`).
    if profile.inner != facts.inner {
        return Err(ConfinementError::AdapterUnserved {
            adapter: adapter(),
            detail: format!(
                "the profile expects {:?} inside, and this adapter wraps {:?}",
                profile.inner, facts.inner
            ),
        });
    }
    let insecure = facts.declared_insecure();
    if insecure && profile.reason().is_none() {
        return Err(ConfinementError::LocalReasonMissing { adapter: adapter() });
    }
    if facts.proves(&Fact::ImageDigestPinned)
        && !profile.image.as_deref().is_some_and(image_is_pinned)
    {
        return Err(ConfinementError::ImageUnpinned { adapter: adapter() });
    }
    if insecure && !facts.proves(&Fact::LocalPermitted) {
        return Err(ConfinementError::LocalRefused { adapter: adapter() });
    }
    let families = profile.required_families();
    let needs = |family: &ContractFamily| {
        families.contains(family)
            || (*family == ContractFamily::Audit && profile.audit.is_some())
            // An isolated network is a promise about egress, and an adapter that proves
            // `host_network` cannot keep it.
            || (*family == ContractFamily::NetworkEgress
                && profile.network == Some(Network::Isolated)
                && facts.proves(&Fact::HostNetwork))
    };
    if let Some(family) = FAMILY_ORDER
        .iter()
        .find(|family| needs(family) && !facts.serves(family))
    {
        return Err(ConfinementError::FamilyUnserved {
            adapter: adapter(),
            family: family.clone(),
        });
    }
    let required = profile.required_facts();
    if let Some(fact) = required.iter().find(|fact| !facts.proves(fact)) {
        return Err(ConfinementError::FactUnserved {
            adapter: adapter(),
            fact: fact.clone(),
            withheld: facts.withheld_for(fact).cloned(),
        });
    }
    Ok(AdmittedProfile {
        id: ProfileId::mint(),
        instance: facts.instance(),
        adapter: adapter(),
        inner: facts.inner,
        families,
        facts: required,
        declared_insecure: insecure,
        profile: profile.clone(),
    })
}

/// The program and arguments one start would run.
#[derive(Debug, Clone)]
pub struct Command {
    pub argv: Vec<String>,
    /// The whole environment: nothing is inherited from the embedding process.
    pub env: BTreeMap<String, String>,
}

/// What `render` returns: the program an adapter would execute and its arguments. Descriptor
/// numbers in it name descriptors opened for the render and closed when it returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Ended,
    /// The adapter lost sight of the run and cannot say how it ended.
    Unknown,
}

/// How a run ended. Serialized, an absent `code` or `signal` is omitted (design 36 D15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RunExit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
}

/// A bound that ended a run (design 36 D1).
///
/// The process-count bound is no variant: the kernel refuses the creation and the run carries
/// on. Nor is the output byte bound of [`IoBounds`]: it cuts the record and ends nothing
/// (design 36 D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RunLimit {
    /// `ProcessBounds.memory_bytes`: the kernel's OOM killer ended the run.
    Memory,
    /// `ProcessBounds.cpu_millis`: the run's cumulative CPU time reached its budget.
    CpuTime,
    /// `ProcessBounds.timeout_ms`: the run's deadline passed.
    WallTime,
    /// The byte ceiling of an egress aperture the run used.
    EgressBytes,
    /// The run's live output waited unread past the stall deadline while the run's leader had
    /// not exited (design 36 D5).
    OutputStall,
}

/// The order in which a limit is named where more than one is true at a run's end (design 36
/// D4): the order in which the host driver names its refusal.
const LIMIT_ORDER: [RunLimit; 5] = [
    RunLimit::OutputStall,
    RunLimit::EgressBytes,
    RunLimit::Memory,
    RunLimit::CpuTime,
    RunLimit::WallTime,
];

/// Of the limits `reached` at a run's end, the one design 36 D4 names: the first of
/// [`LIMIT_ORDER`]. `None` where none was reached.
pub(crate) fn named_limit(reached: &[RunLimit]) -> Option<RunLimit> {
    LIMIT_ORDER
        .into_iter()
        .find(|limit| reached.contains(limit))
}

/// One observation of a run. A declared-insecure run carries its reason in every one.
///
/// `#[non_exhaustive]`: outside this crate an observation is built with
/// [`RunObservation::new`] and field assignments, and a pattern names `..` (design 36 D26).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RunObservation {
    pub run: RunId,
    pub adapter: AdapterName,
    pub inner: Option<InnerAdapter>,
    pub declared_insecure: Option<String>,
    pub state: RunState,
    pub exit: Option<RunExit>,
    /// The bound that ended the run (design 36 D1, D2). `None` while the run runs, for a run no
    /// limit ended, and where the adapter did not observe which limit ended it; each adapter's
    /// module says which limits it observes. Also named on an `Unknown` run whose limit the
    /// adapter observed before it lost sight of how the run ended. No adapter derives it from an
    /// exit code or a signal alone.
    pub limit: Option<RunLimit>,
    /// Processes of the run the kernel's OOM killer ended, once the run has ended; `None` where
    /// the adapter does not observe it (absent, never zero: invariant 3).
    pub memory_oom_kills: Option<u64>,
    /// Process creations refused by a process bound: the `max` counter of `pids.events` in the
    /// run's cgroup (design 36 § 3). Where the cgroup has no `pids.events.local`, the kernel
    /// counts there every creation refused for a process of the cgroup, also by the bound of a
    /// cgroup above it, so `systemd` runs whose run cgroups share a bound above them each count
    /// what that bound refused; where it has that file and the cgroup2 mount has no
    /// `pids_localevents`, it counts the hits of the cgroup's own bound and of any bound beneath
    /// it (design 36 D10). A running run carries the count at the moment of the observation, an
    /// ended one the count once its leader exited; a running observation made between that read
    /// and the end of the tree's kill can carry a higher count, because a process left in the
    /// tree can still be refused (design 36 D13). `Some(0)`: read, and nothing was refused.
    /// `None` where the adapter does not read it or the read failed (invariant 3): `linux` and
    /// `systemd` read it, `oci`, `pod`, `local` and `macos` do not (design 36 D14). A hit ends no
    /// run and sets no `limit`.
    pub process_limit_hits: Option<u64>,
}

impl RunObservation {
    /// An observation of `run` on `adapter` in `state`, with every optional member `None`. The
    /// fields are public: a caller sets the ones it needs.
    #[must_use]
    pub fn new(run: RunId, adapter: AdapterName, state: RunState) -> Self {
        Self {
            run,
            adapter,
            inner: None,
            declared_insecure: None,
            state,
            exit: None,
            limit: None,
            memory_oom_kills: None,
            process_limit_hits: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// Which bytes of the stream `stdout` holds: the head is `stdout[..head_bytes]`, the tail
    /// directly after it, and the omitted bytes sat between the two (design 29 § 5).
    pub stdout_kept: KeptOutput,
    /// Which bytes of the stream `stderr` holds, as `stdout_kept`.
    pub stderr_kept: KeptOutput,
}

/// One confinement mechanism behind the contract.
#[async_trait]
pub trait ConfinementAdapter: Send + Sync {
    /// What this adapter proved when it opened.
    fn facts(&self) -> &AdapterFacts;

    /// The adapter's refusals beyond the contract order, run after it admits and before any
    /// record is written. Most adapters have none.
    ///
    /// # Errors
    ///
    /// A named refusal of something this adapter cannot apply.
    fn check(&self, _profile: &ConfinementProfile) -> Result<(), ConfinementError> {
        Ok(())
    }

    /// Admits `profile` against [`Self::facts`] in the contract's order.
    ///
    /// # Errors
    ///
    /// The first refusal of the contract order, or of [`Self::check`].
    fn admit(&self, profile: &ConfinementProfile) -> Result<AdmittedProfile, ConfinementError> {
        admission(self.facts(), profile, |profile| self.check(profile))
    }

    /// The program and arguments this adapter would run for `command`, without running anything.
    ///
    /// # Errors
    ///
    /// A profile admitted elsewhere, or a host refusal met while rendering.
    fn render(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RenderedCommand, ConfinementError>;

    async fn start(
        &self,
        admitted: &AdmittedProfile,
        command: &Command,
    ) -> Result<RunId, ConfinementError>;

    async fn observe(&self, run: &RunId) -> Result<RunObservation, ConfinementError>;

    async fn output(&self, run: &RunId) -> Result<RunOutput, ConfinementError>;

    /// Returns once the run has ended.
    async fn wait(&self, run: &RunId) -> Result<RunObservation, ConfinementError>;

    async fn signal(&self, run: &RunId, signal: Signal) -> Result<(), ConfinementError>;

    /// Writes one chunk to the run's standard input, returning once the pipe took every byte
    /// (design 32 § 2). At most one write is in flight per run; a second waits for the first.
    /// Dropping the future while it writes closes the run's standard input.
    ///
    /// # Errors
    ///
    /// `session.not-pipe` (422) for a run admitted without [`StdinMode::Piped`], and on an
    /// adapter that does not prove `stdin_pipe`; `session.frame-limit` (429) for an empty write
    /// or one over [`STDIN_CHUNK_BYTES`]; `session.input-limit` (429) past the run's
    /// `limit_bytes`; `session.input-closed` (422) after `close_stdin`, a cancelled write or the
    /// run's observed end; `session.write-failed` (500) when the pipe refused the bytes or the run
    /// ended while the write was pending.
    async fn write_stdin(&self, _run: &RunId, _bytes: &[u8]) -> Result<(), ConfinementError> {
        Err(stdin::not_pipe())
    }

    /// Ends the run's standard input: the child reads end of file. Waits for a pending write, and
    /// returns `Ok` however often it is called.
    ///
    /// # Errors
    ///
    /// `session.not-pipe` (422) for a run admitted without [`StdinMode::Piped`], and on an
    /// adapter that does not prove `stdin_pipe`.
    async fn close_stdin(&self, _run: &RunId) -> Result<(), ConfinementError> {
        Err(stdin::not_pipe())
    }

    /// The next event of a run admitted with [`OutputDelivery::Live`], waiting at most
    /// `timeout`; `None` once both streams ended and every event was read (design 32 § 3).
    ///
    /// Each stream's chunks come in order with contiguous offsets, and the record begins with
    /// them. After the last chunk come, each at most once, `Bounded` for a stream whose every
    /// kept byte was delivered and whose record was truncated, and `Stalled` when delivery
    /// stopped at the stall deadline. Concurrent calls are served one after the other.
    ///
    /// # Errors
    ///
    /// `session.not-pipe` (422) for a run admitted [`OutputDelivery::Recorded`], and on an
    /// adapter that does not prove `output_live`; `session.timeout-invalid` (422) for a zero
    /// `timeout`; `session.read-timeout` (500) when it elapsed with no event.
    async fn read_output(
        &self,
        _run: &RunId,
        _timeout: Duration,
    ) -> Result<Option<OutputEvent>, ConfinementError> {
        Err(live::not_live())
    }

    /// Whether this run's live delivery stopped at the stall deadline. `false` for a run that
    /// did not stall, a run admitted [`OutputDelivery::Recorded`], and a run this adapter does
    /// not track, or no longer does.
    fn output_stalled(&self, _run: &RunId) -> bool {
        false
    }
}

/// `open` for an adapter this tree does not serve yet: always `confinement.adapter-unserved`.
pub(crate) fn unserved(
    adapter: AdapterName,
    detail: &str,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    Err(ConfinementError::AdapterUnserved {
        adapter,
        detail: detail.to_owned(),
    })
}

/// A run identity no adapter recognises.
pub(crate) fn unknown_run() -> ConfinementError {
    ConfinementError::Driver(DriverError::not_found())
}

/// Refuses an empty argv before anything is spawned.
pub(crate) fn check_command(command: &Command) -> Result<(), ConfinementError> {
    if command.argv.first().is_none_or(String::is_empty) {
        return Err(DriverError::refused("exec.argv-invalid", "The argv is empty.", "argv").into());
    }
    Ok(())
}

/// Refuses a start without a workspace.
pub(crate) fn workspace_of(admitted: &AdmittedProfile) -> Result<&PathBuf, ConfinementError> {
    admitted.profile.workspace.as_ref().ok_or_else(|| {
        DriverError::refused(
            WORKSPACE_REQUIRED,
            "A run needs a workspace to start in.",
            "workspace",
        )
        .into()
    })
}

/// Steps 1 and 2 of design 29 § 1, the same on every adapter: the wire's own shape rule for a
/// workspace access (`exec.workspace-access-invalid`, 422), then no writable subtree that is
/// `.git` or lies beneath one ([`WRITABLE_SUBTREE_GIT`], 422).
pub(crate) fn check_workspace_access(profile: &ConfinementProfile) -> Result<(), ConfinementError> {
    substrate_wire::validate_workspace_access(&profile.workspace_access).map_err(|error| {
        DriverError::refused(
            substrate_wire::EXEC_WORKSPACE_ACCESS_INVALID,
            format!("The requested workspace access is invalid: {error}"),
            "workspace_access",
        )
    })?;
    let beneath_git = |subtree: &&String| {
        subtree
            .split('/')
            .any(|component| component.eq_ignore_ascii_case(".git"))
    };
    if let Some(subtree) = profile
        .workspace_access
        .writable_subtrees()
        .and_then(|subtrees| subtrees.iter().find(beneath_git))
    {
        return Err(DriverError::refused(
            WRITABLE_SUBTREE_GIT,
            format!("The writable subtree {subtree} is a .git directory or lies beneath one."),
            "workspace_access.writable_subtrees",
        )
        .into());
    }
    Ok(())
}

/// Steps 1 and 2, then step 3 for an adapter that serves no workspace access but `ReadWrite`:
/// `exec.workspace-scoped-write-unserved` (501).
pub(crate) fn refuse_workspace_access(
    adapter: &str,
    profile: &ConfinementProfile,
) -> Result<(), ConfinementError> {
    check_workspace_access(profile)?;
    if profile.workspace_access.is_read_write() {
        return Ok(());
    }
    Err(DriverError::unserved(
        substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED,
        format!("The {adapter} adapter does not serve a read-only or scoped workspace."),
        "workspace_access",
    )
    .into())
}

/// Validates a workdir relative to the workspace: relative, normal components only.
pub(crate) fn relative_workdir(workdir: &str) -> Result<PathBuf, ConfinementError> {
    let path = std::path::Path::new(workdir);
    if workdir.is_empty()
        || !path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return Err(DriverError::refused(
            substrate_wire::EXEC_WORKDIR_INVALID,
            "The working directory must be a relative path beneath the workspace.",
            "workdir",
        )
        .into());
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REASON: &str = "the test admits a profile on purpose";

    /// Design 36 D26: the constructor leaves every optional member absent, so an observation a
    /// consumer builds names no limit and no count it did not set.
    #[test]
    fn a_new_observation_has_every_optional_member_absent() {
        let observation = RunObservation::new(
            RunId::new("run_new".to_owned()),
            AdapterName::Local,
            RunState::Ended,
        );
        assert_eq!(observation.run.as_str(), "run_new");
        assert_eq!(observation.adapter, AdapterName::Local);
        assert_eq!(observation.state, RunState::Ended);
        assert_eq!(observation.inner, None);
        assert_eq!(observation.declared_insecure, None);
        assert_eq!(observation.exit, None);
        assert_eq!(observation.limit, None);
        assert_eq!(observation.memory_oom_kills, None);
        assert_eq!(observation.process_limit_hits, None);
    }

    /// Design 36 D4: of the limits true at a run's end the first of output stall, egress bytes,
    /// memory, CPU time and wall time is named, whatever order the adapter found them in; none
    /// true names none.
    #[test]
    fn of_several_limits_the_first_in_the_contract_order_is_named() {
        let order = [
            RunLimit::OutputStall,
            RunLimit::EgressBytes,
            RunLimit::Memory,
            RunLimit::CpuTime,
            RunLimit::WallTime,
        ];
        assert_eq!(named_limit(&[]), None);
        for (index, first) in order.iter().enumerate() {
            assert_eq!(named_limit(&[*first]), Some(*first));
            for later in &order[index + 1..] {
                assert_eq!(named_limit(&[*later, *first]), Some(*first), "{later:?}");
                assert_eq!(named_limit(&[*first, *later]), Some(*first), "{later:?}");
            }
        }
    }

    /// Design 36 D1: the limit serializes as one snake-case word.
    #[test]
    fn a_limit_serializes_as_its_snake_case_name() {
        for (limit, word) in [
            (RunLimit::Memory, "memory"),
            (RunLimit::CpuTime, "cpu_time"),
            (RunLimit::WallTime, "wall_time"),
            (RunLimit::EgressBytes, "egress_bytes"),
            (RunLimit::OutputStall, "output_stall"),
        ] {
            assert_eq!(
                serde_json::to_value(limit).expect("serializes"),
                serde_json::json!(word)
            );
        }
    }

    /// Design 36 D15: `run_ended` serializes under its event name, the state in snake case, and
    /// every absent member, an absent `code` or `signal` of the exit among them, is omitted.
    #[test]
    fn a_run_ended_record_serializes_with_its_event_and_omits_what_is_absent() {
        let run = RunId::new("ex_1".to_owned());
        let profile = ProfileId("p1".to_owned());
        let bare = AuditRecord::RunEnded {
            run: run.clone(),
            profile: profile.clone(),
            adapter: AdapterName::Local,
            inner: None,
            declared_insecure: None,
            state: RunState::Unknown,
            exit: None,
            limit: None,
            memory_oom_kills: None,
            process_limit_hits: None,
        };
        assert_eq!(
            serde_json::to_value(&bare).expect("serializes"),
            serde_json::json!({
                "event": "run_ended", "run": "ex_1", "profile": "p1", "adapter": "local",
                "state": "unknown",
            })
        );
        let full = AuditRecord::RunEnded {
            run,
            profile,
            adapter: AdapterName::Systemd,
            inner: Some(InnerAdapter::Linux),
            declared_insecure: Some(REASON.to_owned()),
            state: RunState::Ended,
            exit: Some(RunExit {
                code: None,
                signal: Some(9),
            }),
            limit: Some(RunLimit::Memory),
            memory_oom_kills: Some(1),
            process_limit_hits: Some(0),
        };
        assert_eq!(
            serde_json::to_value(&full).expect("serializes"),
            serde_json::json!({
                "event": "run_ended", "run": "ex_1", "profile": "p1", "adapter": "systemd",
                "inner": "linux", "declared_insecure": REASON, "state": "ended",
                "exit": {"signal": 9}, "limit": "memory", "memory_oom_kills": 1,
                "process_limit_hits": 0,
            })
        );
        assert_eq!(
            serde_json::to_value(RunExit {
                code: Some(0),
                signal: None
            })
            .expect("serializes"),
            serde_json::json!({"code": 0})
        );
        assert_eq!(
            serde_json::to_value(RunState::Running).expect("serializes"),
            serde_json::json!("running")
        );
    }

    type Kept = Arc<parking_lot::Mutex<Vec<serde_json::Value>>>;

    /// A `local` profile admitted with a sink that keeps every record as JSON and sends each
    /// `run_ended` on the returned channel, or with no sink.
    fn admitted_local(
        sink: bool,
    ) -> (
        AdmittedProfile,
        Kept,
        tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    ) {
        let records = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let kept = Arc::clone(&records);
        let (ended, receiver) = tokio::sync::mpsc::unbounded_channel();
        let profile = ConfinementProfile {
            local_reason: Some(REASON.into()),
            audit: sink.then(|| {
                AuditSink::new(move |record: &AuditRecord| {
                    let value = serde_json::to_value(record).expect("record");
                    kept.lock().push(value.clone());
                    if value["event"] == "run_ended" {
                        let _ = ended.send(value);
                    }
                })
            }),
            ..ConfinementProfile::default()
        };
        let admitted = admission(&local::proof(true), &profile, |_| Ok(())).expect("admitted");
        records.lock().clear();
        (admitted, records, receiver)
    }

    fn ended_records(
        records: &parking_lot::Mutex<Vec<serde_json::Value>>,
    ) -> Vec<serde_json::Value> {
        records
            .lock()
            .iter()
            .filter(|record| record["event"] == "run_ended")
            .cloned()
            .collect()
    }

    /// Design 36 D16, D20: the value writes the record of the observation it is given, once,
    /// on a thread of its own; consumed by the write, it writes nothing more when it goes.
    #[tokio::test]
    async fn the_run_end_value_writes_its_observation_once() {
        let (admitted, records, mut ended) = admitted_local(true);
        let run = RunId::new("local-1-1".to_owned());
        let end = admitted.audit_start(&run, "/bin/true");
        assert!(!end.is_inert());
        let mut observation = RunObservation::new(run.clone(), AdapterName::Local, RunState::Ended);
        observation.declared_insecure = Some(REASON.to_owned());
        observation.exit = Some(RunExit {
            code: Some(0),
            signal: None,
        });
        observation.limit = Some(RunLimit::OutputStall);
        observation.process_limit_hits = Some(3);
        end.write(observation).finished().await;
        let record = tokio::time::timeout(std::time::Duration::from_secs(5), ended.recv())
            .await
            .expect("run_ended reaches the sink")
            .expect("the sink lives");
        assert_eq!(
            record,
            serde_json::json!({
                "event": "run_ended", "run": "local-1-1", "profile": admitted.id().as_str(),
                "adapter": "local", "declared_insecure": REASON, "state": "ended",
                "exit": {"code": 0}, "limit": "output_stall", "process_limit_hits": 3,
            })
        );
        assert!(ended.try_recv().is_err(), "a second run_ended");
        let events: Vec<_> = records
            .lock()
            .iter()
            .map(|record| record["event"].clone())
            .collect();
        assert_eq!(events, ["run_started", "run_ended"]);
    }

    /// Design 36 D22: dropped unwritten, the value writes `run_ended` with `state: unknown` and
    /// no exit, limit or count, synchronously on the dropping thread.
    #[test]
    fn the_run_end_value_dropped_unwritten_writes_unknown() {
        let (admitted, records, _) = admitted_local(true);
        let end = admitted.audit_start(&RunId::new("local-2-1".to_owned()), "/bin/true");
        drop(end);
        assert_eq!(
            ended_records(&records),
            [serde_json::json!({
                "event": "run_ended", "run": "local-2-1", "profile": admitted.id().as_str(),
                "adapter": "local", "declared_insecure": REASON, "state": "unknown",
            })]
        );
    }

    /// Design 36 D22: nothing is written from a thread that is panicking.
    #[test]
    fn the_run_end_value_dropped_by_a_panicking_thread_writes_nothing() {
        let (admitted, records, _) = admitted_local(true);
        let end = admitted.audit_start(&RunId::new("local-3-1".to_owned()), "/bin/true");
        let unwound = std::thread::spawn(move || {
            let _end = end;
            panic!("the thread unwinds holding the value");
        })
        .join();
        assert!(unwound.is_err());
        assert!(ended_records(&records).is_empty(), "{:?}", records.lock());
    }

    /// Design 36 D19: without a sink every value `audit_start` makes is inert, and writing or
    /// dropping one fails nothing.
    #[tokio::test]
    async fn without_a_sink_the_run_end_value_is_inert() {
        let (admitted, _, _) = admitted_local(false);
        let run = RunId::new("local-4-1".to_owned());
        let end = admitted.audit_start(&run, "/bin/true");
        assert!(end.is_inert());
        end.write(RunObservation::new(
            run.clone(),
            AdapterName::Local,
            RunState::Ended,
        ))
        .finished()
        .await;
        let dropped = admitted.audit_start(&run, "/bin/true");
        assert!(dropped.is_inert());
        drop(dropped);
    }

    /// Design 36 D15: the record of an ended observation carries its exit, its limit and its
    /// OOM kill count as the observation states them, and a `running` observation is recorded
    /// `unknown`.
    #[test]
    fn the_record_of_an_observation_carries_its_limit_and_its_oom_count() {
        let (admitted, _, _) = admitted_local(true);
        let run = RunId::new("local-6-1".to_owned());
        let end = admitted.audit_start(&run, "/bin/true");
        let pending = end.0.as_ref().expect("a sink");
        let mut observation =
            RunObservation::new(run.clone(), AdapterName::Systemd, RunState::Ended);
        observation.inner = Some(InnerAdapter::Local);
        observation.exit = Some(RunExit {
            code: None,
            signal: Some(9),
        });
        observation.limit = Some(RunLimit::Memory);
        observation.memory_oom_kills = Some(3);
        assert_eq!(
            serde_json::to_value(pending.record(observation.clone())).expect("record"),
            serde_json::json!({
                "event": "run_ended", "run": "local-6-1", "profile": admitted.id().as_str(),
                "adapter": "systemd", "inner": "local", "state": "ended",
                "exit": {"signal": 9}, "limit": "memory", "memory_oom_kills": 3,
            })
        );
        observation.state = RunState::Running;
        assert_eq!(
            serde_json::to_value(pending.record(observation)).expect("record")["state"],
            "unknown"
        );
    }

    /// A sink that reports the name of the thread it was called on.
    fn naming_sink() -> (AuditSink, std::sync::mpsc::Receiver<Option<String>>) {
        let (called, calls) = std::sync::mpsc::channel();
        let called = parking_lot::Mutex::new(called);
        let sink = AuditSink::new(move |_: &AuditRecord| {
            let _ = called
                .lock()
                .send(std::thread::current().name().map(str::to_owned));
        });
        (sink, calls)
    }

    fn end_write(sink: AuditSink) -> EndWrite {
        EndWrite {
            sink,
            record: AuditRecord::RunEnded {
                run: RunId::new("local-7-1".to_owned()),
                profile: ProfileId("p1".to_owned()),
                adapter: AdapterName::Local,
                inner: None,
                declared_insecure: None,
                state: RunState::Ended,
                exit: None,
                limit: None,
                memory_oom_kills: None,
                process_limit_hits: None,
            },
            written: None,
        }
    }

    /// Design 36 D20, amended: the sink is called with `run_ended` on a thread of its own, named
    /// `substrate-run-ended`, on a current-thread runtime and with no runtime at all.
    #[test]
    fn a_run_ended_is_written_on_a_thread_of_its_own() {
        let (sink, calls) = naming_sink();
        drop(deliver(end_write(sink.clone())));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        runtime.block_on(async { deliver(end_write(sink)).finished().await });
        for _ in 0..2 {
            assert_eq!(
                calls
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("the sink is called"),
                Some(RUN_ENDED_THREAD.to_owned())
            );
        }
    }

    /// Design 36 D20, amended: where the system refuses the thread, a current runtime's blocking
    /// pool writes the record, never the calling thread.
    #[test]
    fn a_run_ended_the_system_refuses_a_thread_for_goes_to_the_blocking_pool() {
        let (sink, calls) = naming_sink();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .thread_name("blocking-pool")
            .build()
            .expect("runtime");
        runtime.block_on(async {
            deliver_with(end_write(sink), |_| Err(std::io::Error::other("no thread")))
                .finished()
                .await;
        });
        assert_eq!(
            calls
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("the sink is called"),
            Some("blocking-pool".to_owned())
        );
        assert!(calls.try_recv().is_err(), "the record is written once");
    }

    /// Design 36 D20, amended: where the system refuses the thread and no runtime is current, the
    /// record is written on the calling thread before the call returns, and a sink that panics
    /// there does not unwind into the caller.
    #[test]
    fn a_run_ended_with_no_thread_and_no_runtime_is_written_by_the_caller() {
        let (sink, calls) = naming_sink();
        let caller = std::thread::current().name().map(str::to_owned);
        drop(deliver_with(end_write(sink), |_| {
            Err(std::io::Error::other("no thread"))
        }));
        assert_eq!(
            calls.try_recv().expect("written before the call returned"),
            caller
        );
        let panicking = AuditSink::new(|_: &AuditRecord| panic!("the sink panics"));
        drop(deliver_with(end_write(panicking), |_| {
            Err(std::io::Error::other("no thread"))
        }));
    }

    /// Design 36 D20, amended: where the system refuses the thread and the current
    /// runtime's blocking pool has shut down, so that pool cannot take the record, the record is
    /// written on the calling thread once, and a sink that panics there does not unwind into the
    /// caller.
    #[test]
    fn a_run_ended_a_shut_down_pool_cannot_take_is_written_once_by_the_caller_and_contained() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let handle = runtime.handle().clone();
        drop(runtime);
        let _entered = handle.enter();
        let (sink, calls) = naming_sink();
        let caller = std::thread::current().name().map(str::to_owned);
        drop(deliver_with(end_write(sink), |_| {
            Err(std::io::Error::other("no thread"))
        }));
        assert_eq!(
            calls.try_recv().expect("written before the call returned"),
            caller
        );
        assert!(calls.try_recv().is_err(), "the record is written once");
        let panicking = AuditSink::new(|_: &AuditRecord| panic!("the sink panics"));
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(deliver_with(end_write(panicking), |_| {
                Err(std::io::Error::other("no thread"))
            }));
        }));
        assert!(
            unwound.is_ok(),
            "a sink that panics on the calling thread unwound into the caller"
        );
    }

    /// Design 36 D20, amended: dropped before the write is over, the end task's guard joins the
    /// record's thread, so the record is in the sink when the drop returns; dropped by a thread
    /// that is panicking, it does not wait for the sink, and the record's thread still writes.
    #[test]
    fn an_end_writing_dropped_joins_the_records_thread_unless_its_thread_is_panicking() {
        let (release, released) = std::sync::mpsc::channel::<()>();
        let released = parking_lot::Mutex::new(released);
        let (called, calls) = std::sync::mpsc::channel::<()>();
        let called = parking_lot::Mutex::new(called);
        let holding = AuditSink::new(move |_: &AuditRecord| {
            let _ = released
                .lock()
                .recv_timeout(std::time::Duration::from_secs(5));
            let _ = called.lock().send(());
        });
        let writing = deliver(end_write(holding.clone()));
        release.send(()).expect("the sink waits");
        drop(writing);
        calls
            .try_recv()
            .expect("the record is in the sink once the drop returned");
        let writing = deliver(end_write(holding));
        let unwound = std::thread::spawn(move || {
            let _writing = writing;
            panic!("the thread unwinds holding the guard");
        })
        .join();
        assert!(unwound.is_err());
        assert!(
            calls.try_recv().is_err(),
            "the panicking thread waited for the sink"
        );
        release.send(()).expect("the sink waits");
        calls
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the record's thread still writes");
    }

    /// Design 36 D20, amended 2026-11-24: the guard's drop returns once the record is in the
    /// sink, whatever the sink still has to do when the guard is dropped. The sink takes 300 ms
    /// before it records, so a drop that does not wait for the record's thread returns first.
    #[test]
    fn an_end_writing_dropped_waits_for_a_sink_that_is_still_writing() {
        let (called, calls) = std::sync::mpsc::channel::<()>();
        let called = parking_lot::Mutex::new(called);
        let slow = AuditSink::new(move |_: &AuditRecord| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let _ = called.lock().send(());
        });
        drop(deliver(end_write(slow)));
        calls
            .try_recv()
            .expect("the record is in the sink once the guard's drop returned");
    }

    /// Design 36 D17: an end task whose start failed after it was spawned gets no value, and
    /// writes nothing.
    #[tokio::test]
    async fn an_end_task_whose_sender_was_dropped_writes_nothing() {
        let (admitted, records, _) = admitted_local(true);
        let (sender, receiver) = tokio::sync::oneshot::channel::<RunEndRecord>();
        drop(sender);
        let run = RunId::new("local-5-1".to_owned());
        write_run_end(
            receiver,
            RunObservation::new(run, AdapterName::Local, RunState::Ended),
        )
        .await;
        assert!(records.lock().is_empty(), "{:?}", records.lock());
        drop(admitted);
    }

    /// The operator's label is the generated name, never a second spelling of it.
    #[test]
    fn an_adapters_name_is_its_contract_word() {
        for adapter in ADAPTERS {
            assert_eq!(
                serde_json::to_value(&adapter).expect("adapter name serializes"),
                serde_json::json!(adapter.name())
            );
        }
    }

    fn scoped(subtrees: &[&str]) -> WorkspaceAccess {
        WorkspaceAccess::Scoped {
            writable_subtrees: subtrees
                .iter()
                .map(|subtree| (*subtree).to_owned())
                .collect(),
        }
    }

    /// Admits `access` on `local` and returns the refusal with the audit records it wrote.
    fn refusal(access: &WorkspaceAccess) -> (ConfinementError, Vec<serde_json::Value>) {
        let adapter =
            open(AdapterRequest::Local(local::LocalRequest::default())).expect("local opens");
        let records = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink_records = Arc::clone(&records);
        let profile = ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace_access: access.clone(),
            audit: Some(AuditSink::new(move |record: &AuditRecord| {
                sink_records
                    .lock()
                    .push(serde_json::to_value(record).expect("record"));
            })),
            ..ConfinementProfile::default()
        };
        let error = adapter
            .admit(&profile)
            .expect_err(&format!("{access:?} is refused at admission"));
        let records = records.lock().clone();
        (error, records)
    }

    // A3: a subtree that is `.git` or lies beneath one, in any ASCII case, is refused by name.
    #[test]
    fn a_writable_subtree_at_or_beneath_git_is_refused_at_admission() {
        for subtrees in [
            &[".git"][..],
            &["a/.git/b"],
            &[".GIT"],
            &["out", "src/.Git"],
        ] {
            let (error, records) = refusal(&scoped(subtrees));
            assert_eq!(error.code(), WRITABLE_SUBTREE_GIT, "{subtrees:?}: {error}");
            assert_eq!(error.status(), 422, "{subtrees:?}");
            let ConfinementError::Driver(driver) = &error else {
                panic!("{subtrees:?}: {error}");
            };
            assert!(
                driver.message.contains(subtrees[subtrees.len() - 1]),
                "the refusal names the subtree: {}",
                driver.message
            );
            assert_eq!(records.len(), 1, "{records:?}");
            assert_eq!(records[0]["event"], "profile_refused");
            assert_eq!(records[0]["code"], WRITABLE_SUBTREE_GIT);
            assert_eq!(records[0]["adapter"], "local");
        }
    }

    // A4: the wire's shape rule comes first, the same on every adapter.
    #[test]
    fn an_unsorted_overlapping_or_dotdot_list_is_refused_at_admission() {
        for subtrees in [
            &["src", "out"][..],
            &["out", "out/a"],
            &["../out"],
            &["out/../src"],
            &[],
        ] {
            let (error, records) = refusal(&scoped(subtrees));
            assert_eq!(
                error.code(),
                substrate_wire::EXEC_WORKSPACE_ACCESS_INVALID,
                "{subtrees:?}: {error}"
            );
            assert_eq!(error.status(), 422, "{subtrees:?}");
            assert_eq!(records.len(), 1, "{records:?}");
            assert_eq!(
                records[0]["code"],
                substrate_wire::EXEC_WORKSPACE_ACCESS_INVALID
            );
        }
    }

    // T1a of design 32: a piped standard input outside 1 to 16 MiB is refused at admission, by
    // the shared stream check every adapter's `check` runs first, with its audit record.
    #[test]
    fn a_piped_stdin_bound_outside_one_to_sixteen_mib_is_refused_at_admission() {
        let adapter =
            open(AdapterRequest::Local(local::LocalRequest::default())).expect("local opens");
        for limit_bytes in [0, MAX_STDIN_BYTES + 1] {
            let records = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let sink_records = Arc::clone(&records);
            let error = adapter
                .admit(&ConfinementProfile {
                    local_reason: Some(REASON.into()),
                    stdin: StdinMode::Piped { limit_bytes },
                    audit: Some(AuditSink::new(move |record: &AuditRecord| {
                        sink_records
                            .lock()
                            .push(serde_json::to_value(record).expect("record"));
                    })),
                    ..ConfinementProfile::default()
                })
                .expect_err("refused");
            assert_eq!(
                error.code(),
                substrate_wire::SESSION_LIMIT_UNSERVED,
                "{limit_bytes}: {error}"
            );
            assert_eq!(error.status(), 429);
            let records = records.lock();
            assert_eq!(records.len(), 1, "{records:?}");
            assert_eq!(records[0]["event"], "profile_refused");
            assert_eq!(records[0]["code"], substrate_wire::SESSION_LIMIT_UNSERVED);
        }
    }

    // T1b of design 32: `queued_chunks` outside 1 to 16 and `Live` with a retention other than
    // `Head` are refused at admission by the shared stream check, each with its audit record.
    #[test]
    fn a_live_queue_outside_one_to_sixteen_or_beside_a_tail_is_refused_at_admission() {
        let adapter =
            open(AdapterRequest::Local(local::LocalRequest::default())).expect("local opens");
        let tail = OutputRetention {
            stdout: Retention::Tail,
            stderr: Retention::Head,
        };
        for (queued_chunks, output_retention, code, status) in [
            (
                0,
                OutputRetention::default(),
                substrate_wire::SESSION_LIMIT_UNSERVED,
                429,
            ),
            (
                MAX_QUEUED_CHUNKS + 1,
                OutputRetention::default(),
                substrate_wire::SESSION_LIMIT_UNSERVED,
                429,
            ),
            (1, tail, LIVE_OUTPUT_RETENTION_INVALID, 422),
        ] {
            let records = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let sink_records = Arc::clone(&records);
            let error = adapter
                .admit(&ConfinementProfile {
                    local_reason: Some(REASON.into()),
                    output_delivery: OutputDelivery::Live { queued_chunks },
                    output_retention,
                    audit: Some(AuditSink::new(move |record: &AuditRecord| {
                        sink_records
                            .lock()
                            .push(serde_json::to_value(record).expect("record"));
                    })),
                    ..ConfinementProfile::default()
                })
                .expect_err("refused");
            assert_eq!(error.code(), code, "{queued_chunks}: {error}");
            assert_eq!(error.status(), status, "{queued_chunks}");
            let records = records.lock();
            assert_eq!(records.len(), 1, "{records:?}");
            assert_eq!(records[0]["event"], "profile_refused");
            assert_eq!(records[0]["code"], code);
        }
        for queued_chunks in [1, MAX_QUEUED_CHUNKS] {
            adapter
                .admit(&ConfinementProfile {
                    local_reason: Some(REASON.into()),
                    output_delivery: OutputDelivery::Live { queued_chunks },
                    ..ConfinementProfile::default()
                })
                .expect("in range, with the head kept");
        }
    }

    // The shape rule is checked before `.git`: an unsorted list naming `.git` is invalid first.
    #[test]
    fn the_shape_rule_is_checked_before_the_git_rule() {
        let (error, _) = refusal(&scoped(&["src", ".git"]));
        assert_eq!(error.code(), substrate_wire::EXEC_WORKSPACE_ACCESS_INVALID);
    }

    fn withheld_entry(
        address: &'static str,
        path: &str,
        file_system: Option<&str>,
    ) -> FactWithheld {
        FactWithheld {
            fact: Fact::ExecSubpaths,
            cause: if file_system.is_some() {
                "file-system-unlisted"
            } else {
                "mount-id-unreported"
            },
            address,
            path: path.to_owned(),
            file_system: file_system.map(str::to_owned),
        }
    }

    /// The proof of a `linux` that serves every family, proves the facts a plain run needs and
    /// no executable-list fact, and carries `withheld`.
    fn linux_without_lists(withheld: Vec<FactWithheld>) -> AdapterFacts {
        let mut facts = AdapterFacts::new(
            AdapterName::Linux,
            None,
            &FAMILY_ORDER
                .iter()
                .map(|family| (family.clone(), true))
                .collect::<Vec<_>>(),
            &[
                (Fact::WholeTreeKill, true),
                (Fact::IpcSocketAllowlist, true),
            ],
        );
        facts.withheld = withheld;
        facts
    }

    /// Admits a profile listing `entries` against `facts`, with the audit records it wrote.
    fn admit_listing(
        facts: &AdapterFacts,
        entries: &[&str],
    ) -> (
        Result<AdmittedProfile, ConfinementError>,
        Vec<serde_json::Value>,
    ) {
        let records = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink_records = Arc::clone(&records);
        let profile = ConfinementProfile {
            workspace: Some(std::path::PathBuf::from("/srv/workspaces/ws_1")),
            exec_subpaths: entries.iter().map(|entry| (*entry).to_owned()).collect(),
            audit: Some(AuditSink::new(move |record: &AuditRecord| {
                sink_records
                    .lock()
                    .push(serde_json::to_value(record).expect("record"));
            })),
            ..ConfinementProfile::default()
        };
        let admitted = admission(facts, &profile, |_| Ok(()));
        let records = records.lock().clone();
        (admitted, records)
    }

    /// Admits a profile with both lists against `facts`, with the audit records it wrote.
    fn admit_with_lists(
        facts: &AdapterFacts,
        entries: &[&str],
        denied: &[String],
    ) -> (
        Result<AdmittedProfile, ConfinementError>,
        Vec<serde_json::Value>,
    ) {
        let records = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink_records = Arc::clone(&records);
        let profile = ConfinementProfile {
            workspace: Some(std::path::PathBuf::from("/srv/workspaces/ws_1")),
            local_reason: facts
                .declared_insecure()
                .then(|| "a developer's own machine".to_owned()),
            exec_subpaths: entries.iter().map(|entry| (*entry).to_owned()).collect(),
            exec_denied_paths: denied.to_vec(),
            audit: Some(AuditSink::new(move |record: &AuditRecord| {
                sink_records
                    .lock()
                    .push(serde_json::to_value(record).expect("record"));
            })),
            ..ConfinementProfile::default()
        };
        let admitted = admission(facts, &profile, |_| Ok(()));
        let records = records.lock().clone();
        (admitted, records)
    }

    /// The proof of a `linux` that proves `exec_subpaths`, and `exec_denied_paths` where asked.
    fn linux_with_lists(denied_paths: bool) -> AdapterFacts {
        AdapterFacts::new(
            AdapterName::Linux,
            None,
            &FAMILY_ORDER
                .iter()
                .map(|family| (family.clone(), true))
                .collect::<Vec<_>>(),
            &[
                (Fact::WholeTreeKill, true),
                (Fact::ExecSubpaths, true),
                (Fact::ExecDeniedPaths, denied_paths),
            ],
        )
    }

    /// Design 35 D13: a deny-list beside an empty allow-list is refused by name on every
    /// adapter, before any fact is read: on `local`, which proves neither list fact, and on a
    /// `linux` that proves both. Each refusal is one `profile_refused` record with the code.
    #[test]
    fn a_deny_list_without_an_allow_list_is_refused_on_every_adapter() {
        let denied = ["/usr/bin/id".to_owned()];
        for facts in [local::proof(true), linux_with_lists(true)] {
            let (refused, records) = admit_with_lists(&facts, &[], &denied);
            let error = refused.expect_err("a deny-list alone");
            assert!(
                matches!(
                    &error,
                    ConfinementError::ExecDeniedPathsWithoutSubpaths { adapter }
                        if *adapter == facts.adapter
                ),
                "{error:?}"
            );
            assert_eq!(
                (error.code(), error.status()),
                ("confinement.exec-denied-paths-without-subpaths", 422)
            );
            assert_eq!(records.len(), 1, "{records:?}");
            assert_eq!(records[0]["event"], "profile_refused");
            assert_eq!(records[0]["code"], error.code());
            assert!(records[0].get("fact").is_none(), "{records:?}");
        }
        // An adapter that did not open answers the same before `adapter-unserved`.
        let profile = ConfinementProfile {
            exec_denied_paths: denied.to_vec(),
            ..ConfinementProfile::default()
        };
        assert_eq!(
            admit_unopened(&AdapterName::Macos, &profile).code(),
            EXEC_DENIED_PATHS_WITHOUT_SUBPATHS
        );
    }

    /// Design 35 D10: the fact of the deny-list stands directly after the allow-list's. `local`
    /// proves neither and names `exec_subpaths` for a profile with both lists, with the record;
    /// an adapter that proves `exec_subpaths` alone names `exec_denied_paths`; one that proves
    /// both admits, and the admitted profile requires both. A profile without a deny-list is
    /// admitted on either as before.
    #[test]
    fn both_lists_name_the_first_fact_the_adapter_did_not_prove() {
        let denied = ["bin/tool".to_owned()];
        let named = |facts: &AdapterFacts| {
            let (refused, records) = admit_with_lists(facts, &["bin"], &denied);
            let error = refused.expect_err("an unproved list fact");
            assert_eq!((error.code(), error.status()), (FACT_UNSERVED, 501));
            assert_eq!(records.len(), 1, "{records:?}");
            assert_eq!(records[0]["event"], "profile_refused");
            assert_eq!(records[0]["code"], FACT_UNSERVED);
            match error {
                ConfinementError::FactUnserved { fact, .. } => {
                    assert_eq!(
                        records[0]["fact"],
                        serde_json::to_value(&fact).expect("a fact")
                    );
                    fact
                }
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(named(&local::proof(true)), Fact::ExecSubpaths);
        assert_eq!(named(&linux_with_lists(false)), Fact::ExecDeniedPaths);

        let (admitted, records) = admit_with_lists(&linux_with_lists(true), &["bin"], &denied);
        let admitted = admitted.expect("both facts are proved");
        assert_eq!(
            admitted.profile().required_facts(),
            [Fact::ExecSubpaths, Fact::ExecDeniedPaths]
        );
        assert_eq!(records[0]["event"], "profile_admitted");
        for facts in [linux_with_lists(false), linux_with_lists(true)] {
            let (admitted, _) = admit_with_lists(&facts, &["bin"], &[]);
            let admitted = admitted.expect("no deny-list, as before this story");
            assert_eq!(admitted.profile().required_facts(), [Fact::ExecSubpaths]);
        }
    }

    /// Design 35 D10: the deny-list holds at most `MAX_EXEC_DENIED_PATHS` entries. 64 are
    /// admitted and 65 are refused by name, on an adapter that proves the fact and on `local`.
    #[test]
    fn a_deny_list_over_its_bound_is_refused_by_name() {
        assert_eq!(MAX_EXEC_DENIED_PATHS, 64);
        let entries = |count: usize| -> Vec<String> {
            (0..count)
                .map(|index| format!("bin/tool-{index}"))
                .collect()
        };
        let (admitted, _) = admit_with_lists(&linux_with_lists(true), &["bin"], &entries(64));
        admitted.expect("64 entries");
        for facts in [linux_with_lists(true), local::proof(true)] {
            let (refused, records) = admit_with_lists(&facts, &["bin"], &entries(65));
            let error = refused.expect_err("65 entries");
            assert!(
                matches!(&error, ConfinementError::ExecDeniedPathsExceeded { .. }),
                "{error:?}"
            );
            assert_eq!(
                (error.code(), error.status()),
                ("confinement.exec-denied-paths-exceeded", 422)
            );
            assert_eq!(records[0]["code"], error.code());
        }
    }

    /// Design 35 D27: a profile with a list on an adapter whose open withheld the fact is
    /// refused `confinement.fact-unserved`, and the refusal repeats the proof's first entry for
    /// that fact: the cause, the path, the configuration field and the file-system type.
    #[test]
    fn a_withheld_fact_is_refused_with_its_cause() {
        let first = withheld_entry(
            "config.workspace_root",
            "/var/lib/b10x-substrate/workspaces",
            Some("xfs"),
        );
        let facts = linux_without_lists(vec![
            first.clone(),
            withheld_entry("config.exec_gate", "/usr", Some("xfs")),
        ]);
        let (refused, records) = admit_listing(&facts, &["bin"]);
        let error = refused.expect_err("a list on an adapter that withheld the fact");
        match &error {
            ConfinementError::FactUnserved {
                adapter,
                fact,
                withheld,
            } => {
                assert_eq!(*adapter, AdapterName::Linux);
                assert_eq!(*fact, Fact::ExecSubpaths);
                assert_eq!(
                    withheld.as_ref(),
                    Some(&first),
                    "the first entry for the fact"
                );
            }
            other => panic!("expected exec_subpaths unserved, got {other:?}"),
        }
        assert_eq!(error.code(), FACT_UNSERVED);
        assert_eq!(error.status(), 501);
        assert_eq!(
            error.to_string(),
            "confinement.fact-unserved: Linux did not prove ExecSubpaths: file-system-unlisted: \
             /var/lib/b10x-substrate/workspaces (config.workspace_root) is on xfs; the lists \
             are served on ext4, overlay and tmpfs"
        );

        // The audit record keeps its shape: the code and the fact, and no new member. It is the
        // record the same refusal writes against a proof with no entry.
        let (plain, plain_records) = admit_listing(&linux_without_lists(Vec::new()), &["bin"]);
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0]["event"], "profile_refused");
        assert_eq!(records[0]["code"], FACT_UNSERVED);
        assert_eq!(records[0]["fact"], "exec_subpaths");
        assert_eq!(records, plain_records);

        // Against a proof with no entry: no reason, and the text the refusal had before.
        let plain = plain.expect_err("a list on an adapter that did not prove the fact");
        assert!(
            matches!(
                &plain,
                ConfinementError::FactUnserved { fact, withheld: None, .. }
                    if *fact == Fact::ExecSubpaths
            ),
            "{plain:?}"
        );
        assert_eq!(
            plain.to_string(),
            "confinement.fact-unserved: Linux did not prove ExecSubpaths"
        );

        // A profile without a list is admitted against the proof that withholds.
        let (admitted, records) = admit_listing(&facts, &[]);
        admitted.expect("a profile without a list");
        assert_eq!(records[0]["event"], "profile_admitted", "{records:?}");
    }

    /// Design 35 D27: a kernel that reports no mount ID has no file-system type to name, and an
    /// entry is repeated only by the refusal of the fact it is about.
    #[test]
    fn a_refusal_repeats_only_the_entry_of_its_own_fact() {
        let unreported = withheld_entry("config.workspace_root", "/srv/workspaces", None);
        let facts = linux_without_lists(vec![unreported.clone()]);
        let (refused, _) = admit_listing(&facts, &["bin"]);
        assert_eq!(
            refused.expect_err("withheld").to_string(),
            "confinement.fact-unserved: Linux did not prove ExecSubpaths: mount-id-unreported: \
             the kernel reports no mount ID for /srv/workspaces (config.workspace_root)"
        );

        // `stdin_pipe` is absent from this proof for no named cause.
        let piped = ConfinementProfile {
            workspace: Some(std::path::PathBuf::from("/srv/workspaces/ws_1")),
            stdin: StdinMode::Piped { limit_bytes: 1_024 },
            ..ConfinementProfile::default()
        };
        let error = admission(&facts, &piped, |_| Ok(())).expect_err("stdin_pipe is not proved");
        assert!(
            matches!(
                &error,
                ConfinementError::FactUnserved { fact, withheld: None, .. }
                    if *fact == Fact::StdinPipe
            ),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "confinement.fact-unserved: Linux did not prove StdinPipe"
        );

        // The open's own check names no entry either, and reads as it did.
        let local = AdapterFacts::new(AdapterName::Local, None, &[], &[]);
        let error = check_open(&local).expect_err("local without declared_insecure");
        assert!(
            matches!(
                &error,
                ConfinementError::FactUnserved { withheld: None, .. }
            ),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "confinement.fact-unserved: Local did not prove DeclaredInsecure"
        );
    }
}
