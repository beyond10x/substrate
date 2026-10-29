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
//! - [`NamespaceDriver`]: the open sequence, steps 1 to 5, each refusing before the next, and
//!   step 7, the probe and the snapshot it backs (U10), served by [`NamespaceDriver::serve`].
//!
//! The reason codes below are log and readiness reasons (§ 6.1), not wire refusals: the driver's
//! wire refusals arrive with the successor bundle (§ 11, § 14 Q3). The driver creates no
//! cluster-scoped object and names no Kubernetes client crate.

pub mod binding;
pub mod config;
mod exec;
pub mod lease;
pub mod logs;
pub mod names;
mod namespace;
pub mod ownership;
pub mod pod;
pub mod policy;
pub mod rules;

#[cfg(test)]
mod driver_tests;
#[cfg(test)]
mod exec_tests;

pub use binding::Binding;
pub use config::NamespaceConfig;
pub use namespace::files::EXECUTOR_UNANSWERED;
pub use namespace::workspace::{EXCEEDED_QUOTA, WORKSPACE_CAPACITY};
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
/// Step 7: the probe keeper's executor did not pass the handshake (§ 6.1).
pub const EXECUTOR_HANDSHAKE_FAILED: &str = "kubernetes.executor-handshake-failed";
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

/// The driver port (design 34 § 1). The workspace and file routes are design 34 U8's, the exec
/// routes U9's, and the snapshot U10's: what the probe proved, under the binding and the Lease it
/// was proved under, and every other driver fact absent.
#[async_trait::async_trait]
impl substrate_host::Driver for NamespaceDriver {
    // --- the snapshot (U10) and the exec routes (U9) -------------------------------------

    fn machine(&self) -> substrate_wire::CapabilitySnapshot {
        let snapshot = self.capability_snapshot();
        self.publish_exec_snapshot(&snapshot.snapshot, snapshot.config_generation);
        snapshot
    }

    async fn shutdown(&self) -> Result<(), substrate_host::DriverError> {
        Ok(())
    }

    async fn start_exec(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &substrate_wire::ExecStartInput,
    ) -> substrate_host::DispatchOutcome<substrate_host::ExecObservation> {
        self.exec_start(id, workspace_root_name, input).await
    }

    async fn observe_exec(
        &self,
        id: &str,
    ) -> Result<substrate_host::ExecObservation, substrate_host::DriverError> {
        self.exec_observe(id).await
    }

    async fn output(
        &self,
        id: &str,
        query: &substrate_wire::ExecOutputQuery,
    ) -> Result<substrate_wire::OutputSlice, substrate_host::DriverError> {
        self.exec_output(id, query)
    }

    async fn signal(
        &self,
        id: &str,
        input: &substrate_wire::ExecSignalInput,
    ) -> Result<substrate_host::ExecObservation, substrate_host::DriverError> {
        self.exec_signal(id, input).await
    }

    fn completed_execs(&self) -> Vec<substrate_host::ExecObservation> {
        self.exec_completed()
    }

    fn set_exec_lease(&self, id: &str, lease: Option<substrate_wire::LeaseObservation>) {
        self.exec_set_lease(id, lease);
    }

    fn acknowledge_exec(&self, persisted: &substrate_host::ExecObservation) {
        self.exec_acknowledge(persisted);
    }

    fn discard_superseded_exec(&self, id: &str) {
        self.exec_discard(id);
    }

    // --- names and the dispatch bound (U6) -----------------------------------------------

    fn workspace_root_identity(&self, id: &str) -> Result<String, substrate_host::DriverError> {
        Ok(NamespaceDriver::workspace_root_identity(self, id))
    }

    fn backend_names(
        &self,
        kind: substrate_host::BackendKind,
        id: &str,
    ) -> Vec<substrate_host::BackendName> {
        NamespaceDriver::backend_names(self, kind, id)
    }

    fn dispatch_bound(&self) -> Option<std::time::Duration> {
        Some(NamespaceDriver::dispatch_bound(self))
    }

    // --- the workspace and file routes (U8) ----------------------------------------------

    async fn create_workspace(
        &self,
        id: &str,
        root_name: &str,
        input: &substrate_wire::WorkspaceCreateInput,
    ) -> substrate_host::DispatchOutcome<substrate_wire::Workspace> {
        self.workspace_create(id, root_name, input).await
    }

    async fn observe_workspace(
        &self,
        id: &str,
        root_name: &str,
        previous: &substrate_wire::Workspace,
    ) -> Result<substrate_wire::Workspace, substrate_host::DriverError> {
        // `workspace.adopt` asks to observe an operator's directory as a workspace: this driver
        // adopts none (§ 14 Q3 A), and says so before it looks at any name.
        if previous.origin == substrate_wire::WorkspaceOrigin::Adopted {
            return Err(adopt_unserved());
        }
        self.check_root(id, root_name)?;
        self.workspace_observe(id, root_name, previous).await
    }

    fn adopt_refusal(&self) -> Option<substrate_host::DriverError> {
        Some(adopt_unserved())
    }

    async fn read_workspace_path(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &substrate_wire::FileReadQuery,
    ) -> Result<substrate_host::Listed<substrate_wire::FileReadResult>, substrate_host::DriverError>
    {
        self.check_root(workspace_id, root_name)?;
        self.file_read(workspace_id, path, query).await
    }

    async fn write_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        content: &[u8],
    ) -> Result<substrate_wire::FileObservation, substrate_host::DriverError> {
        self.check_root(workspace_id, root_name)?;
        self.file_write(workspace_id, path, content).await
    }

    async fn delete_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
    ) -> Result<substrate_wire::FileAbsence, substrate_host::DriverError> {
        self.check_root(workspace_id, root_name)?;
        self.file_delete(workspace_id, path).await
    }

    async fn read_workspace_file_v2(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &substrate_wire::FileReadQuery,
    ) -> Result<substrate_wire::DigestedFileSlice, substrate_host::DriverError> {
        self.check_root(workspace_id, root_name)?;
        self.file_read_v2(workspace_id, path, query).await
    }

    async fn list_workspace_tree_v2(
        &self,
        workspace_id: &str,
        root_name: &str,
        query: &substrate_wire::WorkspaceTreeQuery,
    ) -> Result<substrate_host::Listed<substrate_wire::WorkspaceTree>, substrate_host::DriverError>
    {
        self.check_root(workspace_id, root_name)?;
        self.tree_read_v2(workspace_id, query).await
    }

    async fn replace_workspace_file_v2(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        input: &substrate_wire::FileReplaceInput,
    ) -> Result<substrate_wire::FileMutationResult, substrate_host::DriverError> {
        self.check_root(workspace_id, root_name)?;
        self.file_replace_v2(workspace_id, path, input).await
    }

    async fn edit_workspace_file_v2(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        input: &substrate_wire::FileEditInput,
    ) -> Result<substrate_wire::FileMutationResult, substrate_host::DriverError> {
        self.check_root(workspace_id, root_name)?;
        self.file_edit_v2(workspace_id, path, input).await
    }

    async fn patch_workspace_file_v2(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        input: &substrate_wire::FilePatchInput,
    ) -> Result<substrate_wire::FileMutationResult, substrate_host::DriverError> {
        self.check_root(workspace_id, root_name)?;
        self.file_patch_v2(workspace_id, path, input).await
    }

    async fn destroy_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<substrate_host::WorkspaceDestroyProgress, substrate_host::DriverError> {
        self.check_root(workspace_id, root_name)?;
        self.workspace_destroy(workspace_id).await
    }
}

/// `workspace.adopt-unserved` (501), as the wire crate states it: no address, never retriable.
fn adopt_unserved() -> substrate_host::DriverError {
    let refusal = substrate_wire::DRIVER_UNSERVED_REFUSALS
        .iter()
        .find(|refusal| refusal.code == substrate_wire::WORKSPACE_ADOPT_UNSERVED)
        .expect("the wire crate states the adopt refusal");
    substrate_host::DriverError {
        class: substrate_host::DriverErrorClass::Unserved,
        code: refusal.code,
        message: refusal.message.to_owned(),
        address: refusal.address.map(str::to_owned),
        retriable: false,
    }
}
