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

/// One audit record (ADR 0109 decision 2): an admission, a refusal or a start, or a private
/// `TMPDIR` a run left behind.
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

    /// Writes a start's audit record.
    pub(crate) fn audit_start(&self, run: &RunId, program: &str) {
        audit(
            &self.profile,
            &AuditRecord::RunStarted {
                run: run.clone(),
                profile: self.id.clone(),
                adapter: self.adapter.clone(),
                inner: self.inner,
                declared_insecure: self.insecure_reason().map(str::to_owned),
                program: program.to_owned(),
            },
        );
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    Ended,
    /// The adapter lost sight of the run and cannot say how it ended.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunExit {
    pub code: Option<i32>,
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
    /// The run's live output waited unread past the stall deadline.
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
