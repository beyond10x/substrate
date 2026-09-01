#![forbid(unsafe_code)]
//! Sixth adversarial pass. The published refusal register's `retriable` column, read back off the
//! wire.
//!
//! `contracts/substrate-wire/0.10.0/refusals.json` is new in this unit (added by
//! `5c5637b fix(sessions): re-derive two enumerations, and close what they missed`) and its own
//! title is "Every refusal a session can raise, and what a client does with it". Whether a refusal
//! is worth retrying is one of its four columns, and it is the column a client acts on: a `429`
//! that says `retriable: true` is a backoff, and a `429` that says `retriable: false` is a stop.
//!
//! Nothing binds that column to what the daemon sends.
//! `every_refusal_a_pty_attach_can_raise_has_a_row_in_the_register`
//! (`crates/substrate-daemon/tests/pipe_session.rs:1841`) checks that the *code* has a row and
//! reads no other field of it, and `check_pty_refusal_class` (`xtask/src/bundle.rs`) ranges over
//! the code column alone.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use substrate_daemon::{App, Identity, router};
use substrate_host::{
    DispatchOutcome, Driver, DriverError, ExecObservation, HostConfig, HostDriver, PipeFrame,
    WorkspaceDestroyProgress,
};
use substrate_wire::{
    CapabilitySnapshot, ExecOutputQuery, ExecSignalInput, ExecStartInput, FileAbsence,
    FileObservation, FileReadQuery, FileReadResult, HostDriverKind, LeaseObservation, OutputSlice,
    PipeSessionStartInput, Workspace, WorkspaceCreateInput,
};
use tempfile::TempDir;
use tower::ServiceExt as _;

const SUBJECT: &str = "local:1000";
const DEPLOYMENT: &str = "dep_adversary6_register";
const SNAPSHOT: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// How the driver refuses one session start, in the shape the host driver refuses it.
#[derive(Clone, Copy)]
enum StartRefusal {
    /// `crates/substrate-host/src/process.rs:337-341` — the host refuses declared bounds above its
    /// profile with `DriverError::exhausted`, which is `retriable: true`
    /// (`crates/substrate-host/src/lib.rs:180-188`).
    LimitUnserved,
    /// `crates/substrate-host/src/process.rs:450-459` — `pty::open` failed, so the host's global
    /// pty count is full. Design 13: "Allocation failure is `exhausted` and retriable because the
    /// host's pty count is a global resource other tenants can fill and free."
    PtyExhausted,
}

impl StartRefusal {
    fn error(self) -> DriverError {
        match self {
            Self::LimitUnserved => DriverError::exhausted(
                substrate_wire::SESSION_LIMIT_UNSERVED,
                "Raw-pipe bounds exceed the host development profile.",
                "session",
            ),
            Self::PtyExhausted => DriverError::exhausted(
                substrate_wire::SESSION_PTY_EXHAUSTED,
                "pty allocation: EAGAIN",
                "session",
            ),
        }
    }
}

/// A driver that serves workspaces for real and refuses one session start exactly as the host
/// driver refuses it. Everything the start path reads before dispatch — the confinement facts and
/// `sessions.pty` — is published true, so the refusal under test is the one the client receives.
struct RefusingDriver {
    host: Arc<HostDriver>,
    refusal: StartRefusal,
}

impl RefusingDriver {
    fn open(root: &std::path::Path, refusal: StartRefusal) -> Arc<Self> {
        Arc::new(Self {
            host: HostDriver::open(HostConfig::minimum(root)).expect("host driver"),
            refusal,
        })
    }
}

#[async_trait]
impl Driver for RefusingDriver {
    async fn shutdown(&self) -> Result<(), DriverError> {
        self.host.shutdown().await
    }

    fn machine(&self) -> CapabilitySnapshot {
        let mut capability = self.host.machine();
        SNAPSHOT.clone_into(&mut capability.snapshot);
        capability.driver = HostDriverKind::Host;
        "adversary6-fixture".clone_into(&mut capability.driver_version);
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
        capability.facts.sessions_pty = Some(true);
        capability
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
    ) -> Result<FileReadResult, DriverError> {
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

    async fn start_exec(
        &self,
        id: &str,
        workspace_root_name: &str,
        input: &ExecStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        self.host.start_exec(id, workspace_root_name, input).await
    }

    async fn start_pipe_session(
        &self,
        _id: &str,
        _workspace_root_name: &str,
        _input: &PipeSessionStartInput,
    ) -> DispatchOutcome<ExecObservation> {
        DispatchOutcome::NotDispatched(self.refusal.error())
    }

    async fn write_pipe_session(&self, _id: &str, _bytes: &[u8]) -> Result<(), DriverError> {
        Err(DriverError::not_found())
    }

    async fn read_pipe_session(
        &self,
        _id: &str,
        _timeout: Duration,
    ) -> Result<Option<PipeFrame>, DriverError> {
        Err(DriverError::not_found())
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

    fn acknowledge_exec(&self, _persisted: &ExecObservation) {}

    fn discard_superseded_exec(&self, id: &str) {
        self.host.discard_superseded_exec(id);
    }
}

struct Harness {
    _directory: TempDir,
    app: Arc<App>,
}

impl Harness {
    fn open(refusal: StartRefusal) -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        let store = Arc::new(
            substrate_store::Store::open(directory.path().join("state.db")).expect("state store"),
        );
        let driver = RefusingDriver::open(&directory.path().join("workspaces"), refusal);
        let app = App::new(store, driver as Arc<dyn Driver>, DEPLOYMENT);
        Self {
            _directory: directory,
            app,
        }
    }

    async fn call(&self, method: Method, uri: &str, body: Body) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body)
            .expect("request");
        let response = router(Arc::clone(&self.app))
            .layer(Extension(Identity {
                subject: SUBJECT.to_owned(),
                actor: "adversary6".to_owned(),
                principal: None,
            }))
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2_097_152)
            .await
            .expect("response body");
        (
            status,
            serde_json::from_slice(&bytes).expect("JSON response"),
        )
    }

    async fn create_workspace(&self, operation: &str) -> String {
        let (status, workspace) = self
            .call(
                Method::POST,
                "/v1/workspaces",
                mutation(
                    operation,
                    json!({"source": "empty", "labels": {"fixture": "adversary6"}}),
                ),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{workspace}");
        workspace["result"]["id"]
            .as_str()
            .expect("workspace id")
            .to_owned()
    }
}

#[allow(clippy::needless_pass_by_value)]
fn mutation(operation: &str, input: Value) -> Body {
    Body::from(
        serde_json::to_vec(&json!({"op": operation, "input": input})).expect("mutation JSON"),
    )
}

fn pty_start(workspace: &str) -> Value {
    json!({
        "exec": {
            "workspace": workspace,
            "argv": ["/bin/sh"],
            "env": {"allow": [], "set": {}},
            "limits": {
                "timeout_ms": 10_000,
                "output_bytes": 1_048_576,
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
        },
        "input_limit_bytes": 1_048_576,
        "frame_limit_bytes": 65_536,
        "queued_frames": 16,
        "mode": "pty",
        "window": {"columns": 80, "rows": 24}
    })
}

/// The register's own row for one code.
fn register_row(code: &str) -> Value {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("contracts/substrate-wire/0.10.0/refusals.json");
    let register: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("the refusal register")).expect("JSON");
    register["refusals"]
        .as_array()
        .expect("register rows")
        .iter()
        .find(|row| row["code"].as_str() == Some(code))
        .cloned()
        .unwrap_or_else(|| panic!("{code} has no row in 0.10.0/refusals.json"))
}

/// A session refusal a client receives says what the published register says it says.
///
/// Both codes below are raised by `HostDriver` on a plain `POST /v1/sessions`: the first when
/// the body's declared `input_limit_bytes`/`frame_limit_bytes`/`queued_frames` are above the host
/// profile (`crates/substrate-host/src/process.rs:328-342`), the second when `pty::open` fails
/// because the host's pty count is full (`:448-461`). Both are `DriverError::exhausted`, which is
/// `retriable: true` at the port, and both reach the client through
/// `finish_pipe_session_dispatch_absence`
/// (`crates/substrate-daemon/src/app/operations.rs:1156-1183`), whose second statement is
/// `detail.retriable = false;` — applied to every driver refusal, unconditionally, before the body
/// is written and before the same detail is committed to the durable operation ledger.
///
/// So the register and the vector `contracts/substrate-wire/0.10.0/vectors/http/pty-session-exhausted.json`
/// publish `retriable: true` for a 429 the client is told is `retriable: false`. Design 13 states
/// the reason the true value is the true one: "Allocation failure is `exhausted` and retriable
/// because the host's pty count is a global resource other tenants can fill and free."
///
/// Portable lane.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_refusal_carries_the_retriable_the_register_publishes_for_it() {
    let mut wrong = Vec::new();
    for (refusal, operation) in [
        (StartRefusal::LimitUnserved, "01JADV6LIMITUNSERVED001"),
        (StartRefusal::PtyExhausted, "01JADV6PTYEXHAUSTED0001"),
    ] {
        let harness = Harness::open(refusal);
        let workspace = harness.create_workspace("01JADV6WORKSPACE00000001").await;
        let (status, body) = harness
            .call(
                Method::POST,
                "/v1/sessions",
                mutation(operation, pty_start(&workspace)),
            )
            .await;
        let code = body["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let row = register_row(&code);
        // The other three columns of the same row, so the failure isolates the one that drifts
        // rather than merely asserting that something did.
        for (column, published, sent) in [
            (
                "class",
                row["class"].clone(),
                body["error"]["class"].clone(),
            ),
            ("status", row["status"].clone(), json!(status.as_u16())),
            (
                "retriable",
                row["retriable"].clone(),
                body["error"]["retriable"].clone(),
            ),
        ] {
            if sent != published {
                wrong.push(format!(
                    "{code}: 0.10.0/refusals.json publishes {column} {published}, \
                     the daemon sends {column} {sent}"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "0.10.0/refusals.json says it gives every session refusal \"whether it is worth retrying\", \
         and a client acts on that column: a 429 that is retriable is a backoff and a 429 that is \
         not is a stop. These rows are contradicted by the response the daemon actually sends:\n{}",
        wrong.join("\n")
    );
}
