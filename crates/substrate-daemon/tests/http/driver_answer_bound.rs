//! The daemon's own rule for the result of a v2 file change, whatever driver serves it
//! (story:file-change-result-fits-the-ledger-before-the-file-changes).
//!
//! A change is dispatched only when a result for its workspace and path can fit what the ledger
//! reserved, and the answer a driver gives is held to the reserved bytes before the row is
//! completed. The driver here delegates everything to the host driver except the three v2
//! changes, which it answers itself with a diff of several MiB that states no bound, and counts.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use chrono::Utc;
use serde_json::{Value, json};
use substrate_daemon::App;
use substrate_host::{
    DispatchOutcome, Driver, DriverError, ExecObservation, HostConfig, HostDriver,
    WorkspaceDestroyProgress,
};
use substrate_store::{Store, StoreConfig};
use substrate_wire::{
    CapabilitySnapshot, ExecOutputQuery, ExecSignalInput, ExecStartInput, FileAbsence,
    FileEditInput, FileKind, FileMutationResult, FileObservation, FilePatchInput, FileReadQuery,
    FileReadResult, FileReplaceInput, LeaseObservation, MAX_FILE_CONTENT_BASE64_CHARS,
    MAX_FILE_DIFF_TEXT_CHARS, OperationOutcome, OperationState, OutputSlice, UnifiedDiff,
    Workspace, WorkspaceAbsence, WorkspaceCreateInput,
};

use super::{Harness, ledger_scope, mutation};

/// A driver whose v2 file changes answer `diff_text` uncut and unmarked, for the workspace and
/// path it was asked for, and count how often they were dispatched. Its snapshot states the
/// success-result reserve of the ledger it is paired with, as every served snapshot does.
struct AnsweringDriver {
    host: Arc<HostDriver>,
    diff_text: String,
    dispatched: AtomicUsize,
    result_bytes: u64,
}

impl AnsweringDriver {
    fn answer(&self, workspace_id: &str, path: &str) -> FileMutationResult {
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        FileMutationResult {
            kind: FileKind::File,
            workspace: workspace_id.to_owned(),
            path: path.to_owned(),
            size: 7,
            before_sha256: Some("1".repeat(64)),
            after_sha256: "2".repeat(64),
            atomic_replacement: true,
            diff: UnifiedDiff {
                text: self.diff_text.clone(),
                truncated: false,
                binary: false,
            },
            observed_at: Utc::now(),
        }
    }

    fn dispatched(&self) -> usize {
        self.dispatched.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Driver for AnsweringDriver {
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

    async fn replace_workspace_file_v2(
        &self,
        workspace_id: &str,
        _root_name: &str,
        path: &str,
        _input: &FileReplaceInput,
    ) -> Result<FileMutationResult, DriverError> {
        Ok(self.answer(workspace_id, path))
    }

    async fn edit_workspace_file_v2(
        &self,
        workspace_id: &str,
        _root_name: &str,
        path: &str,
        _input: &FileEditInput,
    ) -> Result<FileMutationResult, DriverError> {
        Ok(self.answer(workspace_id, path))
    }

    async fn patch_workspace_file_v2(
        &self,
        workspace_id: &str,
        _root_name: &str,
        path: &str,
        _input: &FilePatchInput,
    ) -> Result<FileMutationResult, DriverError> {
        Ok(self.answer(workspace_id, path))
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

/// A diff text of several MiB as JSON that is also longer than `diff.text` may be: 2 800 000
/// characters costing eleven bytes of JSON for every four.
fn unbounded_diff_text() -> String {
    "\u{1}\"x\n".repeat(700_000)
}

/// A harness whose driver is an [`AnsweringDriver`] and whose ledger reserves
/// `terminal_headroom_bytes` for a terminal result, with one empty workspace.
async fn harness_answering(
    terminal_headroom_bytes: u64,
) -> (Harness, Arc<AnsweringDriver>, String) {
    let directory = tempfile::tempdir().expect("temporary harness");
    let store = Arc::new(
        Store::open_with_config(
            directory.path().join("state.db"),
            StoreConfig {
                operation_terminal_headroom_bytes: terminal_headroom_bytes,
                ..StoreConfig::default()
            },
        )
        .expect("state store"),
    );
    let driver = Arc::new(AnsweringDriver {
        host: HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver"),
        diff_text: unbounded_diff_text(),
        dispatched: AtomicUsize::new(0),
        result_bytes: store.success_result_json_bytes(),
    });
    let app = App::new(
        Arc::clone(&store),
        Arc::clone(&driver) as Arc<dyn Driver>,
        "dep_http_test",
    )
    .expect("application");
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
            "req_answer_create",
            mutation(
                "01JDRIVERANSWERCREATE01",
                json!({ "source": "empty", "labels": {} }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let workspace = created["result"]["id"]
        .as_str()
        .expect("workspace id")
        .to_owned();
    (harness, driver, workspace)
}

/// The three v2 changes of `path`, each as its method, address and an input the daemon admits.
fn changes(workspace: &str, path: &str) -> [(Method, String, Value); 3] {
    [
        (
            Method::PUT,
            format!("/v2/workspaces/{workspace}/files/{path}"),
            json!({
                "content": { "encoding": "base64", "data": "eA==" },
                "expected": { "state": "absent" }
            }),
        ),
        (
            Method::POST,
            format!("/v2/workspaces/{workspace}/file-edits/{path}"),
            json!({
                "expected_sha256": "1".repeat(64),
                "old_text": "a",
                "new_text": "b",
                "match_policy": "exact"
            }),
        ),
        (
            Method::POST,
            format!("/v2/workspaces/{workspace}/file-patches/{path}"),
            json!({
                "expected_sha256": "1".repeat(64),
                "edits": [{ "kind": "insert_before", "line": 1, "text": "x" }]
            }),
        ),
    ]
}

fn json_bytes(value: &Value) -> u64 {
    u64::try_from(serde_json::to_string(value).expect("JSON").len()).expect("usize fits u64")
}

/// The answered result of one completed change: `200`, the diff a prefix of the driver's, cut
/// and marked, inside its published length, every other member as the driver answered it, within
/// `budget` and cut to fill it, and the terminal ledger row holding exactly that result.
fn assert_cut_to_the_budget_and_recorded(
    harness: &Harness,
    driver: &AnsweringDriver,
    operation: &str,
    status: StatusCode,
    answered: &Value,
    budget: u64,
) {
    assert_eq!(status, StatusCode::OK, "{operation}: {}", answered["error"]);
    let result = &answered["result"];
    let text = result["diff"]["text"].as_str().expect("diff text");
    assert!(driver.diff_text.starts_with(text), "{operation}");
    assert!(text.len() < driver.diff_text.len(), "{operation}");
    assert!(
        text.chars().count() <= MAX_FILE_DIFF_TEXT_CHARS,
        "{operation}"
    );
    assert_eq!(result["diff"]["truncated"], true, "{operation}");
    assert_eq!(result["diff"]["binary"], false, "{operation}");
    assert_eq!(result["after_sha256"], "2".repeat(64), "{operation}");
    assert_eq!(result["before_sha256"], "1".repeat(64), "{operation}");
    assert_eq!(result["size"], 7, "{operation}");
    let recorded_bytes = json_bytes(result);
    assert!(
        recorded_bytes <= budget,
        "{operation}: {recorded_bytes} bytes of JSON for a budget of {budget}"
    );
    assert!(
        recorded_bytes + 8 > budget,
        "{operation}: {recorded_bytes} bytes were cut further than the budget {budget} asks"
    );
    let record = harness
        .store
        .operation(&ledger_scope(), operation)
        .expect("ledger read")
        .expect("ledger row");
    assert_eq!(record.state, OperationState::Terminal, "{operation}");
    assert_eq!(
        record.outcome,
        Some(OperationOutcome::Success {
            result: result.clone()
        }),
        "{operation}: the row holds the answered result"
    );
}

/// A driver's answer of several MiB, with a diff longer than its schema publishes and not marked
/// `truncated`, is held by the daemon to what the ledger reserved: each of the three changes
/// answers `200`, its row is completed with the cut result, and a retry answers that result
/// without dispatching again.
#[tokio::test(flavor = "multi_thread")]
async fn a_drivers_answer_over_the_reserved_bytes_is_cut_and_its_row_completes_and_replays() {
    let (harness, driver, workspace) =
        harness_answering(substrate_store::DEFAULT_OPERATION_TERMINAL_HEADROOM_BYTES).await;
    let budget = harness.store.success_result_json_bytes();
    let uncut = driver.answer(&workspace, "main.txt");
    assert!(
        u64::try_from(uncut.json_bytes()).expect("u64") > 4 * 1024 * 1024,
        "the driver's answer is several MiB of JSON"
    );
    assert!(uncut.diff.text.chars().count() > MAX_FILE_DIFF_TEXT_CHARS);
    let before = driver.dispatched();

    for (index, (method, uri, input)) in changes(&workspace, "main.txt").into_iter().enumerate() {
        let operation = format!("01JDRIVERANSWEROVER000{index}");
        let (status, answered) = harness
            .call(
                "local:1000",
                method.clone(),
                &uri,
                "req_answer_over",
                mutation(&operation, input.clone()),
            )
            .await;
        assert_cut_to_the_budget_and_recorded(
            &harness, &driver, &operation, status, &answered, budget,
        );
        assert_eq!(driver.dispatched(), before + index + 1, "{operation}");

        let (status, replayed) = harness
            .call(
                "local:1000",
                method,
                &uri,
                "req_answer_over",
                mutation(&operation, input),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{operation}: replay");
        assert_eq!(replayed, answered, "{operation}: the replay is the answer");
        assert_eq!(
            driver.dispatched(),
            before + index + 1,
            "{operation}: a replay dispatches nothing"
        );
    }
}

/// Whether a result can fit at all is decided by the daemon before dispatch, for every driver,
/// from the widest result its workspace and path can have without a diff text. With a ledger
/// that reserves little: a path whose widest result is exactly the reserved bytes is dispatched
/// and completes with its diff cut to nothing, and a path one character longer is
/// `workspace.write-limit`, recorded as refused, with no driver dispatched.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_no_result_can_fit_is_refused_before_any_driver_is_dispatched() {
    let (harness, driver, workspace) = harness_answering(2_048).await;
    let budget = harness.store.success_result_json_bytes();
    assert_eq!(budget, 2_048 - 35 - 8 - 28);
    let without_path = FileMutationResult::widest_json_bytes(&workspace, "");
    let fitting = usize::try_from(budget).expect("u64 fits usize") - without_path;
    let widest_fitting_path = "a".repeat(fitting);
    assert_eq!(
        u64::try_from(FileMutationResult::widest_json_bytes(
            &workspace,
            &widest_fitting_path
        ))
        .expect("u64"),
        budget
    );

    let one_longer = "a".repeat(fitting + 1);
    for (index, (method, uri, input)) in changes(&workspace, &one_longer).into_iter().enumerate() {
        let operation = format!("01JDRIVERANSWERNOFIT00{index}");
        let (status, refused) = harness
            .call(
                "local:1000",
                method.clone(),
                &uri,
                "req_no_fit",
                mutation(&operation, input.clone()),
            )
            .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
        assert_eq!(refused["error"]["code"], "workspace.write-limit");
        assert_eq!(refused["error"]["class"], "exhausted");
        assert_eq!(refused["error"]["retriable"], false);
        let record = harness
            .store
            .operation(&ledger_scope(), &operation)
            .expect("ledger read")
            .expect("ledger row");
        assert_eq!(
            record.state,
            OperationState::Refused,
            "{operation}: refused before dispatch, never accepted"
        );
        let (status, replayed) = harness
            .call(
                "local:1000",
                method,
                &uri,
                "req_no_fit",
                mutation(&operation, input),
            )
            .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{replayed}");
        assert_eq!(replayed["error"], refused["error"]);
    }
    assert_eq!(driver.dispatched(), 0, "no driver was dispatched");

    for (index, (method, uri, input)) in changes(&workspace, &widest_fitting_path)
        .into_iter()
        .enumerate()
    {
        let operation = format!("01JDRIVERANSWERFITS000{index}");
        let (status, answered) = harness
            .call(
                "local:1000",
                method,
                &uri,
                "req_fits",
                mutation(&operation, input),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{operation}: {}", answered["error"]);
        let result = &answered["result"];
        assert_eq!(result["path"], json!(widest_fitting_path), "{operation}");
        assert_eq!(result["diff"]["truncated"], true, "{operation}");
        assert!(json_bytes(result) <= budget, "{operation}");
        let record = harness
            .store
            .operation(&ledger_scope(), &operation)
            .expect("ledger read")
            .expect("ledger row");
        assert_eq!(record.state, OperationState::Terminal, "{operation}");
        assert_eq!(
            record.outcome,
            Some(OperationOutcome::Success {
                result: result.clone()
            }),
            "{operation}"
        );
        assert_eq!(driver.dispatched(), index + 1, "{operation}");
    }
}

/// With the same small reserve, a change of a short path is dispatched and its answer is cut to
/// fill the reserved bytes.
#[tokio::test(flavor = "multi_thread")]
async fn a_drivers_answer_is_cut_to_whatever_the_ledger_reserves() {
    let (harness, driver, workspace) = harness_answering(2_048).await;
    let budget = harness.store.success_result_json_bytes();
    for (index, (method, uri, input)) in changes(&workspace, "main.txt").into_iter().enumerate() {
        let operation = format!("01JDRIVERANSWERSMALL00{index}");
        let (status, answered) = harness
            .call(
                "local:1000",
                method,
                &uri,
                "req_small",
                mutation(&operation, input),
            )
            .await;
        assert_cut_to_the_budget_and_recorded(
            &harness, &driver, &operation, status, &answered, budget,
        );
    }
}

/// A replacement's `content.data` is held to the two forms its schema publishes before dispatch:
/// data of the longer form's length that does not end `==` is `request.schema-invalid`, recorded
/// as refused, with no driver dispatched, and data of that length ending `==` is dispatched.
#[tokio::test(flavor = "multi_thread")]
async fn a_replacement_content_of_the_longest_length_is_dispatched_only_in_its_published_form() {
    let (harness, driver, workspace) =
        harness_answering(substrate_store::DEFAULT_OPERATION_TERMINAL_HEADROOM_BYTES).await;
    let uri = format!("/v2/workspaces/{workspace}/files/main.txt");
    let replacement = |data: String| {
        json!({
            "content": { "encoding": "base64", "data": data },
            "expected": { "state": "absent" }
        })
    };
    for (index, data) in [
        "A".repeat(MAX_FILE_CONTENT_BASE64_CHARS),
        format!("{}=", "A".repeat(MAX_FILE_CONTENT_BASE64_CHARS - 1)),
        "A".repeat(MAX_FILE_CONTENT_BASE64_CHARS - 1),
    ]
    .into_iter()
    .enumerate()
    {
        let operation = format!("01JCONTENTFORMREFUSED0{index}");
        let (status, refused) = harness
            .call(
                "local:1000",
                Method::PUT,
                &uri,
                "req_content_form",
                mutation(&operation, replacement(data)),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
        assert_eq!(refused["error"]["code"], "request.schema-invalid");
        let record = harness
            .store
            .operation(&ledger_scope(), &operation)
            .expect("ledger read")
            .expect("ledger row");
        assert_eq!(record.state, OperationState::Refused, "{operation}");
    }
    assert_eq!(driver.dispatched(), 0, "no driver was dispatched");

    let (status, answered) = harness
        .call(
            "local:1000",
            Method::PUT,
            &uri,
            "req_content_form",
            mutation(
                "01JCONTENTFORMADMITTED1",
                replacement(format!(
                    "{}==",
                    "A".repeat(MAX_FILE_CONTENT_BASE64_CHARS - 2)
                )),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", answered["error"]);
    assert_eq!(driver.dispatched(), 1);
}
