//! ADR 0036 S2 postures that only the crate can build: the development static-bearer TCP router
//! and the hosted TLS router. Scenario `host-process-refused-over-tcp-bearer-and-hosted-postures`.
//!
//! Both routers here belong to a daemon whose subject *is* the daemon's own uid, so the only
//! thing that can refuse is the posture itself: no grant, subject or body reaches past it.

use std::sync::Arc;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use substrate_host::{HostConfig, HostDriver};
use substrate_store::Store;
use tower::ServiceExt as _;

use super::super::{App, Identity, development_router, hosted_router};

fn app(directory: &tempfile::TempDir) -> Arc<App> {
    let store = Arc::new(Store::open(":memory:").expect("state store"));
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    App::new(store, driver, "dep_process_posture")
}

async fn call(
    router: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let identity = Identity {
        // The daemon's own uid: the posture, not the peer, is what is refused.
        subject: format!("local:{}", nix::unistd::geteuid().as_raw()),
        actor: "posture-test".to_owned(),
        principal: None,
    };
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-request-id", "req_process_posture")
        .body(body.map_or_else(Body::empty, |body| {
            Body::from(serde_json::to_vec(&body).expect("request JSON"))
        }))
        .expect("request");
    let response = router
        .layer(Extension(identity))
        .oneshot(request)
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_097_152)
        .await
        .expect("response bytes");
    (
        status,
        serde_json::from_slice(&bytes).expect("response JSON"),
    )
}

fn valid_start(operation: &str) -> Value {
    json!({
        "op": operation,
        "input": {
            "profile": "host",
            "argv": ["/bin/true"],
            "cwd": "/",
            "env": {},
            "owner": { "session": "s-1", "name": "dev" },
            "reason": "posture probe",
            "lease": { "ttl_ms": 60_000 }
        }
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_refused_over_tcp_bearer_and_hosted_postures() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let app = app(&directory);
    for (posture, index) in [("tcp-bearer", 0_u8), ("hosted-tls", 1_u8)] {
        let router = || {
            if index == 0 {
                development_router(Arc::clone(&app))
            } else {
                hosted_router(Arc::clone(&app), [7; 32])
            }
        };
        let (status, body) = call(
            router(),
            "POST",
            "/v1/processes",
            Some(valid_start(&format!("op_posture_start_{index:04}"))),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{posture}: {body}");
        assert_eq!(
            body["error"]["code"], "process.host-posture-refused",
            "{posture}: {body}"
        );
        for (method, uri, input) in [
            ("GET", "/v1/processes".to_owned(), None),
            ("GET", "/v1/processes/proc_missing".to_owned(), None),
            ("GET", "/v1/processes/proc_missing/logs".to_owned(), None),
            (
                "POST",
                "/v1/processes/proc_missing/stop".to_owned(),
                Some(json!({ "op": format!("op_posture_stop_{index:06}"), "input": {} })),
            ),
            (
                "POST",
                "/v1/processes/stop".to_owned(),
                Some(json!({ "owner_session": "s-1" })),
            ),
            (
                "POST",
                "/v1/processes/proc_missing/lease/renew".to_owned(),
                Some(json!({
                    "op": format!("op_posture_renew_{index:05}"),
                    "input": { "ttl_ms": 60_000 }
                })),
            ),
        ] {
            let (status, body) = call(router(), method, &uri, input).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{posture} {method} {uri}: {body}"
            );
            assert_eq!(
                body["error"]["code"], "process.host-posture-refused",
                "{posture} {method} {uri}: {body}"
            );
        }
    }
}
