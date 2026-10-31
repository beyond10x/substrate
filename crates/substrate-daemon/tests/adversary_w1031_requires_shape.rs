//! Adversary cases for wave 2026-10-31 (design 30 § 4.4, ADR 0048 D4), claim 1 and claim 6 at the
//! daemon: "unknown names, duplicates, empty lists or an empty object are
//! `request.schema-invalid` (422)", and "the `requires` schema is closed".
//!
//! Bundle `0.26.0` states every one of those rules in the schema itself
//! (`schemas/common.json#/$defs/confinement-requires`: closed enums, `uniqueItems`, `minItems: 1`,
//! `minProperties: 1`, `type: array`). A request that breaks one is a request that does not match
//! the closed operation schema, and the daemon answers every other such request
//! `request.schema-invalid` before it looks at the snapshot. Here an unknown name does, while a
//! duplicate, an empty list and an empty object are answered `exec.capability-stale` when the
//! start also names a stale snapshot: the lexical half of `requires` is checked only after the
//! stale check. And a `null` list, which the schema refuses (`type: array`), is read as absent.
//!
//! The daemon runs on the real host driver over a minimum configuration; nothing here depends
//! on the lane, because every case is answered before any fact a lane decides is read, or names
//! a property (`sealed-paths`) the `linux` adapter never proves.
#![forbid(unsafe_code)]

use std::sync::Arc;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use substrate_daemon::{App, Identity, router};
use substrate_host::{Driver, HostConfig, HostDriver};
use substrate_store::Store;
use tempfile::TempDir;
use tower::ServiceExt as _;

const SUBJECT: &str = "local:1000";
const STALE: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

struct Harness {
    _directory: TempDir,
    app: Arc<App>,
    snapshot: String,
}

impl Harness {
    fn open() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("adversary-w1031-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .expect("fixture directory");
        std::fs::create_dir(directory.path().join("workspaces")).expect("workspace root");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver");
        let snapshot = driver.machine().snapshot;
        assert_ne!(snapshot, STALE);
        let app = App::new(store, driver as Arc<dyn Driver>, "dep_adversary_w1031");
        Self {
            _directory: directory,
            app,
            snapshot,
        }
    }

    async fn start(&self, operation: &str, snapshot: &str, requires: Value) -> (StatusCode, Value) {
        let input = json!({
            "workspace": "ws_vector",
            "argv": ["/usr/bin/true"],
            "env": { "allow": [], "set": {} },
            "sandbox": {
                "capability_snapshot": snapshot,
                "network": "none",
                "profile": "workspace",
                "require": true,
                "requires": requires
            },
            "limits": {
                "timeout_ms": 5000,
                "output_bytes": 65536,
                "processes": 16,
                "memory_bytes": 67_108_864,
                "cpu_millis": 1000
            },
            "wait": false
        });
        let body = serde_json::to_vec(&json!({ "op": operation, "input": input })).expect("JSON");
        let request = Request::builder()
            .method(Method::POST)
            .uri("/v1/execs")
            .header("content-type", "application/json")
            .header("x-request-id", "req_adversary_w1031")
            .body(Body::from(body))
            .expect("request");
        let identity = Identity {
            subject: SUBJECT.to_owned(),
            actor: "adversary-w1031".to_owned(),
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
        (status, serde_json::from_slice(&bytes).expect("JSON body"))
    }
}

/// Every `requires` the closed schema refuses is `request.schema-invalid`, whatever snapshot the
/// start names: the control (an unknown name) and the three lexical rules must answer alike.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_w1031_a_requires_outside_the_schema_is_schema_invalid_even_with_a_stale_snapshot()
 {
    let harness = Harness::open();
    let cases = [
        ("01JADVW1031REQSHAPE00001", json!({"families": ["no-such-family"]})),
        ("01JADVW1031REQSHAPE00002", json!({"families": ["wait", "wait"]})),
        ("01JADVW1031REQSHAPE00003", json!({"facts": ["whole-tree-kill", "whole-tree-kill"]})),
        ("01JADVW1031REQSHAPE00004", json!({"families": []})),
        ("01JADVW1031REQSHAPE00005", json!({})),
    ];
    let mut answered = Vec::new();
    for (operation, requires) in cases {
        let (status, body) = harness.start(operation, STALE, requires.clone()).await;
        answered.push(format!(
            "{requires} -> {} {}",
            status.as_u16(),
            body["error"]["code"]
        ));
    }
    let expected = [
        r#"{"families":["no-such-family"]} -> 422 "request.schema-invalid""#,
        r#"{"families":["wait","wait"]} -> 422 "request.schema-invalid""#,
        r#"{"facts":["whole-tree-kill","whole-tree-kill"]} -> 422 "request.schema-invalid""#,
        r#"{"families":[]} -> 422 "request.schema-invalid""#,
        r#"{} -> 422 "request.schema-invalid""#,
    ];
    assert_eq!(answered, expected);
}

/// `families: null` is outside the closed schema (`type: array`), so it is
/// `request.schema-invalid`; it must not be read as an absent list and the start checked on.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_w1031_a_null_requires_list_is_schema_invalid() {
    let harness = Harness::open();
    let snapshot = harness.snapshot.clone();
    let (status, body) = harness
        .start(
            "01JADVW1031REQNULL000001",
            &snapshot,
            json!({"families": null, "facts": ["sealed-paths"]}),
        )
        .await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            json!("request.schema-invalid")
        ),
        "{body}"
    );
}
