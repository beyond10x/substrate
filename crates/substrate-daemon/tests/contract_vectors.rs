#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use substrate_daemon::{App, Authority, Identity, router};
use substrate_host::WorkspaceDestroyProgress;
use substrate_host::{DispatchOutcome, Driver, DriverError, DriverErrorClass, ExecObservation};
use substrate_store::{
    EventCursorError, ExecWrite, LeaseClock, NewOperation, Reservation, Scope, Store, StoreConfig,
    StoredExec, WorkspaceDestroyReservation,
};
use substrate_wire::{
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, CapabilitySnapshot, ConfinementRequest,
    Exec, ExecOutputQuery, ExecSignalInput, ExecStartInput, ExecState, FileAbsence,
    FileObservation, FileReadQuery, FileReadResult, HostDriverKind, NetworkMode, OutputSlice,
    PipeSessionStartInput, SandboxProfile, Workspace, WorkspaceCreateInput, WorkspaceKind,
    WorkspaceState,
};
use tempfile::TempDir;
use tokio::sync::Notify;
use tower::ServiceExt as _;

const FIXED_TIME: &str = "2026-08-13T12:00:00Z";
const SNAPSHOT: &str = "sha256:7777777777777777777777777777777777777777777777777777777777777777";
const EXECUTABLE_VECTORS_0_2: &[&str] = &[
    "vectors/driver/crash-after-dispatch.json",
    "vectors/driver/crash-before-dispatch.json",
    "vectors/driver/event-push-pull-identity.json",
    "vectors/driver/event-retention-gap.json",
    "vectors/driver/event-stream-backpressure.json",
    "vectors/driver/restart-no-redispatch.json",
    "vectors/driver/snapshot-concurrent-mutation.json",
    "vectors/http/event-cross-scope-cursor.json",
    "vectors/http/exec-capacity.json",
    "vectors/http/exec-start.json",
    "vectors/http/input-body-limit.json",
    "vectors/http/ledger-capacity.json",
    "vectors/http/machinery-failure.json",
    "vectors/http/reconciliation-snapshot-create.json",
    "vectors/http/reconciliation-snapshot-empty.json",
    "vectors/http/reconciliation-snapshot-get.json",
    "vectors/http/replay-conflict.json",
    "vectors/http/workspace-capacity.json",
    "vectors/http/write-limit.json",
];

const EXECUTABLE_VECTORS_0_3: &[&str] = &[
    "vectors/driver/crash-after-dispatch.json",
    "vectors/driver/crash-before-dispatch.json",
    "vectors/driver/event-push-pull-identity.json",
    "vectors/driver/event-retention-gap.json",
    "vectors/driver/event-stream-backpressure.json",
    "vectors/driver/restart-no-redispatch.json",
    "vectors/driver/snapshot-concurrent-mutation.json",
    "vectors/http/event-cross-scope-cursor.json",
    "vectors/http/exec-capacity.json",
    "vectors/http/exec-start.json",
    "vectors/http/input-body-limit.json",
    "vectors/http/ledger-capacity.json",
    "vectors/http/machinery-failure.json",
    "vectors/http/pipe-session-missing-lease.json",
    "vectors/http/pipe-session-start.json",
    "vectors/http/reconciliation-snapshot-create.json",
    "vectors/http/reconciliation-snapshot-empty.json",
    "vectors/http/reconciliation-snapshot-get.json",
    "vectors/http/replay-conflict.json",
    "vectors/http/workspace-capacity.json",
    "vectors/http/write-limit.json",
];

#[derive(Clone)]
struct VectorAuthority {
    snapshot_id: String,
    clock_available: Arc<AtomicBool>,
}

impl Default for VectorAuthority {
    fn default() -> Self {
        Self {
            snapshot_id: "snap_vector".to_owned(),
            clock_available: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl Authority for VectorAuthority {
    fn now(&self) -> DateTime<Utc> {
        FIXED_TIME.parse().expect("fixed time")
    }

    fn request_id(&self) -> String {
        "req_vector_fallback".to_owned()
    }

    fn workspace_id(&self) -> String {
        "ws_vector".to_owned()
    }

    fn exec_id(&self) -> String {
        "ex_vector".to_owned()
    }

    fn session_id(&self) -> String {
        "ses_vector".to_owned()
    }

    fn snapshot_id(&self) -> String {
        self.snapshot_id.clone()
    }

    fn lease_clock(&self) -> Result<LeaseClock, String> {
        if !self.clock_available.load(Ordering::SeqCst) {
            return Err("injected unavailable lease clock".to_owned());
        }
        Ok(LeaseClock {
            wall: self.now(),
            boot_id: "00000000-0000-0000-0000-000000000001".to_owned(),
            boottime_ms: 1_000,
        })
    }
}

struct VectorDriver {
    fail_write: bool,
    signal_race_store: Option<Arc<Store>>,
    start_entered: Option<Arc<Notify>>,
    start_release: Option<Arc<Notify>>,
    start_count: AtomicUsize,
    destroy_count: AtomicUsize,
    destroy_pending_remaining: AtomicUsize,
    dispatch_count: AtomicUsize,
    observed_exec: StdMutex<Option<ExecObservation>>,
    output_result: StdMutex<Option<OutputSlice>>,
    output_count: AtomicUsize,
    write_count: AtomicUsize,
    write_limit: Option<usize>,
    created_workspace: StdMutex<Option<(String, String, Workspace)>>,
    workspace_create_outcome: Option<DriverErrorClass>,
    exec_start_outcome: Option<DriverErrorClass>,
    workspace_create_observed: AtomicBool,
    after_observed_hook: StdMutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl VectorDriver {
    fn signal_observation() -> ExecObservation {
        ExecObservation {
            resource: Exec {
                id: "ex_vector".to_owned(),
                kind: substrate_wire::ExecKind::Exec,
                workspace: "ws_vector".to_owned(),
                state: ExecState::Cancelled,
                observed_at: FIXED_TIME.parse().expect("fixed time"),
                requested: ConfinementRequest {
                    capability_snapshot: SNAPSHOT.to_owned(),
                    network: NetworkMode::None,
                    aperture: None,
                    profile: SandboxProfile::Workspace,
                    required: true,
                },
                applied: Some(AppliedConfinement {
                    workspace_access: substrate_wire::WorkspaceAccess::ReadWrite,
                    read_only_roots: Vec::new(),
                    secret_slots: Vec::new(),
                    capability_snapshot: SNAPSHOT.to_owned(),
                    profile: SandboxProfile::Workspace,
                    filesystem: AppliedFilesystem::WorkspaceReadWriteSystemReadOnly,
                    network: AppliedNetwork::None,
                    cgroup: "substrate/vector".to_owned(),
                    capsule: None,
                }),
                exit: Some(substrate_wire::ExecExit {
                    code: None,
                    signal: Some(substrate_wire::Signal::Term),
                }),
                usage: None,
                lease: None,
                refusal: None,
            },
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            output_complete: true,
            cgroup: Some("substrate/vector".to_owned()),
            leader_pid: None,
        }
    }
}

#[async_trait]
impl Driver for VectorDriver {
    fn machine(&self) -> CapabilitySnapshot {
        let facts = substrate_wire::CapabilityFacts {
            exec_namespaces: Some(substrate_wire::NamespaceFacts {
                user: true,
                mount: true,
                pid: true,
                ipc: true,
                uts: true,
                network: true,
            }),
            exec_cgroup_limits: Some(substrate_wire::CgroupLimitFacts {
                processes: true,
                memory: true,
                cpu: true,
            }),
            exec_cgroup_kill: Some(true),
            exec_no_egress: Some(true),
            exec_output_limit_bytes: Some(1_048_576),
            leases_explicit: Some(true),
            ..substrate_wire::CapabilityFacts::default()
        };
        CapabilitySnapshot {
            snapshot: SNAPSHOT.to_owned(),
            driver: HostDriverKind::Host,
            driver_version: "vector".to_owned(),
            config_generation: 1,
            probed_at: FIXED_TIME.parse().expect("fixed time"),
            valid_until: None,
            facts,
        }
    }

    fn workspace_root_identity(&self, id: &str) -> Result<String, DriverError> {
        Ok(format!("root-{id}"))
    }

    async fn create_workspace(
        &self,
        id: &str,
        root_name: &str,
        input: &WorkspaceCreateInput,
    ) -> DispatchOutcome<Workspace> {
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);
        if self.workspace_create_observed.load(Ordering::SeqCst) {
            let workspace = Workspace {
                id: id.to_owned(),
                kind: WorkspaceKind::Workspace,
                labels: input.labels.clone(),
                observed_at: FIXED_TIME.parse().expect("fixed time"),
                state: WorkspaceState::Ready,
                storage: None,
                lease: None,
            };
            *self
                .created_workspace
                .lock()
                .expect("created workspace lock") =
                Some((id.to_owned(), root_name.to_owned(), workspace.clone()));
            if let Some(hook) = self
                .after_observed_hook
                .lock()
                .expect("after-observed hook lock")
                .take()
            {
                hook();
            }
            return DispatchOutcome::Observed(workspace);
        }
        match self.workspace_create_outcome {
            Some(DriverErrorClass::Failed) => {
                let workspace = Workspace {
                    id: id.to_owned(),
                    kind: WorkspaceKind::Workspace,
                    labels: input.labels.clone(),
                    observed_at: FIXED_TIME.parse().expect("fixed time"),
                    state: WorkspaceState::Ready,
                    storage: None,
                    lease: None,
                };
                *self
                    .created_workspace
                    .lock()
                    .expect("created workspace lock") =
                    Some((id.to_owned(), root_name.to_owned(), workspace));
                DispatchOutcome::OutcomeUnknown(DriverError::failed(
                    "workspace.create-unknown",
                    "create outcome is unknown",
                ))
            }
            Some(_) => DispatchOutcome::ContainedAbsent(DriverError::not_found()),
            None => DispatchOutcome::NotDispatched(DriverError::not_found()),
        }
    }

    async fn observe_workspace(
        &self,
        id: &str,
        root_name: &str,
        _previous: &Workspace,
    ) -> Result<Workspace, DriverError> {
        self.created_workspace
            .lock()
            .expect("created workspace lock")
            .as_ref()
            .filter(|(created_id, created_root, _)| created_id == id && created_root == root_name)
            .map(|(_, _, workspace)| workspace.clone())
            .ok_or_else(DriverError::not_found)
    }

    async fn read_workspace_path(
        &self,
        _workspace_id: &str,
        _root_name: &str,
        _path: &str,
        _query: &FileReadQuery,
    ) -> Result<FileReadResult, DriverError> {
        Err(DriverError::not_found())
    }

    async fn write_workspace_file(
        &self,
        workspace_id: &str,
        _root_name: &str,
        path: &str,
        content: &[u8],
    ) -> Result<FileObservation, DriverError> {
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);
        self.write_count.fetch_add(1, Ordering::SeqCst);
        if self.write_limit.is_some_and(|limit| content.len() > limit) {
            return Err(DriverError::exhausted(
                "workspace.write-limit",
                "Requested replacement exceeds the probed limit.",
                "limit",
            ));
        }
        if self.fail_write {
            return Err(DriverError {
                class: DriverErrorClass::Failed,
                code: "workspace.driver-failed",
                message: "Atomic replacement failed after operation acceptance.".to_owned(),
                address: Some("workspace.file".to_owned()),
                retriable: true,
            });
        }
        Ok(FileObservation {
            kind: substrate_wire::FileKind::File,
            workspace: workspace_id.to_owned(),
            path: path.to_owned(),
            size: u64::try_from(content.len()).expect("content length"),
            sha256: "d".repeat(64),
            atomic_replacement: true,
            observed_at: FIXED_TIME.parse().expect("fixed time"),
        })
    }

    async fn delete_workspace_file(
        &self,
        _workspace_id: &str,
        _root_name: &str,
        _path: &str,
    ) -> Result<FileAbsence, DriverError> {
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);
        Err(DriverError::not_found())
    }

    async fn destroy_workspace(
        &self,
        _workspace_id: &str,
        _root_name: &str,
    ) -> Result<WorkspaceDestroyProgress, DriverError> {
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);
        self.destroy_count.fetch_add(1, Ordering::SeqCst);
        if self
            .destroy_pending_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Ok(WorkspaceDestroyProgress::Pending { removed_items: 7 });
        }
        Err(DriverError::not_found())
    }

    async fn start_exec(
        &self,
        id: &str,
        _workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);
        self.start_count.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = &self.start_entered {
            entered.notify_one();
        }
        if let Some(release) = &self.start_release {
            release.notified().await;
        }
        let mut observation = Self::signal_observation();
        id.clone_into(&mut observation.resource.id);
        observation.resource.workspace.clone_from(&input.workspace);
        observation.resource.requested.clone_from(&input.sandbox);
        observation
            .resource
            .applied
            .as_mut()
            .expect("vector confinement")
            .workspace_access
            .clone_from(&input.workspace_access);
        observation.resource.state = ExecState::Running;
        observation.resource.exit = None;
        observation.output_complete = false;
        match self.exec_start_outcome {
            Some(DriverErrorClass::Failed) => DispatchOutcome::OutcomeUnknown(DriverError::failed(
                "exec.start-unknown",
                "start outcome is unknown",
            )),
            Some(_) => DispatchOutcome::ContainedAbsent(DriverError::not_found()),
            None => {
                *self.observed_exec.lock().expect("observed exec lock") = Some(observation.clone());
                if let Some(hook) = self
                    .after_observed_hook
                    .lock()
                    .expect("after-observed hook lock")
                    .take()
                {
                    hook();
                }
                DispatchOutcome::Observed(observation)
            }
        }
    }

    async fn start_pipe_session(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.start_exec(id, workspace_root_name, &input.exec).await
    }

    async fn observe_exec(&self, _id: &str) -> Result<ExecObservation, DriverError> {
        self.observed_exec
            .lock()
            .expect("observed exec lock")
            .clone()
            .ok_or_else(DriverError::not_found)
    }

    async fn output(
        &self,
        _id: &str,
        _query: &ExecOutputQuery,
    ) -> Result<OutputSlice, DriverError> {
        self.output_count.fetch_add(1, Ordering::SeqCst);
        self.output_result
            .lock()
            .expect("output result lock")
            .clone()
            .ok_or_else(DriverError::not_found)
    }

    async fn signal(
        &self,
        _id: &str,
        _input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);
        if let Some(store) = &self.signal_race_store {
            store
                .put_exec(
                    &Scope {
                        deployment: "dep_vector".to_owned(),
                        subject: "local:1000".to_owned(),
                    },
                    &StoredExec {
                        resource: Self::signal_observation().resource,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                        stdout_truncated: false,
                        stderr_truncated: false,
                        output_complete: true,
                        cgroup: Some("substrate/vector".to_owned()),
                        leader_pid: None,
                    },
                )
                .expect("inject concurrent terminal persistence");
            return Err(DriverError::not_found());
        }
        Ok(Self::signal_observation())
    }

    fn completed_execs(&self) -> Vec<ExecObservation> {
        Vec::new()
    }

    fn set_exec_lease(&self, _id: &str, _lease: Option<substrate_wire::LeaseObservation>) {}

    fn acknowledge_exec(&self, _persisted: &ExecObservation) {}

    fn discard_superseded_exec(&self, _id: &str) {}
}

struct Harness {
    directory: TempDir,
    app: Arc<App>,
    store: Arc<Store>,
    driver: Arc<VectorDriver>,
    clock_available: Arc<AtomicBool>,
}

impl Harness {
    fn open(fail_write: bool) -> Self {
        Self::open_with_driver(fail_write, false, false)
    }

    fn open_with_driver(fail_write: bool, signal_race: bool, blocked_start: bool) -> Self {
        Self::open_with_outcomes(fail_write, signal_race, blocked_start, None, None)
    }

    fn open_with_outcomes(
        fail_write: bool,
        signal_race: bool,
        blocked_start: bool,
        workspace_create_outcome: Option<DriverErrorClass>,
        exec_start_outcome: Option<DriverErrorClass>,
    ) -> Self {
        Self::open_custom(
            StoreConfig::default(),
            "snap_vector",
            None,
            fail_write,
            signal_race,
            blocked_start,
            workspace_create_outcome,
            exec_start_outcome,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_custom(
        config: StoreConfig,
        snapshot_id: &str,
        write_limit: Option<usize>,
        fail_write: bool,
        signal_race: bool,
        blocked_start: bool,
        workspace_create_outcome: Option<DriverErrorClass>,
        exec_start_outcome: Option<DriverErrorClass>,
    ) -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        let store = Arc::new(
            Store::open_with_config(directory.path().join("state.db"), config)
                .expect("state store"),
        );
        let driver = Arc::new(VectorDriver {
            fail_write,
            signal_race_store: signal_race.then(|| Arc::clone(&store)),
            start_entered: blocked_start.then(|| Arc::new(Notify::new())),
            start_release: blocked_start.then(|| Arc::new(Notify::new())),
            start_count: AtomicUsize::new(0),
            destroy_count: AtomicUsize::new(0),
            destroy_pending_remaining: AtomicUsize::new(0),
            dispatch_count: AtomicUsize::new(0),
            observed_exec: StdMutex::new(None),
            output_result: StdMutex::new(None),
            output_count: AtomicUsize::new(0),
            write_count: AtomicUsize::new(0),
            write_limit,
            created_workspace: StdMutex::new(None),
            workspace_create_outcome,
            exec_start_outcome,
            workspace_create_observed: AtomicBool::new(false),
            after_observed_hook: StdMutex::new(None),
        });
        let erased: Arc<dyn Driver> = driver.clone();
        let clock_available = Arc::new(AtomicBool::new(true));
        let app = App::with_authority(
            Arc::clone(&store),
            erased,
            "dep_vector",
            Arc::new(VectorAuthority {
                snapshot_id: snapshot_id.to_owned(),
                clock_available: Arc::clone(&clock_available),
            }),
        );
        Self {
            directory,
            app,
            store,
            driver,
            clock_available,
        }
    }

    fn set_stream_fixture(&self, source_scope: &str, generation: u64, next_seq: u64) {
        let connection = rusqlite::Connection::open(self.directory.path().join("state.db"))
            .expect("fixture connection");
        connection
            .execute(
                "DELETE FROM events WHERE deployment = 'dep_vector' AND subject = 'local:1000'",
                [],
            )
            .expect("clear stream fixture events");
        connection
            .execute(
                "INSERT INTO stream_meta (
                    deployment, subject, source_scope, generation, next_seq
                 ) VALUES ('dep_vector', 'local:1000', ?1, ?2, ?3)
                 ON CONFLICT (deployment, subject) DO UPDATE SET
                    source_scope = excluded.source_scope,
                    generation = excluded.generation,
                    next_seq = excluded.next_seq",
                rusqlite::params![
                    source_scope,
                    i64::try_from(generation).expect("generation"),
                    i64::try_from(next_seq).expect("next seq")
                ],
            )
            .expect("seed stream fixture");
    }

    fn fail_terminal_commit_after_observation(&self) {
        let database = self.directory.path().join("state.db");
        self.driver
            .after_observed_hook
            .lock()
            .expect("after-observed hook lock")
            .replace(Arc::new(move || {
                rusqlite::Connection::open(&database)
                    .expect("failure-injection connection")
                    .execute_batch(
                        "CREATE TRIGGER inject_terminal_commit_failure
                         BEFORE UPDATE OF state ON operations
                         WHEN NEW.state = 'terminal'
                         BEGIN
                           SELECT RAISE(ABORT, 'injected terminal commit failure');
                         END;",
                    )
                    .expect("install terminal-commit failure");
            }));
    }

    fn restore_terminal_commits(&self) {
        rusqlite::Connection::open(self.directory.path().join("state.db"))
            .expect("failure-injection connection")
            .execute_batch("DROP TRIGGER inject_terminal_commit_failure;")
            .expect("remove terminal-commit failure");
    }

    fn insert_snapshot_page_fixture(&self, vector: &Value) {
        let response = &vector["expected"]["response"]["body"]["result"];
        let snapshot = response["snapshot"].as_str().expect("snapshot id");
        let generation = response["generation"].as_u64().expect("generation");
        let through_seq = response["through_seq"].as_u64().expect("through seq");
        let connection = rusqlite::Connection::open(self.directory.path().join("state.db"))
            .expect("fixture connection");
        connection
            .execute(
                "INSERT INTO snapshots (
                    deployment, subject, id, source_scope, generation, through_seq,
                    item_count, expires_at
                 ) VALUES ('dep_vector', 'local:1000', ?1, 'scope_vector_subject', ?2, ?3, 3,
                           '2026-08-13T12:05:00Z')",
                rusqlite::params![
                    snapshot,
                    i64::try_from(generation).expect("generation"),
                    i64::try_from(through_seq).expect("through seq")
                ],
            )
            .expect("insert snapshot fixture");
        let items = response["items"].as_array().expect("snapshot items");
        for item in items {
            connection
                .execute(
                    "INSERT INTO snapshot_items (
                        deployment, subject, snapshot_id, ordinal, item_json
                     ) VALUES ('dep_vector', 'local:1000', ?1, ?2, ?3)",
                    rusqlite::params![
                        snapshot,
                        i64::try_from(item["ordinal"].as_u64().expect("ordinal"))
                            .expect("ordinal range"),
                        serde_json::to_string(item).expect("snapshot item JSON")
                    ],
                )
                .expect("insert snapshot item fixture");
        }
        let mut trailing = items.last().expect("second snapshot item").clone();
        trailing["ordinal"] = json!(3);
        trailing["id"] = json!("event:41:6");
        trailing["value"]["seq"] = json!(6);
        connection
            .execute(
                "INSERT INTO snapshot_items (
                    deployment, subject, snapshot_id, ordinal, item_json
                 ) VALUES ('dep_vector', 'local:1000', ?1, 3, ?2)",
                rusqlite::params![
                    snapshot,
                    serde_json::to_string(&trailing).expect("trailing item JSON")
                ],
            )
            .expect("insert trailing snapshot item fixture");
    }

    fn delete_event_fixture(&self, sequence: u64) {
        let connection = rusqlite::Connection::open(self.directory.path().join("state.db"))
            .expect("fixture connection");
        connection
            .execute(
                "DELETE FROM events
                 WHERE deployment = 'dep_vector' AND subject = 'local:1000' AND seq = ?1",
                rusqlite::params![i64::try_from(sequence).expect("event sequence")],
            )
            .expect("delete fixture event");
    }

    async fn execute(&self, vector: &Value) -> Value {
        let request = &vector["action"]["request"];
        let method = Method::from_bytes(request["method"].as_str().expect("method").as_bytes())
            .expect("HTTP method");
        let mut uri = request["path"].as_str().expect("path").to_owned();
        let query = request.get("query").and_then(Value::as_object);
        if query.is_some_and(|query| !query.is_empty()) {
            let query = query.expect("checked query");
            let encoded = serde_urlencoded::to_string(query).expect("query encoding");
            uri.push('?');
            uri.push_str(&encoded);
        }
        let body = if vector["action"]["kind"] == "raw-http" {
            let repeat = &request["body"]["repeat"];
            let count = repeat["count"].as_u64().expect("repeat count");
            let octet = u8::from_str_radix(repeat["octet_hex"].as_str().expect("repeat octet"), 16)
                .expect("hex octet");
            Body::from(vec![octet; usize::try_from(count).expect("body count")])
        } else {
            request.get("body").map_or_else(Body::empty, |body| {
                Body::from(serde_json::to_vec(body).expect("body"))
            })
        };
        let request_id = request["headers"]["x-request-id"]
            .as_str()
            .expect("request id");
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-request-id", request_id)
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: vector["context"]["subject"]
                .as_str()
                .expect("subject")
                .to_owned(),
            actor: "vector-client".to_owned(),
            principal: None,
        };
        let response = router(Arc::clone(&self.app))
            .layer(Extension(identity))
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status().as_u16();
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 2_097_152)
                .await
                .expect("response body"),
        )
        .expect("JSON response");
        json!({ "body": body, "status": status })
    }
}

fn bundle_vector(version: &str, layer: &str, name: &str) -> Value {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("contracts/substrate-wire")
        .join(version);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("bundle.json")).expect("bundle manifest"))
            .expect("manifest JSON");
    let relative = format!("vectors/{layer}/{name}.json");
    assert!(
        manifest["files"]
            .as_array()
            .expect("manifest files")
            .iter()
            .any(|entry| entry["path"] == relative),
        "vector must be selected from the immutable bundle manifest"
    );
    if matches!(version, "0.2.0" | "0.3.0" | "0.4.0") {
        assert!(
            manifest["conformance"]["executable_vectors"]
                .as_array()
                .expect("manifest executable vectors")
                .iter()
                .any(|entry| entry == &relative),
            "0.2 runtime vector must be manifest-selected as executable"
        );
    }
    serde_json::from_slice(&std::fs::read(root.join(relative)).expect("vector bytes"))
        .expect("vector JSON")
}

#[test]
fn successor_executable_manifest_is_the_exact_review_branch_set() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("contracts/substrate-wire/0.4.0");
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(root.join("bundle.json")).expect("bundle manifest bytes"),
    )
    .expect("bundle manifest JSON");
    let actual = manifest["conformance"]["executable_vectors"]
        .as_array()
        .expect("executable vector array")
        .iter()
        .map(|entry| entry.as_str().expect("vector path"))
        .collect::<Vec<_>>();
    assert_eq!(actual, EXECUTABLE_VECTORS_0_3);
}

#[tokio::test(flavor = "multi_thread")]
async fn successor_pipe_session_positive_and_adversarial_vectors_execute_exactly() {
    let positive = bundle_vector("0.4.0", "http", "pipe-session-start");
    let harness = Harness::open(false);
    seed_workspace(&harness.store);
    assert_exact_http(&harness.execute(&positive).await, &positive);
    assert_eq!(harness.driver.start_count.load(Ordering::SeqCst), 1);

    let refusal = bundle_vector("0.4.0", "http", "pipe-session-missing-lease");
    let harness = Harness::open(false);
    seed_workspace(&harness.store);
    assert_exact_http(&harness.execute(&refusal).await, &refusal);
    assert_eq!(harness.driver.start_count.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_scoped_write_vector_executes_the_production_boundary() {
    let vector = bundle_vector("0.12.0", "http", "workspace-scoped-write");
    let harness = Harness::open(false);
    seed_workspace(&harness.store);
    assert_exact_http(&harness.execute(&vector).await, &vector);
    assert_eq!(harness.driver.start_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn pipe_session_clock_refusal_is_durable_when_the_clock_recovers() {
    let start = bundle_vector("0.4.0", "http", "pipe-session-start");
    let harness = Harness::open(false);
    seed_workspace(&harness.store);
    assert_exact_http(&harness.execute(&start).await, &start);

    let renew = json!({
        "context": {"subject": "local:1000"},
        "action": {
            "kind": "http",
            "request": {
                "method": "POST",
                "path": "/v1/pipe-sessions/ses_vector/lease/renew",
                "query": {},
                "headers": {"x-request-id": "req_pipe_clock_unavailable"},
                "body": {
                    "op": "01JPIPECLOCKUNAVAILABLE01",
                    "input": {"ttl_ms": 90000}
                }
            }
        }
    });
    harness.clock_available.store(false, Ordering::SeqCst);
    let refused = harness.execute(&renew).await;
    assert_eq!(refused["status"], 501);
    assert_eq!(refused["body"]["error"]["code"], "lease.clock-unavailable");

    harness.clock_available.store(true, Ordering::SeqCst);
    assert_eq!(
        harness.execute(&renew).await,
        refused,
        "the same operation replays its first durable refusal after clock recovery"
    );
}

#[test]
fn executable_manifest_is_the_exact_review_branch_set() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("contracts/substrate-wire/0.2.0");
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(root.join("bundle.json")).expect("bundle manifest bytes"),
    )
    .expect("bundle manifest JSON");
    let actual = manifest["conformance"]["executable_vectors"]
        .as_array()
        .expect("executable vector array")
        .iter()
        .map(|entry| entry.as_str().expect("vector path"))
        .collect::<Vec<_>>();
    assert_eq!(actual, EXECUTABLE_VECTORS_0_2);
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One table-like test executes the three distinct finite quotas.
async fn manifest_write_conflict_and_quota_vectors_execute_exactly() {
    let write = bundle_vector("0.2.0", "http", "write-limit");
    let harness = Harness::open_custom(
        StoreConfig {
            event_retention: 2,
            ..StoreConfig::default()
        },
        "snap_vector",
        Some(4),
        false,
        false,
        false,
        None,
        None,
    );
    seed_workspace(&harness.store);
    assert_exact_http(&harness.execute(&write).await, &write);
    assert_eq!(harness.driver.write_count.load(Ordering::SeqCst), 1);

    let conflict = bundle_vector("0.2.0", "http", "replay-conflict");
    let harness = Harness::open(false);
    let operation = conflict["setup"][0]["state"]["operation"]
        .as_str()
        .expect("conflicting operation");
    let request_hash = conflict["setup"][0]["state"]["request_hash"]
        .as_str()
        .expect("conflicting request hash");
    seed_accepted_operation(&harness.store, operation, request_hash);
    assert_exact_http(&harness.execute(&conflict).await, &conflict);

    let ledger = bundle_vector("0.2.0", "http", "ledger-capacity");
    let harness = Harness::open_custom(
        constrained_config(),
        "snap_vector",
        None,
        false,
        false,
        false,
        None,
        None,
    );
    seed_accepted_operation(&harness.store, "01JLEDGERCAPACITYSEED01", &"a".repeat(64));
    assert_exact_http(&harness.execute(&ledger).await, &ledger);

    let workspace = bundle_vector("0.2.0", "http", "workspace-capacity");
    let harness = Harness::open_custom(
        StoreConfig {
            snapshot_max_workspaces: 1,
            ..StoreConfig::default()
        },
        "snap_vector",
        None,
        false,
        false,
        false,
        None,
        None,
    );
    seed_workspace(&harness.store);
    assert_exact_http(&harness.execute(&workspace).await, &workspace);

    let exec = bundle_vector("0.2.0", "http", "exec-capacity");
    let harness = Harness::open_custom(
        StoreConfig {
            snapshot_max_execs: 1,
            ..StoreConfig::default()
        },
        "snap_vector",
        None,
        false,
        false,
        false,
        None,
        None,
    );
    seed_workspace(&harness.store);
    seed_running_exec_named(&harness.store, "ex_capacityseed");
    assert_exact_http(&harness.execute(&exec).await, &exec);
    assert_eq!(harness.driver.start_count.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One matrix proves every bound-refusal category identically.
async fn bound_refusal_matrix_is_durable_replayable_conflict_safe_and_never_dispatches() {
    let harness = Harness::open(false);
    let request = refusal_request(
        "POST",
        "/v1/workspaces",
        "01JREFUSALUNKNOWNFIELD01",
        &json!({"source": "empty", "labels": {}, "secret": "forbidden"}),
        &json!({}),
        "req_refusal_unknown",
    );
    let mut changed = request.clone();
    changed["action"]["request"]["body"]["input"]["secret"] = json!("changed");
    assert_bound_refusal_case(&harness, &request, &changed, "request.schema-invalid").await;

    let harness = Harness::open(false);
    let request = refusal_request(
        "POST",
        "/v1/workspaces",
        "01JREFUSALQUERYSHAPE001",
        &json!({"source": "empty", "labels": {}}),
        &json!({"unexpected": "one"}),
        "req_refusal_query",
    );
    let mut changed = request.clone();
    changed["action"]["request"]["query"]["unexpected"] = json!("two");
    let before = harness.driver.dispatch_count.load(Ordering::SeqCst);
    let first = harness.execute(&request).await;
    assert_eq!(first["body"]["error"]["code"], "request.schema-invalid");
    assert_eq!(harness.execute(&request).await, first);
    let current_refusal = harness.execute(&changed).await;
    assert_eq!(current_refusal["status"], 422);
    assert_eq!(
        current_refusal["body"]["error"]["code"],
        "request.schema-invalid"
    );
    assert_eq!(
        harness.driver.dispatch_count.load(Ordering::SeqCst),
        before,
        "current request validation before replay must not dispatch"
    );

    let harness = Harness::open(false);
    let request = refusal_request(
        "PUT",
        "/v1/workspaces/ws_missing/files/file.txt",
        "01JREFUSALMISSINGRESOURCE1",
        &json!({"content": {"encoding": "base64", "data": "aGVsbG8="}}),
        &json!({}),
        "req_refusal_missing",
    );
    let mut changed = request.clone();
    changed["action"]["request"]["body"]["input"]["changed"] = json!(true);
    assert_bound_refusal_case(&harness, &request, &changed, "resource.not-found").await;

    let harness = Harness::open(false);
    seed_workspace(&harness.store);
    let request = refusal_request(
        "PUT",
        "/v1/workspaces/ws_vector/files/file.txt",
        "01JREFUSALBASE64SHAPE001",
        &json!({"content": {"encoding": "base64", "data": "AB=="}}),
        &json!({}),
        "req_refusal_base64",
    );
    let mut changed = request.clone();
    changed["action"]["request"]["body"]["input"]["content"]["data"] = json!("AC==");
    assert_bound_refusal_case(&harness, &request, &changed, "request.schema-invalid").await;

    let harness = Harness::open(false);
    let request = refusal_request(
        "POST",
        "/v1/execs/ex_vector/signal",
        "01JREFUSALGRACESCALAR001",
        &json!({"signal": "TERM", "grace_ms": 30001}),
        &json!({}),
        "req_refusal_grace",
    );
    let mut changed = request.clone();
    changed["action"]["request"]["body"]["input"]["grace_ms"] = json!(30002);
    assert_bound_refusal_case(&harness, &request, &changed, "request.schema-invalid").await;

    let harness = Harness::open(false);
    let request = refusal_request(
        "POST",
        "/v1/workspaces/ws_vector/lease/renew",
        "01JREFUSALTTLBOUNDARY001",
        &json!({"ttl_ms": 999}),
        &json!({}),
        "req_refusal_ttl",
    );
    let mut changed = request.clone();
    changed["action"]["request"]["body"]["input"]["ttl_ms"] = json!(998);
    assert_bound_refusal_case(&harness, &request, &changed, "request.schema-invalid").await;

    let harness = Harness::open_custom(
        StoreConfig {
            snapshot_max_workspaces: 1,
            ..StoreConfig::default()
        },
        "snap_vector",
        None,
        false,
        false,
        false,
        None,
        None,
    );
    seed_workspace(&harness.store);
    let request = refusal_request(
        "POST",
        "/v1/workspaces",
        "01JREFUSALRESOURCECAP001",
        &json!({"source": "empty", "labels": {}}),
        &json!({}),
        "req_refusal_capacity",
    );
    let mut changed = request.clone();
    changed["action"]["request"]["body"]["input"]["labels"] = json!({"changed": "yes"});
    assert_bound_refusal_case(&harness, &request, &changed, "workspace.capacity").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn manifest_empty_snapshot_bootstraps_at_ledger_capacity_exactly() {
    let vector = bundle_vector("0.2.0", "http", "reconciliation-snapshot-empty");
    let snapshot_id = vector["expected"]["response"]["body"]["result"]["id"]
        .as_str()
        .expect("snapshot id");
    let harness = Harness::open_custom(
        constrained_config(),
        snapshot_id,
        None,
        false,
        false,
        false,
        None,
        None,
    );
    seed_accepted_operation(&harness.store, "01JSNAPSHOTLEDGERSEED01", &"b".repeat(64));
    harness.set_stream_fixture("scope_vector_subject", 41, 1);
    assert_exact_http(&harness.execute(&vector).await, &vector);
}

#[tokio::test(flavor = "multi_thread")]
async fn manifest_snapshot_create_materializes_the_exact_barrier() {
    let vector = bundle_vector("0.2.0", "http", "reconciliation-snapshot-create");
    let snapshot_id = vector["expected"]["response"]["body"]["result"]["id"]
        .as_str()
        .expect("snapshot id");
    let harness = Harness::open_custom(
        StoreConfig {
            event_retention: 2,
            ..StoreConfig::default()
        },
        snapshot_id,
        None,
        false,
        false,
        false,
        None,
        None,
    );
    harness.set_stream_fixture("scope_vector_subject", 41, 6);
    seed_workspace_named(
        &harness.store,
        "ws_vector",
        "01JPHASE3EVENTSOURCE0001",
        "vector-client",
    );
    harness.delete_event_fixture(6);
    assert_exact_http(&harness.execute(&vector).await, &vector);
}

#[tokio::test(flavor = "multi_thread")]
async fn manifest_snapshot_page_reads_one_exact_materialized_view() {
    let vector = bundle_vector("0.2.0", "http", "reconciliation-snapshot-get");
    let harness = Harness::open(false);
    harness.insert_snapshot_page_fixture(&vector);
    assert_exact_http(&harness.execute(&vector).await, &vector);
}

#[tokio::test(flavor = "multi_thread")]
async fn manifest_event_source_scope_vector_executes_exactly() {
    let vector = bundle_vector("0.2.0", "http", "event-cross-scope-cursor");
    let harness = Harness::open(false);
    harness.set_stream_fixture("scope_vector_subject", 41, 8);
    assert_exact_http(&harness.execute(&vector).await, &vector);
}

#[test]
fn manifest_push_pull_and_retention_vectors_execute_against_the_store() {
    let identity = bundle_vector("0.2.0", "driver", "event-push-pull-identity");
    let harness = Harness::open(false);
    harness.set_stream_fixture("scope_vector_subject", 41, 6);
    seed_workspace_named(
        &harness.store,
        "ws_vector",
        "01JPHASE3EVENTSOURCE0001",
        "vector-client",
    );
    let page = harness
        .store
        .events(&vector_scope(), None, 100)
        .expect("event page store")
        .expect("event page");
    let actual = json!({
        "pull_cursor": page.next_cursor,
        "push_last_cursor": page.items.last().map(|event| {
            format!("ev2.{}.{}.{}", event.source_scope, event.generation, event.seq)
        }).expect("pushed event"),
    });
    assert_eq!(actual, identity["expected"]["outcome"]);

    let backpressure = bundle_vector("0.2.0", "driver", "event-stream-backpressure");
    let event_count = backpressure["setup"][0]["state"]["event_count"]
        .as_u64()
        .expect("event count");
    let payload_bytes = backpressure["setup"][0]["state"]["observation_payload_bytes"]
        .as_u64()
        .expect("observation payload bytes");
    let max_output_bytes = backpressure["setup"][0]["state"]["max_output_bytes"]
        .as_u64()
        .expect("max output bytes");
    assert!(event_count.saturating_mul(payload_bytes) > max_output_bytes);
    assert_eq!(backpressure["expected"]["outcome"]["recovery"], "pull");

    let retention = bundle_vector("0.2.0", "driver", "event-retention-gap");
    let harness = Harness::open_custom(
        StoreConfig {
            event_retention: 5,
            ..StoreConfig::default()
        },
        "snap_vector",
        None,
        false,
        false,
        false,
        None,
        None,
    );
    harness.set_stream_fixture("scope_vector_subject", 41, 1);
    for index in 1..=6 {
        seed_workspace_named(
            &harness.store,
            &format!("ws_event{index:02}"),
            &format!("01JPHASE3EVENT{index:08}"),
            "vector-client",
        );
    }
    let cursor = retention["action"]["command"]["cursor"]
        .as_str()
        .expect("retention cursor");
    let error = harness
        .store
        .events(&vector_scope(), Some(cursor), 100)
        .expect("event store")
        .expect_err("retention gap");
    assert_eq!(error, EventCursorError::Retention { first: 8, last: 12 });
    assert_eq!(
        json!({"code": "event.retention-gap", "status": "conflict"}),
        retention["expected"]["outcome"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn manifest_crash_windows_reconcile_unknown_without_redispatch() {
    for name in ["crash-after-dispatch", "restart-no-redispatch"] {
        let vector = bundle_vector("0.2.0", "driver", name);
        let operation = vector["setup"]
            .as_array()
            .and_then(|setup| {
                setup
                    .iter()
                    .find_map(|fixture| fixture["state"]["operation"].as_str())
            })
            .or_else(|| vector["action"]["command"]["operation_id"].as_str())
            .expect("crash operation");
        let harness = Harness::open(false);
        seed_workspace(&harness.store);
        let seeded = seed_accepted_operation(&harness.store, operation, &"c".repeat(64));
        if name == "crash-after-dispatch" {
            let path = vector["action"]["command"]["path"]
                .as_str()
                .expect("crash write path");
            let observation = harness
                .driver
                .write_workspace_file("ws_vector", "root-ws_vector", path, b"mutated")
                .await
                .expect("driver mutation completes before simulated crash");
            assert!(observation.atomic_replacement);
            assert_eq!(harness.driver.write_count.load(Ordering::SeqCst), 1);
        }
        let after = "2026-08-13T12:00:01Z".parse().expect("restart time");
        harness
            .store
            .reconcile_after_restart("dep_vector", after, after, 64)
            .expect("restart reconciliation");
        let record = harness
            .store
            .operation(&seeded.scope, operation)
            .expect("operation lookup")
            .expect("operation record");
        assert_eq!(record.state, substrate_wire::OperationState::Unknown);
        if name == "crash-after-dispatch" {
            assert_eq!(harness.driver.write_count.load(Ordering::SeqCst), 1);
            assert_eq!(
                json!({"operation_state_after_restart": record.state}),
                vector["expected"]["outcome"]
            );
        } else {
            assert_eq!(harness.driver.write_count.load(Ordering::SeqCst), 0);
            let page = harness
                .store
                .events(&seeded.scope, None, 100)
                .expect("event store")
                .expect("event page");
            assert_eq!(
                json!({
                    "operation_state": record.state,
                    "transition": page.items.last().expect("unknown event").transition,
                }),
                vector["expected"]["outcome"]
            );
        }
    }
}

#[test]
fn manifest_snapshot_view_ignores_post_barrier_mutation() {
    let vector = bundle_vector("0.2.0", "driver", "snapshot-concurrent-mutation");
    let harness = Harness::open_custom(
        StoreConfig {
            event_retention: 2,
            ..StoreConfig::default()
        },
        "snap_01JPHASE3VECTOR",
        None,
        false,
        false,
        false,
        None,
        None,
    );
    harness.set_stream_fixture("scope_vector_subject", 41, 6);
    seed_workspace_named(
        &harness.store,
        "ws_vector",
        "01JPHASE3EVENTSOURCE0001",
        "vector-client",
    );
    let metadata = harness
        .store
        .complete_snapshot(
            &vector_scope(),
            "vector-client",
            None,
            FIXED_TIME.parse().expect("fixed time"),
            "snap_01JPHASE3VECTOR",
            "2026-08-13T12:05:00Z".parse().expect("expiry"),
        )
        .expect("snapshot");
    seed_workspace_named(
        &harness.store,
        "ws_afterbarrier",
        "01JPHASE3AFTERBARRIER001",
        "vector-client",
    );
    let page = harness
        .store
        .snapshot_page(
            &vector_scope(),
            &metadata.id,
            None,
            100,
            FIXED_TIME.parse().expect("fixed time"),
        )
        .expect("snapshot store")
        .expect("snapshot page");
    assert!(
        page.items
            .iter()
            .all(|item| item.id != "workspace:ws_afterbarrier")
    );
    assert_eq!(
        json!({"stable": true, "through_seq": page.through_seq}),
        vector["expected"]["outcome"]
    );
}

fn seed_workspace(store: &Store) {
    seed_workspace_state(store, WorkspaceState::Ready);
}

fn seed_workspace_named(store: &Store, id: &str, operation_id: &str, actor: &str) {
    let scope = vector_scope();
    let operation = NewOperation {
        scope: scope.clone(),
        operation: operation_id.to_owned(),
        operation_kind: "workspace.create".to_owned(),
        request_hash: "5".repeat(64),
        accepted_at: FIXED_TIME.to_owned(),
        capability_snapshot: Some(SNAPSHOT.to_owned()),
        actor: actor.to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(id.to_owned()),
    };
    let provisional = Workspace {
        id: id.to_owned(),
        kind: WorkspaceKind::Workspace,
        labels: BTreeMap::default(),
        observed_at: FIXED_TIME.parse().expect("fixed time"),
        state: WorkspaceState::Unknown,
        storage: None,
        lease: None,
    };
    assert_eq!(
        store
            .reserve_workspace_create(&operation, &format!("root-{id}"), &provisional, None)
            .expect("reserve named workspace"),
        Reservation::Accepted
    );
    let mut ready = provisional;
    ready.state = WorkspaceState::Ready;
    store
        .complete_workspace(
            &scope,
            operation_id,
            FIXED_TIME,
            201,
            &format!("root-{id}"),
            &ready,
        )
        .expect("complete named workspace");
}

fn seed_workspace_state(store: &Store, state: WorkspaceState) {
    let scope = vector_scope();
    let create = NewOperation {
        scope: scope.clone(),
        operation: "01JSEEDWORKSPACECREATE01".to_owned(),
        operation_kind: "workspace.create".to_owned(),
        request_hash: "0".repeat(64),
        accepted_at: FIXED_TIME.to_owned(),
        capability_snapshot: Some(SNAPSHOT.to_owned()),
        actor: "vector-seed".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some("ws_vector".to_owned()),
    };
    let provisional = Workspace {
        id: "ws_vector".to_owned(),
        kind: WorkspaceKind::Workspace,
        labels: BTreeMap::default(),
        observed_at: FIXED_TIME.parse().expect("fixed time"),
        state: WorkspaceState::Unknown,
        storage: None,
        lease: None,
    };
    assert!(matches!(
        store
            .reserve_workspace_create(&create, "ws_vector", &provisional, None)
            .expect("reserve seed workspace"),
        Reservation::Accepted
    ));
    let mut ready = provisional;
    ready.state = WorkspaceState::Ready;
    store
        .complete_workspace(
            &scope,
            &create.operation,
            FIXED_TIME,
            201,
            "ws_vector",
            &ready,
        )
        .expect("complete seed workspace");
    if state == WorkspaceState::Destroying {
        let destroy = NewOperation {
            scope,
            operation: "01JSEEDWORKSPACEDESTROY1".to_owned(),
            operation_kind: "workspace.destroy".to_owned(),
            request_hash: "1".repeat(64),
            accepted_at: FIXED_TIME.to_owned(),
            capability_snapshot: Some(SNAPSHOT.to_owned()),
            actor: "vector-seed".to_owned(),
            principal: None,
            grant_ref: None,
            platform_principal: None,
            resource: Some("ws_vector".to_owned()),
        };
        assert!(matches!(
            store
                .reserve_workspace_destroy(&destroy, None)
                .expect("reserve seed destroy"),
            WorkspaceDestroyReservation::Admitted { .. }
        ));
    } else {
        assert_eq!(state, WorkspaceState::Ready);
    }
}

fn seed_running_exec(store: &Store) {
    let mut running = VectorDriver::signal_observation();
    running.resource.state = ExecState::Running;
    running.resource.exit = None;
    running.output_complete = false;
    let proposed = stored_observation(&running);
    admit_exec(store, &proposed);
    assert!(matches!(
        store
            .put_exec(&vector_scope(), &proposed)
            .expect("persist running exec observation"),
        ExecWrite::PersistedExact(_)
    ));
}

fn seed_running_exec_named(store: &Store, id: &str) {
    let mut running = VectorDriver::signal_observation();
    id.clone_into(&mut running.resource.id);
    running.resource.state = ExecState::Running;
    running.resource.exit = None;
    running.output_complete = false;
    let proposed = stored_observation(&running);
    let operation = NewOperation {
        scope: vector_scope(),
        operation: format!("01JSEED{id}000000000000"),
        operation_kind: "exec.start".to_owned(),
        request_hash: "8".repeat(64),
        accepted_at: FIXED_TIME.to_owned(),
        capability_snapshot: Some(SNAPSHOT.to_owned()),
        actor: "vector-seed".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(id.to_owned()),
    };
    let mut provisional = proposed.clone();
    provisional.resource.state = ExecState::Accepted;
    assert_eq!(
        store
            .reserve_exec_start(&operation, &provisional, None, None)
            .expect("reserve named exec"),
        Reservation::Accepted
    );
    assert!(matches!(
        store
            .put_exec(&vector_scope(), &proposed)
            .expect("persist named exec"),
        ExecWrite::PersistedExact(_)
    ));
}

fn seed_accepted_operation(store: &Store, operation: &str, request_hash: &str) -> NewOperation {
    let operation = NewOperation {
        scope: vector_scope(),
        operation: operation.to_owned(),
        operation_kind: "workspace.file.write".to_owned(),
        request_hash: request_hash.to_owned(),
        accepted_at: FIXED_TIME.to_owned(),
        capability_snapshot: Some(SNAPSHOT.to_owned()),
        actor: "vector-client".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some("ws_vector".to_owned()),
    };
    assert_eq!(
        store.reserve(&operation).expect("reserve operation"),
        Reservation::Accepted
    );
    operation
}

fn constrained_config() -> StoreConfig {
    StoreConfig {
        operation_subject_max_rows: 1,
        operation_global_max_rows: 1,
        snapshot_max_workspaces: 1,
        snapshot_max_execs: 1,
        snapshot_max_provenance_events: 1,
        ..StoreConfig::default()
    }
}

fn store_exec(store: &Store, observation: &ExecObservation) {
    let proposed = stored_observation(observation);
    let operation = admit_exec(store, &proposed);
    assert!(matches!(
        store
            .complete_exec(
                &operation.scope,
                &operation.operation,
                FIXED_TIME,
                201,
                &proposed.resource,
                &proposed.stdout,
                &proposed.stderr,
                proposed.stdout_truncated,
                proposed.stderr_truncated,
                proposed.output_complete,
                proposed.cgroup.as_deref(),
                proposed.leader_pid,
            )
            .expect("complete seeded exec"),
        ExecWrite::PersistedExact(_)
    ));
}

fn stored_observation(observation: &ExecObservation) -> StoredExec {
    StoredExec {
        resource: observation.resource.clone(),
        stdout: observation.stdout.clone(),
        stderr: observation.stderr.clone(),
        stdout_truncated: observation.stdout_truncated,
        stderr_truncated: observation.stderr_truncated,
        output_complete: observation.output_complete,
        cgroup: observation.cgroup.clone(),
        leader_pid: observation.leader_pid,
    }
}

fn vector_scope() -> Scope {
    Scope {
        deployment: "dep_vector".to_owned(),
        subject: "local:1000".to_owned(),
    }
}

fn admit_exec(store: &Store, proposed: &StoredExec) -> NewOperation {
    let mut provisional = proposed.clone();
    provisional.resource.state = ExecState::Accepted;
    provisional.resource.exit = None;
    provisional.stdout.clear();
    provisional.stderr.clear();
    provisional.stdout_truncated = false;
    provisional.stderr_truncated = false;
    provisional.output_complete = false;
    let operation = NewOperation {
        scope: vector_scope(),
        operation: "01JCONTRACTSEEDSTART00001".to_owned(),
        operation_kind: "exec.start".to_owned(),
        request_hash: "9".repeat(64),
        accepted_at: FIXED_TIME.to_owned(),
        capability_snapshot: Some(SNAPSHOT.to_owned()),
        actor: "vector-fixture".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(proposed.resource.id.clone()),
    };
    assert_eq!(
        store
            .reserve_exec_start(&operation, &provisional, None, None)
            .expect("admit seeded exec"),
        Reservation::Accepted
    );
    operation
}

fn assert_exact_http(actual: &Value, vector: &Value) {
    assert_eq!(actual, &vector["expected"]["response"]);
}

fn refusal_request(
    method: &str,
    path: &str,
    operation: &str,
    input: &Value,
    query: &Value,
    request_id: &str,
) -> Value {
    json!({
        "action": {
            "kind": "http",
            "request": {
                "body": { "op": operation, "input": input },
                "headers": { "x-request-id": request_id },
                "method": method,
                "path": path,
                "query": query
            }
        },
        "context": { "subject": "local:1000" }
    })
}

async fn assert_bound_refusal_case(
    harness: &Harness,
    request: &Value,
    changed: &Value,
    expected_code: &str,
) {
    let before = harness.driver.dispatch_count.load(Ordering::SeqCst);
    let first = harness.execute(request).await;
    assert_eq!(first["body"]["error"]["code"], expected_code);
    let operation = request["action"]["request"]["body"]["op"]
        .as_str()
        .expect("operation id");
    assert_eq!(
        harness
            .store
            .operation(&vector_scope(), operation)
            .expect("operation lookup")
            .expect("refused operation")
            .state,
        substrate_wire::OperationState::Refused
    );
    assert_eq!(harness.execute(request).await, first);
    let conflict = harness.execute(changed).await;
    assert_eq!(conflict["status"], 409);
    assert_eq!(
        conflict["body"]["error"]["code"],
        "operation.request-conflict"
    );
    assert_eq!(
        harness.driver.dispatch_count.load(Ordering::SeqCst),
        before,
        "bound refusal must not dispatch"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn disputed_review_vectors_execute_at_declared_bundle_versions() {
    let depth = bundle_vector("0.1.0", "http", "file-delete-depth");
    let harness = Harness::open(false);
    assert_exact_http(&harness.execute(&depth).await, &depth);

    let cross_subject = bundle_vector("0.1.0", "http", "cross-subject-not-found");
    let other = NewOperation {
        scope: Scope {
            deployment: "dep_vector".to_owned(),
            subject: "local:1001".to_owned(),
        },
        operation: "01JPHASE2OTHERSUBJECT01".to_owned(),
        operation_kind: "workspace.create".to_owned(),
        request_hash: "3".repeat(64),
        accepted_at: FIXED_TIME.to_owned(),
        capability_snapshot: None,
        actor: "other".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: None,
    };
    harness.store.reserve(&other).expect("seed other subject");
    assert_exact_http(&harness.execute(&cross_subject).await, &cross_subject);

    let body_limit = bundle_vector("0.2.0", "http", "input-body-limit");
    assert_exact_http(&harness.execute(&body_limit).await, &body_limit);

    let machinery = bundle_vector("0.2.0", "http", "machinery-failure");
    let harness = Harness::open(true);
    seed_workspace(&harness.store);
    assert_exact_http(&harness.execute(&machinery).await, &machinery);

    let signal = bundle_vector("0.1.0", "http", "exec-signal");
    let harness = Harness::open(false);
    seed_workspace(&harness.store);
    seed_running_exec(&harness.store);
    assert_exact_http(&harness.execute(&signal).await, &signal);

    let crash = bundle_vector("0.2.0", "driver", "crash-before-dispatch");
    let operation = NewOperation {
        scope: Scope {
            deployment: "dep_vector".to_owned(),
            subject: "local:1000".to_owned(),
        },
        operation: crash["action"]["command"]["operation_id"]
            .as_str()
            .expect("operation id")
            .to_owned(),
        operation_kind: "workspace.file.write".to_owned(),
        request_hash: "4".repeat(64),
        accepted_at: FIXED_TIME.to_owned(),
        capability_snapshot: None,
        actor: "vector-client".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some("ws_vector".to_owned()),
    };
    harness.store.reserve(&operation).expect("accepted commit");
    harness
        .store
        .reconcile_after_restart(
            "dep_vector",
            "2026-08-13T12:00:01Z".parse().expect("restart cutoff"),
            "2026-08-13T12:00:01Z".parse().expect("restart time"),
            64,
        )
        .expect("daemon restart reconciliation");
    let state = harness
        .store
        .operation(&operation.scope, &operation.operation)
        .expect("operation lookup")
        .expect("operation")
        .state;
    let actual = json!({ "operation_state_after_restart": state });
    assert_eq!(actual, crash["expected"]["outcome"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn signal_not_found_race_completes_from_the_durable_terminal_observation() {
    let signal = bundle_vector("0.1.0", "http", "exec-signal");
    let harness = Harness::open_with_driver(false, true, false);
    seed_workspace(&harness.store);
    seed_running_exec(&harness.store);
    assert_exact_http(&harness.execute(&signal).await, &signal);
    assert_eq!(
        harness
            .store
            .operation(
                &Scope {
                    deployment: "dep_vector".to_owned(),
                    subject: "local:1000".to_owned(),
                },
                "01JPHASE2EXECSIGNAL00001",
            )
            .expect("operation lookup")
            .expect("signal operation")
            .state,
        substrate_wire::OperationState::Terminal
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn destroying_workspace_refuses_exec_after_restart_and_replays_stably() {
    let start = bundle_vector("0.2.0", "http", "exec-start");
    let harness = Harness::open(false);
    seed_workspace_state(&harness.store, WorkspaceState::Destroying);

    let first = harness.execute(&start).await;
    assert_eq!(first["status"], 409);
    assert_eq!(first["body"]["error"]["code"], "workspace.not-ready");
    assert_eq!(harness.driver.start_count.load(Ordering::SeqCst), 0);
    let replay = harness.execute(&start).await;
    assert_eq!(replay, first);

    let mut changed = start;
    changed["action"]["request"]["body"]["input"]["argv"] = json!(["/usr/bin/false"]);
    let conflict = harness.execute(&changed).await;
    assert_eq!(conflict["status"], 409);
    assert_eq!(
        conflict["body"]["error"]["code"],
        "operation.request-conflict"
    );
    assert_eq!(harness.driver.start_count.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_after_destroying_resumes_cleanup_and_terminalizes_original_operation() {
    let harness = Harness::open(false);
    seed_workspace(&harness.store);
    let scope = Scope {
        deployment: "dep_vector".to_owned(),
        subject: "local:1000".to_owned(),
    };
    let destroy = NewOperation {
        scope: scope.clone(),
        operation: "01JPHASE3CRASHDESTROY01".to_owned(),
        operation_kind: "workspace.destroy".to_owned(),
        request_hash: "e".repeat(64),
        accepted_at: FIXED_TIME.to_owned(),
        capability_snapshot: None,
        actor: "vector-client".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some("ws_vector".to_owned()),
    };
    assert!(matches!(
        harness
            .store
            .reserve_workspace_destroy(&destroy, None)
            .expect("reserve destroy"),
        WorkspaceDestroyReservation::Admitted { .. }
    ));
    harness
        .store
        .reconcile_after_restart(
            "dep_vector",
            "2026-08-13T12:00:01Z".parse().expect("restart cutoff"),
            "2026-08-13T12:00:01Z".parse().expect("restart time"),
            64,
        )
        .expect("restart reconciliation");

    harness.app.reconcile_destroying_workspaces().await;
    assert!(
        harness
            .store
            .workspace(&scope, "ws_vector")
            .expect("workspace lookup")
            .is_none()
    );
    let record = harness
        .store
        .operation(&scope, &destroy.operation)
        .expect("operation lookup")
        .expect("destroy operation");
    assert_eq!(record.state, substrate_wire::OperationState::Terminal);
    assert!(matches!(
        record.outcome,
        Some(substrate_wire::OperationOutcome::Success { .. })
    ));
    assert_eq!(harness.driver.destroy_count.load(Ordering::SeqCst), 1);

    let mut start = bundle_vector("0.2.0", "http", "exec-start");
    start["action"]["request"]["body"]["op"] = json!("01JPHASE3STARTAFTERGC001");
    let missing = harness.execute(&start).await;
    assert_eq!(missing["status"], 404);
    assert_eq!(harness.driver.start_count.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn bounded_destroy_pending_progress_remains_durable_until_absence_is_proved() {
    let harness = Harness::open(false);
    seed_workspace(&harness.store);
    harness
        .driver
        .destroy_pending_remaining
        .store(1, Ordering::SeqCst);
    let request = json!({
        "action": {
            "kind": "http",
            "request": {
                "body": { "op": "01JPHASE3BOUNDEDDESTROY1", "input": {} },
                "headers": { "x-request-id": "req_bounded_destroy" },
                "method": "DELETE",
                "path": "/v1/workspaces/ws_vector",
                "query": {}
            }
        },
        "context": { "subject": "local:1000" }
    });
    let response = harness.execute(&request).await;
    assert_eq!(response["status"], 500);
    assert_eq!(
        response["body"]["error"]["code"],
        "operation.outcome-unknown"
    );
    let scope = vector_scope();
    assert_eq!(
        harness
            .store
            .workspace(&scope, "ws_vector")
            .expect("workspace lookup")
            .expect("destroying workspace")
            .1
            .state,
        WorkspaceState::Destroying
    );
    assert_eq!(
        harness
            .store
            .operation(&scope, "01JPHASE3BOUNDEDDESTROY1")
            .expect("operation lookup")
            .expect("destroy operation")
            .state,
        substrate_wire::OperationState::Accepted
    );
    assert_eq!(harness.driver.destroy_count.load(Ordering::SeqCst), 1);

    let after = "2026-08-13T12:00:01Z".parse().expect("restart time");
    harness
        .store
        .reconcile_after_restart("dep_vector", after, after, 64)
        .expect("restart reconciliation");
    assert_eq!(
        harness
            .store
            .operation(&scope, "01JPHASE3BOUNDEDDESTROY1")
            .expect("operation lookup")
            .expect("destroy operation")
            .state,
        substrate_wire::OperationState::Unknown
    );

    harness.app.reconcile_destroying_workspaces().await;
    assert!(
        harness
            .store
            .workspace(&scope, "ws_vector")
            .expect("workspace lookup")
            .is_none()
    );
    assert_eq!(
        harness
            .store
            .operation(&scope, "01JPHASE3BOUNDEDDESTROY1")
            .expect("operation lookup")
            .expect("destroy operation")
            .state,
        substrate_wire::OperationState::Terminal
    );
    assert_eq!(harness.driver.destroy_count.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_lock_serializes_start_against_destroy_without_dispatch_race() {
    let harness = Arc::new(Harness::open_with_driver(false, false, true));
    seed_workspace(&harness.store);
    let start = bundle_vector("0.2.0", "http", "exec-start");
    let start_harness = Arc::clone(&harness);
    let start_task = tokio::spawn(async move { start_harness.execute(&start).await });
    harness
        .driver
        .start_entered
        .as_ref()
        .expect("start barrier")
        .notified()
        .await;

    let destroy = json!({
        "action": {
            "kind": "http",
            "request": {
                "body": { "op": "01JPHASE3DESTROYRACE001", "input": {} },
                "headers": { "x-request-id": "req_destroy_race" },
                "method": "DELETE",
                "path": "/v1/workspaces/ws_vector",
                "query": {}
            }
        },
        "context": { "subject": "local:1000" }
    });
    let destroy_harness = Arc::clone(&harness);
    let destroy_task = tokio::spawn(async move { destroy_harness.execute(&destroy).await });
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert_eq!(harness.driver.destroy_count.load(Ordering::SeqCst), 0);
    assert!(!destroy_task.is_finished());

    harness
        .driver
        .start_release
        .as_ref()
        .expect("release barrier")
        .notify_one();
    let started = start_task.await.expect("start task");
    assert_eq!(started["status"], 202);
    let destroyed = destroy_task.await.expect("destroy task");
    assert_eq!(destroyed["status"], 409);
    assert_eq!(destroyed["body"]["error"]["code"], "workspace.execs-active");
    assert_eq!(harness.driver.destroy_count.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn typed_dispatch_posture_preserves_or_removes_provisional_membership_exactly() {
    let scope = Scope {
        deployment: "dep_vector".to_owned(),
        subject: "local:1000".to_owned(),
    };
    let start = bundle_vector("0.2.0", "http", "exec-start");
    let unknown =
        Harness::open_with_outcomes(false, false, false, None, Some(DriverErrorClass::Failed));
    seed_workspace(&unknown.store);
    let response = unknown.execute(&start).await;
    assert_eq!(response["status"], 500);
    assert_eq!(
        response["body"]["error"]["code"],
        "operation.outcome-unknown"
    );
    assert_eq!(
        unknown
            .store
            .exec(&scope, "ex_vector")
            .expect("exec lookup")
            .expect("unknown membership")
            .resource
            .state,
        ExecState::Unknown
    );
    assert_eq!(
        unknown
            .store
            .operation(&scope, "01JPHASE2EXECSTART000001")
            .expect("operation lookup")
            .expect("operation")
            .state,
        substrate_wire::OperationState::Unknown
    );
    let destroy = NewOperation {
        scope: scope.clone(),
        operation: "01JUNKNOWNMEMBERDESTROY1".to_owned(),
        operation_kind: "workspace.destroy".to_owned(),
        request_hash: "9".repeat(64),
        accepted_at: FIXED_TIME.to_owned(),
        capability_snapshot: Some(SNAPSHOT.to_owned()),
        actor: "vector-client".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some("ws_vector".to_owned()),
    };
    let WorkspaceDestroyReservation::Refused { answer, .. } = unknown
        .store
        .reserve_workspace_destroy(&destroy, None)
        .expect("destroy admission")
    else {
        panic!("unknown exec membership must durably refuse destroy");
    };
    let substrate_wire::OperationOutcome::Error { error } = answer.outcome else {
        panic!("destroy refusal must persist an error outcome");
    };
    assert_eq!(error.code, "workspace.execs-active");
    assert_eq!(unknown.driver.destroy_count.load(Ordering::SeqCst), 0);

    let absent =
        Harness::open_with_outcomes(false, false, false, None, Some(DriverErrorClass::NotFound));
    seed_workspace(&absent.store);
    let response = absent.execute(&start).await;
    assert_eq!(response["status"], 404);
    assert!(
        absent
            .store
            .exec(&scope, "ex_vector")
            .expect("exec lookup")
            .is_none()
    );
    assert_eq!(absent.execute(&start).await, response);

    let create = bundle_vector("0.1.0", "http", "workspace-create");
    let unknown_workspace =
        Harness::open_with_outcomes(false, false, false, Some(DriverErrorClass::Failed), None);
    let response = unknown_workspace.execute(&create).await;
    assert_eq!(response["status"], 500);
    let (root_name, workspace) = unknown_workspace
        .store
        .workspace(&scope, "ws_vector")
        .expect("workspace lookup")
        .expect("provisional workspace");
    assert_eq!(root_name, "root-ws_vector");
    assert_eq!(workspace.state, WorkspaceState::Unknown);

    for outcome in [None, Some(DriverErrorClass::NotFound)] {
        let absent_workspace = Harness::open_with_outcomes(false, false, false, outcome, None);
        let response = absent_workspace.execute(&create).await;
        assert_eq!(response["status"], 404);
        assert!(
            absent_workspace
                .store
                .workspace(&scope, "ws_vector")
                .expect("workspace lookup")
                .is_none()
        );
        assert_eq!(absent_workspace.execute(&create).await, response);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn same_process_workspace_outcome_unknown_is_periodically_resolved() {
    let create = bundle_vector("0.1.0", "http", "workspace-create");
    let harness =
        Harness::open_with_outcomes(false, false, false, Some(DriverErrorClass::Failed), None);
    let response = harness.execute(&create).await;
    assert_eq!(
        response["body"]["error"]["code"],
        "operation.outcome-unknown"
    );

    harness.app.sweep_expired().await;

    let scope = vector_scope();
    let (root_name, workspace) = harness
        .store
        .workspace(&scope, "ws_vector")
        .expect("workspace lookup")
        .expect("recovered workspace");
    assert_eq!(root_name, "root-ws_vector");
    assert_eq!(workspace.state, WorkspaceState::Ready);
    assert_eq!(
        harness
            .store
            .operation(&scope, "01JPHASE2WORKSPACECREATE")
            .expect("operation lookup")
            .expect("workspace create operation")
            .state,
        substrate_wire::OperationState::Terminal
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn observed_workspace_survives_terminal_commit_failure_and_recovers_without_redispatch() {
    let create = bundle_vector("0.1.0", "http", "workspace-create");
    let harness = Harness::open(false);
    harness
        .driver
        .workspace_create_observed
        .store(true, Ordering::SeqCst);
    harness.fail_terminal_commit_after_observation();

    let response = harness.execute(&create).await;
    assert_eq!(response["status"], 500);
    assert_eq!(response["body"]["error"]["code"], "state.store-failed");
    let scope = vector_scope();
    assert_eq!(
        harness
            .store
            .workspace(&scope, "ws_vector")
            .expect("workspace lookup")
            .expect("provisional workspace")
            .1
            .state,
        WorkspaceState::Unknown
    );
    assert_eq!(
        harness
            .store
            .operation(&scope, "01JPHASE2WORKSPACECREATE")
            .expect("operation lookup")
            .expect("workspace create operation")
            .state,
        substrate_wire::OperationState::Accepted
    );
    assert_eq!(harness.driver.dispatch_count.load(Ordering::SeqCst), 1);

    harness.restore_terminal_commits();
    let after = "2026-08-13T12:00:01Z".parse().expect("restart time");
    harness
        .store
        .reconcile_after_restart("dep_vector", after, after, 64)
        .expect("restart reconciliation");
    harness.app.sweep_expired().await;

    assert_eq!(
        harness
            .store
            .workspace(&scope, "ws_vector")
            .expect("workspace lookup")
            .expect("recovered workspace")
            .1
            .state,
        WorkspaceState::Ready
    );
    assert_eq!(
        harness
            .store
            .operation(&scope, "01JPHASE2WORKSPACECREATE")
            .expect("operation lookup")
            .expect("workspace create operation")
            .state,
        substrate_wire::OperationState::Terminal
    );
    assert_eq!(harness.driver.dispatch_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn same_process_exec_outcome_unknown_is_periodically_completed() {
    let start = bundle_vector("0.2.0", "http", "exec-start");
    let harness =
        Harness::open_with_outcomes(false, false, false, None, Some(DriverErrorClass::Failed));
    seed_workspace(&harness.store);
    let response = harness.execute(&start).await;
    assert_eq!(
        response["body"]["error"]["code"],
        "operation.outcome-unknown"
    );
    let mut observed = VectorDriver::signal_observation();
    observed.resource.state = ExecState::Running;
    observed.resource.exit = None;
    observed.output_complete = false;
    *harness
        .driver
        .observed_exec
        .lock()
        .expect("observed exec lock") = Some(observed);

    harness.app.sweep_expired().await;

    let scope = vector_scope();
    assert_eq!(
        harness
            .store
            .exec(&scope, "ex_vector")
            .expect("exec lookup")
            .expect("recovered exec")
            .resource
            .state,
        ExecState::Running
    );
    assert_eq!(
        harness
            .store
            .operation(&scope, "01JPHASE2EXECSTART000001")
            .expect("operation lookup")
            .expect("exec start operation")
            .state,
        substrate_wire::OperationState::Terminal
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn observed_exec_survives_terminal_commit_failure_and_recovers_without_redispatch() {
    let start = bundle_vector("0.2.0", "http", "exec-start");
    let harness = Harness::open(false);
    seed_workspace(&harness.store);
    harness.fail_terminal_commit_after_observation();

    let response = harness.execute(&start).await;
    assert_eq!(response["status"], 500);
    assert_eq!(response["body"]["error"]["code"], "state.store-failed");
    let scope = vector_scope();
    assert_eq!(
        harness
            .store
            .exec(&scope, "ex_vector")
            .expect("exec lookup")
            .expect("provisional exec")
            .resource
            .state,
        ExecState::Accepted
    );
    assert_eq!(
        harness
            .store
            .operation(&scope, "01JPHASE2EXECSTART000001")
            .expect("operation lookup")
            .expect("exec start operation")
            .state,
        substrate_wire::OperationState::Accepted
    );
    assert_eq!(harness.driver.start_count.load(Ordering::SeqCst), 1);

    harness.restore_terminal_commits();
    let after = "2026-08-13T12:00:01Z".parse().expect("restart time");
    harness
        .store
        .reconcile_after_restart("dep_vector", after, after, 64)
        .expect("restart reconciliation");
    harness.app.sweep_expired().await;

    assert_eq!(
        harness
            .store
            .exec(&scope, "ex_vector")
            .expect("exec lookup")
            .expect("recovered exec")
            .resource
            .state,
        ExecState::Running
    );
    assert_eq!(
        harness
            .store
            .operation(&scope, "01JPHASE2EXECSTART000001")
            .expect("operation lookup")
            .expect("exec start operation")
            .state,
        substrate_wire::OperationState::Terminal
    );
    assert_eq!(harness.driver.start_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn superseded_driver_observation_never_serves_losing_output_bytes() {
    let harness = Harness::open(false);
    seed_workspace(&harness.store);
    let mut durable = VectorDriver::signal_observation();
    durable.resource.state = ExecState::Exited;
    durable.resource.exit = Some(substrate_wire::ExecExit {
        code: Some(0),
        signal: None,
    });
    durable.stdout = b"durable winner".to_vec();
    durable.output_complete = true;
    store_exec(&harness.store, &durable);

    let mut losing = durable.clone();
    losing.resource.state = ExecState::Cancelled;
    losing.resource.exit = Some(substrate_wire::ExecExit {
        code: None,
        signal: Some(substrate_wire::Signal::Kill),
    });
    losing.stdout = b"losing driver bytes".to_vec();
    *harness
        .driver
        .observed_exec
        .lock()
        .expect("observed exec lock") = Some(losing);

    let request = json!({
        "action": {
            "kind": "http",
            "request": {
                "headers": { "x-request-id": "req_superseded_output" },
                "method": "GET",
                "path": "/v1/execs/ex_vector/output",
                "query": { "stream": "stdout", "offset": 0, "limit_bytes": 1024 }
            }
        },
        "context": { "subject": "local:1000" }
    });
    let response = harness.execute(&request).await;
    assert_eq!(response["status"], 200);
    assert_eq!(
        response["body"]["result"]["content"]["data"],
        "ZHVyYWJsZSB3aW5uZXI="
    );
    assert_eq!(harness.driver.output_count.load(Ordering::SeqCst), 0);
}

#[test]
fn bundle_paths_are_repository_relative() {
    assert!(Path::new("contracts/substrate-wire/0.1.0").is_relative());
}
