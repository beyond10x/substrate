//! The backend identity bound into the capability snapshot (design 34 § 6.3, ADR 0047 decision 9,
//! § 14 Q1 B): what a start's admitted snapshot promised about the cluster, re-read before every
//! Pod create.

use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::policy::PolicySet;

/// The domain separator of [`Binding::digest`].
const BINDING_DOMAIN: &str = "substrate-kubernetes-binding/1";

/// Every backend fact a snapshot binds beside the probed facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The API server URL the kubeconfig names.
    pub server: String,
    /// Lowercase hex SHA-256 of the cluster's decoded `certificate-authority-data`.
    pub ca_sha256: String,
    /// The namespace.
    pub namespace: String,
    /// This process's Lease holder identity.
    pub lease_holder: String,
    /// The default-deny policy's identity and the digest of the namespace's whole policy set.
    pub policies: PolicySet,
    /// The executor image, by digest.
    pub executor_image: String,
    /// The exec image, by digest.
    pub exec_image: String,
    /// The PVC storage class.
    pub storage_class: String,
}

impl Binding {
    /// SHA-256 over the canonical JSON of every member, under a domain separator. A change to any
    /// member changes it.
    ///
    /// # Panics
    ///
    /// Never: canonical JSON refuses only floating-point numbers, and the material holds strings.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let material = json!({
            "domain": BINDING_DOMAIN,
            "server": self.server,
            "ca_sha256": self.ca_sha256,
            "namespace": self.namespace,
            "lease_holder": self.lease_holder,
            "default_deny_policy": {
                "uid": self.policies.default_deny_uid,
                "resource_version": self.policies.default_deny_resource_version,
                "spec_sha256": self.policies.default_deny_spec,
            },
            "policy_set_sha256": self.policies.set_digest,
            "executor_image": self.executor_image,
            "exec_image": self.exec_image,
            "storage_class": self.storage_class,
        });
        let canonical = substrate_wire::canonical_json(&material)
            .expect("the binding holds strings alone, which always serialize canonically");
        Sha256::digest(canonical.as_bytes()).into()
    }

    /// The snapshot's `config_generation`: the first eight bytes of [`Binding::digest`], big
    /// endian, never 0. A moved binding moves it.
    #[must_use]
    pub fn config_generation(&self) -> u64 {
        let digest = self.digest();
        let mut first = [0_u8; 8];
        first.copy_from_slice(&digest[..8]);
        u64::from_be_bytes(first).max(1)
    }
}
