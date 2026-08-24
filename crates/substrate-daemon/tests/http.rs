#![forbid(unsafe_code)]

use std::sync::Arc;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use serde_json::{Value, json};
use substrate_daemon::{App, CONTRACT_BUNDLE, CONTRACT_BUNDLE_SHA256, Identity, router};
use substrate_host::{HostConfig, HostDriver};
use substrate_store::{ExecWrite, NewOperation, Reservation, Scope, Store, StoredExec};
use substrate_wire::{
    ConfinementRequest, Exec, ExecExit, ExecKind, ExecState, NetworkMode, SandboxProfile,
};
use tempfile::TempDir;
use tower::ServiceExt as _;

struct Harness {
    _directory: TempDir,
    app: Arc<App>,
    store: Arc<Store>,
}

impl Harness {
    fn open() -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver");
        let app = App::new(Arc::clone(&store), driver, "dep_http_test");
        Self {
            _directory: directory,
            app,
            store,
        }
    }

    async fn call(
        &self,
        subject: &str,
        method: Method,
        uri: &str,
        request_id: &str,
        body: Body,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-request-id", request_id)
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: subject.to_owned(),
            actor: "http-test".to_owned(),
            principal: None,
        };
        let response = router(Arc::clone(&self.app))
            .layer(Extension(identity))
            .oneshot(request)
            .await
            .expect("router response");
        assert_eq!(response.headers()["x-b10x-contract"], CONTRACT_BUNDLE);
        assert_eq!(
            response.headers()["x-b10x-contract-bundle-sha256"],
            CONTRACT_BUNDLE_SHA256
        );
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2_097_152)
            .await
            .expect("response body");
        let body = serde_json::from_slice(&bytes).expect("JSON response");
        (status, body)
    }
}

#[allow(clippy::needless_pass_by_value)] // Test call sites construct one-shot JSON values.
fn mutation(operation: &str, input: Value) -> Body {
    Body::from(
        serde_json::to_vec(&json!({ "op": operation, "input": input })).expect("mutation JSON"),
    )
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One sequential black-box journey proves lifecycle ordering.
async fn twelve_route_vertical_slice_is_scoped_durable_and_observed() {
    let harness = Harness::open();
    let (status, machine) = harness
        .call(
            "local:1000",
            Method::GET,
            "/v1/machine",
            "req_machine_test",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let snapshot = machine["result"]["snapshot"]
        .as_str()
        .expect("capability snapshot")
        .to_owned();
    assert_eq!(machine["result"]["driver"], "host");

    let create_input = json!({ "source": "empty", "labels": { "vector": "journey" } });
    let create_operation = "01JPHASE2CREATEHTTPTEST";
    let (status, created) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/workspaces",
            "req_create_test",
            mutation(create_operation, create_input.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let workspace = created["result"]["id"]
        .as_str()
        .expect("workspace id")
        .to_owned();
    assert!(workspace.starts_with("ws_"));

    let (status, replayed) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/workspaces",
            "req_replay_test",
            mutation(create_operation, create_input),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed["result"]["id"], workspace);
    assert_eq!(replayed["request_id"], "req_replay_test");

    let (status, conflict) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/workspaces",
            "req_conflict_test",
            mutation(
                create_operation,
                json!({ "source": "empty", "labels": { "changed": "yes" } }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["error"]["code"], "operation.request-conflict");

    let (status, hidden) = harness
        .call(
            "local:2000",
            Method::GET,
            &format!("/v1/workspaces/{workspace}"),
            "req_hidden_test",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(hidden["error"]["code"], "resource.not-found");

    let (status, observed) = harness
        .call(
            "local:1000",
            Method::GET,
            &format!("/v1/workspaces/{workspace}"),
            "req_get_ws_test",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(observed["result"]["state"], "ready");

    let file_uri = format!("/v1/workspaces/{workspace}/files/main.txt");
    let (status, written) = harness
        .call(
            "local:1000",
            Method::PUT,
            &file_uri,
            "req_write_test",
            mutation(
                "01JPHASE2WRITEHTTPTEST1",
                json!({ "content": { "encoding": "base64", "data": "aGVsbG8=" } }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(written["result"]["atomic_replacement"], true);
    assert_eq!(
        written["result"]["sha256"],
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );

    let (status, read) = harness
        .call(
            "local:1000",
            Method::GET,
            &format!("{file_uri}?mode=file&offset=0&limit_bytes=5"),
            "req_read_test",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["result"]["content"]["data"], "aGVsbG8=");
    assert_eq!(read["result"]["eof"], true);

    let exec_id = "ex_seed";
    let exec = Exec {
        id: exec_id.to_owned(),
        kind: ExecKind::Exec,
        workspace: workspace.clone(),
        state: ExecState::Exited,
        observed_at: Utc::now(),
        requested: ConfinementRequest {
            capability_snapshot: snapshot.clone(),
            network: NetworkMode::None,
            profile: SandboxProfile::Workspace,
            required: true,
        },
        applied: None,
        exit: Some(ExecExit {
            code: Some(0),
            signal: None,
        }),
        lease: None,
    };
    let scope = Scope {
        deployment: "dep_http_test".to_owned(),
        subject: "local:1000".to_owned(),
    };
    let seed_operation = NewOperation {
        scope: scope.clone(),
        operation: "01JPHASE3SEEDHTTPEXEC01".to_owned(),
        operation_kind: "exec.start".to_owned(),
        request_hash: "8".repeat(64),
        accepted_at: Utc::now().to_rfc3339(),
        capability_snapshot: Some(snapshot.clone()),
        actor: "http-test-fixture".to_owned(),
        principal: None,
        resource: Some(exec_id.to_owned()),
    };
    let mut provisional = exec.clone();
    provisional.state = ExecState::Accepted;
    provisional.exit = None;
    assert_eq!(
        harness
            .store
            .reserve_exec_start(
                &seed_operation,
                &StoredExec {
                    resource: provisional,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    stdout_truncated: false,
                    stderr_truncated: false,
                    output_complete: false,
                    cgroup: None,
                    leader_pid: None,
                },
                None,
                None,
            )
            .expect("admit terminal exec fixture"),
        Reservation::Accepted
    );
    assert!(matches!(
        harness
            .store
            .complete_exec(
                &scope,
                &seed_operation.operation,
                &Utc::now().to_rfc3339(),
                StatusCode::CREATED.as_u16(),
                &exec,
                b"hello",
                &[],
                false,
                false,
                true,
                None,
                None,
            )
            .expect("complete terminal exec fixture"),
        ExecWrite::PersistedExact(_)
    ));

    let (status, observed_exec) = harness
        .call(
            "local:1000",
            Method::GET,
            "/v1/execs/ex_seed",
            "req_exec_get_test",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(observed_exec["result"]["state"], "exited");

    let (status, output) = harness
        .call(
            "local:1000",
            Method::GET,
            "/v1/execs/ex_seed/output?stream=stdout&offset=0&limit_bytes=5",
            "req_output_test",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(output["result"]["content"]["data"], "aGVsbG8=");
    assert_eq!(output["result"]["eof"], true);

    let (status, signal) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/execs/ex_seed/signal",
            "req_signal_test",
            mutation(
                "01JPHASE2SIGNALHTTPTEST",
                json!({ "signal": "TERM", "grace_ms": 1000 }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(signal["operation"], "01JPHASE2SIGNALHTTPTEST");
    assert_eq!(signal["result"]["state"], "exited");

    let (status, unserved) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/execs",
            "req_exec_start_test",
            mutation(
                "01JPHASE2STARTHTTPTEST1",
                json!({
                    "workspace": workspace,
                    "argv": ["/usr/bin/true"],
                    "env": { "allow": [], "set": {} },
                    "sandbox": {
                        "capability_snapshot": snapshot,
                        "network": "none",
                        "profile": "workspace",
                        "require": true
                    },
                    "limits": {
                        "timeout_ms": 5000,
                        "output_bytes": 65536,
                        "processes": 16,
                        "memory_bytes": 67_108_864,
                        "cpu_millis": 1000
                    },
                    "wait": false
                }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(unserved["error"]["code"], "exec.sandbox-unavailable");

    let (status, operation) = harness
        .call(
            "local:1000",
            Method::GET,
            &format!("/v1/ops/{create_operation}"),
            "req_operation_test",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(operation["result"]["state"], "terminal");
    assert_eq!(operation["result"]["resource"], created["result"]["id"]);

    let (status, deleted) = harness
        .call(
            "local:1000",
            Method::DELETE,
            &file_uri,
            "req_delete_test",
            mutation("01JPHASE2DELETEHTTPTEST", json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["result"]["absent"], true);

    let (status, destroyed) = harness
        .call(
            "local:1000",
            Method::DELETE,
            &format!(
                "/v1/workspaces/{}",
                created["result"]["id"].as_str().expect("id")
            ),
            "req_destroy_test",
            mutation("01JPHASE2DESTROYHTTPTEST", json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(destroyed["result"]["absent"], true);
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn v2_file_mutations_enforce_digest_cas_and_return_bounded_diffs() {
    let harness = Harness::open();
    let (_, created) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/workspaces",
            "req_v2_create",
            mutation(
                "01JV2CREATEHTTPTEST0001",
                json!({ "source": "empty", "labels": { "vector": "file-v2" } }),
            ),
        )
        .await;
    let workspace = created["result"]["id"].as_str().expect("workspace id");
    let file = format!("/v2/workspaces/{workspace}/files/src/main.txt");
    let (status, created_file) = harness
        .call(
            "local:1000",
            Method::PUT,
            &file,
            "req_v2_write",
            mutation(
                "01JV2WRITEHTTPTEST00001",
                json!({
                    "content": { "encoding": "base64", "data": "aGVsbG8K" },
                    "expected": { "state": "absent" },
                    "create_parents": true
                }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let digest = created_file["result"]["after_sha256"]
        .as_str()
        .expect("after digest")
        .to_owned();
    assert_eq!(created_file["result"]["before_sha256"], Value::Null);

    let (status, tree) = harness
        .call(
            "local:1000",
            Method::GET,
            &format!("/v2/workspaces/{workspace}/tree?limit_items=10&include_hidden=false"),
            "req_v2_tree",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tree["result"]["truncated"], false);
    assert_eq!(
        tree["result"]["items"],
        json!([
            {"path": "src", "kind": "directory", "size": null},
            {"path": "src/main.txt", "kind": "file", "size": 6}
        ])
    );

    let (status, bounded) = harness
        .call(
            "local:1000",
            Method::GET,
            &format!("/v2/workspaces/{workspace}/tree?limit_items=1&include_hidden=false"),
            "req_v2_tree_bounded",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bounded["result"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(bounded["result"]["truncated"], true);

    let (status, read) = harness
        .call(
            "local:1000",
            Method::GET,
            &format!("{file}?mode=file&offset=0&limit_bytes=64"),
            "req_v2_read",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["result"]["sha256"], digest);
    assert_eq!(read["result"]["size"], 6);

    let edit = format!("/v2/workspaces/{workspace}/file-edits/src/main.txt");
    let (status, edited) = harness
        .call(
            "local:1000",
            Method::POST,
            &edit,
            "req_v2_edit",
            mutation(
                "01JV2EDITHTTPTEST000001",
                json!({
                    "expected_sha256": digest,
                    "old_text": "hello",
                    "new_text": "hello world",
                    "match_policy": "line_whitespace_normalized"
                }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        edited["result"]["diff"]["text"]
            .as_str()
            .expect("diff")
            .contains("+hello world")
    );

    let (status, stale) = harness
        .call(
            "local:1000",
            Method::POST,
            &edit,
            "req_v2_stale",
            mutation(
                "01JV2EDITHTTPTEST000002",
                json!({
                    "expected_sha256": read["result"]["sha256"],
                    "old_text": "hello world",
                    "new_text": "stale",
                    "match_policy": "exact"
                }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stale["error"]["code"], "workspace.stale-content");
}

#[tokio::test(flavor = "multi_thread")]
async fn strict_limits_and_path_escape_are_typed_before_dispatch() {
    let harness = Harness::open();
    let (status, query) = harness
        .call(
            "local:1000",
            Method::GET,
            "/v1/machine?unexpected=true",
            "req_query_test",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(query["error"]["code"], "request.schema-invalid");

    let (status, strict) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/workspaces",
            "req_strict_test",
            mutation(
                "01JPHASE2STRICTHTTPTEST",
                json!({ "source": "empty", "labels": {}, "secret": "forbidden" }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(strict["error"]["code"], "request.schema-invalid");

    let (status, malformed_git) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/workspaces",
            "req_git_shape_test",
            mutation(
                "01JPHASE2GITSHAPEHTTP1",
                json!({
                    "source": { "git": { "source": "", "ref": "main", "depth": 0 } },
                    "labels": {}
                }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(malformed_git["error"]["code"], "request.schema-invalid");

    let (status, escape) = harness
        .call(
            "local:1000",
            Method::GET,
            "/v1/workspaces/ws_missing/files/%2e%2e%2fetc%2fpasswd?mode=file&offset=0&limit_bytes=16",
            "req_escape_test",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(escape["error"]["code"], "workspace.path-escape");

    let oversized = Body::from(vec![b' '; 2_097_153]);
    let (status, limit) = harness
        .call(
            "local:1000",
            Method::POST,
            "/v1/workspaces",
            "req_body_limit_test",
            oversized,
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limit["error"]["code"], "request.body-limit");
}
