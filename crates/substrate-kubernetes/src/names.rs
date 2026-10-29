//! Object names, labels and the operation annotation (design 34 § 4, ADR 0047 decision 4).
//!
//! Every name is derived before dispatch from the deployment and the resource id, so the daemon
//! can reserve it in the transaction that records `accepted` and nothing about it is chosen by a
//! request. No subject, uid or credential is written into the cluster.

use std::collections::BTreeMap;

use sha2::{Digest as _, Sha256};
use substrate_host::{BackendKind, BackendName, BackendRole};

/// The label every object the driver creates carries, with [`MANAGED_BY`].
pub const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
/// The value of [`MANAGED_BY_LABEL`].
pub const MANAGED_BY: &str = "b10x-substrate";
/// The label naming the deployment an object belongs to, by [`deployment_label`].
pub const DEPLOYMENT_LABEL: &str = "b10x-substrate/deployment";
/// The label naming what an object is to its resource: `pvc`, `keeper` or `pod`.
pub const ROLE_LABEL: &str = "b10x-substrate/role";
/// The annotation carrying `sha256:<operation digest>`: what proves a create whose answer was
/// lost was ours.
pub const OPERATION_ANNOTATION: &str = "b10x-substrate/operation";

/// The domain separator of [`resource_hash`].
const NAME_DOMAIN: &[u8] = b"substrate-kubernetes-name\0";
/// Characters of a resource hash or a deployment label: 130 bits of base32.
const HASH_CHARS: usize = 26;
/// Random bytes of a probe nonce: 80 bits, 16 base32 characters.
const NONCE_BYTES: usize = 10;

/// `<h>`: the first 26 characters of lowercase RFC 4648 base32 (no padding) of SHA-256 over
/// `"substrate-kubernetes-name\0" ‖ u32be(len) ‖ deployment ‖ u32be(len) ‖ resource id`.
#[must_use]
pub fn resource_hash(deployment: &str, id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(NAME_DOMAIN);
    for part in [deployment, id] {
        hasher.update(u32::try_from(part.len()).unwrap_or(u32::MAX).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    truncated_base32(&hasher.finalize())
}

/// The value of [`DEPLOYMENT_LABEL`]: 26 characters of base32 of SHA-256 of the deployment.
#[must_use]
pub fn deployment_label(deployment: &str) -> String {
    truncated_base32(&Sha256::digest(deployment.as_bytes()))
}

/// The workspace's PVC, `sbx-ws-<h>`: also the workspace's root identity.
#[must_use]
pub fn workspace_pvc(deployment: &str, id: &str) -> String {
    format!("sbx-ws-{}", resource_hash(deployment, id))
}

/// The workspace's keeper Pod, `sbx-ws-<h>-keeper`.
#[must_use]
pub fn keeper_pod(deployment: &str, id: &str) -> String {
    format!("sbx-ws-{}-keeper", resource_hash(deployment, id))
}

/// The exec's Pod, `sbx-ex-<h>`.
#[must_use]
pub fn exec_pod(deployment: &str, id: &str) -> String {
    format!("sbx-ex-{}", resource_hash(deployment, id))
}

/// A fresh probe nonce: 16 lowercase base32 characters from the operating system's generator.
///
/// # Panics
///
/// When the operating system's generator cannot be read: no probe may reuse another's names.
#[must_use]
pub fn probe_nonce() -> String {
    let mut bytes = [0_u8; NONCE_BYTES];
    getrandom::fill(&mut bytes).expect("the operating system's random generator is readable");
    base32(&bytes)
}

/// The objects of one probe, `sbx-pr-<nonce>-{pvc,keeper,pod}`, in creation order.
#[must_use]
pub fn probe_names(nonce: &str) -> Vec<BackendName> {
    [
        (BackendRole::Pvc, "pvc"),
        (BackendRole::Keeper, "keeper"),
        (BackendRole::Pod, "pod"),
    ]
    .into_iter()
    .map(|(role, suffix)| BackendName {
        role,
        name: format!("sbx-pr-{nonce}-{suffix}"),
    })
    .collect()
}

/// The names [`crate::NamespaceDriver::backend_names`] answers for one resource: a workspace's PVC
/// and keeper, an exec's Pod. A probe's are reserved by nonce ([`probe_names`]), so none here.
#[must_use]
pub fn backend_names(deployment: &str, kind: BackendKind, id: &str) -> Vec<BackendName> {
    match kind {
        BackendKind::Workspace => vec![
            BackendName {
                role: BackendRole::Pvc,
                name: workspace_pvc(deployment, id),
            },
            BackendName {
                role: BackendRole::Keeper,
                name: keeper_pod(deployment, id),
            },
        ],
        BackendKind::Exec => vec![BackendName {
            role: BackendRole::Pod,
            name: exec_pod(deployment, id),
        }],
        BackendKind::Probe => Vec::new(),
    }
}

/// The four labels of § 4: managed-by, the deployment, the resource label naming `value` (the
/// resource hash, or a probe's nonce) and the role.
#[must_use]
pub fn labels(
    deployment: &str,
    kind: BackendKind,
    value: &str,
    role: BackendRole,
) -> BTreeMap<&'static str, String> {
    let resource = match kind {
        BackendKind::Workspace => "b10x-substrate/workspace",
        BackendKind::Exec => "b10x-substrate/exec",
        BackendKind::Probe => "b10x-substrate/probe",
    };
    BTreeMap::from([
        (MANAGED_BY_LABEL, MANAGED_BY.to_owned()),
        (DEPLOYMENT_LABEL, deployment_label(deployment)),
        (resource, value.to_owned()),
        (ROLE_LABEL, role_name(role).to_owned()),
    ])
}

/// The value of [`ROLE_LABEL`] for `role`.
#[must_use]
pub fn role_name(role: BackendRole) -> &'static str {
    match role {
        BackendRole::Pvc => "pvc",
        BackendRole::Keeper => "keeper",
        BackendRole::Pod => "pod",
    }
}

/// The value of [`OPERATION_ANNOTATION`] for a ledger row's `operation_digest` (lowercase hex).
#[must_use]
pub fn operation_annotation(operation_digest: &str) -> String {
    format!("sha256:{operation_digest}")
}

fn truncated_base32(digest: &[u8]) -> String {
    let mut text = base32(digest);
    text.truncate(HASH_CHARS);
    text
}

/// Lowercase RFC 4648 base32 of `bytes`, without padding.
fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut text = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits = 0_u32;
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            text.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 {
        text.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::base32;

    #[test]
    fn base32_is_rfc_4648_lowercase_without_padding() {
        for (input, expected) in [
            ("", ""),
            ("f", "my"),
            ("fo", "mzxq"),
            ("foo", "mzxw6"),
            ("foob", "mzxw6yq"),
            ("fooba", "mzxw6ytb"),
            ("foobar", "mzxw6ytboi"),
        ] {
            assert_eq!(base32(input.as_bytes()), expected, "{input:?}");
        }
    }
}
