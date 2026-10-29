//! The capability probe and the snapshot it backs (design 34 § 6.2, § 6.3, § 12 U10).
//!
//! One probe creates a PVC, a keeper and one exec Pod under rows reserved by nonce, observes what
//! it can, and removes the three under the uid rule. It proves `workspace.guarded-io`,
//! `workspace.openat2-beneath` and `workspace.atomic-replace` from the keeper's `fs check`, as the
//! host derives the three from its own `openat2` proof, and `exec.argv-only` from the probe Pod's
//! `probe-child` reading back the argv the driver sent, byte for byte.
//!
//! What it never publishes: `exec.namespaces`, `exec.cgroup-limits`, `exec.cgroup-kill` and
//! `exec.no-egress`, which a namespace cannot state; design 30's families and
//! `confinement.whole-tree-kill`, which the current bundle does not carry; and
//! `confinement.no-egress`, because O3 did not hold on the lane, so nothing here observes it.
//! Each stays absent, and admission refuses an exec without them (invariant 3).
//!
//! A proof lasts until `valid_until`, the next run plus 60 s, and only under the binding and the
//! Lease it was taken under: a failed or late probe, a lost Lease or a new binding leaves the
//! facts absent, never stale.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use substrate_executor::frame::ExitRecord;
use substrate_executor::message::Request;
use substrate_executor::probe::{PROBE_ARGUMENTS, PROBE_CHILD, report};
use substrate_executor::spec::RunSpec;
use substrate_host::{BackendKind, BackendRecord, BackendRole};
use substrate_kubernetes_client::{RUN_CONTAINER, container_state};
use substrate_wire::{
    CapabilityFacts, CapabilitySnapshot, ExecLimits, HostDriverKind, MAX_FILE_BYTES, MAX_IO_BYTES,
    MAX_LIST_ITEMS, Signal, StreamBoundsFacts,
};

use super::files::CallFailure;
use super::workspace::{CREATE_BOUND, Created};
use super::{NamespaceDriver, Removal, lock, refusal};
use crate::exec::{LogRead, shows_end, uid_of};
use crate::lease::RENEW_EVERY;
use crate::logs::{self, RunEnd};
use crate::pod::{self, EXECUTOR_MOUNT, ExecPod, KILL_GRACE_SECONDS, RunBounds};
use crate::{API_FAILED, EXECUTOR_HANDSHAKE_FAILED, StartupRefusal, names, policy};

/// What a probe that failed waits before the next attempt: long enough not to churn the
/// namespace's objects, short against the probe interval.
pub const PROBE_RETRY: Duration = Duration::from_mins(1);
/// What a proof outlives the next scheduled run by (§ 6.2).
pub const VALIDITY_SLACK: Duration = Duration::from_mins(1);
/// The probe Pod's bounds: a short wall bound, one core over it, and room for the executor.
const PROBE_TIMEOUT_MS: u64 = 30_000;
const PROBE_CPU_MILLIS: u64 = 30_000;
const PROBE_MEMORY_BYTES: u64 = 64 * 1_048_576;
const PROBE_OUTPUT_BYTES: u64 = 65_536;
/// How often the probe asks for the Pod's end and the keeper's node.
const PROBE_POLL: Duration = Duration::from_millis(200);
/// What the end may take beyond the probe's own deadline: scheduling, image pulls and the API.
const PROBE_SLACK: Duration = Duration::from_mins(1);

/// One probe's proofs, bound to the binding generation and the Lease epoch it ran under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Probed {
    generation: u64,
    lease_epoch: u64,
    at: DateTime<Utc>,
    valid_until: DateTime<Utc>,
    guarded_io: bool,
    argv_only: bool,
}

/// The last proof and when the next probe is due; `None` is due now. `lease_epoch` moves on
/// every lost or retaken Lease, which a same-holder retake does not show in `config_generation`.
#[derive(Debug, Default)]
pub(crate) struct ProbeState {
    proved: Option<Probed>,
    next_at: Option<DateTime<Utc>>,
    lease_epoch: u64,
}

/// What one run observed.
struct Observation {
    guarded_io: bool,
    argv_only: bool,
}

impl NamespaceDriver {
    /// One probe, now: leftovers of earlier probes are removed first, then the PVC, the keeper
    /// and the probe Pod are created under fresh rows, observed and removed. A proof the run
    /// could not make is absent; the next run is due after the probe interval, or after
    /// [`PROBE_RETRY`] when the run failed before it observed anything.
    ///
    /// # Errors
    ///
    /// `kubernetes.executor-handshake-failed` when the probe keeper's executor is not this
    /// release; `kubernetes.api-failed` when the driver is not open or an object could not be
    /// created. Every fact the probe proves is then absent.
    pub async fn probe(&self) -> Result<(), StartupRefusal> {
        let probed = self.probe_once().await;
        self.opening_settled.send_replace(true);
        probed
    }

    /// Whether the first open sequence has settled (§ 6.1): the open was refused, or the first
    /// probe after it ran, whatever it proved. The snapshot taken from then on is the one the
    /// daemon's readiness reads (ADR 0045 D5 A).
    #[must_use]
    pub fn opening_settled(&self) -> bool {
        *self.opening_settled.borrow()
    }

    /// Resolves once [`NamespaceDriver::opening_settled`] holds, at once when it already does.
    pub async fn opening_settles(&self) {
        let mut settled = self.opening_settled.subscribe();
        // The sender lives as long as `self`, so the wait ends only by the flag.
        let _ = settled.wait_for(|settled| *settled).await;
    }

    async fn probe_once(&self) -> Result<(), StartupRefusal> {
        let Some(binding) = self.binding() else {
            return Err(refusal(API_FAILED, "the namespace driver is not open"));
        };
        let generation = binding.config_generation();
        let lease_epoch = lock(&self.probe).lease_epoch;
        self.remove_probe_leftovers().await;
        let nonce = names::probe_nonce();
        let mut sent = Vec::new();
        let observed = self.probe_with(&nonce, &mut sent).await;
        self.remove_probe_objects(&nonce, &sent).await;
        let now = self.clock.wall();
        let interval = Duration::from_secs(self.config.probe_interval_seconds);
        let mut state = lock(&self.probe);
        if state.lease_epoch != lease_epoch {
            // The Lease was lost while this run was in flight: it neither proves nor delays
            // anything under the Lease held now, which takes its own run (§ 6.1).
            tracing::info!("the probe outlived the Lease it began under; its result is dropped");
            return observed.map(|_| ());
        }
        match observed {
            Ok(observation) => {
                state.proved = Some(Probed {
                    generation,
                    lease_epoch,
                    at: now,
                    valid_until: now + interval + VALIDITY_SLACK,
                    guarded_io: observation.guarded_io,
                    argv_only: observation.argv_only,
                });
                state.next_at = Some(now + interval);
                Ok(())
            }
            Err(refused) => {
                state.proved = None;
                state.next_at = Some(now + PROBE_RETRY);
                Err(refused)
            }
        }
    }

    /// Whether a probe is due: the driver is open and holds the Lease, and no proof was taken
    /// under the current binding, or the next run's time has come.
    pub fn probe_due(&self) -> bool {
        let Some(binding) = self.binding() else {
            return false;
        };
        if !self.lease_held() {
            return false;
        }
        let state = lock(&self.probe);
        let superseded = state.proved.as_ref().is_some_and(|proved| {
            proved.generation != binding.config_generation()
                || proved.lease_epoch != state.lease_epoch
        });
        superseded || state.next_at.is_none_or(|at| self.clock.wall() >= at)
    }

    /// Forgets the last proof and moves the Lease epoch: the Lease was lost or retaken, so steps
    /// 4 to 7 run again (§ 6.1), and a probe still in flight proves nothing under the new Lease.
    pub fn forget_probe(&self) {
        let mut state = lock(&self.probe);
        let lease_epoch = state.lease_epoch.wrapping_add(1);
        *state = ProbeState {
            lease_epoch,
            ..ProbeState::default()
        };
    }

    /// Serves the driver until the task is dropped: the Lease is renewed every
    /// [`RENEW_EVERY`] while it is held and the open sequence runs again while it is not; the
    /// probe runs whenever it is due. Each refusal is logged with its reason code and leaves the
    /// facts absent.
    pub async fn serve(self: Arc<Self>) {
        let lease = Arc::clone(&self);
        let lease = async move {
            loop {
                lease.lease_round().await;
                tokio::time::sleep(RENEW_EVERY).await;
            }
        };
        let probe = async move {
            loop {
                if self.probe_due() {
                    match self.probe().await {
                        Ok(()) => tracing::info!("the namespace driver's probe ran"),
                        Err(refused) => tracing::warn!(
                            code = refused.code,
                            message = %refused.message,
                            "the namespace driver's probe failed; its facts are absent"
                        ),
                    }
                }
                tokio::time::sleep(RENEW_EVERY).await;
            }
        };
        tokio::join!(lease, probe);
    }

    /// One round of the Lease task: renew a held Lease, or run the open sequence again.
    pub async fn lease_round(&self) {
        if self.opened() && self.lease_held() {
            self.renew().await;
            return;
        }
        self.forget_probe();
        match self.open().await {
            Ok(()) => tracing::info!(
                namespace = %self.config.namespace,
                "the namespace driver is open"
            ),
            Err(refused) => {
                tracing::warn!(
                    code = refused.code,
                    message = %refused.message,
                    "the namespace driver is not open; every driver fact is absent"
                );
                self.opening_settled.send_replace(true);
            }
        }
    }

    /// The snapshot (§ 6.3): the daemon's own facts always; the declared bounds while the driver
    /// is open, which takes a probe under the current binding that passed the handshake, and
    /// holds the Lease; each proved fact only under that binding and Lease, and before its
    /// `valid_until`.
    pub(crate) fn capability_snapshot(&self) -> CapabilitySnapshot {
        use sha2::Digest as _;
        let binding = self.binding();
        let config_generation = binding
            .as_ref()
            .map_or(0, crate::binding::Binding::config_generation);
        let held = binding.is_some() && self.lease_held();
        // Open is all seven steps of § 6.1: the binding, the Lease, and a probe under this
        // binding that passed the executor handshake. Only then are the declared bounds served.
        let opened = {
            let state = lock(&self.probe);
            state.proved.clone().filter(|proved| {
                held && proved.generation == config_generation
                    && proved.lease_epoch == state.lease_epoch
            })
        };
        let serving = opened.is_some();
        let now = self.clock.wall();
        let proved = opened.filter(|proved| now < proved.valid_until);
        let guarded = proved.as_ref().is_some_and(|proved| proved.guarded_io);
        let argv_only = proved.as_ref().is_some_and(|proved| proved.argv_only);
        let output_limit = pod::output_limit_bytes(self.config.container_log_max_bytes);
        let facts = CapabilityFacts {
            events_pull: Some(true),
            events_stream: Some(true),
            events_retention_events: Some(self.event_retention),
            leases_explicit: self.lease_clock.then_some(true),
            leases_clock_tolerance_ms: self
                .lease_clock
                .then_some(substrate_wire::LEASE_CLOCK_TOLERANCE_MS),
            events_stream_bounds: Some(StreamBoundsFacts::events()),
            snapshot_provenance_events: Some(substrate_wire::MAX_SNAPSHOT_PROVENANCE_EVENTS),
            workspace_guarded_io: guarded.then_some(true),
            workspace_openat2_beneath: guarded.then_some(true),
            workspace_atomic_replace: guarded.then_some(true),
            workspace_max_current: serving.then_some(self.config.max_current_workspaces),
            workspace_max_file_bytes: guarded.then_some(MAX_FILE_BYTES),
            workspace_read_limit_bytes: guarded.then_some(MAX_IO_BYTES),
            workspace_list_limit_items: guarded.then_some(MAX_LIST_ITEMS),
            exec_argv_only: argv_only.then_some(true),
            exec_output_limit_bytes: (serving && output_limit > 0).then_some(output_limit),
            exec_max_current: serving.then_some(self.config.max_current_execs),
            exec_signals: serving.then(|| vec![Signal::Term, Signal::Kill]),
            // O3 did not hold on the lane: nothing here observes it, so it is never published.
            confinement_no_egress: None,
            ..CapabilityFacts::default()
        };
        let valid_until = proved.as_ref().map(|proved| proved.valid_until);
        let driver_version = env!("CARGO_PKG_VERSION");
        let material = serde_json::to_vec(&serde_json::json!({
            "driver": HostDriverKind::Kubernetes,
            "driver_version": driver_version,
            "config_generation": config_generation,
            "valid_until": valid_until,
            "facts": facts,
        }))
        .expect("snapshot material serializes");
        CapabilitySnapshot {
            snapshot: format!("sha256:{}", policy::hex(&sha2::Sha256::digest(material))),
            driver: HostDriverKind::Kubernetes,
            driver_version: driver_version.to_owned(),
            config_generation,
            probed_at: proved.map_or(now, |proved| proved.at),
            valid_until,
            facts,
        }
    }

    async fn probe_with(
        &self,
        nonce: &str,
        sent: &mut Vec<String>,
    ) -> Result<Observation, StartupRefusal> {
        let ledger = |error: substrate_host::DriverError| {
            refusal(
                API_FAILED,
                format!("the probe's ledger rows: {}", error.message),
            )
        };
        self.ledger
            .reserve_probe(nonce, &names::probe_names(nonce))
            .map_err(ledger)?;
        let rows = self
            .ledger
            .recorded(BackendKind::Probe, nonce)
            .map_err(ledger)?;
        let row = |role: BackendRole| {
            rows.iter()
                .find(|row| row.role == role && row.settled_at.is_none())
                .cloned()
                .ok_or_else(|| {
                    refusal(
                        API_FAILED,
                        format!("no ledger row names the probe's {}", names::role_name(role)),
                    )
                })
        };
        let (pvc, keeper, pod) = (
            row(BackendRole::Pvc)?,
            row(BackendRole::Keeper)?,
            row(BackendRole::Pod)?,
        );
        sent.push(pvc.name.clone());
        let claim = self.render_pvc_as(BackendKind::Probe, nonce, &pvc);
        let pvc_uid = self
            .create_probe_object(&pvc, &self.api.claims(), claim)
            .await?;
        sent.push(keeper.name.clone());
        let held = self.render_keeper_as(BackendKind::Probe, nonce, &keeper, &pvc.name, &pvc_uid);
        self.create_probe_object(&keeper, &self.api.pods(), held)
            .await?;
        self.await_handshake(&keeper.name)
            .await
            .map_err(|error| refusal(EXECUTOR_HANDSHAKE_FAILED, error.message))?;
        let guarded_io = match self
            .keeper_call(&keeper.name, &Request::Check {}, &[])
            .await
        {
            Ok((answer, _)) => answer["openat2_beneath"] == Value::Bool(true),
            Err(CallFailure::Unanswered(why)) => {
                tracing::warn!(why, "the probe keeper's check went unanswered");
                false
            }
            Err(CallFailure::Failed(error)) => {
                tracing::warn!(code = error.code, "the probe keeper's check failed");
                false
            }
        };
        sent.push(pod.name.clone());
        let argv_only = match self
            .probe_argv(nonce, &pvc.name, &pvc_uid, &keeper.name, &pod)
            .await
        {
            Ok(()) => true,
            Err(why) => {
                tracing::warn!(why, "exec.argv-only was not proved");
                false
            }
        };
        Ok(Observation {
            guarded_io,
            argv_only,
        })
    }

    async fn create_probe_object(
        &self,
        row: &BackendRecord,
        collection: &str,
        object: Value,
    ) -> Result<String, StartupRefusal> {
        match self.create_recorded(row, collection, object).await {
            Created::Bound(uid) => Ok(uid),
            Created::Refused(error) | Created::Unknown(error) => Err(refusal(
                API_FAILED,
                format!(
                    "the probe's {} was not created: {}: {}",
                    names::role_name(row.role),
                    error.code,
                    error.message
                ),
            )),
        }
    }

    /// The probe Pod on the keeper's node: `probe-child` under the executor, its log decoded, its
    /// applied bounds and its exit held to the spec, and its stdout held to the report of the
    /// argv sent.
    async fn probe_argv(
        &self,
        nonce: &str,
        pvc: &str,
        pvc_uid: &str,
        keeper: &str,
        row: &BackendRecord,
    ) -> Result<(), String> {
        let node = self.scheduled_node(keeper).await?;
        let mut argv = vec![
            format!("{EXECUTOR_MOUNT}/substrate-executor"),
            PROBE_CHILD.to_owned(),
        ];
        argv.extend(PROBE_ARGUMENTS.map(str::to_owned));
        let config = &self.config;
        let output_limit = pod::output_limit_bytes(config.container_log_max_bytes);
        let limits = ExecLimits {
            timeout_ms: PROBE_TIMEOUT_MS,
            output_bytes: PROBE_OUTPUT_BYTES.min(output_limit),
            processes: config.pod_pids_limit,
            memory_bytes: PROBE_MEMORY_BYTES,
            cpu_millis: PROBE_CPU_MILLIS,
        };
        let bounds = RunBounds::resolve(&limits, config.pod_pids_limit, output_limit)
            .map_err(|why| format!("the probe's bounds: {why}"))?;
        let spec = RunSpec {
            argv: argv.clone(),
            env: BTreeMap::new(),
            memory_bytes: bounds.memory_bytes,
            cpu_millicores: bounds.millicores,
            output_bytes: bounds.output_bytes,
        };
        let rendered = pod::render_exec_pod(&ExecPod {
            namespace: self.api.namespace(),
            name: &row.name,
            labels: names::labels(&self.deployment, BackendKind::Probe, nonce, row.role),
            operation: names::operation_annotation(&row.operation_digest),
            claim: (pvc, pvc_uid),
            node: &node,
            executor_image: &config.executor_image,
            exec_image: &config.exec_image,
            run_as_user: config.run_as_user,
            spec_parts: spec.encode_parts(),
            bounds,
            container_log_max_bytes: config.container_log_max_bytes,
            read_only: true,
        });
        let uid = match self.create_recorded(row, &self.api.pods(), rendered).await {
            Created::Bound(uid) => uid,
            Created::Refused(error) | Created::Unknown(error) => {
                return Err(format!("the probe Pod: {}: {}", error.code, error.message));
            }
        };
        let bound = Duration::from_secs(bounds.deadline_seconds) + self.api.timeout + PROBE_SLACK;
        let ended = self.ended(&row.name, &uid, bound).await?;
        let code = container_state(&ended, RUN_CONTAINER)["terminated"]["exitCode"].as_i64();
        let log = match self
            .read_owned_log(&row.name, &uid, pod::log_budget_bytes(bounds.output_bytes))
            .await
        {
            LogRead::Read(bytes) => bytes,
            LogRead::Overlong => return Err("the probe's log passes its budget".to_owned()),
            LogRead::Gone => return Err("the probe's log is gone".to_owned()),
            LogRead::Unreadable(why) => return Err(why),
            LogRead::NotOurs => return Err("the probe Pod's name holds another uid".to_owned()),
        };
        let decoded = logs::decode(&log, bounds.output_bytes)
            .map_err(|why| format!("the probe's log: {why}"))?;
        if decoded.applied.memory_bytes != Some(spec.memory_bytes)
            || decoded.applied.cpu_millicores != Some(spec.cpu_millicores)
        {
            return Err("the probe Pod's applied bounds are not its spec's".to_owned());
        }
        match decoded.end {
            RunEnd::Exit {
                stdout,
                stderr,
                stdout_truncated: false,
                stderr_truncated: false,
                record: ExitRecord::Code(0),
            } if code == Some(0) && stderr.is_empty() && stdout == report(&argv).as_bytes() => {
                Ok(())
            }
            RunEnd::Exit { .. } => {
                Err("the probe child's argv did not read back byte for byte".to_owned())
            }
            RunEnd::Refused(refused) => Err(format!("the probe child was refused: {refused:?}")),
            RunEnd::SpawnFailed { errno } => {
                Err(format!("the probe child could not start: errno {errno}"))
            }
        }
    }

    /// The keeper's node, once the scheduler has bound it, within [`CREATE_BOUND`].
    async fn scheduled_node(&self, keeper: &str) -> Result<String, String> {
        let path = format!("{}/{keeper}", self.api.pods());
        tokio::time::timeout(CREATE_BOUND, async {
            loop {
                match self.api.get(&path).await {
                    Ok(Some(held)) => {
                        if let Some(node) = held["spec"]["nodeName"]
                            .as_str()
                            .filter(|node| !node.is_empty())
                        {
                            return Ok(node.to_owned());
                        }
                    }
                    Ok(None) => return Err("the probe keeper is absent".to_owned()),
                    Err(error) => tracing::info!(%error, "the probe asks for its keeper again"),
                }
                tokio::time::sleep(PROBE_POLL).await;
            }
        })
        .await
        .unwrap_or_else(|_| Err("the probe keeper was not scheduled in time".to_owned()))
    }

    /// The probe Pod once its container is shown terminated under `uid`, within `bound`.
    async fn ended(&self, name: &str, uid: &str, bound: Duration) -> Result<Value, String> {
        let path = format!("{}/{name}", self.api.pods());
        tokio::time::timeout(bound, async {
            loop {
                match self.api.get(&path).await {
                    Ok(Some(pod)) if uid_of(&pod) == Some(uid) => {
                        if shows_end(&pod) {
                            return Ok(pod);
                        }
                    }
                    Ok(_) => return Err("the probe Pod is gone or replaced".to_owned()),
                    Err(error) => tracing::info!(%error, "the probe asks for its Pod again"),
                }
                tokio::time::sleep(PROBE_POLL).await;
            }
        })
        .await
        .unwrap_or_else(|_| Err("the probe Pod did not end in time".to_owned()))
    }

    /// Removes the objects of earlier probes that are still recorded unsettled: a crash, or a
    /// deletion that was still pending when its probe ended.
    async fn remove_probe_leftovers(&self) {
        let rows = match self.unsettled_by_name() {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(code = error.code, "the probe's leftovers could not be read");
                return;
            }
        };
        let mut leftovers: Vec<BackendRecord> = rows
            .into_values()
            .filter(|row| row.kind == BackendKind::Probe)
            .collect();
        leftovers.sort_by_key(|row| removal_order(row.role));
        for row in leftovers {
            self.remove_probe_row(&row).await;
        }
    }

    /// Removes this probe's objects, the Pod first and the PVC last, each under the uid rule. A
    /// row whose create was never sent is settled at once: nothing was stored under it.
    async fn remove_probe_objects(&self, nonce: &str, sent: &[String]) {
        let mut rows = match self.ledger.recorded(BackendKind::Probe, nonce) {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(code = error.code, "the probe's rows could not be read");
                return;
            }
        };
        rows.sort_by_key(|row| removal_order(row.role));
        for row in rows.iter().filter(|row| row.settled_at.is_none()) {
            if row.uid.is_none() && !sent.contains(&row.name) {
                if let Err(error) = self.ledger.settle(&row.name) {
                    tracing::warn!(code = error.code, "a probe row could not be settled");
                }
                continue;
            }
            self.remove_probe_row(row).await;
        }
    }

    async fn remove_probe_row(&self, row: &BackendRecord) {
        let grace = match row.role {
            BackendRole::Pvc => 0,
            BackendRole::Keeper | BackendRole::Pod => KILL_GRACE_SECONDS,
        };
        match self.remove_owned(row, grace).await {
            Ok(Removal::Absent) => {}
            Ok(Removal::Pending | Removal::Unconfirmed) => {
                tracing::info!(name = row.name, "a probe object is left for the next probe");
            }
            // Decision 4: never modified, and the row stays open.
            Ok(Removal::Collision) => tracing::warn!(
                name = row.name,
                "a probe object's name is held by an object that is not ours"
            ),
            Err(error) => tracing::warn!(
                code = error.code,
                name = row.name,
                "a probe object could not be removed"
            ),
        }
    }
}

const fn removal_order(role: BackendRole) -> u8 {
    match role {
        BackendRole::Pod => 0,
        BackendRole::Keeper => 1,
        BackendRole::Pvc => 2,
    }
}
