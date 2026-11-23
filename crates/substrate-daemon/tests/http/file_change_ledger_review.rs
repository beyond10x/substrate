//! Independent verification through the daemon's routes and its own ledger
//! (review of story:file-change-result-fits-the-ledger-before-the-file-changes).
//!
//! The rule: every admitted v2 file replacement, edit and patch ends with a successful,
//! replayable terminal record, and a text over its published length is refused before dispatch.
//! These cases put the widest members the result can have against the shipped ledger headroom.

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use base64::Engine as _;
use serde_json::{Value, json};
use substrate_daemon::{Identity, router};
use substrate_store::DEFAULT_OPERATION_TERMINAL_HEADROOM_BYTES;
use substrate_wire::{MAX_FILE_BYTES, OperationOutcome, OperationState};
use tower::ServiceExt as _;

use super::{Harness, SEED, ledger_scope, mutation, sha256_hex};

/// What a success completion adds to its ledger row besides the outcome: the longest
/// `terminal_at` the daemon writes (35) and the status charge (8).
const TERMINAL_AT_AND_STATUS_BYTES: usize = 35 + 8;

fn headroom() -> usize {
    usize::try_from(DEFAULT_OPERATION_TERMINAL_HEADROOM_BYTES).expect("u64 fits usize")
}

/// Every path beneath `root` whose last name is `name`.
fn named(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name() == name {
                found.push(path.clone());
            }
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(path);
            }
        }
    }
    found
}

impl Harness {
    /// One call whose answer is returned as the bytes the daemon wrote.
    async fn call_bytes(
        &self,
        method: Method,
        uri: &str,
        request_id: &str,
        body: Body,
    ) -> (StatusCode, Vec<u8>) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-request-id", request_id)
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: "local:1000".to_owned(),
            actor: "http-test".to_owned(),
            principal: None,
        };
        let response = router(Arc::clone(&self.app))
            .layer(Extension(identity))
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2_097_152)
            .await
            .expect("response body");
        (status, bytes.to_vec())
    }

    /// The terminal success row of `operation`: its result, after holding the recorded outcome
    /// to the shipped headroom.
    fn recorded_success(&self, operation: &str) -> Value {
        let record = self
            .store
            .operation(&ledger_scope(), operation)
            .expect("ledger read")
            .expect("ledger row");
        assert_eq!(record.state, OperationState::Terminal, "{operation}");
        let outcome = record.outcome.expect("a terminal row holds its outcome");
        let outcome_bytes = serde_json::to_string(&outcome).expect("outcome JSON").len();
        assert!(
            outcome_bytes + TERMINAL_AT_AND_STATUS_BYTES <= headroom(),
            "{operation}: the recorded outcome is {outcome_bytes} bytes"
        );
        let OperationOutcome::Success { result } = outcome else {
            panic!("{operation}: the recorded outcome is a failure");
        };
        result
    }
}

/// The widest members a result can have, through the routes: a path of 4 095 bytes whose every
/// character costs six bytes of JSON, and a file at the limit made of such characters, so that
/// the uncut diff of each change costs megabytes of JSON. The replacement that creates the file,
/// an edit and a patch of it each answer `200`, leave a terminal success row within the shipped
/// headroom whose diff was cut to fill it, and replay.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One file walked through the three changes in order.
async fn the_three_changes_of_the_widest_members_each_complete_their_ledger_row() {
    let harness = Harness::open();
    let workspace = harness.workspace_with_seed_file().await;
    let path = vec!["\u{1}".repeat(255); 16].join("/");
    assert_eq!(path.len(), 4_095);
    let encoded_path = vec!["%01".repeat(255); 16].join("/");
    let size = usize::try_from(MAX_FILE_BYTES).expect("u64 fits usize");
    // A first line the edit and the patch can name, then one line of control characters to the
    // file limit.
    let tail = "\u{1}".repeat(size - 2);
    let first = format!("X\n{tail}");
    let second = format!("Y\n{tail}");
    let third = format!("Z\n{tail}");

    let replace = json!({
        "content": {
            "encoding": "base64",
            "data": base64::engine::general_purpose::STANDARD.encode(&first),
        },
        "expected": { "state": "absent" },
        "create_parents": true
    });
    let edit = json!({
        "expected_sha256": sha256_hex(first.as_bytes()),
        "old_text": "X",
        "new_text": "Y",
        "match_policy": "exact"
    });
    let line_patch = json!({
        "expected_sha256": sha256_hex(second.as_bytes()),
        "edits": [{ "kind": "replace_range", "start_line": 1, "end_line": 1, "text": "Z" }]
    });

    let steps = [
        (
            "01JWIDESTREPLACE0000001",
            Method::PUT,
            format!("/v2/workspaces/{workspace}/files/{encoded_path}"),
            replace,
            Value::Null,
            sha256_hex(first.as_bytes()),
        ),
        (
            "01JWIDESTEDIT0000000001",
            Method::POST,
            format!("/v2/workspaces/{workspace}/file-edits/{encoded_path}"),
            edit,
            json!(sha256_hex(first.as_bytes())),
            sha256_hex(second.as_bytes()),
        ),
        (
            "01JWIDESTPATCH000000001",
            Method::POST,
            format!("/v2/workspaces/{workspace}/file-patches/{encoded_path}"),
            line_patch,
            json!(sha256_hex(second.as_bytes())),
            sha256_hex(third.as_bytes()),
        ),
    ];
    for (operation, method, uri, input, before, after) in steps {
        let (status, answered) = harness
            .call(
                "local:1000",
                method.clone(),
                &uri,
                "req_widest",
                mutation(operation, input.clone()),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{operation}: {}",
            serde_json::to_string(&answered["error"]).expect("JSON")
        );
        let result = &answered["result"];
        assert_eq!(result["path"], json!(path), "{operation}");
        assert_eq!(result["before_sha256"], before, "{operation}");
        assert_eq!(result["after_sha256"], json!(after), "{operation}");
        assert_eq!(result["size"], json!(MAX_FILE_BYTES), "{operation}");
        assert_eq!(result["diff"]["truncated"], true, "{operation}");
        assert_eq!(result["diff"]["binary"], false, "{operation}");

        let recorded = harness.recorded_success(operation);
        assert_eq!(&recorded, result, "{operation}: the row holds the answer");
        let recorded_bytes = serde_json::to_string(&recorded).expect("JSON").len();
        // The diff was cut to fill what the ledger reserved: less than one character's cost and
        // the width the members of this result leave unused separates the two.
        let budget = headroom() - TERMINAL_AT_AND_STATUS_BYTES - 28;
        assert!(recorded_bytes <= budget, "{operation}: {recorded_bytes}");
        assert!(
            recorded_bytes + 96 > budget,
            "{operation}: a result of {recorded_bytes} bytes was not cut to the budget {budget}"
        );

        let (status, replayed) = harness
            .call(
                "local:1000",
                method,
                &uri,
                "req_widest",
                mutation(operation, input),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{operation}: replay");
        assert_eq!(replayed, answered, "{operation}: the replay is the answer");
    }
}

/// A replacement's `content.data` over its published length is `request.schema-invalid` before
/// dispatch: the row is `refused`, never accepted, no parent it asked for exists, and the same
/// request for a workspace that does not exist is the same refusal, so it is decided before
/// workspace admission.
#[tokio::test(flavor = "multi_thread")]
async fn a_replacement_content_over_its_published_length_is_refused_before_dispatch() {
    const OPERATION: &str = "01JCONTENTOVERLENGTH001";
    let harness = Harness::open();
    let workspace = harness.workspace_with_seed_file().await;
    // Canonical base64 of 1 048 581 zero bytes: four characters over the published length.
    let input = json!({
        "content": { "encoding": "base64", "data": "A".repeat(1_398_104 + 4) },
        "expected": { "state": "absent" },
        "create_parents": true
    });
    let (status, refused) = harness
        .call(
            "local:1000",
            Method::PUT,
            &format!("/v2/workspaces/{workspace}/files/fresh/dir/file.bin"),
            "req_content_over",
            mutation(OPERATION, input.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "request.schema-invalid");
    let record = harness
        .store
        .operation(&ledger_scope(), OPERATION)
        .expect("ledger read")
        .expect("ledger row");
    assert_eq!(
        record.state,
        OperationState::Refused,
        "refused before dispatch, never accepted"
    );
    assert!(
        named(harness.directory.path(), "fresh").is_empty(),
        "a refused replacement created a parent directory"
    );
    assert_eq!(
        harness.file_sha256(&workspace, "main.txt").await,
        sha256_hex(SEED.as_bytes())
    );

    let (status, absent) = harness
        .call(
            "local:1000",
            Method::PUT,
            "/v2/workspaces/ws_01JABSENTWORKSPACE000000000/files/fresh/dir/file.bin",
            "req_content_over_absent",
            mutation("01JCONTENTOVERLENGTH002", input),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{absent}");
    assert_eq!(absent["error"]["code"], "request.schema-invalid");
}

/// JSON may spell a lone surrogate, which no text holds. The request is refused and the file is
/// the one that was there.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_spelled_with_a_lone_surrogate_is_refused_and_changes_nothing() {
    let harness = Harness::open();
    let workspace = harness.workspace_with_seed_file().await;
    let digest = sha256_hex(SEED.as_bytes());
    for (operation, texts) in [
        (
            "01JLONESURROGATE0000001",
            r#""old_text":"seed","new_text":"\ud800""#,
        ),
        (
            "01JLONESURROGATE0000002",
            r#""old_text":"\udc00","new_text":"x""#,
        ),
    ] {
        let body = format!(
            r#"{{"op":"{operation}","input":{{"expected_sha256":"{digest}",{texts},"match_policy":"exact"}}}}"#
        );
        let (status, refused) = harness
            .call(
                "local:1000",
                Method::POST,
                &format!("/v2/workspaces/{workspace}/file-edits/main.txt"),
                "req_lone_surrogate",
                Body::from(body),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
        assert_eq!(refused["error"]["code"], "request.schema-invalid");
        assert_eq!(harness.file_sha256(&workspace, "main.txt").await, digest);
    }
}

/// The bound counts characters. A text of 600 000 two-byte characters is over 1 048 576 bytes
/// and within the published length, so it is not `request.schema-invalid`: it is dispatched, and
/// the answer is the driver's own for a file that would exceed the limit, with the file
/// unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_over_the_bound_in_bytes_and_within_it_in_characters_is_dispatched() {
    const OPERATION: &str = "01JTEXTBYTESNOTCHARS001";
    let harness = Harness::open();
    let workspace = harness.workspace_with_seed_file().await;
    let text = "\u{e9}".repeat(600_000);
    assert!(text.len() > 1_048_576);
    let (status, answered) = harness
        .call(
            "local:1000",
            Method::POST,
            &format!("/v2/workspaces/{workspace}/file-edits/main.txt"),
            "req_bytes_not_chars",
            mutation(
                OPERATION,
                json!({
                    "expected_sha256": sha256_hex(SEED.as_bytes()),
                    "old_text": "seed",
                    "new_text": text,
                    "match_policy": "exact"
                }),
            ),
        )
        .await;
    assert_ne!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answered}");
    assert_eq!(answered["error"]["code"], "workspace.write-limit");
    let record = harness
        .store
        .operation(&ledger_scope(), OPERATION)
        .expect("ledger read")
        .expect("ledger row");
    assert_eq!(
        record.state,
        OperationState::Terminal,
        "dispatched: accepted, then completed with the driver's refusal"
    );
    assert_eq!(
        harness.file_sha256(&workspace, "main.txt").await,
        sha256_hex(SEED.as_bytes())
    );
}

/// A retry of a completed change answers the stored result and does not replace the file again:
/// the file is the same inode after the retry. The two answers are compared as the bytes the
/// daemon wrote, and what that comparison shows is printed, not asserted: the rule is the same
/// logical outcome (ADR 0005).
#[tokio::test(flavor = "multi_thread")]
async fn a_retry_after_success_answers_the_stored_result_and_does_not_write_again() {
    const OPERATION: &str = "01JRETRYAFTERSUCCESS001";
    let harness = Harness::open();
    let workspace = harness.workspace_with_seed_file().await;
    let uri = format!("/v2/workspaces/{workspace}/file-edits/main.txt");
    let input = json!({
        "expected_sha256": sha256_hex(SEED.as_bytes()),
        "old_text": "seed",
        "new_text": "changed",
        "match_policy": "exact"
    });
    let (status, first) = harness
        .call_bytes(
            Method::POST,
            &uri,
            "req_retry",
            mutation(OPERATION, input.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let files = named(harness.directory.path(), "main.txt");
    assert_eq!(files.len(), 1, "{files:?}");
    let written = std::fs::metadata(&files[0]).expect("metadata");

    let (status, second) = harness
        .call_bytes(Method::POST, &uri, "req_retry", mutation(OPERATION, input))
        .await;
    assert_eq!(status, StatusCode::OK);
    let after_retry = std::fs::metadata(&files[0]).expect("metadata");
    assert_eq!(
        after_retry.ino(),
        written.ino(),
        "the retry replaced the file"
    );
    assert_eq!(after_retry.mtime_nsec(), written.mtime_nsec());
    assert_eq!(std::fs::read(&files[0]).expect("file"), b"changed\n");

    let first_value: Value = serde_json::from_slice(&first).expect("JSON");
    let second_value: Value = serde_json::from_slice(&second).expect("JSON");
    assert_eq!(second_value, first_value, "the retry is the same result");
    eprintln!(
        "review: the retry's answer is byte-identical to the first answer: {}",
        first == second
    );
}
