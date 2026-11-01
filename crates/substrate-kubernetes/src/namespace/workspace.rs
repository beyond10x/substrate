//! The workspace path (design 34 § 5, § 7.2, § 12 U8): a workspace is one `PersistentVolumeClaim`
//! and one keeper Pod that mounts it. Create posts the PVC, binds its uid, posts the keeper with an
//! owner reference to that uid, binds the keeper's uid, and answers `ready` once the keeper passes
//! the executor handshake. Observation answers the restart column of § 5; destroy removes the
//! workspace's exec Pods, then its keeper, then its PVC, each under the uid rule (§ 4), and is
//! `Pending` until each reads absent.
//!
//! O1 did not hold on the lane: deleting a claim does not remove the Pods that use it, so the
//! Pods-first order here is the only cleanup, and nothing relies on garbage collection.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use substrate_executor::Handshake;
use substrate_host::{
    BackendKind, BackendRecord, BackendRole, DispatchOutcome, DriverError, UidBinding,
    WorkspaceDestroyProgress,
};
use substrate_kubernetes_client::{ApiError, ListForm};
use substrate_wire::{Workspace, WorkspaceAbsence, WorkspaceCreateInput};

use super::{NamespaceDriver, Removal, api_failed};
use crate::API_FAILED;
use crate::names::{self, MANAGED_BY, MANAGED_BY_LABEL, OPERATION_ANNOTATION, ROLE_LABEL};
use crate::ownership::{Foreign, Ownership, ownership};

/// The workspace's capacity refusal (429, `exhausted`): the namespace's `ResourceQuota` refused
/// one of its objects (design 34 § 8, O7).
pub const WORKSPACE_CAPACITY: &str = "workspace.capacity";
/// What the API server writes in a `ResourceQuota` refusal, after the object's name. O7 observed
/// `persistentvolumeclaims "<name>" is forbidden: exceeded quota: <quota>, requested: …`, so the
/// message is matched for this text anywhere, never only at its start (D184).
pub const EXCEEDED_QUOTA: &str = "is forbidden: exceeded quota: ";

/// D184: a `ResourceQuota` refusal is a 403 whose `Status.reason` is `Forbidden` and whose
/// message contains [`EXCEEDED_QUOTA`], all three; the workspace and the exec path both ask this.
pub(crate) fn is_quota_refusal(status: u16, reason: &str, message: &str) -> bool {
    status == 403 && reason == "Forbidden" && message.contains(EXCEEDED_QUOTA)
}
/// The keeper's container, which `pods/exec` names.
pub const KEEPER_CONTAINER: &str = "keeper";
/// Where the keeper mounts the PVC: the executor's guarded root.
pub const WORKSPACE_MOUNT: &str = substrate_executor::serve::WORKSPACE_ROOT;
/// The executor's path in its own image.
pub const EXECUTOR_PATH: &str = "/substrate-executor";
/// How long a create waits for the keeper's handshake before it answers `OutcomeUnknown` and
/// leaves the workspace to the provisional loop (D11).
pub const CREATE_BOUND: Duration = Duration::from_mins(1);
/// The pause between two handshake attempts while a create waits.
const HANDSHAKE_POLL: Duration = Duration::from_millis(500);
/// The bytes a handshake answer may hold: one short JSON line.
const HANDSHAKE_BYTES: usize = 4096;
/// What the API writes after the object's name when its authorization refuses a request; its
/// reason follows. A 403 with nothing after it is bare: kind v1.37 refuses an exec upgrade so when
/// the Role lacks `pods/exec` `create` (D349).
const FORBIDDEN: &str = "is forbidden:";
/// The verbs the API authorizes an exec upgrade under (ADR 0047's addendum of 2026-10-29), in the
/// order a bare refusal's access review asks them.
const EXEC_VERBS: [&str; 2] = ["create", "get"];
/// How long a bare refusal's access reviews may take together: the handshake ends after them,
/// never after the client's bound on each call.
const ACCESS_REVIEW_BOUND: Duration = Duration::from_secs(5);
/// What a bare refusal's error says once the access review denied verbs of [`EXEC_VERBS`],
/// before them. The namespace lane reads its S6 create case's refusal by this text.
const ACCESS_REVIEW_DENIES: &str = "a SelfSubjectAccessReview denies pods/exec ";
/// The keeper's grace on `TERM`, and the grace its deletion asks for.
const KEEPER_GRACE_SECONDS: u64 = 5;
/// The grace an exec Pod's deletion asks for during a destroy: `kill`'s (§ 7.3).
const EXEC_GRACE_SECONDS: u64 = 1;
/// The keeper's requests and limits, equal: room for one `fs` call at the wire's own file bound
/// beside the keeper itself.
const KEEPER_MEMORY_BYTES: u64 = 128 * 1_048_576;
const KEEPER_CPU: &str = "250m";
const KEEPER_EPHEMERAL_BYTES: u64 = 64 * 1_048_576;
/// Items per page of the exec-Pod list a destroy reads, and its pages: the bound of § 5.2.
const POD_PAGE_ITEMS: usize = 100;
const POD_PAGES: usize = 64;

/// What one create of a recorded object came to.
pub(super) enum Created {
    /// The object is stored and its uid is bound to its row.
    Bound(String),
    /// The API answered a refusal: nothing was stored.
    Refused(DriverError),
    /// The object may be stored: the answer was lost, or the object under its name is not ours.
    Unknown(DriverError),
}

/// What one read of a recorded object found.
enum Resolved {
    /// Ours, with this uid, bound to the row (adopted now if its create answer was lost).
    Ours(String),
    /// Absent, with no create answer bound: `past` once the N9 bound has passed since the row was
    /// reserved, and only then is the absence final (§ 5.1).
    AbsentUnbound { past: bool },
    /// Absent after its uid was bound: physical absence.
    AbsentBound,
    /// The object under the row's name is not ours: untouched and counted.
    Collision(String),
}

impl NamespaceDriver {
    /// Refuses a root name that is not the PVC name derived for `id`: the daemon routes a file or
    /// lifecycle call by the root it recorded, and a different one is never another workspace's.
    ///
    /// # Errors
    ///
    /// Failed `kubernetes.api-failed` for any other root name.
    pub fn check_root(&self, id: &str, root_name: &str) -> Result<(), DriverError> {
        if root_name == self.workspace_root_identity(id) {
            Ok(())
        } else {
            Err(DriverError::failed(
                API_FAILED,
                "the workspace's root name is not the PVC name the driver derives for it",
            ))
        }
    }

    /// The workspace's two rows, as the daemon reserved them before dispatch.
    pub(super) fn workspace_rows(
        &self,
        id: &str,
    ) -> Result<(BackendRecord, BackendRecord), DriverError> {
        let rows = self.ledger.recorded(BackendKind::Workspace, id)?;
        let row = |role: BackendRole, name: String| {
            rows.iter()
                .find(|row| row.role == role && row.name == name)
                .cloned()
                .ok_or_else(|| {
                    DriverError::failed(
                        API_FAILED,
                        format!(
                            "no ledger row names the workspace's {} {name:?}: nothing is created \
                             without one",
                            names::role_name(role)
                        ),
                    )
                })
        };
        Ok((
            row(BackendRole::Pvc, names::workspace_pvc(&self.deployment, id))?,
            row(BackendRole::Keeper, names::keeper_pod(&self.deployment, id))?,
        ))
    }

    /// `workspace.create` (design 34 § 5, first row).
    pub async fn workspace_create(
        &self,
        id: &str,
        root_name: &str,
        input: &WorkspaceCreateInput,
    ) -> DispatchOutcome<Workspace> {
        if !input.source.is_empty() {
            return DispatchOutcome::NotDispatched(DriverError::unserved(
                "workspace.source-unserved",
                "The namespace driver serves only empty workspaces.",
                "workspace.git",
            ));
        }
        if input.storage.is_some() {
            return DispatchOutcome::NotDispatched(DriverError::unserved(
                "workspace.storage-quota-unserved",
                "The namespace driver has not observed a requested storage quota enforced; every \
                 workspace has the operator's size.",
                "storage",
            ));
        }
        if let Err(error) = self.check_root(id, root_name) {
            return DispatchOutcome::NotDispatched(error);
        }
        let (pvc_row, keeper_row) = match self.workspace_rows(id) {
            Ok(rows) => rows,
            Err(error) => return DispatchOutcome::NotDispatched(error),
        };
        let pvc_uid = match self
            .create_recorded(&pvc_row, &self.claims_path(), self.render_pvc(id, &pvc_row))
            .await
        {
            Created::Bound(uid) => uid,
            Created::Refused(error) => return DispatchOutcome::NotDispatched(error),
            Created::Unknown(error) => return DispatchOutcome::OutcomeUnknown(error),
        };
        let keeper = self.render_keeper(id, &keeper_row, &pvc_row.name, &pvc_uid);
        match self
            .create_recorded(&keeper_row, &self.api.pods(), keeper)
            .await
        {
            Created::Bound(_) => {}
            Created::Refused(error) => {
                // The keeper was not stored, so the PVC alone was: it is removed now under its
                // bound uid, and the create is contained only once it reads absent.
                let mut bound = pvc_row.clone();
                bound.uid = Some(pvc_uid);
                return match self.remove_owned(&bound, 0).await {
                    Ok(Removal::Absent) => DispatchOutcome::ContainedAbsent(error),
                    _ => DispatchOutcome::OutcomeUnknown(error),
                };
            }
            Created::Unknown(error) => return DispatchOutcome::OutcomeUnknown(error),
        }
        match self.await_handshake(&keeper_row.name).await {
            Ok(()) => DispatchOutcome::Observed(Workspace {
                id: id.to_owned(),
                kind: substrate_wire::WorkspaceKind::Workspace,
                labels: input.labels.clone(),
                observed_at: Utc::now(),
                state: substrate_wire::WorkspaceState::Ready,
                origin: substrate_wire::WorkspaceOrigin::Created,
                root_name: substrate_wire::RootName::from(root_name),
                storage: None,
                lease: None,
            }),
            Err(error) => DispatchOutcome::OutcomeUnknown(error),
        }
    }

    /// `workspace.get` and the provisional loop (design 34 § 5, the restart column of the first
    /// row). Both objects ours and the handshake passing: `ready`. An object whose create answer
    /// may still land keeps the workspace `unknown` (a retriable failure). Past the N9 bound with
    /// the keeper never stored, the PVC is deleted under its uid, nothing is created again, and
    /// the answer is `NotFound`, which the daemon records as `failed`.
    ///
    /// # Errors
    ///
    /// `NotFound` for a workspace whose objects are gone or were never stored past the bound;
    /// `kubernetes.api-failed` (retriable) for anything not yet decided, a collision, or a lost
    /// keeper, which is never recreated (§ 15); `kubernetes.executor-protocol` for an executor of
    /// another release.
    pub async fn workspace_observe(
        &self,
        id: &str,
        root_name: &str,
        previous: &Workspace,
    ) -> Result<Workspace, DriverError> {
        let (pvc_row, keeper_row) = self.workspace_rows(id)?;
        let pvc_uid = match self.resolve(&pvc_row).await? {
            Resolved::Ours(uid) => uid,
            Resolved::AbsentUnbound { past: false } => return Err(undecided(&pvc_row)),
            Resolved::AbsentUnbound { past: true } => {
                // Never stored past the bound, so the keeper, which needs its uid, was never
                // created either: both rows close.
                self.ledger.settle(&pvc_row.name)?;
                self.ledger.settle(&keeper_row.name)?;
                return Err(DriverError::not_found());
            }
            Resolved::AbsentBound => return Err(DriverError::not_found()),
            Resolved::Collision(reason) => return Err(collided(&reason)),
        };
        match self.resolve(&keeper_row).await? {
            Resolved::Ours(_) => {}
            Resolved::AbsentUnbound { past: false } => return Err(undecided(&keeper_row)),
            Resolved::AbsentUnbound { past: true } => {
                self.ledger.settle(&keeper_row.name)?;
                let mut bound = pvc_row;
                bound.uid = Some(pvc_uid);
                self.remove_owned(&bound, 0).await?;
                return Err(DriverError::not_found());
            }
            Resolved::AbsentBound => {
                return Err(DriverError::failed(
                    API_FAILED,
                    "the workspace's keeper is gone; the namespace driver does not recreate it",
                ));
            }
            Resolved::Collision(reason) => return Err(collided(&reason)),
        }
        match self.handshake(&keeper_row.name).await {
            Handshaken::Current => {}
            Handshaken::Other(error) | Handshaken::Refused(error) | Handshaken::NotReady(error) => {
                return Err(error);
            }
        }
        let mut observed = previous.clone();
        id.clone_into(&mut observed.id);
        observed.kind = substrate_wire::WorkspaceKind::Workspace;
        observed.state = substrate_wire::WorkspaceState::Ready;
        observed.origin = substrate_wire::WorkspaceOrigin::Created;
        observed.root_name = substrate_wire::RootName::from(root_name);
        observed.storage = None;
        observed.observed_at = Utc::now();
        Ok(observed)
    }

    /// `workspace.destroy` (design 34 § 5, `destroying`): every exec Pod owned by the workspace's
    /// PVC, then the keeper, then the PVC, each deleted under the uid rule. Each step starts only
    /// once the one before reads absent; until the PVC does, the answer is `Pending`.
    ///
    /// # Errors
    ///
    /// `kubernetes.api-failed` for an answer that cannot be read, or an object under one of the
    /// workspace's names that is not ours, which is never deleted.
    pub async fn workspace_destroy(
        &self,
        id: &str,
    ) -> Result<WorkspaceDestroyProgress, DriverError> {
        let (pvc_row, keeper_row) = self.workspace_rows(id)?;
        let mut removed = 0;
        if let Some(pvc_uid) = pvc_row
            .uid
            .as_deref()
            .filter(|_| pvc_row.settled_at.is_none())
        {
            let mut pending = false;
            for row in self.exec_rows_owned_by(pvc_uid).await? {
                match self.remove_owned(&row, EXEC_GRACE_SECONDS).await? {
                    Removal::Absent => removed += 1,
                    Removal::Pending | Removal::Unconfirmed => pending = true,
                    Removal::Collision => return Err(collided(&row.name)),
                }
            }
            if pending {
                return Ok(WorkspaceDestroyProgress::Pending {
                    removed_items: removed,
                });
            }
        }
        for (row, grace) in [(&keeper_row, KEEPER_GRACE_SECONDS), (&pvc_row, 0)] {
            if row.settled_at.is_some() {
                continue;
            }
            match self.remove_owned(row, grace).await? {
                Removal::Absent => removed += 1,
                Removal::Pending | Removal::Unconfirmed => {
                    return Ok(WorkspaceDestroyProgress::Pending {
                        removed_items: removed,
                    });
                }
                Removal::Collision => return Err(collided(&row.name)),
            }
        }
        Ok(WorkspaceDestroyProgress::Absent(WorkspaceAbsence {
            kind: substrate_wire::WorkspaceKind::Workspace,
            id: id.to_owned(),
            absent: true,
            observed_at: Utc::now(),
        }))
    }

    /// The ledger rows of every exec Pod of this deployment whose owner reference names the PVC
    /// uid `pvc_uid`. A Pod with no row is not ours: it is counted and the destroy refuses to go
    /// on, because the PVC cannot go while a Pod uses it.
    async fn exec_rows_owned_by(&self, pvc_uid: &str) -> Result<Vec<BackendRecord>, DriverError> {
        let selector = format!(
            "{MANAGED_BY_LABEL}={MANAGED_BY},{}={},{ROLE_LABEL}={}",
            names::DEPLOYMENT_LABEL,
            self.deployment_label,
            names::role_name(BackendRole::Pod)
        );
        let query: String = url_encoded(&selector);
        let listed = self
            .api
            .list(
                &format!("{}?labelSelector={query}", self.api.pods()),
                "PodList",
                ListForm::Metadata,
                POD_PAGE_ITEMS,
                POD_PAGES,
            )
            .await
            .map_err(|error| api_failed(&error))?;
        let owned: Vec<(String, &Value)> = listed
            .items
            .iter()
            .filter(|pod| {
                pod["metadata"]["ownerReferences"]
                    .as_array()
                    .is_some_and(|owners| owners.iter().any(|owner| owner["uid"] == pvc_uid))
            })
            .filter_map(|pod| {
                pod["metadata"]["name"]
                    .as_str()
                    .map(|name| (name.to_owned(), pod))
            })
            .collect();
        if owned.is_empty() {
            return Ok(Vec::new());
        }
        let rows = self.unsettled_by_name()?;
        owned
            .into_iter()
            .map(|(name, pod)| {
                rows.get(&name)
                    .filter(|row| row.kind == BackendKind::Exec)
                    .cloned()
                    .ok_or_else(|| {
                        self.collision(
                            Some(Foreign::of(BackendRole::Pod, pod)),
                            &format!("{name:?} uses the workspace and has no exec row"),
                        );
                        collided(&name)
                    })
            })
            .collect()
    }

    /// Every unsettled row, by name, read page by page.
    pub(super) fn unsettled_by_name(&self) -> Result<BTreeMap<String, BackendRecord>, DriverError> {
        let mut rows = BTreeMap::new();
        let mut after: Option<String> = None;
        loop {
            let page = self.ledger.unsettled(after.as_deref())?;
            let Some(last) = page.last().map(|row| row.name.clone()) else {
                return Ok(rows);
            };
            for row in page {
                rows.insert(row.name.clone(), row);
            }
            if after.as_deref() == Some(last.as_str()) {
                return Ok(rows);
            }
            after = Some(last);
        }
    }

    /// Creates the object `row` records in `collection` and binds its uid. A lost answer is read
    /// once by name and adopted when the operation annotation proves it ours (§ 4); nothing is
    /// sent again.
    pub(super) async fn create_recorded(
        &self,
        row: &BackendRecord,
        collection: &str,
        object: Value,
    ) -> Created {
        let stored = match self.api.create(collection, &object).await {
            Ok(created) => created,
            Err(ApiError::Answered {
                status,
                reason,
                detail,
            }) if is_quota_refusal(status, &reason, &detail) => {
                return Created::Refused(DriverError::exhausted(
                    WORKSPACE_CAPACITY,
                    "The namespace's ResourceQuota refused the workspace's objects; destroy a \
                     workspace before retrying with a new operation id.",
                    "workspace",
                ));
            }
            Err(ApiError::Answered { status: 409, .. }) => match self.resolve(row).await {
                Ok(Resolved::Ours(uid)) => return Created::Bound(uid),
                Ok(Resolved::Collision(reason)) => return Created::Unknown(collided(&reason)),
                Ok(_) => {
                    return Created::Unknown(DriverError::failed(
                        API_FAILED,
                        format!("{:?} already existed and then read absent", row.name),
                    ));
                }
                Err(error) => return Created::Unknown(error),
            },
            Err(
                error @ ApiError::Answered {
                    status: 400..=499, ..
                },
            ) => {
                return Created::Refused(api_failed(&error));
            }
            Err(error) => {
                // Unanswered, a server error or an unreadable answer: the object may be stored.
                return match self.resolve(row).await {
                    Ok(Resolved::Ours(uid)) => Created::Bound(uid),
                    Ok(Resolved::Collision(reason)) => Created::Unknown(collided(&reason)),
                    Ok(_) => Created::Unknown(api_failed(&error)),
                    Err(resolved) => Created::Unknown(resolved),
                };
            }
        };
        let Some(uid) = stored["metadata"]["uid"]
            .as_str()
            .filter(|uid| !uid.is_empty())
        else {
            return Created::Unknown(DriverError::failed(
                API_FAILED,
                format!("the create of {:?} answered no uid", row.name),
            ));
        };
        match self.ledger.bind_uid(&row.name, uid) {
            Ok(UidBinding::Bound) => Created::Bound(uid.to_owned()),
            Ok(other) => Created::Unknown(collided(&format!(
                "{:?} could not be bound: {other:?}",
                row.name
            ))),
            Err(error) => Created::Unknown(error),
        }
    }

    /// Reads the object `row` records and decides it under the uid rule, adopting the uid of an
    /// object whose create answer was lost.
    async fn resolve(&self, row: &BackendRecord) -> Result<Resolved, DriverError> {
        let path = format!("{}/{}", self.collection(row.role), row.name);
        let Some(object) = self
            .api
            .get(&path)
            .await
            .map_err(|error| api_failed(&error))?
        else {
            return Ok(match row.uid {
                Some(_) => Resolved::AbsentBound,
                None => Resolved::AbsentUnbound {
                    past: self.past_dispatch_bound(row),
                },
            });
        };
        let foreign = || Some(Foreign::of(row.role, &object));
        match ownership(&object, Some(row), &self.deployment_label) {
            Ownership::Ours { uid, adopt: false } => Ok(Resolved::Ours(uid)),
            Ownership::Ours { uid, adopt: true } => match self.ledger.bind_uid(&row.name, &uid)? {
                UidBinding::Bound => Ok(Resolved::Ours(uid)),
                other => {
                    let reason = format!("{:?} could not be adopted: {other:?}", row.name);
                    self.collision(foreign(), &reason);
                    Ok(Resolved::Collision(reason))
                }
            },
            Ownership::Collision(reason) => {
                self.collision(foreign(), &reason);
                Ok(Resolved::Collision(reason))
            }
        }
    }

    /// Runs the handshake until it passes, an executor of another release answers, the API
    /// refuses the exec in a way no retry clears, or [`CREATE_BOUND`] passes on the driver's
    /// monotonic clock.
    pub(super) async fn await_handshake(&self, keeper: &str) -> Result<(), DriverError> {
        let started = self.clock.now();
        loop {
            match self.handshake(keeper).await {
                Handshaken::Current => return Ok(()),
                Handshaken::Other(error) | Handshaken::Refused(error) => return Err(error),
                Handshaken::NotReady(error) => {
                    if self.clock.now().saturating_duration_since(started) >= CREATE_BOUND {
                        return Err(DriverError::failed(
                            API_FAILED,
                            format!(
                                "the keeper did not pass the executor handshake within {} s: {}",
                                CREATE_BOUND.as_secs(),
                                error.message
                            ),
                        ));
                    }
                }
            }
            tokio::time::sleep(HANDSHAKE_POLL).await;
        }
    }

    /// One `substrate-executor version` call in the keeper (design 34 § 7.1).
    async fn handshake(&self, keeper: &str) -> Handshaken {
        let output = match self
            .api
            .exec(
                keeper,
                KEEPER_CONTAINER,
                &[EXECUTOR_PATH, "version"],
                &[],
                HANDSHAKE_BYTES,
            )
            .await
        {
            Ok(output) => output,
            Err(error) if refused_for_good(&error) => {
                return Handshaken::Refused(self.refused_handshake(keeper, &error).await);
            }
            Err(error) => return Handshaken::NotReady(api_failed(&error)),
        };
        let line = std::str::from_utf8(&output.stdout).unwrap_or_default();
        match Handshake::parse(line.trim_end_matches('\n')) {
            Ok(handshake)
                if output.exit_code() == Some(0)
                    && !output.stdout_truncated
                    && handshake.is_current() =>
            {
                Handshaken::Current
            }
            _ => Handshaken::Other(DriverError::failed(
                substrate_executor::message::PROTOCOL_FAILURE,
                format!(
                    "the keeper's executor is not {} at {}",
                    substrate_executor::PROTOCOL,
                    substrate_executor::VERSION
                ),
            )),
        }
    }

    /// The error of a handshake the API refused for good (design 34 § 7.1): its answer, and for a
    /// bare 403, one that gives no reason after [`FORBIDDEN`], what a `SelfSubjectAccessReview`
    /// of each of [`EXEC_VERBS`] on the keeper's `pods/exec` found: the verbs it denies, that it
    /// admits both and so does not decide the refused verb, or, when a review is refused, fails or
    /// outlasts [`ACCESS_REVIEW_BOUND`], that the verb is unknown. The API's answer stays the
    /// refusal either way.
    async fn refused_handshake(&self, keeper: &str, error: &ApiError) -> DriverError {
        let refused = api_failed(error);
        if !is_bare_refusal(error) {
            return refused;
        }
        let review =
            match tokio::time::timeout(ACCESS_REVIEW_BOUND, self.denied_exec_verbs(keeper)).await {
                Ok(Ok(denied)) if denied.is_empty() => format!(
                    "a SelfSubjectAccessReview admits pods/exec {}: it does not decide the refused \
                     verb",
                    EXEC_VERBS.join(" and ")
                ),
                Ok(Ok(denied)) => format!("{ACCESS_REVIEW_DENIES}{}", denied.join(" and ")),
                Ok(Err(failed)) => format!("the refused verb is unknown: {failed}"),
                Err(_) => format!(
                    "the refused verb is unknown: the SelfSubjectAccessReview did not answer \
                     within {} s",
                    ACCESS_REVIEW_BOUND.as_secs()
                ),
            };
        DriverError::failed(
            API_FAILED,
            format!("{} ({review})", refused.message.trim_end()),
        )
    }

    /// The verbs of [`EXEC_VERBS`] a `SelfSubjectAccessReview` of the keeper's `pods/exec` in the
    /// namespace denies, asked one after the other. The first review that fails ends the asking.
    async fn denied_exec_verbs(&self, keeper: &str) -> Result<Vec<&'static str>, String> {
        let mut denied = Vec::new();
        for verb in EXEC_VERBS {
            match self.api.access_review("", "pods/exec", verb, keeper).await {
                Ok(true) => {}
                Ok(false) => denied.push(verb),
                Err(error) => {
                    return Err(format!(
                        "the SelfSubjectAccessReview of pods/exec {verb} failed: {error}"
                    ));
                }
            }
        }
        Ok(denied)
    }

    fn claims_path(&self) -> String {
        self.api.claims()
    }

    /// The metadata of the object `row` records for a resource of `kind` whose resource label is
    /// `value`: the workspace's resource hash, or a probe's nonce.
    fn metadata(&self, kind: BackendKind, value: &str, row: &BackendRecord) -> Value {
        json!({
            "name": row.name,
            "labels": names::labels(&self.deployment, kind, value, row.role),
            "annotations": {
                OPERATION_ANNOTATION: names::operation_annotation(&row.operation_digest),
            },
        })
    }

    /// The workspace's PVC: the operator's class and size, written once by one node.
    pub(crate) fn render_pvc(&self, id: &str, row: &BackendRecord) -> Value {
        let hash = names::resource_hash(&self.deployment, id);
        self.render_pvc_as(BackendKind::Workspace, &hash, row)
    }

    /// A PVC as [`NamespaceDriver::render_pvc`] renders one, labelled for `kind` and `value`.
    pub(super) fn render_pvc_as(
        &self,
        kind: BackendKind,
        value: &str,
        row: &BackendRecord,
    ) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "PersistentVolumeClaim",
            "metadata": self.metadata(kind, value, row),
            "spec": {
                "accessModes": ["ReadWriteOnce"],
                "storageClassName": self.config.storage_class,
                "resources": {"requests": {
                    "storage": self.config.workspace_size_bytes.to_string(),
                }},
            },
        })
    }

    /// The keeper Pod (design 34 § 7.2): the executor image as PID 1 `keeper`, the PVC at
    /// `/workspace`, rendered as an exec Pod is without its init container. No token, no
    /// environment beyond the image's, no privilege, a read-only root, and an owner reference to
    /// the PVC by uid that does not make it the controller.
    pub(crate) fn render_keeper(
        &self,
        id: &str,
        row: &BackendRecord,
        pvc: &str,
        pvc_uid: &str,
    ) -> Value {
        let hash = names::resource_hash(&self.deployment, id);
        self.render_keeper_as(BackendKind::Workspace, &hash, row, pvc, pvc_uid)
    }

    /// A keeper as [`NamespaceDriver::render_keeper`] renders one, labelled for `kind` and
    /// `value`.
    pub(super) fn render_keeper_as(
        &self,
        kind: BackendKind,
        value: &str,
        row: &BackendRecord,
        pvc: &str,
        pvc_uid: &str,
    ) -> Value {
        let mut metadata = self.metadata(kind, value, row);
        metadata["ownerReferences"] = json!([{
            "apiVersion": "v1",
            "kind": "PersistentVolumeClaim",
            "name": pvc,
            "uid": pvc_uid,
            "controller": false,
        }]);
        let user = self.config.run_as_user;
        let resources = json!({
            "cpu": KEEPER_CPU,
            "memory": KEEPER_MEMORY_BYTES.to_string(),
            "ephemeral-storage": KEEPER_EPHEMERAL_BYTES.to_string(),
        });
        json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": metadata,
            "spec": {
                "automountServiceAccountToken": false,
                "enableServiceLinks": false,
                "hostIPC": false,
                "hostNetwork": false,
                "hostPID": false,
                "restartPolicy": "Never",
                "terminationGracePeriodSeconds": KEEPER_GRACE_SECONDS,
                "securityContext": {
                    "runAsNonRoot": true,
                    "runAsUser": user,
                    "runAsGroup": user,
                    "fsGroup": user,
                    "seccompProfile": {"type": "RuntimeDefault"},
                },
                "volumes": [{"name": "workspace", "persistentVolumeClaim": {"claimName": pvc}}],
                "containers": [{
                    "name": KEEPER_CONTAINER,
                    "image": self.config.executor_image,
                    "imagePullPolicy": "IfNotPresent",
                    "command": [EXECUTOR_PATH, "keeper"],
                    "resources": {"requests": resources, "limits": resources},
                    "securityContext": {
                        "runAsNonRoot": true,
                        "runAsUser": user,
                        "runAsGroup": user,
                        "seccompProfile": {"type": "RuntimeDefault"},
                        "allowPrivilegeEscalation": false,
                        "privileged": false,
                        "readOnlyRootFilesystem": true,
                        "capabilities": {"drop": ["ALL"]},
                    },
                    "volumeMounts": [{"name": "workspace", "mountPath": WORKSPACE_MOUNT}],
                    "stdin": false,
                    "tty": false,
                }],
            },
        })
    }
}

/// What one handshake call showed.
enum Handshaken {
    /// The executor speaks this protocol at this release.
    Current,
    /// An executor answered, and it is another protocol or release: waiting does not change it.
    Other(DriverError),
    /// The API refused the exec 401 or 403: a credential it does not accept, or a Role without
    /// `pods/exec` `create`. Waiting does not change it, so the API's answer is the error, with
    /// what a bare 403's access review found.
    Refused(DriverError),
    /// No executor answered yet: the keeper may still be starting.
    NotReady(DriverError),
}

/// Whether the API's answer to an exec is a refusal no retry clears: 401 or 403. Every other
/// answer may pass on the next try, 4xx included: the API answers an exec into a keeper the
/// scheduler has not bound yet, or whose container is not running yet, with a 4xx, and the
/// handshake begins right after the keeper's create.
fn refused_for_good(error: &ApiError) -> bool {
    matches!(error.status(), Some(401 | 403))
}

/// Whether the API's answer is a 403 that gives no reason after [`FORBIDDEN`].
fn is_bare_refusal(error: &ApiError) -> bool {
    match error {
        ApiError::Answered {
            status: 403,
            detail,
            ..
        } => detail
            .rsplit_once(FORBIDDEN)
            .is_some_and(|(_, reason)| reason.trim().is_empty()),
        _ => false,
    }
}

/// An object whose create may still be stored keeps the workspace `unknown`.
fn undecided(row: &BackendRecord) -> DriverError {
    DriverError::failed(
        API_FAILED,
        format!(
            "the {} {:?} reads absent and its create may still be stored until the dispatch \
             bound passes",
            names::role_name(row.role),
            row.name
        ),
    )
}

fn collided(reason: &str) -> DriverError {
    DriverError::failed(
        API_FAILED,
        format!("a backend object is not ours and is left untouched: {reason}"),
    )
}

fn url_encoded(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
#[path = "verb_review_tests.rs"]
mod verb_review_tests;
