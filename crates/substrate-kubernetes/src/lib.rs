//! The Kubernetes namespace driver (ADR 0047, `docs/design/34-kubernetes-namespace-driver.md`):
//! workspaces on `PersistentVolumeClaim`s and executions as one-shot Pods in one namespace the
//! daemon claims.
//!
//! This is the driver's core (design 34 § 12 U6):
//!
//! - [`config`]: the closed kubeconfig context extension [`EXTENSION`] (§ 3);
//! - [`names`]: every object name, label and the operation annotation, derived before dispatch
//!   from the deployment and the resource id (§ 4);
//! - [`ownership`]: whether an object is ours by name, label and uid (§ 4, decision 4);
//! - [`lease`]: the namespace claim on this process's monotonic clock (§ 6.1 step 3, D6);
//! - [`rules`]: the Role the credential may hold and the review that refuses more (step 2);
//! - [`policy`]: the default-deny check and the policy-set digest (step 4, § 14 Q1 B);
//! - [`Binding`]: the backend identity bound into the snapshot and re-read before a Pod create
//!   (§ 6.3);
//! - [`NamespaceDriver`]: the open sequence, steps 1 to 5, each refusing before the next.
//!
//! The reason codes below are log and readiness reasons (§ 6.1), not wire refusals: the driver's
//! wire refusals arrive with the successor bundle (§ 11, § 14 Q3). The driver creates no
//! cluster-scoped object and names no Kubernetes client crate.

pub mod binding;
pub mod config;
pub mod lease;
pub mod names;
mod namespace;
pub mod ownership;
pub mod policy;
pub mod rules;

#[cfg(test)]
mod driver_tests;

pub use binding::Binding;
pub use config::NamespaceConfig;
pub use namespace::{NamespaceDriver, Removal};

/// The kubeconfig context extension that carries the driver's settings.
pub const EXTENSION: &str = "b10x-substrate/namespace-driver";
/// The coordination `Lease` that claims the namespace.
pub const LEASE_NAME: &str = "b10x-substrate-driver";

/// Step 1: the kubeconfig or the extension is refused.
pub const CONFIG_INVALID: &str = "kubernetes.config-invalid";
/// Step 2: the credential visibly holds more than the driver's Role.
pub const RBAC_EXCESS: &str = "kubernetes.rbac-excess";
/// Step 3: another deployment holds the Lease.
pub const LEASE_FOREIGN: &str = "kubernetes.lease-foreign";
/// Step 3: a live instance of this deployment holds the Lease.
pub const LEASE_HELD: &str = "kubernetes.lease-held";
/// Step 4: the operator's policy is absent or does not deny everything.
pub const POLICY_NOT_DEFAULT_DENY: &str = "kubernetes.policy-not-default-deny";
/// Step 5: a privileged Pod is admitted, so the namespace does not enforce `restricted`.
pub const POD_SECURITY_UNENFORCED: &str = "kubernetes.pod-security-unenforced";
/// The cluster API answered nothing a step can read (§ 8's failure code).
pub const API_FAILED: &str = "kubernetes.api-failed";
/// A start whose binding moved, or whose Lease is lost (§ 6.3, § 8).
pub const CAPABILITY_STALE: &str = "exec.capability-stale";

/// Why the driver did not open: a reason code above and a message naming the fault, never a
/// credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupRefusal {
    /// The reason code.
    pub code: &'static str,
    /// What was refused.
    pub message: String,
}

impl std::fmt::Display for StartupRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for StartupRefusal {}
