//! Adversarial end-to-end cases against ADR 0014. Added by the adversarial pass; no fixes here.
#![forbid(unsafe_code)]

use std::sync::Arc;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use substrate_daemon::{App, Identity, router};
use substrate_host::{HostConfig, HostDriver};
use substrate_store::Store;
use tempfile::TempDir;
use tower::ServiceExt as _;

struct Harness {
    _directory: TempDir,
    app: Arc<App>,
}

impl Harness {
    fn open() -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver");
        let app = App::new(store, driver, "dep_adversarial");
        Self {
            _directory: directory,
            app,
        }
    }

    async fn call(&self, method: Method, uri: &str, request_id: &str, body: Body) -> (u16, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-request-id", request_id)
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: "local:1000".to_owned(),
            actor: "adversarial".to_owned(),
            principal: None,
        };
        let response = router(Arc::clone(&self.app))
            .layer(Extension(identity))
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 2_097_152)
            .await
            .expect("response body");
        (
            status,
            serde_json::from_slice(&bytes).expect("JSON response"),
        )
    }
}

fn mutation(operation: &str, input: &Value) -> Body {
    Body::from(
        serde_json::to_vec(&json!({ "op": operation, "input": input })).expect("mutation JSON"),
    )
}

/// The request-side ceiling guard runs over raw client bytes before any name validation
/// (`crates/substrate-daemon/src/app/operations.rs:411` calls
/// `substrate_wire::validate_aperture_request` first), and it slices the first four *bytes* of
/// every `/`-separated term (`crates/substrate-wire/src/lib.rs:1941`). A name whose byte index 4
/// falls inside a multi-byte character is a `String` slice panic, not a refusal. Before ADR 0014
/// the same request was an ordinary `exec.aperture-name-invalid` at 422.
#[tokio::test(flavor = "multi_thread")]
async fn a_non_ascii_aperture_name_is_refused_rather_than_panicking_the_handler() {
    let harness = Harness::open();
    let (status, machine) = harness
        .call(Method::GET, "/v1/machine", "req_adv_machine", Body::empty())
        .await;
    assert_eq!(status, 200, "{machine}");
    let snapshot = machine["result"]["snapshot"]
        .as_str()
        .expect("capability snapshot")
        .to_owned();

    let (status, created) = harness
        .call(
            Method::POST,
            "/v1/workspaces",
            "req_adv_create",
            mutation(
                "01JPADVERSARIALCREATE01",
                &json!({ "source": "empty", "labels": {} }),
            ),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    let workspace = created["result"]["id"].as_str().expect("id").to_owned();

    let (status, refused) = harness
        .call(
            Method::POST,
            "/v1/execs",
            "req_adv_ceiling_utf8",
            mutation(
                "01JPADVERSARIALSTART001",
                &json!({
                    "workspace": workspace,
                    "argv": ["/usr/bin/true"],
                    "env": { "allow": [], "set": {} },
                    "sandbox": {
                        "capability_snapshot": snapshot,
                        "network": "aperture",
                        "aperture": "ab\u{20ac}cd",
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
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY.as_u16(),
        "{refused}"
    );
    assert_eq!(refused["error"]["code"], "exec.aperture-name-invalid");
}
