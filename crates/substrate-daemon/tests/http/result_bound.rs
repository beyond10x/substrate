//! The result bound of ADR 0053, for every driver: a start or a v1 file change whose widest
//! result the ledger cannot record is refused `operation.result-limit` before any driver is
//! called, a start at the bound and every later mutation of it complete, and a driver answer
//! with a member past its wire bound ends its row `operation.result-unrecorded` with nothing cut.
//!
//! The driver here publishes the facts an exec and a session start need, answers each start
//! itself without running anything, and counts every call that would have an effect. The ledger
//! reserves what each case sets, so "no result can fit" needs no large input.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use chrono::Utc;
use parking_lot::Mutex;
use serde_json::{Value, json};
use substrate_daemon::App;
use substrate_host::{
    DispatchOutcome, Driver, DriverError, ExecObservation, HostConfig, HostDriver,
    WorkspaceDestroyProgress,
};
use substrate_store::{Store, StoreConfig};
use substrate_wire::{
    CapabilitySnapshot, ErrorClass, Exec, ExecKind, ExecOutputQuery, ExecRefusal, ExecSignalInput,
    ExecStartInput, ExecState, FileAbsence, FileObservation, FileReadQuery, FileReadResult,
    HostDriverKind, LeaseObservation, OperationOutcome, OperationState, OutputSlice,
    PipeSessionStartInput, Workspace, WorkspaceAbsence, WorkspaceCreateInput,
};

use super::{Harness, ledger_scope, mutation};

const SNAPSHOT: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
/// The bytes a terminal success adds around its result: `terminal_at`, the status charge and the
/// outcome's envelope (`StoreConfig::success_result_json_bytes`).
const AROUND_A_RESULT: u64 = 35 + 8 + 28;

/// A driver that serves workspaces through the host driver, answers every start and signal
/// itself, and counts each call that would have an effect.
struct CountingDriver {
    host: Arc<HostDriver>,
    starts: AtomicUsize,
    signals: AtomicUsize,
    writes: AtomicUsize,
    deletes: AtomicUsize,
    /// The `refusal.code` each started exec states, when a case sets one.
    refusal_code: Option<String>,
    last: Mutex<Option<Exec>>,
}

impl CountingDriver {
    fn open(root: &std::path::Path, refusal_code: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            host: HostDriver::open(HostConfig::minimum(root)).expect("host driver"),
            starts: AtomicUsize::new(0),
            signals: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
            refusal_code,
            last: Mutex::new(None),
        })
    }

    fn effects(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
            + self.signals.load(Ordering::SeqCst)
            + self.writes.load(Ordering::SeqCst)
            + self.deletes.load(Ordering::SeqCst)
    }

    fn started(&self, id: &str, input: &ExecStartInput) -> DispatchOutcome<ExecObservation> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let resource = Exec {
            unrecorded_output: None,
            id: id.to_owned(),
            kind: ExecKind::Exec,
            workspace: input.workspace.clone(),
            state: ExecState::Running,
            observed_at: Utc::now(),
            requested: input.sandbox.clone(),
            applied: None,
            exit: None,
            usage: None,
            lease: None,
            refusal: self.refusal_code.as_ref().map(|code| ExecRefusal {
                class: ErrorClass::Exhausted,
                code: code.clone(),
                message: "A bound of this run was reached.".to_owned(),
            }),
        };
        *self.last.lock() = Some(resource.clone());
        DispatchOutcome::Observed(observation(resource))
    }
}

fn observation(resource: Exec) -> ExecObservation {
    ExecObservation {
        resource,
        stdout: Vec::new(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        stdout_kept: None,
        stderr_kept: None,
        output_complete: false,
        cgroup: None,
        leader_pid: None,
    }
}

#[async_trait]
impl Driver for CountingDriver {
    fn machine(&self) -> CapabilitySnapshot {
        let mut capability = self.host.machine();
        SNAPSHOT.clone_into(&mut capability.snapshot);
        capability.driver = HostDriverKind::Host;
        capability.facts.exec_namespaces = Some(substrate_wire::NamespaceFacts {
            user: true,
            mount: true,
            pid: true,
            ipc: true,
            uts: true,
            network: true,
        });
        capability.facts.exec_cgroup_limits = Some(substrate_wire::CgroupLimitFacts {
            processes: true,
            memory: true,
            cpu: true,
        });
        capability.facts.exec_cgroup_kill = Some(true);
        capability.facts.exec_no_egress = Some(true);
        capability.facts.leases_explicit = Some(true);
        capability.facts.exec_output_limit_bytes = Some(1_048_576);
        capability
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
        self.writes.fetch_add(1, Ordering::SeqCst);
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
        self.deletes.fetch_add(1, Ordering::SeqCst);
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
        _workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.started(id, input)
    }

    async fn start_pipe_session(
        &self,
        id: &str,
        _workspace_root_name: &str,
        input: &PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.started(id, &input.exec)
    }

    async fn observe_exec(&self, _id: &str) -> Result<ExecObservation, DriverError> {
        Err(DriverError::not_found())
    }

    async fn output(
        &self,
        _id: &str,
        _query: &ExecOutputQuery,
    ) -> Result<OutputSlice, DriverError> {
        Err(DriverError::not_found())
    }

    async fn signal(
        &self,
        _id: &str,
        _input: &ExecSignalInput,
    ) -> Result<ExecObservation, DriverError> {
        self.signals.fetch_add(1, Ordering::SeqCst);
        let mut resource = self
            .last
            .lock()
            .clone()
            .ok_or_else(DriverError::not_found)?;
        resource.state = ExecState::Cancelled;
        resource.observed_at = Utc::now();
        Ok(observation(resource))
    }

    fn completed_execs(&self) -> Vec<ExecObservation> {
        Vec::new()
    }

    fn set_exec_lease(&self, _id: &str, _lease: Option<LeaseObservation>) {}

    fn acknowledge_exec(&self, _persisted: &ExecObservation) {}

    fn discard_superseded_exec(&self, _id: &str) {}
}

/// A workspace id of the length the daemon mints: `ws_` and a ULID.
fn minted_workspace_id() -> String {
    format!("ws_{}", "0".repeat(26))
}

fn exec_input(workspace: &str) -> Value {
    json!({
        "workspace": workspace,
        "argv": ["/bin/true"],
        "env": { "allow": [], "set": {} },
        "limits": {
            "timeout_ms": 10_000,
            "output_bytes": 65_536,
            "processes": 8,
            "memory_bytes": 67_108_864,
            "cpu_millis": 1_000
        },
        "sandbox": {
            "require": true,
            "profile": "workspace",
            "network": "none",
            "capability_snapshot": SNAPSHOT
        },
        "wait": false,
        "lease_ttl_ms": 60_000
    })
}

fn session_input(workspace: &str) -> Value {
    json!({
        "exec": exec_input(workspace),
        "input_limit_bytes": 65_536,
        "frame_limit_bytes": 16_384,
        "queued_frames": 16,
        "mode": "pipes"
    })
}

/// The widest exec a start of `input` can lead to, in bytes of compact JSON.
fn exec_width(input: &Value) -> u64 {
    let typed: ExecStartInput = serde_json::from_value(input.clone()).expect("exec input");
    u64::try_from(Exec::widest_json_bytes(&typed)).expect("u64")
}

/// A harness over a [`CountingDriver`] whose ledger reserves `result_bytes` for a success
/// result, with one empty workspace.
async fn harness(
    result_bytes: u64,
    refusal_code: Option<String>,
) -> (Harness, Arc<CountingDriver>, String) {
    let directory = tempfile::tempdir().expect("temporary harness");
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
    let driver = CountingDriver::open(&directory.path().join("workspaces"), refusal_code);
    let app = App::new(
        Arc::clone(&store),
        Arc::clone(&driver) as Arc<dyn Driver>,
        "dep_http_test",
    );
    let harness = Harness {
        directory,
        app,
        store,
    };
    let (status, created) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/workspaces",
            "req_bound_create",
            mutation(
                "01JRESULTBOUNDCREATE001",
                json!({ "source": "empty", "labels": {} }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let workspace = created["result"]["id"]
        .as_str()
        .expect("workspace id")
        .to_owned();
    assert_eq!(workspace.len(), minted_workspace_id().len());
    (harness, driver, workspace)
}

fn rows(harness: &Harness, table: &str) -> i64 {
    let connection =
        rusqlite::Connection::open(harness.directory.path().join("state.db")).expect("ledger");
    connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("row count")
}

/// One request refused `operation.result-limit`: 429, class `exhausted`, not retriable, address
/// `result`, recorded as refused before dispatch, and the same operation id replays it.
async fn assert_result_limit(
    harness: &Harness,
    method: &Method,
    uri: &str,
    operation: &str,
    input: &Value,
) {
    let (status, refused) = harness
        .call(
            "local:1000",
            method.clone(),
            uri,
            "req_result_limit",
            mutation(operation, input.clone()),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "{operation}: {refused}"
    );
    assert_eq!(refused["error"]["code"], "operation.result-limit");
    assert_eq!(refused["error"]["class"], "exhausted");
    assert_eq!(refused["error"]["retriable"], false);
    assert_eq!(refused["error"]["address"], "result");
    assert_eq!(
        refused["error"]["message"],
        "The result of this operation exceeds what the operation ledger can record."
    );
    let record = harness
        .store
        .operation(&ledger_scope(), operation)
        .expect("ledger read")
        .expect("ledger row");
    assert_eq!(record.state, OperationState::Refused, "{operation}");
    let (status, replayed) = harness
        .call(
            "local:1000",
            method.clone(),
            uri,
            "req_result_limit",
            mutation(operation, input.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{replayed}");
    assert_eq!(replayed["error"], refused["error"], "{operation}: replay");
}

/// An exec start whose widest result is one byte past the reserve is refused before any driver
/// is called: no exec row and no lease exist, and a retry replays the refusal.
#[tokio::test(flavor = "multi_thread")]
async fn an_exec_start_no_result_can_fit_is_refused_before_any_driver_is_dispatched() {
    let width = exec_width(&exec_input(&minted_workspace_id()));
    let (harness, driver, workspace) = harness(width - 1, None).await;
    assert_result_limit(
        &harness,
        &Method::POST,
        "/v1/execs",
        "01JRESULTLIMITEXEC00001",
        &exec_input(&workspace),
    )
    .await;
    assert_eq!(driver.effects(), 0, "no driver was called");
    assert_eq!(rows(&harness, "execs"), 0);
    assert_eq!(rows(&harness, "leases"), 0);
}

/// A session start is decided by the exec input it embeds: one byte past the reserve is refused
/// before any driver is called, with no session, exec or lease row.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_start_no_exec_result_can_fit_is_refused_before_any_driver_is_dispatched() {
    let width = exec_width(&exec_input(&minted_workspace_id()));
    let (harness, driver, workspace) = harness(width - 1, None).await;
    assert_result_limit(
        &harness,
        &Method::POST,
        "/v1/sessions",
        "01JRESULTLIMITSESSION01",
        &session_input(&workspace),
    )
    .await;
    assert_eq!(driver.effects(), 0, "no driver was called");
    assert_eq!(rows(&harness, "sessions"), 0);
    assert_eq!(rows(&harness, "execs"), 0);
    assert_eq!(rows(&harness, "leases"), 0);
}

/// The reserve a small ledger leaves, and the longest path whose widest v1 file result fits it.
const SMALL_RESERVE: u64 = 2_048;

/// A v1 write of a path one character past the longest whose widest observation fits is refused
/// before any driver writes; the path at the bound is written and its row completes.
#[tokio::test(flavor = "multi_thread")]
async fn a_v1_file_write_no_result_can_fit_is_refused_before_any_driver_is_dispatched() {
    let (harness, driver, workspace) = harness(SMALL_RESERVE, None).await;
    let fitting = usize::try_from(SMALL_RESERVE).expect("usize")
        - FileObservation::widest_json_bytes(&workspace, "");
    let input = json!({ "content": { "encoding": "base64", "data": "eA==" } });
    let over = "a".repeat(fitting + 1);
    assert_result_limit(
        &harness,
        &Method::PUT,
        &format!("/v1/workspaces/{workspace}/files/{over}"),
        "01JRESULTLIMITWRITE0001",
        &input,
    )
    .await;
    assert_eq!(driver.effects(), 0, "no driver was called");

    let at_the_bound = "a".repeat(fitting);
    assert_eq!(
        u64::try_from(FileObservation::widest_json_bytes(
            &workspace,
            &at_the_bound
        ))
        .expect("u64"),
        SMALL_RESERVE
    );
    let (status, written) = harness
        .call(
            "local:1000",
            Method::PUT,
            &format!("/v1/workspaces/{workspace}/files/{at_the_bound}"),
            "req_write_fits",
            mutation("01JRESULTFITSWRITE00001", input),
        )
        .await;
    assert_eq!(driver.writes.load(Ordering::SeqCst), 1);
    if status == StatusCode::OK {
        let record = harness
            .store
            .operation(&ledger_scope(), "01JRESULTFITSWRITE00001")
            .expect("ledger read")
            .expect("ledger row");
        assert_eq!(
            record.outcome,
            Some(OperationOutcome::Success {
                result: written["result"].clone()
            })
        );
    } else {
        // A file system whose names are shorter than the path refuses the write itself: the
        // driver was called, which is what the bound decides.
        assert_ne!(
            written["error"]["code"], "operation.result-limit",
            "{written}"
        );
    }
}

/// A v1 delete of a path one character past the longest whose widest absence fits is refused
/// before any driver deletes; the path at the bound is dispatched.
#[tokio::test(flavor = "multi_thread")]
async fn a_v1_file_delete_no_result_can_fit_is_refused_before_any_driver_is_dispatched() {
    let (harness, driver, workspace) = harness(SMALL_RESERVE, None).await;
    let fitting = usize::try_from(SMALL_RESERVE).expect("usize")
        - FileAbsence::widest_json_bytes(&workspace, "");
    let over = "a".repeat(fitting + 1);
    assert_result_limit(
        &harness,
        &Method::DELETE,
        &format!("/v1/workspaces/{workspace}/files/{over}"),
        "01JRESULTLIMITDELETE001",
        &json!({}),
    )
    .await;
    assert_eq!(driver.effects(), 0, "no driver was called");

    let at_the_bound = "a".repeat(fitting);
    let (_, answered) = harness
        .call(
            "local:1000",
            Method::DELETE,
            &format!("/v1/workspaces/{workspace}/files/{at_the_bound}"),
            "req_delete_fits",
            mutation("01JRESULTFITSDELETE0001", json!({})),
        )
        .await;
    assert_ne!(
        answered["error"]["code"], "operation.result-limit",
        "{answered}"
    );
    assert_eq!(driver.deletes.load(Ordering::SeqCst), 1, "{answered}");
}

/// One mutation answered `200` and its row terminal with exactly that result, then replayed.
async fn assert_completes_and_replays(
    harness: &Harness,
    uri: &str,
    operation: &str,
    input: &Value,
) -> Value {
    let (status, answered) = harness
        .call(
            "local:1000",
            Method::POST,
            uri,
            "req_at_the_bound",
            mutation(operation, input.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{operation}: {answered}");
    let record = harness
        .store
        .operation(&ledger_scope(), operation)
        .expect("ledger read")
        .expect("ledger row");
    assert_eq!(record.state, OperationState::Terminal, "{operation}");
    assert_eq!(
        record.outcome,
        Some(OperationOutcome::Success {
            result: answered["result"].clone()
        }),
        "{operation}"
    );
    let (status, replayed) = harness
        .call(
            "local:1000",
            Method::POST,
            uri,
            "req_at_the_bound",
            mutation(operation, input.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{operation}: replay");
    assert_eq!(
        replayed["result"], answered["result"],
        "{operation}: replay"
    );
    answered
}

/// An exec start whose widest result is exactly the reserve is dispatched and completes, and its
/// lease renewal and its signal each end terminal and replay: the start decided every later form.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_at_the_bound_completes_and_every_later_mutation_of_it_completes() {
    let width = exec_width(&exec_input(&minted_workspace_id()));
    let (harness, driver, workspace) = harness(width, None).await;
    let (status, started) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/execs",
            "req_at_the_bound",
            mutation("01JRESULTBOUNDSTART0001", exec_input(&workspace)),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert_eq!(driver.starts.load(Ordering::SeqCst), 1);
    let exec = started["result"]["id"]
        .as_str()
        .expect("exec id")
        .to_owned();

    assert_completes_and_replays(
        &harness,
        &format!("/v1/execs/{exec}/lease/renew"),
        "01JRESULTBOUNDRENEW0001",
        &json!({ "ttl_ms": 60_000 }),
    )
    .await;
    let signalled = assert_completes_and_replays(
        &harness,
        &format!("/v1/execs/{exec}/signal"),
        "01JRESULTBOUNDSIGNAL001",
        &json!({ "signal": "TERM", "grace_ms": 0 }),
    )
    .await;
    assert_eq!(signalled["result"]["state"], "cancelled");
    assert_eq!(driver.signals.load(Ordering::SeqCst), 1);
}

/// The fact a daemon publishes, `operation.result-max-json-bytes`, is the reserve its ledger
/// decides against (ADR 0053 decision 4): with the reserve lowered to an exec start's widest
/// result the machine states that figure and the start is dispatched; one byte lower, the machine
/// states the lower figure and the same start is refused before dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn the_published_result_bound_is_the_reserve_the_daemon_decides_against() {
    let width = exec_width(&exec_input(&minted_workspace_id()));
    for (reserve, dispatched) in [(width, 1), (width - 1, 0)] {
        let (harness, driver, workspace) = harness(reserve, None).await;
        let (status, machine) = harness
            .call(
                "local:1000",
                Method::GET,
                "/v1/machine",
                "req_result_bound_machine",
                axum::body::Body::empty(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{machine}");
        assert_eq!(
            machine["result"]["facts"]["operation.result-max-json-bytes"],
            json!(reserve),
            "{reserve}"
        );
        let (status, answered) = harness
            .call(
                "local:1000",
                Method::POST,
                "/v1/execs",
                "req_result_bound_start",
                mutation("01JRESULTBOUNDFACT00001", exec_input(&workspace)),
            )
            .await;
        if dispatched == 1 {
            assert_eq!(status, StatusCode::ACCEPTED, "{answered}");
        } else {
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{answered}");
            assert_eq!(answered["error"]["code"], "operation.result-limit");
        }
        assert_eq!(
            driver.starts.load(Ordering::SeqCst),
            dispatched,
            "{reserve}"
        );
    }
}

/// A daemon over the ledger as shipped publishes the wire constant.
#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_over_the_shipped_reserve_publishes_the_wire_constant() {
    let (harness, _driver, _workspace) = harness(
        substrate_store::DEFAULT_OPERATION_TERMINAL_HEADROOM_BYTES - AROUND_A_RESULT,
        None,
    )
    .await;
    let (status, machine) = harness
        .call(
            "local:1000",
            Method::GET,
            "/v1/machine",
            "req_result_bound_shipped",
            axum::body::Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{machine}");
    assert_eq!(
        machine["result"]["facts"]["operation.result-max-json-bytes"],
        json!(substrate_wire::OPERATION_RESULT_MAX_JSON_BYTES)
    );
}

/// A request another published code refuses keeps that code, though no result of it could fit
/// either: a stale capability snapshot, a path outside the workspace, and a session start whose
/// exec breaks its own shape.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_another_code_refuses_keeps_that_code() {
    let (harness, driver, workspace) = harness(SMALL_RESERVE, None).await;
    let mut stale = exec_input(&workspace);
    stale["sandbox"]["capability_snapshot"] = json!(format!("sha256:{}", "c".repeat(64)));
    let mut no_session = session_input(&workspace);
    no_session["exec"]["wait"] = json!(true);
    let escape = format!("../{}", "a".repeat(4_000));
    for (method, uri, operation, input, code) in [
        (
            Method::POST,
            "/v1/execs".to_owned(),
            "01JRESULTKEEPSTALE00001",
            stale,
            "exec.capability-stale",
        ),
        (
            Method::POST,
            "/v1/sessions".to_owned(),
            "01JRESULTKEEPSESSION001",
            no_session,
            "request.schema-invalid",
        ),
        (
            Method::PUT,
            format!("/v1/workspaces/{workspace}/files/{escape}"),
            "01JRESULTKEEPWRITE00001",
            json!({ "content": { "encoding": "base64", "data": "eA==" } }),
            "workspace.path-escape",
        ),
        (
            Method::DELETE,
            format!("/v1/workspaces/{workspace}/files/{escape}"),
            "01JRESULTKEEPDELETE0001",
            json!({}),
            "workspace.path-escape",
        ),
    ] {
        let (_, refused) = harness
            .call(
                "local:1000",
                method,
                &uri,
                "req_keeps",
                mutation(operation, input),
            )
            .await;
        assert_eq!(refused["error"]["code"], code, "{operation}: {refused}");
    }
    assert_eq!(driver.effects(), 0);
}

/// ADR 0053 decision 3, on both kinds that carry an exec input: the width is decided after the
/// lexical bound of every list the exec input carries and before workspace admission. A list
/// one entry past its wire bound keeps its own published code though no result could fit, and a
/// start naming a workspace that does not exist is refused `operation.result-limit`, not
/// `resource.not-found`.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One case per list the exec input carries, on both kinds.
async fn the_width_is_decided_after_every_list_bound_and_before_workspace_admission() {
    let (harness, driver, workspace) = harness(SMALL_RESERVE, None).await;
    let over = |member: &str, value: Value| {
        let mut input = exec_input(&workspace);
        if member == "apertures" {
            input["sandbox"]["network"] = json!("aperture");
            input["sandbox"]["apertures"] = value;
        } else if member == "env.set" {
            input["env"]["set"] = value;
        } else {
            input[member] = value;
        }
        input
    };
    let entries = |count: u32, entry: &dyn Fn(u32) -> Value| {
        Value::Array((0..count).map(entry).collect::<Vec<_>>())
    };
    let lists = [
        (
            over("argv", entries(257, &|n| json!(format!("a{n}")))),
            "request.schema-invalid",
        ),
        (
            over(
                "env.set",
                Value::Object((0..65).map(|n| (format!("NAME_{n}"), json!("v"))).collect()),
            ),
            "request.schema-invalid",
        ),
        (
            over(
                "apertures",
                entries(substrate_wire::MAX_EGRESS_APERTURES + 1, &|n| {
                    json!(format!("ap{n}"))
                }),
            ),
            substrate_wire::EXEC_APERTURES_TOO_MANY,
        ),
        (
            over(
                "secret_slots",
                entries(
                    substrate_wire::MAX_SECRET_SLOTS + 1,
                    &|n| json!({ "slot": format!("s{n}"), "fd": substrate_wire::MIN_SECRET_SLOT_FD + n }),
                ),
            ),
            "exec.secret-slot-descriptor-invalid",
        ),
        (
            over(
                "read_only_files",
                entries(
                    substrate_wire::MAX_READ_ONLY_FILES + 1,
                    &|n| json!({ "host_path": format!("/srv/f{n}"), "mount": format!("/ro/f{n}") }),
                ),
            ),
            substrate_wire::READ_ONLY_FILES_EXCEEDED,
        ),
        (
            over(
                "writable_roots",
                entries(
                    substrate_wire::MAX_WRITABLE_ROOTS + 1,
                    &|n| json!({ "host": format!("w{n}"), "mount": format!("/w/{n}") }),
                ),
            ),
            substrate_wire::EXEC_WRITABLE_ROOTS_EXCEEDED,
        ),
        (
            over(
                "read_only_roots",
                entries(
                    substrate_wire::MAX_READ_ONLY_ROOTS + 1,
                    &|n| json!({ "host_path": format!("/srv/r{n}"), "mount": format!("/ro/r{n}") }),
                ),
            ),
            substrate_wire::READ_ONLY_ROOT_INVALID,
        ),
    ];
    for (index, (input, code)) in lists.into_iter().enumerate() {
        let session = json!({
            "exec": input.clone(),
            "input_limit_bytes": 65_536,
            "frame_limit_bytes": 16_384,
            "queued_frames": 16,
            "mode": "pipes"
        });
        for (uri, body, kind) in [("/v1/execs", input, 'E'), ("/v1/sessions", session, 'S')] {
            let operation = format!("01JRESULTORDERLIST{kind}{index:04}");
            let (status, refused) = harness
                .call(
                    "local:1000",
                    Method::POST,
                    uri,
                    "req_order",
                    mutation(&operation, body),
                )
                .await;
            assert_eq!(refused["error"]["code"], code, "{uri} {code}: {refused}");
            assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "{uri} {code}");
        }
    }
    assert_result_limit(
        &harness,
        &Method::POST,
        "/v1/execs",
        "01JRESULTORDERABSENTE01",
        &exec_input(&minted_workspace_id()),
    )
    .await;
    assert_result_limit(
        &harness,
        &Method::POST,
        "/v1/sessions",
        "01JRESULTORDERABSENTS01",
        &session_input(&minted_workspace_id()),
    )
    .await;
    assert_eq!(driver.effects(), 0);
}

/// A driver's answer whose refusal `code` is one character past its wire bound ends the start's
/// row terminal with `operation.result-unrecorded`: 500, class `failed`, not retriable, address
/// `result`. The row names the exec, the exec reads back whole with the code as the driver gave
/// it, and the same operation id replays the error.
#[tokio::test(flavor = "multi_thread")]
async fn a_driver_answer_over_a_member_bound_ends_the_row_terminal_and_cuts_nothing() {
    let code = "c".repeat(substrate_wire::MAX_EXEC_REFUSAL_CODE_CHARS + 1);
    let (harness, driver, workspace) = harness(
        substrate_store::DEFAULT_OPERATION_TERMINAL_HEADROOM_BYTES - AROUND_A_RESULT,
        Some(code.clone()),
    )
    .await;
    let operation = "01JRESULTUNRECORDED0001";
    let (status, answered) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/execs",
            "req_unrecorded",
            mutation(operation, exec_input(&workspace)),
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{answered}");
    assert_eq!(answered["error"]["code"], "operation.result-unrecorded");
    assert_eq!(answered["error"]["class"], "failed");
    assert_eq!(answered["error"]["retriable"], false);
    assert_eq!(answered["error"]["address"], "result");
    assert_eq!(
        answered["error"]["message"],
        "The operation took effect, but its result exceeds what the operation ledger can record. \
         Read the resource."
    );
    assert_eq!(driver.starts.load(Ordering::SeqCst), 1);
    let record = harness
        .store
        .operation(&ledger_scope(), operation)
        .expect("ledger read")
        .expect("ledger row");
    assert_eq!(record.state, OperationState::Terminal);
    let exec = record.resource.clone().expect("the row names the exec");
    assert!(matches!(
        record.outcome,
        Some(OperationOutcome::Error { ref error }) if error.code == "operation.result-unrecorded"
    ));
    let (status, read) = harness
        .call(
            "local:1000",
            Method::GET,
            &format!("/v1/execs/{exec}"),
            "req_unrecorded_read",
            axum::body::Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(
        read["result"]["refusal"]["code"],
        json!(code),
        "nothing is cut"
    );
    assert_eq!(read["result"]["state"], "running", "the run is not changed");
    let (status, replayed) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/execs",
            "req_unrecorded",
            mutation(operation, exec_input(&workspace)),
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{replayed}");
    assert_eq!(replayed["error"], answered["error"]);
    assert_eq!(
        driver.starts.load(Ordering::SeqCst),
        1,
        "a replay starts nothing"
    );
}
