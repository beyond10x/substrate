//! The namespace claim (ADR 0047 decision 2, design 34 § 6.1 step 3 and § 13 D6).
//!
//! One coordination `Lease`, [`crate::LEASE_NAME`], holder `<deployment>/<nonce>`,
//! `leaseDurationSeconds: 30`. Every rule is read on this process's monotonic clock, never on a
//! timestamp in the Lease, so no clock skew between two daemons can hand one the other's claim:
//!
//! - a Lease another instance of the same deployment holds is taken over only once its
//!   `resourceVersion` has stayed unchanged for [`TAKEOVER_AFTER`] of this process's own
//!   observation (client-go's rule);
//! - a Lease another deployment holds is never taken over, however old: two deployments would
//!   split one namespace's objects between two ledgers;
//! - the claim is **held** while the last successful renewal is under [`HELD_FOR`] old, and is
//!   renewed every [`RENEW_EVERY`].

use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{Value, json};
use substrate_kubernetes_client::{Api, ApiError};

use crate::LEASE_NAME;
use crate::names::{self, DEPLOYMENT_LABEL, MANAGED_BY, MANAGED_BY_LABEL};

/// The Lease's `leaseDurationSeconds`.
pub const LEASE_DURATION_SECONDS: u64 = 30;
/// How long a same-deployment Lease's `resourceVersion` must stay unchanged before it is taken
/// over.
pub const TAKEOVER_AFTER: Duration = Duration::from_secs(30);
/// How often a held Lease is renewed, and how often a refused claim is retried.
pub const RENEW_EVERY: Duration = Duration::from_secs(10);
/// How long after its last successful renewal the claim is still held.
pub const HELD_FOR: Duration = Duration::from_secs(20);

/// A monotonic clock. The shipped driver reads [`MonotonicClock`]; a test steps its own.
pub trait Clock: Send + Sync {
    /// Now, on this process's monotonic clock.
    fn now(&self) -> Instant;
}

/// [`Instant::now`].
#[derive(Debug, Clone, Copy, Default)]
pub struct MonotonicClock;

impl Clock for MonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Why a claim attempt did not hold the Lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Another deployment holds it.
    Foreign(String),
    /// Another live instance of this deployment holds it, or took it first.
    Held(String),
    /// The API answered nothing usable.
    Api(String),
}

/// This process's claim on the Lease.
#[derive(Debug)]
pub struct LeaseClaim {
    deployment: String,
    holder: String,
    /// The Lease as this claim last wrote it, and when that write was answered.
    held: Option<(Value, Instant)>,
    /// Another holder's `resourceVersion` and when this process first saw it.
    observed: Option<(String, Instant)>,
}

impl LeaseClaim {
    /// A claim for `deployment`, holder `<deployment>/<nonce>`.
    pub fn new(deployment: &str, nonce: &str) -> Self {
        Self {
            deployment: deployment.to_owned(),
            holder: format!("{deployment}/{nonce}"),
            held: None,
            observed: None,
        }
    }

    /// The holder identity this claim writes.
    pub fn holder(&self) -> &str {
        &self.holder
    }

    /// Whether the last successful renewal is under [`HELD_FOR`] old at `now`.
    pub fn held(&self, now: Instant) -> bool {
        self.renewed_at()
            .is_some_and(|renewed| now.saturating_duration_since(renewed) < HELD_FOR)
    }

    /// When this claim's last write of the Lease was answered, while it holds one.
    pub fn renewed_at(&self) -> Option<Instant> {
        self.held.as_ref().map(|(_, renewed)| *renewed)
    }

    /// One attempt to hold the Lease at `now`: created when absent, renewed when this claim
    /// holds it, taken when free, or when another instance of this deployment left its
    /// `resourceVersion` unchanged for [`TAKEOVER_AFTER`].
    ///
    /// # Errors
    ///
    /// [`Refused`], naming the holder; the caller retries after [`RENEW_EVERY`].
    pub async fn acquire(&mut self, api: &Api, now: Instant) -> Result<(), Refused> {
        let path = path(api);
        let Some(lease) = api.get(&path).await.map_err(|error| api_refused(&error))? else {
            self.observed = None;
            let created = api
                .create(&api.leases(), &self.lease(0))
                .await
                .map_err(|error| match error.status() {
                    Some(409) => Refused::Held("another holder created the Lease first".into()),
                    _ => api_refused(&error),
                })?;
            self.held = Some((created, now));
            return Ok(());
        };
        let holder = lease["spec"]["holderIdentity"].as_str().unwrap_or_default();
        let transitions = lease["spec"]["leaseTransitions"].as_u64().unwrap_or(0);
        if holder == self.holder {
            return self.write(api, &lease, now, None).await;
        }
        if holder.is_empty() {
            return self.write(api, &lease, now, Some(transitions + 1)).await;
        }
        if holder.rsplit_once('/').map(|(deployment, _)| deployment) != Some(&self.deployment) {
            self.observed = None;
            return Err(Refused::Foreign(format!(
                "the Lease {LEASE_NAME} is held by another deployment, as {holder:?}"
            )));
        }
        let version = lease["metadata"]["resourceVersion"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        match &self.observed {
            Some((seen, since))
                if *seen == version && now.saturating_duration_since(*since) >= TAKEOVER_AFTER =>
            {
                self.write(api, &lease, now, Some(transitions + 1)).await
            }
            Some((seen, _)) if *seen == version => Err(held_by(holder)),
            _ => {
                self.observed = Some((version, now));
                Err(held_by(holder))
            }
        }
    }

    /// One renewal at `now`. A renewal that is not answered leaves the claim as it was, so it
    /// lapses [`HELD_FOR`] after the last one that was; a Lease another holder wrote meanwhile
    /// ends the claim at once, and so does a Lease that is gone. A Lease still naming this
    /// holder at another `resourceVersion` (a renewal whose answer was lost) is renewed once more
    /// from what the read showed.
    pub async fn renew(&mut self, api: &Api, now: Instant) {
        let Some((lease, _)) = self.held.clone() else {
            return;
        };
        if self.write(api, &lease, now, None).await.is_ok() {
            return;
        }
        match api.get(&path(api)).await {
            Ok(Some(current)) if current["spec"]["holderIdentity"] == self.holder.as_str() => {
                let _ = self.write(api, &current, now, None).await;
            }
            Ok(_) => self.held = None,
            Err(_) => {}
        }
    }

    /// Forgets the claim: what a caller does when it stops serving.
    pub fn release(&mut self) {
        self.held = None;
    }

    /// Writes `lease` as this claim's, conditional on its `resourceVersion`; `transitions` is
    /// the new `leaseTransitions` when the holder changes.
    async fn write(
        &mut self,
        api: &Api,
        lease: &Value,
        now: Instant,
        transitions: Option<u64>,
    ) -> Result<(), Refused> {
        let mut replacement = lease.clone();
        let stamp = micro_time();
        replacement["spec"]["holderIdentity"] = json!(self.holder);
        replacement["spec"]["leaseDurationSeconds"] = json!(LEASE_DURATION_SECONDS);
        replacement["spec"]["renewTime"] = json!(stamp);
        if let Some(transitions) = transitions {
            replacement["spec"]["acquireTime"] = json!(stamp);
            replacement["spec"]["leaseTransitions"] = json!(transitions);
        }
        let written = api
            .update(&path(api), &replacement)
            .await
            .map_err(|error| match error.status() {
                Some(409) => Refused::Held("the Lease changed while it was claimed".into()),
                _ => api_refused(&error),
            })?;
        self.observed = None;
        self.held = Some((written, now));
        Ok(())
    }

    fn lease(&self, transitions: u64) -> Value {
        let stamp = micro_time();
        json!({
            "apiVersion": "coordination.k8s.io/v1",
            "kind": "Lease",
            "metadata": {
                "name": LEASE_NAME,
                "labels": {
                    MANAGED_BY_LABEL: MANAGED_BY,
                    DEPLOYMENT_LABEL: names::deployment_label(&self.deployment),
                },
            },
            "spec": {
                "holderIdentity": self.holder,
                "leaseDurationSeconds": LEASE_DURATION_SECONDS,
                "acquireTime": stamp,
                "renewTime": stamp,
                "leaseTransitions": transitions,
            },
        })
    }
}

fn path(api: &Api) -> String {
    format!("{}/{LEASE_NAME}", api.leases())
}

fn held_by(holder: &str) -> Refused {
    Refused::Held(format!(
        "the Lease {LEASE_NAME} is held by a live instance of this deployment, as {holder:?}"
    ))
}

fn api_refused(error: &ApiError) -> Refused {
    Refused::Api(format!("the Lease {LEASE_NAME}: {error}"))
}

/// A Kubernetes `MicroTime` for now. Informational for other readers only: no rule here reads it.
fn micro_time() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}
