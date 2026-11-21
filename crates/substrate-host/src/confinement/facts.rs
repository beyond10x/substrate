//! What an adapter proved when it opened, and the orders a refusal reads it in.
//!
//! The generated enums are sorted alphabetically, so nothing here relies on their variant order:
//! [`FAMILY_ORDER`] is ADR 0109 decision 8's order and [`FACT_ORDER`] the model's `Fact` order,
//! both written out.

use serde::{Deserialize, Serialize};

use super::{AdapterName, ContractFamily, Fact};

/// The contract families in ADR 0109 decision 8's order. A refusal names the first family in this
/// order that a profile needs and the adapter does not serve.
pub const FAMILY_ORDER: [ContractFamily; 10] = [
    ContractFamily::ProcessRun,
    ContractFamily::ProcessWatch,
    ContractFamily::OutputRead,
    ContractFamily::Wait,
    ContractFamily::Workspace,
    ContractFamily::FilesystemView,
    ContractFamily::NetworkEgress,
    ContractFamily::NetworkIngress,
    ContractFamily::Limits,
    ContractFamily::Audit,
];

/// The facts in the model's `Fact` order. A refusal names the first fact in this order that a
/// profile requires and the adapter did not prove.
pub const FACT_ORDER: [Fact; 13] = [
    Fact::DeclaredInsecure,
    Fact::PathsAreHostPaths,
    Fact::WholeTreeKill,
    Fact::LocalPermitted,
    Fact::IpcSocketAllowlist,
    Fact::IpcSignalsContained,
    Fact::SealedPaths,
    Fact::ExecSubpaths,
    Fact::PrivateTmpdir,
    Fact::HostNetwork,
    Fact::ImageDigestPinned,
    Fact::StdinPipe,
    Fact::OutputLive,
];

/// The adapter `systemd` wraps (ADR 0109 decision 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InnerAdapter {
    Local,
    Linux,
}

impl InnerAdapter {
    pub const fn adapter(self) -> AdapterName {
        match self {
            Self::Local => AdapterName::Local,
            Self::Linux => AdapterName::Linux,
        }
    }
}

/// The two profile fields ADR 0109 F1 B reserves, by the name a refusal carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReservedField {
    IngressRoutes,
    RateBounds,
}

/// An opened adapter's proof: the families it serves and the facts it proved, each in its contract
/// order, read once at open and never raised afterwards.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdapterFacts {
    pub adapter: AdapterName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner: Option<InnerAdapter>,
    pub served_families: Vec<ContractFamily>,
    pub facts: Vec<Fact>,
    /// Why the adapter does not prove a fact its probe could have proved (design 35 D27): one
    /// entry per fact and path, in the order the open placed the paths. Empty, and then not
    /// serialized, unless the open withheld a fact for a named cause; a fact that is absent
    /// because no helper is configured, or because its probe did not hold, has no entry.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub withheld: Vec<FactWithheld>,
    /// The opened adapter this proof belongs to: an admitted profile starts only there.
    #[serde(skip)]
    instance: u64,
}

/// Why an opened adapter does not prove a fact its probe could have proved (design 35 D27).
///
/// `linux` opened with `HostConfig.exec_gate` set proves no executable-list fact where a path
/// of the open has no place the lists can be decided by, for one of two causes: the path lies
/// on a file-system type outside `ext4`, `overlay` and `tmpfs` (`file-system-unlisted`), or the
/// kernel reports no mount ID for it (`mount-id-unreported`). The adapter opens, a profile that
/// carries a list is refused `confinement.fact-unserved`, and that refusal repeats the entry.
/// Every other cause refuses the open `config.exec-gate-unserved` instead.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FactWithheld {
    pub fact: Fact,
    /// `file-system-unlisted` or `mount-id-unreported`.
    pub cause: &'static str,
    /// `config.exec_gate`, `config.workspace_root` or `config.writable_roots`: the
    /// configuration field the path belongs to. `config.exec_gate` for the helper's own file,
    /// a base directory of every view and the directory the probes build their workspace in.
    pub address: &'static str,
    pub path: String,
    /// The type field of the path's mount row; absent for `mount-id-unreported`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_system: Option<String>,
}

impl AdapterFacts {
    /// Builds the proof from what holds, keeping only the members that hold and the contract's
    /// order whatever order the caller listed them in.
    pub fn new(
        adapter: AdapterName,
        inner: Option<InnerAdapter>,
        served: &[(ContractFamily, bool)],
        proved: &[(Fact, bool)],
    ) -> Self {
        let held = |family: &ContractFamily| {
            served
                .iter()
                .any(|(candidate, holds)| *holds && candidate == family)
        };
        let proved_fact = |fact: &Fact| {
            proved
                .iter()
                .any(|(candidate, holds)| *holds && candidate == fact)
        };
        Self {
            adapter,
            inner,
            served_families: FAMILY_ORDER.iter().filter(|f| held(f)).cloned().collect(),
            facts: FACT_ORDER
                .iter()
                .filter(|f| proved_fact(f))
                .cloned()
                .collect(),
            withheld: Vec::new(),
            instance: super::next_instance(),
        }
    }

    /// The first entry that says why `fact` is withheld, which a refusal naming it repeats.
    pub(crate) fn withheld_for(&self, fact: &Fact) -> Option<&FactWithheld> {
        self.withheld.iter().find(|entry| entry.fact == *fact)
    }

    pub(crate) fn instance(&self) -> u64 {
        self.instance
    }

    pub fn serves(&self, family: &ContractFamily) -> bool {
        self.served_families.contains(family)
    }

    pub fn proves(&self, fact: &Fact) -> bool {
        self.facts.contains(fact)
    }

    /// Whether a run on this adapter is declared insecure: `local`, or `systemd` around it.
    pub fn declared_insecure(&self) -> bool {
        self.proves(&Fact::DeclaredInsecure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_orders_name_every_variant_once() {
        for family in &FAMILY_ORDER {
            assert_eq!(
                FAMILY_ORDER.iter().filter(|other| *other == family).count(),
                1
            );
        }
        for fact in &FACT_ORDER {
            assert_eq!(FACT_ORDER.iter().filter(|other| *other == fact).count(), 1);
        }
    }

    #[test]
    fn a_proof_keeps_the_contract_order() {
        let facts = AdapterFacts::new(
            AdapterName::Local,
            None,
            &[
                (ContractFamily::Audit, true),
                (ContractFamily::ProcessRun, true),
            ],
            &[
                (Fact::HostNetwork, true),
                (Fact::DeclaredInsecure, true),
                (Fact::SealedPaths, false),
            ],
        );
        assert_eq!(
            facts.served_families,
            [ContractFamily::ProcessRun, ContractFamily::Audit]
        );
        assert_eq!(facts.facts, [Fact::DeclaredInsecure, Fact::HostNetwork]);
    }
}
