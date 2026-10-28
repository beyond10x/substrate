//! The profile a client requests (ADR 0109 decision 1). Each field maps to the contract family or
//! the fact it requires, so an adapter that does not serve it refuses it by that name.

use std::path::PathBuf;

use substrate_wire::{
    ReadOnlyFile, ReadOnlyRoot, SecretSlotRequest, WorkspaceAccess, WritableHostRoot,
};

use super::AuditSink;
use super::facts::{FACT_ORDER, FAMILY_ORDER, InnerAdapter};
use super::{ContractFamily, Fact};
use crate::output_retention::OutputRetention;

/// What a run may reach and how it is bounded.
///
/// | field | requires |
/// |---|---|
/// | `families`, `facts` | themselves: what the client needs beyond its fields |
/// | `workspace`, `workdir` | `workspace` |
/// | `workspace_access` other than `ReadWrite` | `workspace` |
/// | `output_retention` other than `Head` on both streams | `output_read` |
/// | `writable_roots`, `read_only_roots`, `read_only_files` | `filesystem_view` |
/// | `sealed_paths` | the fact `sealed_paths` |
/// | `exec_subpaths` | the fact `exec_subpaths` |
/// | `private_tmpdir` | the fact `private_tmpdir` |
/// | `network: Egress` | `network_egress` |
/// | `network: Isolated` | `network_egress` on an adapter that proves `host_network` |
/// | `ipc.socket_allowlist` | the fact `ipc_socket_allowlist` |
/// | `ipc.signals_contained` | the fact `ipc_signals_contained` |
/// | `process_bounds`, `io_bounds` | `limits` |
/// | `stdin: Piped` | `process_run` and the fact `stdin_pipe` (design 32) |
/// | `audit` | `audit` is checked, and records go to the sink |
/// | `image` | an `image_digest_pinned` adapter's digest rule |
/// | `ingress_routes`, `rate_bounds` | reserved: always refused (ADR 0109 F1 B) |
///
/// `secret_slots` names no family: an adapter that cannot seal a slot refuses it with the host's
/// own `exec.secret-slots-unserved`. Neither does the mode of `workspace_access` beyond its
/// family: an adapter that cannot bind a read-only or scoped workspace refuses it with the host's
/// own `exec.workspace-scoped-write-unserved` (design 29 § 4).
#[derive(Debug, Clone, Default)]
pub struct ConfinementProfile {
    /// Families the client needs beyond what its fields imply.
    pub families: Vec<ContractFamily>,
    /// Facts the client requires beyond what its fields imply.
    pub facts: Vec<Fact>,
    /// The adapter a `systemd` profile expects to run inside. Absent for every other adapter; a
    /// value the opened adapter does not wrap is `confinement.adapter-unserved`.
    pub inner: Option<InnerAdapter>,
    /// Why a declared-insecure run is acceptable. Required, and not blank, wherever the opened
    /// adapter proves `declared_insecure` (ADR 0109 decision 4).
    pub local_reason: Option<String>,
    /// The host directory the run starts in and may write: the named writable subtree. Under
    /// `linux` it is a directory directly beneath the driver's workspace root, mounted at
    /// `/workspace`; under `local` it is the working directory itself.
    pub workspace: Option<PathBuf>,
    /// A relative path beneath `workspace` to start in (design 26).
    pub workdir: Option<String>,
    /// What the run may write inside `workspace` (ADR 0023). `ReadWrite`, the default, is the
    /// whole workspace, as before.
    pub workspace_access: WorkspaceAccess,
    /// Operator-declared writable host roots outside the workspace (ADR 0038 kind 1).
    pub writable_roots: Vec<WritableHostRoot>,
    /// Host directories mounted read-only (ADR 0010).
    pub read_only_roots: Vec<ReadOnlyRoot>,
    /// Single host files mounted read-only (ADR 0033).
    pub read_only_files: Vec<ReadOnlyFile>,
    /// Paths inside the workspace that stay unwritable.
    pub sealed_paths: Vec<String>,
    /// Subpaths exec is admitted beneath, beyond the literal program.
    pub exec_subpaths: Vec<String>,
    /// A per-run `TMPDIR`, invisible to another run and gone after `wait`.
    pub private_tmpdir: bool,
    /// The network policy. Absent states none, and the adapter's facts say what the run gets:
    /// `linux` keeps its no-egress floor, `local` proves `host_network`.
    pub network: Option<Network>,
    pub ipc: Ipc,
    pub process_bounds: Option<ProcessBounds>,
    pub io_bounds: Option<IoBounds>,
    /// Which bytes of each output stream the run keeps within its bound (design 29 § 5). `Head`
    /// on both, the default, is what every adapter kept before.
    pub output_retention: OutputRetention,
    /// The run's standard input (design 32). `Null`, the default, is `/dev/null`, as before.
    pub stdin: StdinMode,
    pub secret_slots: Vec<SecretSlotRequest>,
    /// Where admission, refusal and start records go (ADR 0109 decision 2).
    pub audit: Option<AuditSink>,
    /// The image an `oci` or `pod` run uses, `name@sha256:<64 lowercase hex>`.
    pub image: Option<String>,
    /// Reserved by ADR 0109 F1 B: any value is `confinement.reserved-field-unserved`.
    pub ingress_routes: Option<serde_json::Value>,
    /// Reserved by ADR 0109 F1 B: any value is `confinement.reserved-field-unserved`.
    pub rate_bounds: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    /// No network at all.
    Isolated,
    /// Only the named operator-declared egress apertures (ADR 0013, design 26).
    Egress { apertures: Vec<String> },
}

/// A run's standard input (design 32 § 2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum StdinMode {
    /// `/dev/null`.
    #[default]
    Null,
    /// A pipe the embedder writes with `write_stdin` and ends with `close_stdin`, taking at most
    /// `limit_bytes` in total: 1 to [`MAX_STDIN_BYTES`](super::MAX_STDIN_BYTES).
    Piped { limit_bytes: u64 },
}

impl StdinMode {
    /// Whether the run's standard input is a pipe.
    pub fn is_piped(self) -> bool {
        matches!(self, Self::Piped { .. })
    }
}

/// IPC the run is confined to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ipc {
    /// `socket(2)` is held to the adapter's allowlist (ADR 0039).
    pub socket_allowlist: bool,
    /// No signal reaches a process outside the run's tree.
    pub signals_contained: bool,
}

/// Bounds on the run's process tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessBounds {
    pub timeout_ms: u64,
    pub processes: u32,
    pub memory_bytes: u64,
    /// Cumulative CPU time across the tree.
    pub cpu_millis: u64,
}

/// Bounds on what the run produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoBounds {
    pub output_bytes: u64,
}

impl ConfinementProfile {
    /// The families this profile needs, explicit and implied by its fields, in decision 8's order.
    ///
    /// The audit sink is not among them: it is where records go, and the record lists `audit`
    /// only when the client named it. Admission still refuses a sink on an adapter that does not
    /// serve `audit`.
    ///
    /// Nor is `network_egress` for an isolated network: admission requires it only of an adapter
    /// that proves `host_network`, which the profile alone cannot know.
    pub fn required_families(&self) -> Vec<ContractFamily> {
        let implied = |family: &ContractFamily| match family {
            ContractFamily::ProcessRun => self.stdin.is_piped(),
            ContractFamily::Workspace => {
                self.workspace.is_some()
                    || self.workdir.is_some()
                    || !self.workspace_access.is_read_write()
            }
            ContractFamily::OutputRead => !self.output_retention.is_head(),
            ContractFamily::FilesystemView => {
                !self.writable_roots.is_empty()
                    || !self.read_only_roots.is_empty()
                    || !self.read_only_files.is_empty()
            }
            ContractFamily::NetworkEgress => {
                matches!(self.network, Some(Network::Egress { .. }))
            }
            ContractFamily::Limits => self.process_bounds.is_some() || self.io_bounds.is_some(),
            _ => false,
        };
        FAMILY_ORDER
            .iter()
            .filter(|family| self.families.contains(family) || implied(family))
            .cloned()
            .collect()
    }

    /// The facts this profile requires, explicit and implied by its fields, in the `Fact` order.
    pub fn required_facts(&self) -> Vec<Fact> {
        let implied = |fact: &Fact| match fact {
            Fact::SealedPaths => !self.sealed_paths.is_empty(),
            Fact::ExecSubpaths => !self.exec_subpaths.is_empty(),
            Fact::PrivateTmpdir => self.private_tmpdir,
            Fact::IpcSocketAllowlist => self.ipc.socket_allowlist,
            Fact::IpcSignalsContained => self.ipc.signals_contained,
            Fact::StdinPipe => self.stdin.is_piped(),
            _ => false,
        };
        FACT_ORDER
            .iter()
            .filter(|fact| self.facts.contains(fact) || implied(fact))
            .cloned()
            .collect()
    }

    /// The reason, when it is present and not blank: it holds at least one [visible
    /// character](is_visible). Whitespace, format characters (U+200B, U+FEFF, U+2060 and the
    /// like) and fillers that render as nothing are blank to every reader of the audit record.
    pub fn reason(&self) -> Option<&str> {
        self.local_reason
            .as_deref()
            .filter(|reason| reason.chars().any(is_visible))
    }
}

/// Whether `character` shows as a letter or a digit to a reader: alphanumeric, and not one of the
/// Hangul fillers Unicode classes as letters although they render as nothing.
///
/// The rule admits by class rather than refusing a list of invisible characters, so a format,
/// control, combining, whitespace or symbol character added to Unicode later is not visible by
/// default. A reason is words; one made only of punctuation or symbols is no reason either.
fn is_visible(character: char) -> bool {
    character.is_alphanumeric()
        && !matches!(character, '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}')
}

/// Whether `image` names its content by digest: `name@sha256:` and exactly 64 lowercase
/// hexadecimal characters, which end the reference. `@sha256:` appears exactly once, and the name
/// before it is not empty, holds no `@`, no whitespace and no control character, and does not
/// start with `-` (it would read as an option to a runtime). A tag alone, an upper-case or short
/// digest, a second digest or anything after the digest is not pinned.
pub fn image_is_pinned(image: &str) -> bool {
    let Some((name, digest)) = image.split_once("@sha256:") else {
        return false;
    };
    !name.is_empty()
        && !name.starts_with('-')
        && !name.chars().any(|character| {
            character == '@' || character.is_whitespace() || character.is_control()
        })
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_imply_families_and_facts_in_the_contract_order() {
        let profile = ConfinementProfile {
            families: vec![ContractFamily::Audit, ContractFamily::ProcessRun],
            workdir: Some("sub".into()),
            process_bounds: Some(ProcessBounds {
                timeout_ms: 1,
                processes: 1,
                memory_bytes: 1,
                cpu_millis: 1,
            }),
            read_only_roots: vec![ReadOnlyRoot {
                host_path: "/usr".into(),
                mount: "/opt/usr".into(),
            }],
            sealed_paths: vec![".git".into()],
            ipc: Ipc {
                socket_allowlist: true,
                signals_contained: false,
            },
            facts: vec![Fact::WholeTreeKill],
            ..ConfinementProfile::default()
        };
        assert_eq!(
            profile.required_families(),
            [
                ContractFamily::ProcessRun,
                ContractFamily::Workspace,
                ContractFamily::FilesystemView,
                ContractFamily::Limits,
                ContractFamily::Audit,
            ]
        );
        assert_eq!(
            profile.required_facts(),
            [
                Fact::WholeTreeKill,
                Fact::IpcSocketAllowlist,
                Fact::SealedPaths
            ]
        );
    }

    // Design 32 § 2: a piped standard input requires running a process and the fact that proves
    // the pipe; the default `/dev/null` requires neither.
    #[test]
    fn a_piped_stdin_implies_process_run_and_stdin_pipe() {
        let piped = ConfinementProfile {
            stdin: StdinMode::Piped { limit_bytes: 1 },
            ..ConfinementProfile::default()
        };
        assert_eq!(piped.required_families(), [ContractFamily::ProcessRun]);
        assert_eq!(piped.required_facts(), [Fact::StdinPipe]);
        let null = ConfinementProfile::default();
        assert_eq!(null.stdin, StdinMode::Null);
        assert!(null.required_families().is_empty());
        assert!(null.required_facts().is_empty());
    }

    #[test]
    fn a_blank_reason_is_no_reason() {
        let mut profile = ConfinementProfile::default();
        assert_eq!(profile.reason(), None);
        profile.local_reason = Some(" \t".into());
        assert_eq!(profile.reason(), None);
        profile.local_reason = Some("why".into());
        assert_eq!(profile.reason(), Some("why"));
    }
}
