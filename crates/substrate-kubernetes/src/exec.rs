//! The exec path (design 34 § 5 and § 7.3): a start creates one Pod under the binding re-read
//! immediately before it, an exec ends only once its container is shown stopped, its output is
//! the executor's framed log, signals are uid-preconditioned deletions, and garbage rounds settle
//! the rows of ended execs once their Pods read absent.
//!
//! Nothing here fabricates an end: absence, a `Failed` phase and a watch gap never count as one
//! (§ 5.2), a force-deleted Pod is `unknown` with no exit (design 28), and bounds the executor
//! read back differently leave the exec `unknown`, counted.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use substrate_executor::frame::{ExitRecord, Refusal};
use substrate_executor::spec::RunSpec;
use substrate_guarded::environment;
use substrate_host::{
    BackendKind, BackendRecord, BackendRole, DispatchOutcome, DriverError, DriverErrorClass,
    ExecObservation, UidBinding,
};
use substrate_kubernetes_client::gone::WATCH_LIMIT;
use substrate_kubernetes_client::{
    ApiError, ListForm, MAX_API_BODY_BYTES, Method, PodEnd, PodTrace, RUN_CONTAINER, WatchError,
    WatchEvent, container_state,
};
use substrate_wire::{
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, Base64Content, Base64Encoding, Exec,
    ExecExit, ExecKind, ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState,
    LeaseObservation, NetworkMode, OutputSlice, OutputStream, SandboxProfile, Signal,
    WorkspaceAccess, WritableRoot,
};

use crate::logs::{self, EXECUTOR_PROTOCOL, RunEnd};
use crate::names::{self, DEPLOYMENT_LABEL, MANAGED_BY, MANAGED_BY_LABEL};
use crate::namespace::workspace::is_quota_refusal;
use crate::ownership::{Foreign, Ownership, ownership};
use crate::pod::{self, ExecPod, KILL_GRACE_SECONDS, RunBounds, TERM_GRACE_SECONDS};
use crate::{API_FAILED, NamespaceDriver, Removal};

/// A create the namespace refused for its quota (O7, D184): the `ResourceQuota` admission refusal.
pub const EXEC_CAPACITY: &str = "exec.capacity";
/// Reads of one exec's log before it counts as unreadable (design 28).
const LOG_ATTEMPTS: usize = 5;
const LOG_RETRY: Duration = Duration::from_millis(200);
/// Items per page of the exec Pod list, and its pages (§ 5.2).
const LIST_PAGE_ITEMS: usize = 100;
const LIST_PAGES: usize = 64;
/// Pages of unsettled ledger rows one garbage round reads.
const GARBAGE_PAGES: usize = 64;
/// How often a waiting start asks again, and what it allows beyond the run's own deadline for
/// scheduling, image pulls and the API's request timeout.
const WAIT_POLL: Duration = Duration::from_millis(200);
const WAIT_SLACK: Duration = Duration::from_mins(1);
/// How long past its wall bound a Pod not shown ended is left before the driver deletes it
/// (design 28): `activeDeadlineSeconds` is the kubelet's, and a Pod it never started never ends.
const WALL_BOUND_GRACE: Duration = Duration::from_mins(1);
/// How long after the API accepted the driver's deletion of an exec Pod the kubelet has to
/// confirm it (design 28): a Pod still holding the bound uid then leaves the exec `unknown`.
const DELETION_CONFIRM: Duration = Duration::from_mins(1);

/// The exec state one driver keeps.
#[derive(Debug, Default)]
pub(crate) struct Execs {
    /// The snapshot the daemon admits starts under, the binding generation it was taken at, and
    /// the node its probe observed (design 34 § 6.4): the node is bound to the snapshot here,
    /// beside it, and is neither in its digest nor on the wire.
    published: Mutex<Option<(String, u64, Option<String>)>>,
    tracked: Mutex<BTreeMap<String, Tracked>>,
    /// Pods of ended execs whose rows a garbage round settles once they read absent.
    garbage: Mutex<BTreeSet<String>>,
    /// Where the next watch round starts; `None` lists again.
    watch_version: Mutex<Option<String>>,
    unapplied: AtomicU64,
}

#[derive(Debug, Clone)]
struct Tracked {
    observation: ExecObservation,
    pod: String,
    uid: String,
    trace: PodTrace,
    /// The last read of the Pod that showed its container terminated under the bound uid.
    terminated: Option<Value>,
    output_bytes: u64,
    /// A signal intent not yet shown ended (§ 5 signal row).
    signal: Option<Signal>,
    /// Why the output cannot be served: code and message.
    output_failure: Option<(&'static str, String)>,
    /// When, on the driver's wall clock, a Pod not shown ended is deleted: the wall bound plus
    /// [`WALL_BOUND_GRACE`] past the start (design 28, design 34 § 9). A recovered exec is ended
    /// where it is recovered and carries none.
    wall_deletion_at: Option<DateTime<Utc>>,
    /// When, on the driver's wall clock, the API first accepted the driver's deletion of the Pod
    /// (a signal or the wall-bound deletion), or, its answer lost, when a read first showed the
    /// Pod's `deletionTimestamp` ([`read_deleting`]); [`DELETION_CONFIRM`] later a Pod still
    /// holding the bound uid ends the exec `unknown`.
    deletion_accepted_at: Option<DateTime<Utc>>,
    /// Whether this driver committed the exec's end ([`NamespaceDriver::end`]). Not the
    /// observation's state: a recovered exec starts from the daemon's record, `unknown`, and is
    /// still ended once.
    ended: bool,
}

/// How a gone Pod's container ended, as the watch shows it.
enum Gone {
    Terminated(Value),
    NeverBound,
    Unproven(String),
    Unanswered(String),
}

pub(crate) enum LogRead {
    Read(Vec<u8>),
    Overlong,
    Gone,
    Unreadable(String),
    /// The name was not shown holding the bound uid on both sides of the read, so what it
    /// answered is not evidence about this exec's run.
    NotOurs,
}

/// What a read of the exec Pod's name shows holding it.
enum Holder {
    Ours,
    Other,
    Absent,
}

impl NamespaceDriver {
    /// Publishes the snapshot starts are admitted under, the binding generation it was taken at,
    /// and the node its probe observed; a start naming any other snapshot is
    /// `exec.capability-stale`, and one whose workspace keeper is on any other node, or under a
    /// snapshot with no observed node, is `exec.sandbox-unavailable` (§ 6.4).
    pub fn publish_exec_snapshot(&self, snapshot: &str, generation: u64, node: Option<&str>) {
        *lock(&self.execs().published) =
            Some((snapshot.to_owned(), generation, node.map(str::to_owned)));
    }

    /// The node published with the snapshot, if any.
    #[cfg(test)]
    pub(crate) fn published_exec_node(&self) -> Option<String> {
        lock(&self.execs().published)
            .as_ref()
            .and_then(|(_, _, node)| node.clone())
    }

    /// Execs whose executor read back bounds other than its spec's (§ 7.3 run step 1).
    pub fn limits_unapplied_total(&self) -> u64 {
        self.execs().unapplied.load(Ordering::Relaxed)
    }

    /// § 5 exec start: every request-level refusal first, then the Lease and the binding re-read
    /// (§ 6.3), then the keeper's node against the probe's (§ 6.4), then one Pod create whose uid
    /// is bound to the exec's `pod` row.
    pub async fn exec_start(
        &self,
        id: &str,
        root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        let prepared = match self.prepare_start(id, root_name, input).await {
            Ok(prepared) => prepared,
            Err(error) => return DispatchOutcome::NotDispatched(error),
        };
        let (row, rendered, output_bytes, deadline_seconds) = prepared;
        let created = match self.create_pod(&rendered, &row).await {
            DispatchOutcome::Observed(created) => created,
            DispatchOutcome::NotDispatched(error) => return DispatchOutcome::NotDispatched(error),
            DispatchOutcome::ContainedAbsent(error) => {
                return DispatchOutcome::ContainedAbsent(error);
            }
            DispatchOutcome::OutcomeUnknown(error) => {
                return DispatchOutcome::OutcomeUnknown(error);
            }
        };
        let mut trace = PodTrace::since(created["metadata"]["resourceVersion"].as_str());
        trace.note(&created);
        let observation = running(id, input, &row.name);
        let wall_deletion_at =
            chrono::Duration::from_std(Duration::from_secs(deadline_seconds) + WALL_BOUND_GRACE)
                .ok()
                .and_then(|after| self.wall_now().checked_add_signed(after));
        lock(&self.execs().tracked).insert(
            id.to_owned(),
            Tracked {
                observation: observation.clone(),
                pod: row.name.clone(),
                uid: uid_of(&created).unwrap_or_default().to_owned(),
                trace,
                terminated: None,
                output_bytes,
                signal: None,
                output_failure: None,
                wall_deletion_at,
                deletion_accepted_at: None,
                ended: false,
            },
        );
        if !input.wait {
            return DispatchOutcome::Observed(observation);
        }
        let bound = Duration::from_secs(deadline_seconds) + self.api().timeout + WAIT_SLACK;
        let waited = tokio::time::timeout(bound, async {
            loop {
                match self.exec_observe(id).await {
                    Ok(observation) if is_terminal(observation.resource.state) => {
                        return observation;
                    }
                    Ok(_) => {}
                    Err(error) => tracing::info!(code = error.code, "a waiting start asks again"),
                }
                tokio::time::sleep(WAIT_POLL).await;
            }
        })
        .await;
        match waited {
            Ok(observation) => DispatchOutcome::Observed(observation),
            Err(_) => DispatchOutcome::OutcomeUnknown(DriverError::failed(
                API_FAILED,
                "the exec did not show an end within its wait bound",
            )),
        }
    }

    /// The checks a start makes before it touches the cluster's mutable state, then the binding
    /// re-read: the row, the rendered Pod, the output bound and the wall bound.
    async fn prepare_start(
        &self,
        id: &str,
        root_name: &str,
        input: &ExecStartInput,
    ) -> Result<(BackendRecord, Value, u64, u64), DriverError> {
        if let Some(refusal) = unserved(input) {
            return Err(refusal);
        }
        let env = run_environment(input)?;
        let config = self.config();
        let output_limit = pod::output_limit_bytes(config.container_log_max_bytes);
        let bounds = RunBounds::resolve(&input.limits, config.pod_pids_limit, output_limit)
            .map_err(|why| {
                DriverError::exhausted(
                    "exec.limit-unserved",
                    format!("Exec bounds exceed what this namespace holds: {why}."),
                    "limits",
                )
            })?;
        let name = names::exec_pod(self.deployment(), id);
        let row = self
            .ledger()
            .recorded(BackendKind::Exec, id)?
            .into_iter()
            .find(|row| row.role == BackendRole::Pod && row.name == name && row.uid.is_none())
            .ok_or_else(|| {
                DriverError::failed(API_FAILED, "the exec's pod row is not reserved unbound")
            })?;
        let (claim_uid, node) = self.placement(&input.workspace, root_name).await?;
        let (generation, probed) = self.admitted(&input.sandbox.capability_snapshot)?;
        self.check_binding(generation).await?;
        observed_on(&node, probed.as_deref())?;
        let spec = RunSpec {
            argv: input.argv.clone(),
            env,
            memory_bytes: bounds.memory_bytes,
            cpu_millicores: bounds.millicores,
            output_bytes: bounds.output_bytes,
        };
        let rendered = pod::render_exec_pod(&ExecPod {
            namespace: self.api().namespace(),
            name: &name,
            labels: names::labels(
                self.deployment(),
                BackendKind::Exec,
                &names::resource_hash(self.deployment(), id),
                BackendRole::Pod,
            ),
            operation: names::operation_annotation(&row.operation_digest),
            claim: (root_name, &claim_uid),
            node: &node,
            executor_image: &config.executor_image,
            exec_image: &config.exec_image,
            run_as_user: config.run_as_user,
            spec_parts: spec.encode_parts(),
            bounds,
            container_log_max_bytes: config.container_log_max_bytes,
            read_only: input.workspace_access == WorkspaceAccess::ReadOnly,
        });
        Ok((row, rendered, bounds.output_bytes, bounds.deadline_seconds))
    }

    /// The generation the start's snapshot was published at and the node its probe observed, or
    /// `exec.capability-stale`.
    fn admitted(&self, snapshot: &str) -> Result<(u64, Option<String>), DriverError> {
        match &*lock(&self.execs().published) {
            Some((published, generation, node)) if published == snapshot => {
                Ok((*generation, node.clone()))
            }
            _ => Err(DriverError::refused(
                crate::CAPABILITY_STALE,
                "The admitted capability snapshot is stale: the start names a snapshot this \
                 driver does not publish.",
                "capability_snapshot",
            )),
        }
    }

    /// The workspace's PVC uid and its keeper's node: an exec Pod is owned by the one and pinned
    /// to the other (§ 7.3). Both rows must be bound, and the PVC and the keeper must each read as
    /// the bound one (§ 4 uid rule): the Pod mounts the claim by name.
    async fn placement(
        &self,
        workspace: &str,
        root_name: &str,
    ) -> Result<(String, String), DriverError> {
        let rows = self.ledger().recorded(BackendKind::Workspace, workspace)?;
        let bound = |role| {
            rows.iter()
                .find(|row| row.role == role && row.settled_at.is_none())
                .and_then(|row| Some((row.name.clone(), row.uid.clone()?)))
        };
        let unplaced = |why: &str| {
            DriverError::failed(
                API_FAILED,
                format!("the exec's workspace cannot hold it: {why}"),
            )
        };
        let (claim, claim_uid) = bound(BackendRole::Pvc).ok_or_else(|| unplaced("no bound PVC"))?;
        if claim != root_name {
            return Err(unplaced("the PVC is not the workspace's root"));
        }
        let held_claim = self
            .api()
            .get(&format!("{}/{claim}", self.api().claims()))
            .await
            .map_err(|error| api_failed(&error))?
            .ok_or_else(|| unplaced("the PVC is absent"))?;
        if uid_of(&held_claim) != Some(claim_uid.as_str()) {
            return Err(unplaced("the PVC is not the bound one"));
        }
        let (keeper, keeper_uid) =
            bound(BackendRole::Keeper).ok_or_else(|| unplaced("no bound keeper"))?;
        let held = self
            .api()
            .get(&format!("{}/{keeper}", self.api().pods()))
            .await
            .map_err(|error| api_failed(&error))?
            .ok_or_else(|| unplaced("the keeper is absent"))?;
        if uid_of(&held) != Some(keeper_uid.as_str()) {
            return Err(unplaced("the keeper is not the bound one"));
        }
        let node = held["spec"]["nodeName"]
            .as_str()
            .filter(|node| !node.is_empty())
            .ok_or_else(|| unplaced("the keeper is not scheduled"))?;
        Ok((claim_uid, node.to_owned()))
    }

    /// One Pod create and the bind of its uid. A quota refusal is `exec.capacity`, a name held
    /// by an object that is not ours a retriable failure, and a create without an answer is
    /// adopted when the Pod it stored reads present with the row's annotation.
    async fn create_pod(&self, rendered: &Value, row: &BackendRecord) -> DispatchOutcome<Value> {
        let answer = self
            .api()
            .call(
                Method::POST,
                &self.api().pods(),
                Some(rendered),
                MAX_API_BODY_BYTES,
            )
            .await;
        match answer {
            Ok((200..=202, body)) => match serde_json::from_slice::<Value>(&body) {
                Ok(created) if created.is_object() => self.bind(row, created),
                _ => self.recover(row, "the create answered no object").await,
            },
            Ok((403, body)) => {
                let message = self.api().status_message(&body);
                let reason = self.api().status_reason(&body);
                DispatchOutcome::NotDispatched(if is_quota_refusal(403, &reason, &message) {
                    DriverError::exhausted(
                        EXEC_CAPACITY,
                        format!("The namespace's quota refused the exec Pod: {message}"),
                        "exec",
                    )
                } else {
                    DriverError::failed(
                        API_FAILED,
                        format!("the exec Pod create answered 403: {message}"),
                    )
                })
            }
            Ok((409 | 500.., _)) | Err(_) => {
                self.recover(row, "the create's answer was not a creation")
                    .await
            }
            Ok((status, body)) => DispatchOutcome::NotDispatched(DriverError::failed(
                API_FAILED,
                format!(
                    "the exec Pod create answered {status}: {}",
                    self.api().status_message(&body)
                ),
            )),
        }
    }

    /// After a create that may have stored the Pod: ours by annotation is adopted, an object
    /// that is not ours holds the name, and nothing yet is an unknown outcome (§ 5.1).
    async fn recover(&self, row: &BackendRecord, why: &str) -> DispatchOutcome<Value> {
        let path = format!("{}/{}", self.api().pods(), row.name);
        match self.api().get(&path).await {
            Ok(Some(object)) => match ownership(&object, Some(row), self.deployment_label()) {
                Ownership::Ours { .. } => self.bind(row, object),
                Ownership::Collision(reason) => {
                    DispatchOutcome::NotDispatched(self.held_by_another(&object, &reason))
                }
            },
            Ok(None) => DispatchOutcome::OutcomeUnknown(DriverError::failed(
                API_FAILED,
                format!("{why}, and no exec Pod reads present yet"),
            )),
            Err(error) => DispatchOutcome::OutcomeUnknown(api_failed(&error)),
        }
    }

    fn bind(&self, row: &BackendRecord, created: Value) -> DispatchOutcome<Value> {
        let Some(uid) = uid_of(&created) else {
            return DispatchOutcome::OutcomeUnknown(DriverError::failed(
                API_FAILED,
                "the created exec Pod carries no uid",
            ));
        };
        match self.ledger().bind_uid(&row.name, uid) {
            Ok(UidBinding::Bound) => DispatchOutcome::Observed(created),
            Ok(other) => DispatchOutcome::OutcomeUnknown(DriverError::failed(
                API_FAILED,
                format!("the exec Pod's uid was not bound: {other:?}"),
            )),
            Err(error) => DispatchOutcome::OutcomeUnknown(error),
        }
    }

    /// The exec as the API shows it now: its end once the container is shown terminated under
    /// the bound uid, the gone-Pod evidence once the Pod reads absent or replaced.
    ///
    /// # Errors
    ///
    /// `resource.not-found` for an exec this driver does not track; `kubernetes.api-failed` when
    /// the API answers nothing usable, which proves nothing either way.
    pub async fn exec_observe(&self, id: &str) -> Result<ExecObservation, DriverError> {
        let tracked = self.tracked(id)?;
        if is_terminal(tracked.observation.resource.state) {
            return Ok(tracked.observation);
        }
        let path = format!("{}/{}", self.api().pods(), tracked.pod);
        match self
            .api()
            .get(&path)
            .await
            .map_err(|error| api_failed(&error))?
        {
            Some(pod) if uid_of(&pod) == Some(tracked.uid.as_str()) => {
                let accepted = self.note(id, &pod);
                if self.deletion_unconfirmed(accepted) {
                    Ok(self.unconfirmed(id, &tracked))
                } else if shows_end(&pod) {
                    self.finish(id, &pod, true).await
                } else if self.past_wall_bound(&tracked) {
                    self.delete_past_wall_bound(id, &tracked).await
                } else {
                    let mut current = tracked.observation;
                    current.resource.observed_at = Utc::now();
                    Ok(current)
                }
            }
            Some(_) => {
                tracing::warn!(pod = tracked.pod, "an exec Pod was replaced under its name");
                self.gone(id).await
            }
            None => self.gone(id).await,
        }
    }

    /// Whether the exec's Pod, not shown ended, is due its wall-bound deletion on the driver's
    /// wall clock and that deletion is not already the exec's signal intent.
    fn past_wall_bound(&self, tracked: &Tracked) -> bool {
        tracked.signal != Some(Signal::Kill)
            && tracked
                .wall_deletion_at
                .is_some_and(|at| self.wall_now() >= at)
    }

    /// Design 28: whether [`DELETION_CONFIRM`] has passed, on the driver's wall clock, since
    /// `accepted`, when the API accepted the driver's deletion of the exec's Pod. A caller that
    /// has just read the Pod holding the bound uid then ends the exec `unknown` before it reads
    /// anything else of that Pod, a terminated container included: the kubelet has not confirmed
    /// the deletion within its bound, and one history ends the same whenever it is first read.
    fn deletion_unconfirmed(&self, accepted: Option<DateTime<Utc>>) -> bool {
        accepted
            .and_then(|at| {
                at.checked_add_signed(chrono::Duration::from_std(DELETION_CONFIRM).ok()?)
            })
            .is_some_and(|due| self.wall_now() >= due)
    }

    /// Ends the exec `unknown` because its Pod still holds the bound uid [`DELETION_CONFIRM`]
    /// after its deletion was accepted (design 28).
    fn unconfirmed(&self, id: &str, tracked: &Tracked) -> ExecObservation {
        tracing::warn!(
            pod = tracked.pod,
            "an exec Pod's deletion was not confirmed within its bound; the run is unknown and \
             the garbage round deletes it again"
        );
        self.end(
            id,
            &tracked.observation,
            ExecState::Unknown,
            None,
            Output::Unknown,
        )
    }

    /// Delivers the deletion of a tracked exec's Pod under `uid` ([`Self::deliver`]) and records
    /// the first one the API accepted; a deletion delivered again does not move it. When the
    /// answer is lost (unanswered, a server error), the Pod is read once: holding the bound uid
    /// with `deletionTimestamp` set, it shows that the API acted on the deletion, which counts as
    /// accepted at this read ([`Self::note`]). The lost answer is still returned.
    async fn deliver_tracked(
        &self,
        id: &str,
        pod: &str,
        uid: &str,
        grace: u64,
    ) -> Result<bool, DriverError> {
        match self.deliver(pod, uid, grace).await {
            Ok(true) => {
                let now = self.wall_now();
                if let Some(entry) = lock(&self.execs().tracked).get_mut(id) {
                    entry.deletion_accepted_at.get_or_insert(now);
                }
                Ok(true)
            }
            Ok(false) => Ok(false),
            Err(error) => {
                let path = format!("{}/{pod}", self.api().pods());
                if let Ok(Some(read)) = self.api().get(&path).await
                    && uid_of(&read) == Some(uid)
                {
                    self.note(id, &read);
                }
                Err(error)
            }
        }
    }

    /// Design 28 and design 34 § 9: a Pod not ended 60 s after its wall bound is deleted as
    /// `signal: Kill` deletes one, under the bound uid, and the intent is kept so the exec ends as
    /// that signal once the Pod is shown gone and the garbage round delivers it again until then.
    /// A Pod still holding the bound uid [`DELETION_CONFIRM`] after the deletion was accepted ends
    /// the exec `unknown` instead. A Pod another uid holds under the name is never deleted: the
    /// precondition refuses it.
    async fn delete_past_wall_bound(
        &self,
        id: &str,
        tracked: &Tracked,
    ) -> Result<ExecObservation, DriverError> {
        tracing::warn!(
            pod = tracked.pod,
            "an exec Pod passed its wall bound and is deleted"
        );
        if let Some(entry) = lock(&self.execs().tracked).get_mut(id) {
            entry.signal = Some(Signal::Kill);
        }
        self.deliver_tracked(id, &tracked.pod, &tracked.uid, KILL_GRACE_SECONDS)
            .await?;
        let path = format!("{}/{}", self.api().pods(), tracked.pod);
        match self
            .api()
            .get(&path)
            .await
            .map_err(|error| api_failed(&error))?
        {
            Some(pod) if uid_of(&pod) == Some(tracked.uid.as_str()) => {
                let accepted = self.note(id, &pod);
                if self.deletion_unconfirmed(accepted) {
                    Ok(self.unconfirmed(id, tracked))
                } else if shows_end(&pod) {
                    self.finish(id, &pod, true).await
                } else {
                    let mut current = tracked.observation.clone();
                    current.resource.observed_at = Utc::now();
                    Ok(current)
                }
            }
            Some(_) | None => self.gone(id).await,
        }
    }

    /// Restart reconciliation of one exec (design 34 § 5, the exec start row's restart column;
    /// § 5.1): `previous` is the exec as the daemon recorded it. An exec this process tracks is
    /// observed as usual. Any other is decided by its `pod` row and the Pod under its name, and
    /// only while the driver is open and holds the Lease (§ 6.1 step 6), so a process that has
    /// not claimed the namespace deletes nothing:
    ///
    /// - ours and its container shown `terminated`: its end, read as [`Self::exec_observe`]
    ///   reads one (the log only under the bound uid, the exit from the API, the Pod deleted);
    /// - ours and anything else, a Pod already being deleted included: deleted with grace 1 s
    ///   under its uid, and the exec stays `unknown` with no exit (N8 A);
    /// - a create whose answer was lost is first adopted by its operation annotation;
    /// - absent with a bound uid: physical absence, `NotFound`, the row settled;
    /// - absent with no uid bound: `NotFound`, the row settled, once the N9 bound has passed on
    ///   the driver's wall clock since the row was reserved; before that a retriable failure,
    ///   because the create may still land.
    ///
    /// # Errors
    ///
    /// `NotFound` as above; `kubernetes.api-failed` (retriable) without the Lease, before the
    /// bound, for an object under the name that is not ours, or for an answer that proves
    /// nothing. Nothing is committed on an error, so the next round asks again.
    pub async fn exec_recover(
        &self,
        previous: &ExecObservation,
    ) -> Result<ExecObservation, DriverError> {
        let id = previous.resource.id.as_str();
        if lock(&self.execs().tracked).contains_key(id) {
            return self.exec_observe(id).await;
        }
        if !self.opened() || !self.lease_held() {
            return Err(DriverError::failed(
                API_FAILED,
                "the namespace driver does not hold its Lease; reconciliation waits for it",
            ));
        }
        let name = names::exec_pod(self.deployment(), id);
        let Some(row) = self
            .ledger()
            .recorded(BackendKind::Exec, id)?
            .into_iter()
            .find(|row| {
                row.role == BackendRole::Pod && row.name == name && row.settled_at.is_none()
            })
        else {
            return Err(DriverError::not_found());
        };
        let path = format!("{}/{name}", self.api().pods());
        let Some(pod) = self
            .api()
            .get(&path)
            .await
            .map_err(|error| api_failed(&error))?
        else {
            if row.uid.is_none() && !self.past_dispatch_bound(&row) {
                return Err(DriverError::failed(
                    API_FAILED,
                    "the exec Pod reads absent, but its create was never answered and may \
                     still land within the dispatch bound",
                ));
            }
            self.ledger().settle(&name)?;
            return Err(DriverError::not_found());
        };
        let uid = match ownership(&pod, Some(&row), self.deployment_label()) {
            Ownership::Collision(reason) => return Err(self.held_by_another(&pod, &reason)),
            Ownership::Ours { uid, adopt: false } => uid,
            Ownership::Ours { uid, adopt: true } => match self.ledger().bind_uid(&name, &uid)? {
                UidBinding::Bound => uid,
                other => {
                    return Err(self.held_by_another(
                        &pod,
                        &format!("{name:?} could not be adopted: {other:?}"),
                    ));
                }
            },
        };
        let mut trace = PodTrace::since(pod["metadata"]["resourceVersion"].as_str());
        trace.note(&pod);
        let output_bytes = run_output_bytes(&pod);
        let ended = shows_end(&pod) && pod["metadata"]["deletionTimestamp"].is_null();
        let recorded = unproven_output(previous);
        lock(&self.execs().tracked).insert(
            id.to_owned(),
            Tracked {
                observation: recorded.clone(),
                pod: name.clone(),
                uid: uid.clone(),
                trace,
                terminated: None,
                output_bytes: output_bytes.unwrap_or_default(),
                signal: None,
                output_failure: None,
                wall_deletion_at: None,
                deletion_accepted_at: None,
                ended: false,
            },
        );
        let recovered = match output_bytes {
            Some(_) if ended => self.finish(id, &pod, true).await,
            _ => match self.deliver(&name, &uid, KILL_GRACE_SECONDS).await {
                Ok(_) => Ok(self.end(id, &recorded, ExecState::Unknown, None, Output::Unknown)),
                Err(error) => Err(error),
            },
        };
        if recovered.is_err() {
            lock(&self.execs().tracked).remove(id);
        }
        recovered
    }

    /// § 4: an object under an exec Pod's name that is not ours, met at a start or at restart
    /// reconciliation, is counted once, never modified, and the exec stays as it is recorded.
    fn held_by_another(&self, object: &Value, reason: &str) -> DriverError {
        self.collision(Some(Foreign::of(BackendRole::Pod, object)), reason);
        DriverError::failed(
            API_FAILED,
            format!("the exec Pod's name is held by an object that is not ours: {reason}"),
        )
    }

    /// Notes a read of the exec's Pod under its bound uid ([`read_deleting`] included) and
    /// returns when its deletion was accepted.
    fn note(&self, id: &str, pod: &Value) -> Option<DateTime<Utc>> {
        let now = self.wall_now();
        let mut tracked = lock(&self.execs().tracked);
        let tracked = tracked.get_mut(id)?;
        if tracked.trace.note(pod) && shows_end(pod) {
            tracked.terminated = Some(pod.clone());
        }
        read_deleting(tracked, pod, now);
        tracked.deletion_accepted_at
    }

    /// The Pod is gone or replaced: its end is what the watch from the last version read shows,
    /// and nothing else (design 28's gone-Pod rule).
    async fn gone(&self, id: &str) -> Result<ExecObservation, DriverError> {
        let tracked = self.tracked(id)?;
        let shown = match tracked.terminated.clone() {
            Some(pod) => Gone::Terminated(pod),
            None => self.replay(&tracked).await,
        };
        match shown {
            Gone::Terminated(pod) => self.finish(id, &pod, false).await,
            Gone::NeverBound => {
                let state = if tracked.signal.is_some() {
                    ExecState::Cancelled
                } else {
                    ExecState::Unknown
                };
                Ok(self.end(id, &tracked.observation, state, None, Output::Empty))
            }
            Gone::Unproven(why) => {
                tracing::warn!(why, "an exec Pod is gone without a shown end");
                Ok(self.end(
                    id,
                    &tracked.observation,
                    ExecState::Unknown,
                    None,
                    Output::Unknown,
                ))
            }
            Gone::Unanswered(why) => Err(DriverError::failed(API_FAILED, why)),
        }
    }

    async fn replay(&self, tracked: &Tracked) -> Gone {
        let Some(version) = tracked.trace.resource_version() else {
            return Gone::Unproven("no resourceVersion of the run was read to watch from".into());
        };
        let path = format!(
            "{}?watch=1&fieldSelector=metadata.name%3D{}&resourceVersion={}&timeoutSeconds={}",
            self.api().pods(),
            tracked.pod,
            encode(version),
            WATCH_LIMIT.as_secs()
        );
        let mut trace = tracked.trace.clone();
        let read = async {
            let mut watch = match self.api().watch(&path).await {
                Ok(watch) => watch,
                Err(error @ (WatchError::Expired | WatchError::Refused(_))) => {
                    return Gone::Unproven(error.to_string());
                }
                Err(error) => return Gone::Unanswered(error.to_string()),
            };
            loop {
                match watch.next_line().await {
                    Ok(Some(line)) => {
                        let object = serde_json::from_slice::<Value>(&line)
                            .map(|event| event["object"].clone())
                            .unwrap_or_default();
                        match self
                            .api()
                            .quoted_watch_event(&line, &tracked.pod, &mut trace)
                        {
                            Some(PodEnd::Stopped) if shows_end(&object) => {
                                return Gone::Terminated(object);
                            }
                            Some(PodEnd::Stopped) => return Gone::NeverBound,
                            Some(PodEnd::Unproven(why)) => return Gone::Unproven(why),
                            Some(PodEnd::Unanswered(why)) => return Gone::Unanswered(why),
                            None => {}
                        }
                    }
                    Ok(None) => {
                        return Gone::Unproven(
                            "the watch ended without showing the Pod's deletion".into(),
                        );
                    }
                    Err(WatchError::Bound) => {
                        return Gone::Unproven("the watch passed its byte bound".into());
                    }
                    Err(_) => return Gone::Unanswered("the watch stream failed".into()),
                }
            }
        };
        let shown = tokio::time::timeout(WATCH_LIMIT, read)
            .await
            .unwrap_or_else(|_| {
                Gone::Unproven(format!(
                    "the watch showed no end of the container within {WATCH_LIMIT:?}"
                ))
            });
        match shown {
            Gone::Unproven(why) => Gone::Unproven(self.api().redact(&why)),
            Gone::Unanswered(why) => Gone::Unanswered(self.api().redact(&why)),
            shown @ (Gone::Terminated(_) | Gone::NeverBound) => shown,
        }
    }

    /// The container is shown terminated under the bound uid: read the log, take the exit from
    /// the API, commit the end and delete the Pod (§ 5 exec start, after column).
    ///
    /// The exit alone cannot tell a run from a refusal: the executor exits 125 when the limits
    /// read back differ and says so only in its log (§ 7.3). So a log that is gone or unreadable
    /// leaves the exec `unknown` with no exit. A log read while the name was not shown holding
    /// the bound uid is no output of this exec; the exit is still the one `pod` shows.
    async fn finish(
        &self,
        id: &str,
        pod: &Value,
        present: bool,
    ) -> Result<ExecObservation, DriverError> {
        let tracked = self.tracked(id)?;
        let terminated = &container_state(pod, RUN_CONTAINER)["terminated"];
        let code = terminated["exitCode"].as_i64();
        let deadline = pod["status"]["reason"] == "DeadlineExceeded";
        let budget = pod::log_budget_bytes(tracked.output_bytes);
        let read = self
            .read_owned_log(&tracked.pod, &tracked.uid, budget)
            .await;
        if let (LogRead::Unreadable(why), true) = (&read, present) {
            return Err(DriverError::failed(API_FAILED, why.clone()));
        }
        let (exit, output) = match read {
            LogRead::NotOurs => {
                tracing::warn!(
                    pod = tracked.pod,
                    "an exec Pod's name was not shown holding its uid around the log read"
                );
                (api_exit(code, None), Output::Unknown)
            }
            LogRead::Gone => lost("the exec Pod's log is gone".to_owned()),
            LogRead::Unreadable(why) => lost(why),
            LogRead::Overlong => (
                api_exit(code, None),
                Output::Failed(
                    EXECUTOR_PROTOCOL,
                    format!("the exec's log passes its budget of {budget} bytes"),
                ),
            ),
            LogRead::Read(bytes) => match logs::decode(&bytes, tracked.output_bytes) {
                Err(why) => (
                    api_exit(code, None),
                    Output::Failed(EXECUTOR_PROTOCOL, format!("the exec's log: {why}")),
                ),
                Ok(decoded) => match decoded.end {
                    RunEnd::Refused(refusal) => {
                        if refusal == Refusal::LimitsUnapplied {
                            self.execs().unapplied.fetch_add(1, Ordering::Relaxed);
                        }
                        tracing::warn!(?refusal, ?decoded.applied, "an exec's argv was not started");
                        (None, Output::Empty)
                    }
                    RunEnd::SpawnFailed { .. } => (api_exit(code, None), Output::Empty),
                    RunEnd::Exit {
                        stdout,
                        stderr,
                        stdout_truncated,
                        stderr_truncated,
                        record,
                    } => (
                        api_exit(code, Some(&record)),
                        Output::Read {
                            stdout,
                            stderr,
                            stdout_truncated,
                            stderr_truncated,
                        },
                    ),
                },
            },
        };
        let state = if exit.is_none() {
            ExecState::Unknown
        } else if tracked.signal.is_some() || deadline {
            ExecState::Cancelled
        } else {
            ExecState::Exited
        };
        let ended = self.end(id, &tracked.observation, state, exit, output);
        if present {
            let path = format!("{}/{}", self.api().pods(), tracked.pod);
            if let Err(error) = self
                .api()
                .delete_preconditioned(&path, KILL_GRACE_SECONDS, &tracked.uid)
                .await
            {
                tracing::info!(%error, "an ended exec's Pod is left to the garbage round");
            }
        }
        Ok(ended)
    }

    /// `pods/log` addresses a Pod by name only (§ 4 uid rule): the log is read only when a GET
    /// right before shows the name holding the bound uid, and what it answered is this exec's
    /// only when a GET right after shows the same uid.
    pub(crate) async fn read_owned_log(&self, pod: &str, uid: &str, budget: u64) -> LogRead {
        match self.holder(pod, uid).await {
            Ok(Holder::Ours) => {}
            Ok(Holder::Other) => return LogRead::NotOurs,
            Ok(Holder::Absent) => return LogRead::Gone,
            Err(why) => return LogRead::Unreadable(why),
        }
        let read = self.read_log(pod, budget).await;
        if !matches!(read, LogRead::Read(_) | LogRead::Overlong) {
            return read;
        }
        match self.holder(pod, uid).await {
            Ok(Holder::Ours) => read,
            Ok(Holder::Other | Holder::Absent) => LogRead::NotOurs,
            Err(why) => LogRead::Unreadable(why),
        }
    }

    async fn holder(&self, pod: &str, uid: &str) -> Result<Holder, String> {
        let path = format!("{}/{}", self.api().pods(), pod);
        match self.api().get(&path).await {
            Ok(Some(object)) if uid_of(&object) == Some(uid) => Ok(Holder::Ours),
            Ok(Some(_)) => Ok(Holder::Other),
            Ok(None) => Ok(Holder::Absent),
            Err(error) => Err(format!("the exec Pod beside its log: {error}")),
        }
    }

    async fn read_log(&self, pod: &str, budget: u64) -> LogRead {
        let limit = usize::try_from(budget).unwrap_or(usize::MAX);
        let mut last = String::new();
        for attempt in 0..LOG_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(LOG_RETRY).await;
            }
            match self.api().log(pod, RUN_CONTAINER, limit).await {
                Ok((200, bytes)) if bytes.len() > limit => return LogRead::Overlong,
                Ok((200, bytes)) => return LogRead::Read(bytes.to_vec()),
                Ok((404, _)) => return LogRead::Gone,
                Ok((status, body)) => {
                    last = format!(
                        "the exec's log answered {status}: {}",
                        self.api().status_message(&body)
                    );
                }
                Err(error) => last = format!("the exec's log: {error}"),
            }
        }
        LogRead::Unreadable(last)
    }

    /// Commits `state` for a tracked exec and returns its observation.
    ///
    /// The caller decided the end from `base`, its snapshot of the exec taken before an await, so
    /// a concurrent observation may have ended the exec meanwhile. An exec already ended keeps
    /// its recorded end, which is returned. One the daemon has since acknowledged or discarded,
    /// and so this driver forgot, is answered as the caller decided it from `base` and is not
    /// tracked again; the daemon's store holds that exec terminal already, keeps it and answers
    /// this one `Superseded`.
    fn end(
        &self,
        id: &str,
        base: &ExecObservation,
        state: ExecState,
        exit: Option<ExecExit>,
        output: Output,
    ) -> ExecObservation {
        let mut tracked = lock(&self.execs().tracked);
        let Some(entry) = tracked.get_mut(id) else {
            let mut late = base.clone();
            settle(&mut late, state, exit, output);
            return late;
        };
        if entry.ended {
            return entry.observation.clone();
        }
        if let Some(failure) = settle(&mut entry.observation, state, exit, output) {
            entry.output_failure = Some(failure);
        }
        entry.ended = true;
        entry.signal = None;
        lock(&self.execs().garbage).insert(entry.pod.clone());
        entry.observation.clone()
    }

    /// One bounded slice of a tracked exec's output.
    ///
    /// # Errors
    ///
    /// `exec.output-limit` past the published limit, `resource.not-found`, or the protocol
    /// failure that made the output unservable.
    pub fn exec_output(
        &self,
        id: &str,
        query: &ExecOutputQuery,
    ) -> Result<OutputSlice, DriverError> {
        if query.limit_bytes > pod::output_limit_bytes(self.config().container_log_max_bytes) {
            return Err(DriverError::exhausted(
                "exec.output-limit",
                "Requested output range exceeds the probed limit.",
                "limit",
            ));
        }
        let tracked = self.tracked(id)?;
        if let Some((code, message)) = tracked.output_failure {
            return Err(DriverError {
                class: DriverErrorClass::Failed,
                code,
                message,
                address: None,
                retriable: false,
            });
        }
        let observation = tracked.observation;
        let (source, truncated) = match query.stream {
            OutputStream::Stdout => (&observation.stdout, observation.stdout_truncated),
            OutputStream::Stderr => (&observation.stderr, observation.stderr_truncated),
        };
        let start = usize::try_from(query.offset)
            .unwrap_or(usize::MAX)
            .min(source.len());
        let limit = usize::try_from(query.limit_bytes).unwrap_or(usize::MAX);
        let end = start.saturating_add(limit).min(source.len());
        Ok(OutputSlice {
            exec: id.to_owned(),
            stream: query.stream,
            offset: query.offset,
            returned_bytes: (end - start) as u64,
            next_offset: end as u64,
            eof: observation.output_complete && end == source.len(),
            truncated,
            content: Base64Content {
                encoding: Base64Encoding::Base64,
                data: base64::engine::general_purpose::STANDARD.encode(&source[start..end]),
            },
            observed_at: Utc::now(),
        })
    }

    /// § 5 signal: `term` deletes with grace 5 s and `kill` with 1 s, each under the bound uid;
    /// the intent is kept until the end is shown. `int` is `exec.signal-unserved`.
    ///
    /// # Errors
    ///
    /// `exec.signal-unserved` for `int`, `resource.not-found`, or `kubernetes.api-failed`.
    pub async fn exec_signal(
        &self,
        id: &str,
        input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        let grace = match input.signal {
            Signal::Term => TERM_GRACE_SECONDS,
            Signal::Kill => KILL_GRACE_SECONDS,
            Signal::Int => {
                let message = substrate_wire::DRIVER_UNSERVED_REFUSALS
                    .iter()
                    .find(|refusal| refusal.code == substrate_wire::EXEC_SIGNAL_UNSERVED)
                    .map_or(
                        "The requested signal is not one this driver delivers.",
                        |r| r.message,
                    );
                return Err(DriverError::unserved(
                    substrate_wire::EXEC_SIGNAL_UNSERVED,
                    message,
                    "signal",
                ));
            }
        };
        let tracked = self.tracked(id)?;
        if is_terminal(tracked.observation.resource.state) {
            return Ok(tracked.observation);
        }
        if let Some(entry) = lock(&self.execs().tracked).get_mut(id)
            && entry.signal != Some(Signal::Kill)
        {
            entry.signal = Some(input.signal);
        }
        self.deliver_tracked(id, &tracked.pod, &tracked.uid, grace)
            .await?;
        Ok(tracked.observation)
    }

    /// Deletes the exec Pod under `uid`: `true` once the API accepted the deletion, `false` when
    /// the precondition refused it because another uid holds the name.
    async fn deliver(&self, pod: &str, uid: &str, grace: u64) -> Result<bool, DriverError> {
        let path = format!("{}/{pod}", self.api().pods());
        match self.api().delete_preconditioned(&path, grace, uid).await {
            Ok(_) => Ok(true),
            Err(error) if error.status() == Some(409) => {
                tracing::warn!(
                    pod,
                    "an exec Pod was replaced; the replacement is untouched"
                );
                Ok(false)
            }
            Err(error) => Err(api_failed(&error)),
        }
    }

    /// Every tracked exec that has ended.
    pub fn exec_completed(&self) -> Vec<ExecObservation> {
        lock(&self.execs().tracked)
            .values()
            .filter(|tracked| is_terminal(tracked.observation.resource.state))
            .map(|tracked| tracked.observation.clone())
            .collect()
    }

    /// Records the lease the daemon holds for an exec.
    pub fn exec_set_lease(&self, id: &str, lease: Option<LeaseObservation>) {
        if let Some(tracked) = lock(&self.execs().tracked).get_mut(id) {
            tracked.observation.resource.lease = lease;
        }
    }

    /// Forgets an ended exec once the daemon persisted exactly this observation of it.
    pub fn exec_acknowledge(&self, persisted: &ExecObservation) {
        let mut tracked = lock(&self.execs().tracked);
        if tracked.get(&persisted.resource.id).is_some_and(|entry| {
            is_terminal(entry.observation.resource.state) && entry.observation == *persisted
        }) {
            tracked.remove(&persisted.resource.id);
        }
    }

    /// Forgets an ended exec the daemon superseded.
    pub fn exec_discard(&self, id: &str) {
        let mut tracked = lock(&self.execs().tracked);
        if tracked
            .get(id)
            .is_some_and(|entry| is_terminal(entry.observation.resource.state))
        {
            tracked.remove(id);
        }
    }

    /// One maintenance round (§ 5 garbage and signal rows): every signal intent not yet shown
    /// ended is delivered again as the same deletion, an exec whose deletion the kubelet has not
    /// confirmed within [`DELETION_CONFIRM`] is observed, which ends it `unknown` while its Pod
    /// holds the bound uid (design 28), and every ended exec's Pod is deleted under the uid rule,
    /// its row settled once it reads absent. Returns the rows settled.
    ///
    /// # Errors
    ///
    /// The ledger's own failure to page its rows.
    pub async fn exec_garbage_round(&self) -> Result<usize, DriverError> {
        let intents: Vec<(String, String, String, u64)> = lock(&self.execs().tracked)
            .iter()
            .filter(|(_, tracked)| !is_terminal(tracked.observation.resource.state))
            .filter_map(|(id, tracked)| {
                let grace = match tracked.signal? {
                    Signal::Kill => KILL_GRACE_SECONDS,
                    Signal::Term | Signal::Int => TERM_GRACE_SECONDS,
                };
                Some((id.clone(), tracked.pod.clone(), tracked.uid.clone(), grace))
            })
            .collect();
        for (id, pod, uid, grace) in intents {
            match self.deliver_tracked(&id, &pod, &uid, grace).await {
                Ok(_) => {}
                Err(error) => tracing::info!(
                    code = error.code,
                    pod,
                    "a signal is delivered again next round"
                ),
            }
            let due = lock(&self.execs().tracked)
                .get(&id)
                .is_some_and(|tracked| self.deletion_unconfirmed(tracked.deletion_accepted_at));
            if due && let Err(error) = self.exec_observe(&id).await {
                tracing::info!(code = error.code, id, "an exec is read again next round");
            }
        }
        let garbage = lock(&self.execs().garbage).clone();
        if garbage.is_empty() {
            return Ok(0);
        }
        let mut seen = BTreeSet::new();
        let mut settled = 0;
        let mut after: Option<String> = None;
        let mut whole = false;
        for _ in 0..GARBAGE_PAGES {
            let page = self.ledger().unsettled(after.as_deref())?;
            let Some(last) = page.last() else {
                whole = true;
                break;
            };
            after = Some(last.name.clone());
            for row in page {
                if row.kind != BackendKind::Exec
                    || row.role != BackendRole::Pod
                    || !garbage.contains(&row.name)
                {
                    continue;
                }
                seen.insert(row.name.clone());
                match self.remove_owned(&row, KILL_GRACE_SECONDS).await {
                    Ok(Removal::Absent) => {
                        settled += 1;
                        lock(&self.execs().garbage).remove(&row.name);
                    }
                    Ok(Removal::Collision) => {
                        lock(&self.execs().garbage).remove(&row.name);
                    }
                    Ok(Removal::Pending | Removal::Unconfirmed) => {}
                    Err(error) => {
                        tracing::info!(code = error.code, "an exec Pod is removed next round");
                    }
                }
            }
        }
        if whole {
            let mut held = lock(&self.execs().garbage);
            held.retain(|pod| seen.contains(pod));
        }
        Ok(settled)
    }

    /// One watch round (§ 5.2): with no version, list the driver's Pods and re-read every
    /// running exec by name; otherwise watch from the version for at most `timeout`, ending each
    /// exec whose container an event shows terminated. A 410, an `ERROR` event or a failed
    /// stream lists again at once.
    ///
    /// # Errors
    ///
    /// `kubernetes.api-failed` when the list cannot be read; the round after starts from it.
    pub async fn exec_watch_round(&self, timeout: Duration) -> Result<(), DriverError> {
        let version = lock(&self.execs().watch_version).clone();
        let Some(version) = version else {
            return self.relist().await;
        };
        let path = format!(
            "{}?watch=1&labelSelector={}&resourceVersion={}&timeoutSeconds={}&allowWatchBookmarks=true",
            self.api().pods(),
            self.selector(),
            encode(&version),
            timeout.as_secs().max(1)
        );
        let streamed = async {
            let mut watch = self.api().watch(&path).await?;
            while let Some(event) = watch.next_event().await? {
                self.on_event(event).await;
            }
            Ok::<(), WatchError>(())
        };
        match tokio::time::timeout(timeout + self.api().timeout, streamed).await {
            Ok(Ok(())) | Err(_) => Ok(()),
            Ok(Err(error)) => {
                tracing::info!(%error, "the exec watch lists again");
                *lock(&self.execs().watch_version) = None;
                self.relist().await
            }
        }
    }

    async fn on_event(&self, event: WatchEvent) {
        if let Some(version) = event.object["metadata"]["resourceVersion"].as_str() {
            *lock(&self.execs().watch_version) = Some(version.to_owned());
        }
        if !matches!(event.kind.as_str(), "ADDED" | "MODIFIED" | "DELETED") {
            return;
        }
        let Some(name) = event.object["metadata"]["name"].as_str() else {
            return;
        };
        let line = serde_json::to_vec(&json!({"type": event.kind, "object": event.object}))
            .unwrap_or_default();
        let now = self.wall_now();
        let decided = {
            let mut tracked = lock(&self.execs().tracked);
            let Some((id, entry)) = tracked.iter_mut().find(|(_, entry)| {
                entry.pod == name && !is_terminal(entry.observation.resource.state)
            }) else {
                return;
            };
            let decided = self.api().quoted_watch_event(&line, name, &mut entry.trace);
            let ours = uid_of(&event.object) == Some(entry.uid.as_str());
            if ours && shows_end(&event.object) {
                entry.terminated = Some(event.object.clone());
            }
            if ours && event.kind != "DELETED" {
                read_deleting(entry, &event.object, now);
            }
            // Design 28: the bound uid still present once the deletion's bound has passed ends
            // the exec unknown, whatever its container shows; a fresh read decides it.
            let unconfirmed = ours
                && event.kind != "DELETED"
                && self.deletion_unconfirmed(entry.deletion_accepted_at);
            decided.map(|decided| (id.clone(), decided, unconfirmed))
        };
        let Some((id, decided, unconfirmed)) = decided else {
            return;
        };
        let result = match decided {
            PodEnd::Stopped if unconfirmed => self.exec_observe(&id).await.map(drop),
            PodEnd::Stopped if shows_end(&event.object) => self
                .finish(&id, &event.object, event.kind != "DELETED")
                .await
                .map(drop),
            PodEnd::Stopped | PodEnd::Unproven(_) => self.gone(&id).await.map(drop),
            PodEnd::Unanswered(why) => Err(DriverError::failed(API_FAILED, why)),
        };
        if let Err(error) = result {
            tracing::info!(code = error.code, id, "an exec is read again next round");
        }
    }

    async fn relist(&self) -> Result<(), DriverError> {
        let listed = self
            .api()
            .list(
                &format!("{}?labelSelector={}", self.api().pods(), self.selector()),
                "PodList",
                ListForm::Metadata,
                LIST_PAGE_ITEMS,
                LIST_PAGES,
            )
            .await
            .map_err(|error| api_failed(&error))?;
        *lock(&self.execs().watch_version) = listed.resource_version;
        let running: Vec<String> = lock(&self.execs().tracked)
            .iter()
            .filter(|(_, tracked)| !is_terminal(tracked.observation.resource.state))
            .map(|(id, _)| id.clone())
            .collect();
        for id in running {
            if let Err(error) = self.exec_observe(&id).await {
                tracing::info!(code = error.code, id, "an exec is read again next round");
            }
        }
        Ok(())
    }

    /// The driver's own Pods: managed by substrate, for this deployment (§ 5.2).
    fn selector(&self) -> String {
        encode(&format!(
            "{MANAGED_BY_LABEL}={MANAGED_BY},{DEPLOYMENT_LABEL}={}",
            self.deployment_label()
        ))
    }

    fn tracked(&self, id: &str) -> Result<Tracked, DriverError> {
        lock(&self.execs().tracked)
            .get(id)
            .cloned()
            .ok_or_else(DriverError::not_found)
    }
}

enum Output {
    Read {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        stdout_truncated: bool,
        stderr_truncated: bool,
    },
    /// The run wrote nothing: it never started.
    Empty,
    /// What it wrote is not known.
    Unknown,
    /// Its output is not servable, for this reason.
    Failed(&'static str, String),
}

/// § 8: the start members this driver does not serve, each refused by name, in the daemon's
/// check order. The daemon refuses most of them first; this is the driver's own backstop.
/// § 6.4 (D339): the exec Pod is pinned to its workspace keeper's node, and the snapshot's proofs
/// hold only on the node the probe observed. A keeper on any other node, or a snapshot published
/// with no observed node, is `exec.sandbox-unavailable` (501), naming both nodes.
fn observed_on(keeper: &str, probed: Option<&str>) -> Result<(), DriverError> {
    let why = match probed {
        Some(probed) if probed == keeper => return Ok(()),
        Some(probed) => format!("the capability probe observed node {probed}"),
        None => "the capability probe observed no node".to_owned(),
    };
    Err(DriverError::unserved(
        substrate_host::seatbelt::SANDBOX_UNAVAILABLE,
        format!(
            "Required confinement is not observed where this exec would run: the workspace's \
             keeper is on node {keeper}, and {why}."
        ),
        substrate_wire::CONFINEMENT_NO_EGRESS_FACT,
    ))
}

fn unserved(input: &ExecStartInput) -> Option<DriverError> {
    let refuse = |code: &'static str, message: &str, address: &str| {
        Some(DriverError::unserved(code, message, address))
    };
    let sandbox = &input.sandbox;
    if sandbox.network == NetworkMode::Aperture
        || sandbox.aperture.is_some()
        || !sandbox.apertures.is_empty()
    {
        return refuse(
            "exec.egress-apertures-unserved",
            "Egress apertures are not served by this driver.",
            "aperture",
        );
    }
    if !input.secret_slots.is_empty() {
        return refuse(
            "exec.secret-slots-unserved",
            "Sealed secret slots are not served by this driver.",
            "secret_slots",
        );
    }
    if !input.read_only_files.is_empty() {
        return refuse(
            substrate_wire::READ_ONLY_FILES_UNSERVED,
            "Read-only host files are not served by this driver.",
            "read_only_files",
        );
    }
    if input
        .writable_roots
        .iter()
        .any(|root| matches!(root, WritableRoot::Host(_)))
    {
        return refuse(
            substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED,
            "Writable host roots are not served by this driver.",
            "writable_roots",
        );
    }
    if !input.writable_roots.is_empty() {
        return refuse(
            substrate_wire::EXEC_WRITABLE_ROOTS_GIT_UNSERVED,
            "Writable Git roots are not served by this driver.",
            "writable_roots",
        );
    }
    if input.workdir.is_some() {
        return refuse(
            substrate_wire::EXEC_WORKDIR_UNSERVED,
            "A working directory is not served by this driver.",
            "workdir",
        );
    }
    if !input.read_only_roots.is_empty() {
        return refuse(
            "exec.sandbox-unavailable",
            "Read-only host roots are not served by this driver.",
            "read_only_roots",
        );
    }
    if !input.measurements.is_empty() {
        return refuse(
            substrate_wire::EXEC_METRICS_UNSERVED,
            "Exec measurements are not served by this driver.",
            "measurements",
        );
    }
    if input.scratch.is_some() {
        return refuse(
            substrate_wire::EXEC_SCRATCH_QUOTA_UNSERVED,
            "A scratch quota is not served by this driver.",
            "scratch",
        );
    }
    if matches!(input.workspace_access, WorkspaceAccess::Scoped { .. }) {
        return refuse(
            substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED,
            "Scoped workspace writes are not served by this driver.",
            "workspace_access",
        );
    }
    if input.capsule.is_some() {
        let message = substrate_wire::DRIVER_UNSERVED_REFUSALS
            .iter()
            .find(|refusal| refusal.code == substrate_wire::EXEC_INLINE_CAPSULE_UNSERVED)
            .map_or(
                "An inline execution capsule is not served by this driver.",
                |r| r.message,
            );
        return refuse(
            substrate_wire::EXEC_INLINE_CAPSULE_UNSERVED,
            message,
            "capsule",
        );
    }
    None
}

/// The host rule (§ 7.3): `allow` names at their baseline values, then `set`.
fn run_environment(input: &ExecStartInput) -> Result<BTreeMap<String, String>, DriverError> {
    if input.env.allow.len() > 5 {
        return Err(environment::refused());
    }
    environment::check(&input.env.set)?;
    let mut env: BTreeMap<String, String> = input
        .env
        .allow
        .iter()
        .map(|name| {
            (
                name.as_str().to_owned(),
                environment::baseline_value(*name).to_owned(),
            )
        })
        .collect();
    env.extend(input.env.set.clone());
    Ok(env)
}

fn running(id: &str, input: &ExecStartInput, pod: &str) -> ExecObservation {
    ExecObservation {
        resource: Exec {
            unrecorded_output: None,
            id: id.to_owned(),
            kind: ExecKind::Exec,
            workspace: input.workspace.clone(),
            state: ExecState::Running,
            observed_at: Utc::now(),
            requested: input.sandbox.clone(),
            applied: Some(AppliedConfinement {
                requires: input.sandbox.requires.clone(),
                capability_snapshot: input.sandbox.capability_snapshot.clone(),
                cgroup: Some(pod.to_owned()),
                filesystem: AppliedFilesystem::WorkspaceReadWriteSystemReadOnly,
                network: AppliedNetwork::None,
                profile: SandboxProfile::Workspace,
                workspace_access: input.workspace_access.clone(),
                capsule: None,
                read_only_roots: Vec::new(),
                read_only_files: Vec::new(),
                secret_slots: Vec::new(),
                workdir: None,
                writable_roots: Vec::new(),
            }),
            exit: None,
            usage: None,
            lease: None,
            refusal: None,
        },
        stdout: Vec::new(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        stdout_kept: None,
        stderr_kept: None,
        output_complete: false,
        cgroup: Some(pod.to_owned()),
        leader_pid: None,
    }
}

/// The exec a restarted driver reconciles, with no output proven: the daemon's restart pass
/// records an `unknown` exec with `output_complete` set, and only a log read under the bound uid
/// may make its output whole (invariant 3), so every end that reads none leaves it incomplete.
fn unproven_output(recorded: &ExecObservation) -> ExecObservation {
    ExecObservation {
        stdout: Vec::new(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        stdout_kept: None,
        stderr_kept: None,
        output_complete: false,
        ..recorded.clone()
    }
}

/// Writes an end decided as `state`, `exit` and `output` into `observation`, and returns why its
/// output cannot be served when it cannot.
fn settle(
    observation: &mut ExecObservation,
    state: ExecState,
    exit: Option<ExecExit>,
    output: Output,
) -> Option<(&'static str, String)> {
    observation.resource.state = state;
    observation.resource.exit = exit;
    observation.resource.observed_at = Utc::now();
    match output {
        Output::Read {
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
        } => {
            observation.stdout = stdout;
            observation.stderr = stderr;
            observation.stdout_truncated = stdout_truncated;
            observation.stderr_truncated = stderr_truncated;
            observation.output_complete = true;
            None
        }
        Output::Empty => {
            observation.output_complete = true;
            None
        }
        Output::Unknown => None,
        Output::Failed(code, why) => Some((code, why)),
    }
}

/// A log that is gone or unreadable: the exit alone cannot tell a run from the executor's
/// limits-unapplied refusal (exit 125, stated only in the log), so the end is unknown and no exit
/// is fabricated (§ 7.3); the output is unservable for `why`.
fn lost(why: String) -> (Option<ExecExit>, Output) {
    tracing::warn!(why, "an exec's log is lost, so its end is unknown");
    (None, Output::Failed(API_FAILED, why))
}

/// The exit the API states, with the signal name only when the executor's record agrees with
/// that code (§ 7.3 *Reading output*).
fn api_exit(code: Option<i64>, record: Option<&ExitRecord>) -> Option<ExecExit> {
    let code = code?;
    if let Some(ExitRecord::Signal(name)) = record {
        let signal = match name.as_str() {
            "int" => Some(Signal::Int),
            "term" => Some(Signal::Term),
            "kill" => Some(Signal::Kill),
            _ => None,
        };
        if let Some(signal) = signal
            && code == 128 + i64::from(signal.number())
        {
            return Some(ExecExit {
                code: None,
                signal: Some(signal),
            });
        }
    }
    u8::try_from(code).ok().map(|code| ExecExit {
        code: Some(code),
        signal: None,
    })
}

/// The output bound of the run spec the Pod's `run` container carries (§ 7.3), which a restarted
/// process reads its log against: `None` for a Pod whose command is not the executor's run.
fn run_output_bytes(pod: &Value) -> Option<u64> {
    let command = pod["spec"]["containers"]
        .as_array()?
        .iter()
        .find(|container| container["name"] == RUN_CONTAINER)?["command"]
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<&str>>>()?;
    let executor = format!("{}/substrate-executor", pod::EXECUTOR_MOUNT);
    match command.as_slice() {
        [path, "run", parts @ ..] if *path == executor => RunSpec::decode_parts(parts)
            .ok()
            .map(|spec| spec.output_bytes),
        _ => None,
    }
}

/// A terminal observation needs the container `terminated` in a Pod with `restartPolicy:
/// Never` (§ 5.2); the caller has checked the uid.
pub(crate) fn shows_end(pod: &Value) -> bool {
    container_state(pod, RUN_CONTAINER)["terminated"].is_object()
        && pod["spec"]["restartPolicy"] == "Never"
}

pub(crate) fn uid_of(object: &Value) -> Option<&str> {
    object["metadata"]["uid"].as_str()
}

/// A read at `now` of `pod` under the exec's bound uid. Carrying `deletionTimestamp` while the
/// driver holds a signal intent (a kill, a term, or the wall bound's deletion) and has recorded no
/// acceptance, it shows a deletion the API acted on whose answer the driver never read: that
/// deletion counts as accepted at this read, on the driver's own clock, so the [`DELETION_CONFIRM`]
/// bound runs from here. The server's timestamp is never compared with the driver's clock.
fn read_deleting(tracked: &mut Tracked, pod: &Value, now: DateTime<Utc>) {
    if tracked.signal.is_some()
        && uid_of(pod) == Some(tracked.uid.as_str())
        && !pod["metadata"]["deletionTimestamp"].is_null()
    {
        tracked.deletion_accepted_at.get_or_insert(now);
    }
}

const fn is_terminal(state: ExecState) -> bool {
    matches!(
        state,
        ExecState::Exited | ExecState::Cancelled | ExecState::Expired | ExecState::Unknown
    )
}

/// Percent-encodes a query value: every byte but an unreserved one.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn api_failed(error: &ApiError) -> DriverError {
    DriverError::failed(API_FAILED, format!("the cluster API: {error}"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
