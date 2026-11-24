//! The result bound of ADR 0053 on the host-process routes, behind the port: a start whose widest
//! record the ledger cannot record is refused `operation.result-limit` before anything starts, a
//! start at the bound and every later mutation of it complete, the daemon's own completions of
//! it complete, and a stop is never refused for this cause.

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use chrono::Utc;
use serde_json::{Value, json};
use substrate_host::{
    DispatchOutcome, Driver, DriverError, ExecObservation, HostConfig, HostDriver,
    WorkspaceDestroyProgress,
};
use substrate_store::{
    HOST_PROCESS_RENEW_KIND, HOST_PROCESS_START_KIND, HOST_PROCESS_STOP_KIND,
    HostProcessLeaseRequest, HostProcessLimits, HostProcessStartReservation, NewHostProcess,
    NewOperation, Scope, Store, StoreConfig,
};
use substrate_wire::{
    CapabilitySnapshot, ExecOutputQuery, ExecSignalInput, ExecStartInput, FileAbsence,
    FileObservation, FileReadQuery, FileReadResult, LeaseObservation, OutputSlice, ProcessOwner,
    ProcessRecord, Workspace, WorkspaceAbsence, WorkspaceCreateInput,
};

use super::{FakeSupervisor, Harness, MovableAuthority, SUBJECT, input, text, with};

/// The bytes a terminal success adds around its result (`StoreConfig::success_result_json_bytes`).
const AROUND_A_RESULT: u64 = 35 + 8 + 28;

/// The host driver, whose snapshot states `result_bytes` as `operation.result-max-json-bytes`:
/// a daemon is constructed only over a snapshot that states its ledger's reserve (ADR 0053
/// decision 4, amendment of 2026-11-24), and these cases lower that reserve.
struct StatingDriver {
    host: Arc<HostDriver>,
    result_bytes: u64,
}

#[async_trait]
impl Driver for StatingDriver {
    fn machine(&self) -> CapabilitySnapshot {
        let mut machine = self.host.machine();
        machine.facts.operation_result_max_json_bytes = self.result_bytes;
        machine
    }

    async fn shutdown(&self) -> Result<(), DriverError> {
        self.host.shutdown().await
    }

    fn workspace_root_identity(&self, id: &str) -> Result<String, DriverError> {
        self.host.workspace_root_identity(id)
    }

    async fn create_workspace(
        &self,
        id: &str,
        root_name: &str,
        input: &WorkspaceCreateInput,
    ) -> DispatchOutcome<Workspace> {
        self.host.create_workspace(id, root_name, input).await
    }

    async fn observe_workspace(
        &self,
        id: &str,
        root_name: &str,
        previous: &Workspace,
    ) -> Result<Workspace, DriverError> {
        self.host.observe_workspace(id, root_name, previous).await
    }

    async fn read_workspace_path(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        query: &FileReadQuery,
    ) -> Result<substrate_host::Listed<FileReadResult>, DriverError> {
        self.host
            .read_workspace_path(workspace_id, root_name, path, query)
            .await
    }

    async fn write_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
        content: &[u8],
    ) -> Result<FileObservation, DriverError> {
        self.host
            .write_workspace_file(workspace_id, root_name, path, content)
            .await
    }

    async fn delete_workspace_file(
        &self,
        workspace_id: &str,
        root_name: &str,
        path: &str,
    ) -> Result<FileAbsence, DriverError> {
        self.host
            .delete_workspace_file(workspace_id, root_name, path)
            .await
    }

    async fn destroy_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<WorkspaceDestroyProgress, DriverError> {
        self.host.destroy_workspace(workspace_id, root_name).await
    }

    async fn forget_workspace(
        &self,
        workspace_id: &str,
        root_name: &str,
    ) -> Result<WorkspaceAbsence, DriverError> {
        self.host.forget_workspace(workspace_id, root_name).await
    }

    async fn start_exec(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.host.start_exec(id, workspace_root_name, input).await
    }

    async fn observe_exec(&self, id: &str) -> Result<ExecObservation, DriverError> {
        self.host.observe_exec(id).await
    }

    async fn output(&self, id: &str, query: &ExecOutputQuery) -> Result<OutputSlice, DriverError> {
        self.host.output(id, query).await
    }

    async fn signal(
        &self,
        id: &str,
        input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        self.host.signal(id, input).await
    }

    fn completed_execs(&self) -> Vec<ExecObservation> {
        self.host.completed_execs()
    }

    fn set_exec_lease(&self, id: &str, lease: Option<LeaseObservation>) {
        self.host.set_exec_lease(id, lease);
    }

    fn acknowledge_exec(&self, persisted: &ExecObservation) {
        self.host.acknowledge_exec(persisted);
    }

    fn discard_superseded_exec(&self, id: &str) {
        self.host.discard_superseded_exec(id);
    }
}

impl Harness {
    /// A daemon whose host-process driver is [`FakeSupervisor`] and whose ledger reserves what
    /// `result_bytes` answers for the harness's directory, before anything is written.
    fn faked_reserving(
        result_bytes: impl FnOnce(&tempfile::TempDir) -> u64,
    ) -> (Self, Arc<FakeSupervisor>) {
        let directory = tempfile::tempdir().expect("temporary harness");
        let result_bytes = result_bytes(&directory);
        let store = Arc::new(
            Store::open_with_config(
                directory.path().join("state.db"),
                StoreConfig {
                    operation_terminal_headroom_bytes: result_bytes + AROUND_A_RESULT,
                    ..StoreConfig::default()
                },
            )
            .expect("state store"),
        );
        assert_eq!(store.success_result_json_bytes(), result_bytes);
        let authority = Arc::new(MovableAuthority::default());
        let workspaces = directory.path().join("workspaces");
        std::fs::create_dir_all(&workspaces).expect("workspace root");
        let driver = Arc::new(StatingDriver {
            host: HostDriver::open(HostConfig::minimum(workspaces)).expect("host driver"),
            result_bytes,
        });
        let mut app = Self::app_over(driver, &directory, &store, &authority, None);
        let fake = Arc::new(FakeSupervisor::default());
        Arc::get_mut(&mut app)
            .expect("unshared application")
            .host_processes = Some(Arc::clone(&fake) as Arc<dyn substrate_host::ProcessSupervisor>);
        (
            Self {
                directory,
                store,
                authority,
                app,
                cgroup: None,
            },
            fake,
        )
    }
}

/// The working directory `Harness::workdir` makes for `name`, made before the harness exists.
fn workdir(directory: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
    let path = directory.path().join("work").join(name);
    std::fs::create_dir_all(&path).expect("caller working directory");
    path.canonicalize().expect("canonical working directory")
}

/// The widest record a start of `input` can lead to, in bytes of compact JSON.
fn record_width(input: &Value) -> u64 {
    let strings = |member: &str| -> Vec<String> {
        input[member]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| text(item).to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let env_names: Vec<String> = input["env"]
        .as_object()
        .expect("env")
        .keys()
        .cloned()
        .collect();
    let owner = ProcessOwner {
        name: text(&input["owner"]["name"]).to_owned(),
        session: text(&input["owner"]["session"]).to_owned(),
    };
    u64::try_from(ProcessRecord::widest_json_bytes(
        &strings("argv"),
        text(&input["cwd"]),
        &env_names,
        &owner,
        text(&input["reason"]),
    ))
    .expect("u64")
}

fn bound_input(cwd: &std::path::Path, ttl_ms: u64) -> Value {
    let mut start = input(&["/bin/true"], cwd, "s-bound", "preview");
    start["lease"] = json!({ "ttl_ms": ttl_ms });
    start
}

/// Each operation of `kind` is terminal and its outcome a success.
fn assert_every_row_completed(harness: &Harness, kind: &str, expected: usize) {
    let rows = harness.operations(kind);
    assert_eq!(rows.len(), expected, "{kind}: {rows:?}");
    for (_, state, _) in &rows {
        assert_eq!(state, "terminal", "{kind}: {rows:?}");
    }
    let connection =
        rusqlite::Connection::open(harness.directory.path().join("state.db")).expect("ledger");
    let unrecorded: i64 = connection
        .query_row(
            "SELECT count(*) FROM operations WHERE operation_kind = ?1
             AND outcome_json NOT LIKE '{\"kind\":\"success\"%'",
            [kind],
            |row| row.get(0),
        )
        .expect("ledger outcomes");
    assert_eq!(unrecorded, 0, "{kind}: every outcome is a success");
}

fn host_process_rows(harness: &Harness) -> i64 {
    let connection =
        rusqlite::Connection::open(harness.directory.path().join("state.db")).expect("ledger");
    connection
        .query_row("SELECT count(*) FROM host_processes", [], |row| row.get(0))
        .expect("record count")
}

/// A start whose widest record is one byte past the reserve is refused `operation.result-limit`
/// before anything is started: no record, no lease, nothing dispatched, and a retry replays it.
#[tokio::test(flavor = "multi_thread")]
async fn a_process_start_no_result_can_fit_is_refused_before_anything_is_started() {
    let (harness, fake) = Harness::faked_reserving(|directory| {
        record_width(&bound_input(&workdir(directory, "over"), 60_000)) - 1
    });
    let start = bound_input(&harness.workdir("over"), 60_000);
    let refused = harness
        .start("op_result_limit_start01", start.clone())
        .await;
    assert_eq!(refused.0, StatusCode::TOO_MANY_REQUESTS, "{}", refused.1);
    assert_eq!(refused.1["error"]["code"], "operation.result-limit");
    assert_eq!(refused.1["error"]["class"], "exhausted");
    assert_eq!(refused.1["error"]["retriable"], false);
    assert_eq!(refused.1["error"]["address"], "result");
    assert!(fake.started.lock().is_empty(), "nothing was started");
    assert_eq!(host_process_rows(&harness), 0, "no record exists");
    let rows = harness.operations(HOST_PROCESS_START_KIND);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "refused");
    let replayed = harness.start("op_result_limit_start01", start).await;
    assert_eq!(replayed.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(replayed.1["error"], refused.1["error"]);
    assert!(fake.started.lock().is_empty());
}

/// Two processes whose widest record is exactly the reserve start; one is renewed and stopped,
/// the other stopped by its owner, and after the supervisor clock collects their ends every
/// start, renewal and stop row is terminal with its success result.
#[tokio::test(flavor = "multi_thread")]
async fn a_process_started_at_the_bound_is_stopped_and_renewed_and_every_row_completes() {
    let (harness, fake) = Harness::faked_reserving(|directory| {
        record_width(&bound_input(&workdir(directory, "bound"), 60_000))
    });
    let start = bound_input(&harness.workdir("bound"), 60_000);
    let first = harness
        .started("op_result_bound_start01", start.clone())
        .await;
    // A name of the same length, so the second record is exactly as wide as the first.
    let second = harness
        .started(
            "op_result_bound_start02",
            with(
                start,
                "owner",
                json!({ "session": "s-bound", "name": "weivery" }),
            ),
        )
        .await;

    let renew = json!({ "ttl_ms": 120_000 });
    let renewed = harness
        .renew("op_result_bound_renew01", &first, renew.clone())
        .await;
    assert_eq!(renewed.0, StatusCode::OK, "{}", renewed.1);
    let replayed = harness
        .renew("op_result_bound_renew01", &first, renew)
        .await;
    assert_eq!(replayed.1["result"], renewed.1["result"]);

    let stopped = harness.stop("op_result_bound_stop0001", &first).await;
    assert_eq!(stopped.0, StatusCode::OK, "{}", stopped.1);
    let replayed = harness.stop("op_result_bound_stop0001", &first).await;
    assert_eq!(replayed.1["result"], stopped.1["result"]);

    let (status, by_owner) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/processes/stop",
            Some(json!({ "owner_session": "s-bound" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{by_owner}");
    assert_eq!(by_owner["result"]["stopped"][0]["process"], second.as_str());
    assert_eq!(fake.stops().len(), 2);

    harness.ended(&first).await;
    harness.ended(&second).await;
    assert_every_row_completed(&harness, HOST_PROCESS_START_KIND, 2);
    assert_every_row_completed(&harness, HOST_PROCESS_RENEW_KIND, 1);
    assert_every_row_completed(&harness, HOST_PROCESS_STOP_KIND, 2);
}

/// A process whose widest record is exactly the reserve, left to its lease: the supervisor clock
/// stops it at expiry under its own operation, collects the end the driver reports (an exit by
/// `SIGTERM`), and every row it opened ends terminal with its success result.
#[tokio::test(flavor = "multi_thread")]
async fn a_process_started_at_the_bound_that_exits_or_expires_completes_its_open_rows() {
    let (harness, fake) = Harness::faked_reserving(|directory| {
        record_width(&bound_input(&workdir(directory, "expiry"), 1_000))
    });
    let start = bound_input(&harness.workdir("expiry"), 1_000);
    let id = harness.started("op_result_expiry_start1", start).await;
    harness.authority.advance(5_000);
    let ended = harness.ended(&id).await;
    assert_eq!(ended["ended_by"], "lease-expiry", "{ended}");
    assert_eq!(fake.stops().len(), 1);
    assert_every_row_completed(&harness, HOST_PROCESS_START_KIND, 1);
    assert_every_row_completed(&harness, HOST_PROCESS_STOP_KIND, 1);
}

/// A record the ledger cannot record, which no start decided under this rule made (it is
/// written to the store directly), is still stopped: the stop is accepted and begun, never
/// answered `operation.result-limit`, and its row ends terminal with
/// `operation.result-unrecorded` (ADR 0053 decisions 8 and 9).
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_is_never_refused_for_the_result_bound() {
    let (harness, fake) = Harness::faked_reserving(|_| 20_000);
    let id = format!("proc_{}", "0".repeat(26));
    let scope = Scope {
        deployment: "dep_processes".to_owned(),
        subject: SUBJECT.to_owned(),
    };
    let new = NewOperation {
        scope: scope.clone(),
        operation: "op_result_never_start1".to_owned(),
        operation_kind: HOST_PROCESS_START_KIND.to_owned(),
        request_hash: "e".repeat(64),
        accepted_at: harness.authority_now(),
        capability_snapshot: None,
        actor: "process-test".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(id.clone()),
    };
    let process = NewHostProcess {
        id: id.clone(),
        owner_session: "s-never".to_owned(),
        owner_name: "wide".to_owned(),
        reason: "a record wider than the ledger records".to_owned(),
        argv: vec!["/bin/true".to_owned(), "a".repeat(30_000)],
        cwd: harness.workdir("never").to_str().expect("utf-8").to_owned(),
        env_names: Vec::new(),
        env_digest: format!("sha256:{}", "0".repeat(64)),
        lease: HostProcessLeaseRequest::Ttl { ttl_ms: 60_000 },
        timeout_ms: None,
        expires_at: None,
    };
    assert!(matches!(
        harness
            .store
            .reserve_host_process_start(&new, &process, &HostProcessLimits::default())
            .expect("reserve"),
        HostProcessStartReservation::Accepted(_)
    ));
    harness
        .store
        .mark_host_process_running(&scope, &id, Utc::now())
        .expect("running");

    let stopped = harness.stop("op_result_never_stop01", &id).await;
    assert_ne!(
        stopped.1["error"]["code"], "operation.result-limit",
        "{}",
        stopped.1
    );
    assert_eq!(
        stopped.0,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{}",
        stopped.1
    );
    assert_eq!(stopped.1["error"]["code"], "operation.result-unrecorded");
    assert_eq!(fake.stops().len(), 1, "the stop was begun");
    let rows = harness.operations(HOST_PROCESS_STOP_KIND);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "terminal");
    assert_eq!(rows[0].2.as_deref(), Some(id.as_str()));
    let replayed = harness.stop("op_result_never_stop01", &id).await;
    assert_eq!(replayed.1["error"], stopped.1["error"]);
    assert_eq!(fake.stops().len(), 1, "a replay stops nothing again");
}

impl Harness {
    fn authority_now(&self) -> String {
        use substrate_daemon::Authority as _;
        self.authority.now().to_rfc3339()
    }
}
