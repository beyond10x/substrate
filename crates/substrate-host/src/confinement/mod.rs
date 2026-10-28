//! Confinement as one contract served by declared adapters (atlas ADR 0109, ADR 0044).
//!
//! A client opens an adapter by name ([`open`]), reads what it proved ([`AdapterFacts`]: the
//! contract families it serves and the facts it proved at open), and has a
//! [`ConfinementProfile`] admitted against that proof. A profile that needs anything the adapter
//! lacks is refused by name, in the order of the domain `substrate.confinement`
//! (`spec/confinement/confinement.yaml`):
//!
//! 1. a reserved field (`confinement.reserved-field-unserved`, 501);
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
pub use facts::{AdapterFacts, FACT_ORDER, FAMILY_ORDER, InnerAdapter, ReservedField};
pub use profile::{
    ConfinementProfile, IoBounds, Ipc, Network, ProcessBounds, StdinMode, image_is_pinned,
};
// A run's standard input (design 32 § 2).
pub use stdin::{MAX_STDIN_BYTES, STDIN_CHUNK_BYTES, run_with_stdin};
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
        }),
        _ => Ok(()),
    }
}

/// The contract's answer to a profile addressed to an adapter that did not open: a reserved field
/// first, then `confinement.adapter-unserved`. It is audited like every refusal.
pub fn admit_unopened(adapter: &AdapterName, profile: &ConfinementProfile) -> ConfinementError {
    let error = reserved(profile)
        .err()
        .unwrap_or(ConfinementError::AdapterUnserved {
            adapter: adapter.clone(),
            detail: "the adapter did not open on this host".to_owned(),
        });
    audit(profile, &AuditRecord::refused(adapter, None, &error));
    error
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
    #[error("confinement.fact-unserved: {adapter:?} did not prove {fact:?}")]
    FactUnserved { adapter: AdapterName, fact: Fact },
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
            Self::Driver(error) => error.code,
        }
    }

    /// The HTTP-style status the model gives the code.
    pub fn status(&self) -> u16 {
        match self {
            Self::LocalRefused { .. } => 403,
            Self::LocalReasonMissing { .. } | Self::ImageUnpinned { .. } => 422,
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

/// One observation of a run. A declared-insecure run carries its reason in every one.
#[derive(Debug, Clone, PartialEq)]
pub struct RunObservation {
    pub run: RunId,
    pub adapter: AdapterName,
    pub inner: Option<InnerAdapter>,
    pub declared_insecure: Option<String>,
    pub state: RunState,
    pub exit: Option<RunExit>,
    /// Processes of the run the kernel's OOM killer ended, once the run has ended; `None` where
    /// the adapter does not observe it (absent, never zero: invariant 3).
    pub memory_oom_kills: Option<u64>,
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

    // The shape rule is checked before `.git`: an unsorted list naming `.git` is invalid first.
    #[test]
    fn the_shape_rule_is_checked_before_the_git_rule() {
        let (error, _) = refusal(&scoped(&["src", ".git"]));
        assert_eq!(error.code(), substrate_wire::EXEC_WORKSPACE_ACCESS_INVALID);
    }
}
