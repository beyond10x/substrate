//! The driver core: the open sequence of design 34 § 6.1 steps 1 to 5, the Lease it holds while
//! serving, the binding it re-reads before every Pod create, and deletion under the uid rule.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use substrate_host::{
    BackendKind, BackendLedger, BackendName, BackendRecord, BackendRole, DriverError, UidBinding,
};
use substrate_kubernetes_client::{Api, ApiError, ListForm, RUN_CONTAINER, kubeconfig};

use crate::binding::Binding;
use crate::config::{self, NamespaceConfig};
use crate::lease::{self, Clock, LeaseClaim, MonotonicClock};
use crate::ownership::{Ownership, ownership};
use crate::policy::{self, PolicySet, hex};
use crate::{
    API_FAILED, CAPABILITY_STALE, CONFIG_INVALID, LEASE_FOREIGN, LEASE_HELD,
    POD_SECURITY_UNENFORCED, POLICY_NOT_DEFAULT_DENY, RBAC_EXCESS, StartupRefusal, names, rules,
};

pub(crate) mod files;
pub(crate) mod probe;
#[cfg(test)]
mod probe_tests;
pub(crate) mod workspace;
#[cfg(test)]
mod workspace_tests;

/// The `User-Agent` the driver's requests send.
const USER_AGENT: &str = "b10x-substrate-kubernetes";
/// Items per page of a policy list, and its pages: the bound of § 5.2.
const POLICY_PAGE_ITEMS: usize = 100;
const POLICY_PAGES: usize = 64;
/// What Pod Security admission writes when a Pod violates the `restricted` level.
const RESTRICTED_VIOLATION: &str = "violates PodSecurity \"restricted:";
/// The daemon's default event retention, until [`NamespaceDriver::with_event_retention`] states
/// the configured one.
const DEFAULT_EVENT_RETENTION: u64 = 10_000;

/// Whether this host serves explicit leases (the daemon keeps lease time, whatever the driver).
fn lease_clock_is_provable() -> bool {
    #[cfg(target_os = "linux")]
    {
        substrate_host::probe_lease_clock()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// What [`NamespaceDriver::remove_owned`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// The object is gone and its row is settled.
    Absent,
    /// The deletion was accepted and the object still reads present: ask again.
    Pending,
    /// The object under the row's name is not ours (§ 4): untouched, counted, and the row is
    /// unchanged.
    Collision,
    /// The object reads absent, but no create answer is bound to the row and the N9 bound
    /// (§ 5.1, [`NamespaceDriver::dispatch_bound`]) has not passed since the row was reserved:
    /// an unanswered create may still be stored. The row stays open: ask again.
    Unconfirmed,
}

/// The Kubernetes namespace driver's core: one claimed namespace, its settings, its Lease and the
/// binding a snapshot promises.
pub struct NamespaceDriver {
    api: Api,
    config: NamespaceConfig,
    deployment: String,
    deployment_label: String,
    server: String,
    ca_sha256: String,
    ledger: Arc<dyn BackendLedger>,
    clock: Arc<dyn Clock>,
    holder: String,
    claim: tokio::sync::Mutex<LeaseClaim>,
    /// The claim's last successful write, mirrored so [`NamespaceDriver::lease_held`] needs no
    /// await.
    renewed: Mutex<Option<Instant>>,
    binding: Mutex<Option<Binding>>,
    collisions: AtomicU64,
    incomplete_reviews: AtomicU64,
    execs: crate::exec::Execs,
    probe: Mutex<probe::ProbeState>,
    /// Whether the first open sequence has finished or failed (§ 6.1): the open was refused, or
    /// the first probe after it ran. Set once and never cleared.
    opening_settled: tokio::sync::watch::Sender<bool>,
    /// The daemon's event retention, which the snapshot states as `events.retention-events`.
    event_retention: u64,
    /// Whether this host serves explicit leases, which the daemon keeps.
    lease_clock: bool,
}

impl std::fmt::Debug for NamespaceDriver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NamespaceDriver")
            .field("api", &self.api)
            .field("deployment", &self.deployment)
            .field("holder", &self.holder)
            .finish_non_exhaustive()
    }
}

impl NamespaceDriver {
    /// Step 1: loads the kubeconfig at `path` through design 28's loader and reads the
    /// extension. Nothing is sent to the cluster.
    ///
    /// # Errors
    ///
    /// `kubernetes.config-invalid`, naming the fault.
    pub fn load(
        path: &Path,
        deployment: &str,
        ledger: Arc<dyn BackendLedger>,
    ) -> Result<Self, StartupRefusal> {
        let bytes = kubeconfig::read(path).map_err(config_invalid)?;
        Self::parse(&bytes, deployment, ledger)
    }

    /// Step 1 over kubeconfig `bytes`.
    ///
    /// # Errors
    ///
    /// `kubernetes.config-invalid`, naming the fault.
    pub fn parse(
        bytes: &[u8],
        deployment: &str,
        ledger: Arc<dyn BackendLedger>,
    ) -> Result<Self, StartupRefusal> {
        Self::parse_with_clock(bytes, deployment, ledger, Arc::new(MonotonicClock))
    }

    pub(crate) fn parse_with_clock(
        bytes: &[u8],
        deployment: &str,
        ledger: Arc<dyn BackendLedger>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StartupRefusal> {
        if deployment.is_empty()
            || !deployment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(config_invalid(
                "the deployment must be a non-empty stable identifier".to_owned(),
            ));
        }
        let (mut api, config) =
            kubeconfig::parse(bytes, config::settings).map_err(config_invalid)?;
        api.user_agent = USER_AGENT;
        let (server, ca_sha256) = cluster_identity(bytes).map_err(config_invalid)?;
        let claim = LeaseClaim::new(deployment, &names::probe_nonce());
        Ok(Self {
            api,
            config,
            deployment: deployment.to_owned(),
            deployment_label: names::deployment_label(deployment),
            server,
            ca_sha256,
            ledger,
            clock,
            holder: claim.holder().to_owned(),
            claim: tokio::sync::Mutex::new(claim),
            renewed: Mutex::new(None),
            binding: Mutex::new(None),
            collisions: AtomicU64::new(0),
            incomplete_reviews: AtomicU64::new(0),
            execs: crate::exec::Execs::default(),
            probe: Mutex::new(probe::ProbeState::default()),
            opening_settled: tokio::sync::watch::Sender::new(false),
            event_retention: DEFAULT_EVENT_RETENTION,
            lease_clock: lease_clock_is_provable(),
        })
    }

    /// States the daemon's event retention, which the snapshot publishes as
    /// `events.retention-events`.
    #[must_use]
    pub fn with_event_retention(mut self, events: u64) -> Self {
        self.event_retention = events;
        self
    }

    /// Steps 2 to 5, once: the rules review, the Lease, the default-deny policy and the
    /// privileged dry run, in that order, each refusing before the next is tried. On success the
    /// binding is computed and the driver is open; on any refusal it is not, whatever it was
    /// before. The caller retries a refused attempt after [`lease::RENEW_EVERY`].
    ///
    /// # Errors
    ///
    /// The step's reason code: `kubernetes.rbac-excess`, `kubernetes.lease-foreign`,
    /// `kubernetes.lease-held`, `kubernetes.policy-not-default-deny`,
    /// `kubernetes.pod-security-unenforced`, or `kubernetes.api-failed` for an answer no step can
    /// read.
    pub async fn open(&self) -> Result<(), StartupRefusal> {
        let opened = self.open_steps().await;
        *lock(&self.binding) = opened.as_ref().ok().cloned();
        opened.map(|_| ())
    }

    async fn open_steps(&self) -> Result<Binding, StartupRefusal> {
        self.check_rules().await?;
        self.claim_lease().await?;
        let policies = self
            .read_policies()
            .await
            .map_err(|failure| match failure {
                PolicyFailure::Api(message) => refusal(API_FAILED, message),
                PolicyFailure::NotDefaultDeny(message) => refusal(POLICY_NOT_DEFAULT_DENY, message),
            })?;
        self.check_pod_security().await?;
        Ok(self.binding_over(policies))
    }

    async fn check_rules(&self) -> Result<(), StartupRefusal> {
        let status = self
            .api
            .rules_review()
            .await
            .map_err(|error| refusal(API_FAILED, format!("the rules review: {error}")))?;
        let review = rules::review(&status);
        if review.incomplete {
            self.incomplete_reviews.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                namespace = %self.config.namespace,
                "the rules review is incomplete: only the rules it shows were checked"
            );
        }
        if review.excess_total == 0 {
            return Ok(());
        }
        Err(refusal(
            RBAC_EXCESS,
            format!(
                "the credential holds {} grant(s) beyond the driver's Role: {}",
                review.excess_total,
                review.excess.join(", ")
            ),
        ))
    }

    async fn claim_lease(&self) -> Result<(), StartupRefusal> {
        let mut claim = self.claim.lock().await;
        let attempt = claim.acquire(&self.api, self.clock.now()).await;
        *lock(&self.renewed) = claim.renewed_at();
        attempt.map_err(|refused| match refused {
            lease::Refused::Foreign(message) => refusal(LEASE_FOREIGN, message),
            lease::Refused::Held(message) => refusal(LEASE_HELD, message),
            lease::Refused::Api(message) => refusal(API_FAILED, message),
        })
    }

    async fn read_policies(&self) -> Result<PolicySet, PolicyFailure> {
        let listed = self
            .api
            .list(
                &self.api.policies(),
                "NetworkPolicyList",
                ListForm::Whole,
                POLICY_PAGE_ITEMS,
                POLICY_PAGES,
            )
            .await
            .map_err(|error| PolicyFailure::Api(format!("the NetworkPolicy list: {error}")))?;
        policy::read(&listed.items, &self.config.default_deny_policy)
            .map_err(|refused| PolicyFailure::NotDefaultDeny(refused.0))
    }

    /// Step 5: a `dryRun=All` create of a `privileged` Pod. Admitted, or refused for a level
    /// below `restricted`, the namespace does not enforce `restricted`.
    async fn check_pod_security(&self) -> Result<(), StartupRefusal> {
        let nonce = names::probe_nonce();
        let labels = names::labels(
            &self.deployment,
            BackendKind::Probe,
            &nonce,
            BackendRole::Pod,
        );
        let pod = json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {"name": format!("sbx-pr-{nonce}-dry-run"), "labels": labels},
            "spec": {
                "automountServiceAccountToken": false,
                "restartPolicy": "Never",
                "containers": [{
                    "name": RUN_CONTAINER,
                    "image": self.config.exec_image,
                    "securityContext": {"privileged": true},
                }],
            },
        });
        match self.api.create_dry_run(&self.api.pods(), &pod).await {
            Ok(_) => Err(refusal(
                POD_SECURITY_UNENFORCED,
                "a privileged Pod was admitted in a dry run: the namespace does not enforce Pod \
                 Security restricted",
            )),
            Err(ApiError::Answered {
                status: 403,
                detail,
                ..
            }) if detail.contains(RESTRICTED_VIOLATION) => Ok(()),
            Err(ApiError::Answered {
                status: 403,
                detail,
                ..
            }) if detail.contains("violates PodSecurity") => Err(refusal(
                POD_SECURITY_UNENFORCED,
                format!(
                    "the namespace enforces a Pod Security level other than restricted: {detail}"
                ),
            )),
            Err(error) => Err(refusal(
                API_FAILED,
                format!("the privileged dry run: {error}"),
            )),
        }
    }

    fn binding_over(&self, policies: PolicySet) -> Binding {
        Binding {
            server: self.server.clone(),
            ca_sha256: self.ca_sha256.clone(),
            namespace: self.config.namespace.clone(),
            lease_holder: self.holder.clone(),
            policies,
            executor_image: self.config.executor_image.clone(),
            exec_image: self.config.exec_image.clone(),
            storage_class: self.config.storage_class.clone(),
        }
    }

    /// Whether the last [`NamespaceDriver::open`] succeeded and no later one refused.
    pub fn opened(&self) -> bool {
        lock(&self.binding).is_some()
    }

    /// The binding the last successful open computed.
    pub fn binding(&self) -> Option<Binding> {
        lock(&self.binding).clone()
    }

    /// Whether the Lease is held now: the last successful write of it is under
    /// [`lease::HELD_FOR`] old on this process's monotonic clock.
    pub fn lease_held(&self) -> bool {
        lock(&self.renewed).is_some_and(|renewed| {
            self.clock.now().saturating_duration_since(renewed) < lease::HELD_FOR
        })
    }

    /// The holder identity this process writes into the Lease, `<deployment>/<nonce>`.
    pub fn lease_holder(&self) -> &str {
        &self.holder
    }

    /// One renewal of a held Lease; the caller renews every [`lease::RENEW_EVERY`]. A renewal
    /// that fails leaves the claim to lapse [`lease::HELD_FOR`] after the last one that did not.
    pub async fn renew(&self) {
        let mut claim = self.claim.lock().await;
        claim.renew(&self.api, self.clock.now()).await;
        *lock(&self.renewed) = claim.renewed_at();
    }

    /// What `start_exec` asks immediately before it creates a Pod (§ 6.3), for a start admitted
    /// under the snapshot whose `config_generation` is `admitted`: the driver's binding is still
    /// that one, the namespace's policy set, read again now, is the one it bound, and the Lease
    /// is held. A re-open over a changed set binds a new generation, so a start admitted under
    /// the old one is stale even though the driver is open again.
    ///
    /// Everything read before the policy `GET` is read again after it: a `GET` that runs long
    /// cannot outlast the Lease, or a refused re-open, and still pass on a check made before it.
    ///
    /// # Errors
    ///
    /// `exec.capability-stale` (refused, address `capability_snapshot`) for a lost Lease, a
    /// driver that is not open, a binding other than the admitted one, or a moved policy set;
    /// `kubernetes.api-failed` when the policies cannot be read, which proves nothing either way.
    pub async fn check_binding(&self, admitted: u64) -> Result<(), DriverError> {
        let Some(bound) = self.binding() else {
            return Err(stale("the namespace driver is not open"));
        };
        if bound.config_generation() != admitted {
            return Err(stale(
                "the namespace driver was opened again over another binding since the start was \
                 admitted",
            ));
        }
        match self.read_policies().await {
            Ok(policies) if policies == bound.policies => {}
            Ok(_) => {
                return Err(stale(
                    "the namespace's NetworkPolicy set changed since the snapshot",
                ));
            }
            Err(PolicyFailure::NotDefaultDeny(message)) => return Err(stale(&message)),
            Err(PolicyFailure::Api(message)) => {
                return Err(DriverError::failed(API_FAILED, message));
            }
        }
        if self.binding().as_ref() != Some(&bound) {
            return Err(stale(
                "the namespace driver's binding changed while the policies were read",
            ));
        }
        if !self.lease_held() {
            return Err(stale("the namespace Lease is not held"));
        }
        Ok(())
    }

    /// Deletes the object `row` records under the uid rule (§ 4): only when it is ours, with
    /// `preconditions.uid`, so a same-named replacement is refused by the API server and never
    /// deleted. A Pod is deleted with `grace_seconds`. An object whose create answer was lost is
    /// adopted (its uid bound) before it is deleted. The row is settled once the object reads
    /// absent, except that a row with no bound uid read absent stays open until the N9 bound has
    /// passed since it was reserved ([`Removal::Unconfirmed`]): until then a create whose answer
    /// was lost may still land, and must still be found and removed.
    ///
    /// # Errors
    ///
    /// `kubernetes.api-failed` when the API answers nothing usable; the ledger's own failure.
    pub async fn remove_owned(
        &self,
        row: &BackendRecord,
        grace_seconds: u64,
    ) -> Result<Removal, DriverError> {
        let path = format!("{}/{}", self.collection(row.role), row.name);
        let Some(object) = self
            .api
            .get(&path)
            .await
            .map_err(|error| api_failed(&error))?
        else {
            if row.uid.is_none() && !self.past_dispatch_bound(row) {
                return Ok(Removal::Unconfirmed);
            }
            self.ledger.settle(&row.name)?;
            return Ok(Removal::Absent);
        };
        let uid = match ownership(&object, Some(row), &self.deployment_label) {
            Ownership::Collision(reason) => return Ok(self.collision(&reason)),
            Ownership::Ours { uid, adopt: false } => uid,
            Ownership::Ours { uid, adopt: true } => match self.ledger.bind_uid(&row.name, &uid)? {
                UidBinding::Bound => uid,
                other => {
                    return Ok(
                        self.collision(&format!("{:?} could not be adopted: {other:?}", row.name))
                    );
                }
            },
        };
        match self
            .api
            .delete_preconditioned(&path, grace_seconds, &uid)
            .await
        {
            Ok(true) => {}
            Ok(false) => match self
                .api
                .get(&path)
                .await
                .map_err(|error| api_failed(&error))?
            {
                Some(present) if present["metadata"]["uid"] == uid.as_str() => {
                    return Ok(Removal::Pending);
                }
                _ => {}
            },
            Err(error) if error.status() == Some(409) => {
                return Ok(self.collision(&format!(
                    "{:?} was replaced between its read and its deletion",
                    row.name
                )));
            }
            Err(error) => return Err(api_failed(&error)),
        }
        self.ledger.settle(&row.name)?;
        Ok(Removal::Absent)
    }

    /// Whether the N9 bound has passed since `row` was reserved, on the driver's wall clock: only
    /// then is an unbound row's absence final, because an unanswered create may be stored until
    /// it (§ 5.1). A `created_at` after now counts as not passed.
    pub(crate) fn past_dispatch_bound(&self, row: &BackendRecord) -> bool {
        (self.clock.wall() - row.created_at)
            .to_std()
            .is_ok_and(|elapsed| elapsed > self.dispatch_bound())
    }

    /// Now, on the driver's wall clock: what an exec's wall bound is measured on (§ 9).
    pub(crate) fn wall_now(&self) -> chrono::DateTime<chrono::Utc> {
        self.clock.wall()
    }

    fn collection(&self, role: BackendRole) -> String {
        match role {
            BackendRole::Pvc => self.api.claims(),
            BackendRole::Keeper | BackendRole::Pod => self.api.pods(),
        }
    }

    pub(crate) fn collision(&self, reason: &str) -> Removal {
        self.collisions.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            reason,
            "a backend object collides with the ledger and is left untouched"
        );
        Removal::Collision
    }

    /// Collisions seen since this process started: `substrate_kubernetes_collisions_total`.
    pub fn collisions_total(&self) -> u64 {
        self.collisions.load(Ordering::Relaxed)
    }

    /// Rules reviews that answered `incomplete: true` since this process started.
    pub fn incomplete_reviews_total(&self) -> u64 {
        self.incomplete_reviews.load(Ordering::Relaxed)
    }

    /// The workspace's root identity: its PVC name, `sbx-ws-<h>`.
    pub fn workspace_root_identity(&self, id: &str) -> String {
        names::workspace_pvc(&self.deployment, id)
    }

    /// The objects the driver will create for one resource, named before any is (§ 4).
    pub fn backend_names(&self, kind: BackendKind, id: &str) -> Vec<BackendName> {
        names::backend_names(&self.deployment, kind, id)
    }

    /// The N9 bound (§ 5.1): the client's timeout plus the API server's stated request timeout.
    pub fn dispatch_bound(&self) -> Duration {
        self.api.timeout + Duration::from_secs(self.config.api_request_timeout_seconds)
    }

    /// The settings the extension states.
    pub fn config(&self) -> &NamespaceConfig {
        &self.config
    }

    /// The client of the claimed namespace.
    pub fn api(&self) -> &Api {
        &self.api
    }

    /// The deployment this driver serves.
    pub fn deployment(&self) -> &str {
        &self.deployment
    }

    /// The ledger the driver records its backend objects in.
    pub(crate) fn ledger(&self) -> &Arc<dyn BackendLedger> {
        &self.ledger
    }

    /// The value of the deployment label on every object this driver creates.
    pub(crate) fn deployment_label(&self) -> &str {
        &self.deployment_label
    }

    pub(crate) fn execs(&self) -> &crate::exec::Execs {
        &self.execs
    }
}

enum PolicyFailure {
    Api(String),
    NotDefaultDeny(String),
}

/// The server URL and the SHA-256 of the decoded CA data of the current context's cluster, read
/// after the loader admitted `bytes`, so every lookup it makes has already succeeded once.
fn cluster_identity(bytes: &[u8]) -> Result<(String, String), String> {
    let config: Value = serde_json::from_slice(bytes)
        .map_err(|_| "the cluster configuration is not JSON".to_owned())?;
    let named = |list: &str, name: &Value| {
        config[list]
            .as_array()
            .and_then(|entries| entries.iter().find(|entry| entry["name"] == *name))
            .cloned()
    };
    let context =
        named("contexts", &config["current-context"]).ok_or("the current context is absent")?;
    let cluster = named("clusters", &context["context"]["cluster"])
        .ok_or("the context's cluster is absent")?;
    let server = cluster["cluster"]["server"]
        .as_str()
        .ok_or("the cluster names no server")?
        .to_owned();
    let authority = base64::engine::general_purpose::STANDARD
        .decode(
            cluster["cluster"]["certificate-authority-data"]
                .as_str()
                .ok_or("the cluster names no certificate-authority-data")?,
        )
        .map_err(|_| "certificate-authority-data is not base64".to_owned())?;
    Ok((server, hex(&Sha256::digest(&authority))))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn refusal(code: &'static str, message: impl Into<String>) -> StartupRefusal {
    StartupRefusal {
        code,
        message: message.into(),
    }
}

/// Whether an open refused with `code` settles the first open sequence (§ 6.1, ADR 0045 D5 A):
/// a refusal no retry clears does; a Lease another instance or deployment holds, and an API
/// that did not answer or answered nothing a step can read, are retried every
/// [`lease::RENEW_EVERY`] and do not.
fn settles_opening(code: &str) -> bool {
    matches!(
        code,
        CONFIG_INVALID | RBAC_EXCESS | POLICY_NOT_DEFAULT_DENY | POD_SECURITY_UNENFORCED
    )
}

fn config_invalid(message: String) -> StartupRefusal {
    refusal(CONFIG_INVALID, message)
}

fn stale(why: &str) -> DriverError {
    DriverError::refused(
        CAPABILITY_STALE,
        format!("The admitted capability snapshot is stale: {why}."),
        "capability_snapshot",
    )
}

fn api_failed(error: &ApiError) -> DriverError {
    DriverError::failed(API_FAILED, format!("the cluster API: {error}"))
}
