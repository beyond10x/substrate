//! Host processes (ADR 0036 S1-S7, S11), driven through the router.
//!
//! Each case is named for the conformance scenario of `spec/host-processes/` that the stories
//! `host-process-start`, `-cgroup-tree`, `-lease`, `-stop-by-owner`, `-listing`, `-logs` and
//! `-deadlines` name as their acceptance.
//!
//! **Two lanes.** The portable cases run on a daemon that serves no host process, as the
//! portable lane's daemon does: they prove the S2 refusals and every named start refusal, which
//! are decided before anything is recorded or dispatched. The delegated cases need a cgroup v2
//! subtree the test process is itself inside, which `scripts/delegated-lane.sh` provides through
//! `SUBSTRATE_VECTORS_CGROUP_ROOT`. Without it each of them returns before asserting anything:
//! absent, never reported as passed (AGENTS.md invariant 3).
//!
//! **One clock.** The daemon reads every host-process instant from its authority. The cases here
//! give it a wall clock they can move forward, so a lease expiry or a deadline is reached by
//! moving the clock and running one maintenance sweep, the way the daemon's own sweeper does every
//! 250 ms, rather than by sleeping through it.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use substrate_daemon::{App, Authority, Identity, SystemAuthority, router};
use substrate_host::host_process::{
    Confinement, EndedBy, HostProcessStarted, HostProcessSupervisor, HostProfile, LogTail, StopStep,
};
use substrate_host::{
    DriverError, HostConfig, HostDriver, HostProcessEnd, HostProcessObservation, HostProcessSpec,
    LogStreamSelection, ProcessExit, ProcessLogs, ProcessSupervisor, Reattached, StopCause,
    StoredProcessLogs,
};
use substrate_store::{LeaseClock, Store};
use tempfile::TempDir;
use tower::ServiceExt as _;

/// The daemon's own uid in these cases; the subject a host process is served to.
const UID: u32 = 4_242;
const SUBJECT: &str = "local:4242";
/// A peer of another uid.
const FOREIGN: &str = "local:4243";
const SECRET: &str = "s3cr3t-value-that-must-never-be-stored";

/// A wall clock the cases move forward; everything else is the system authority's.
#[derive(Default)]
struct MovableAuthority {
    offset_ms: AtomicI64,
}

impl MovableAuthority {
    fn advance(&self, ms: i64) {
        self.offset_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Authority for MovableAuthority {
    fn now(&self) -> DateTime<Utc> {
        Utc::now() + chrono::Duration::milliseconds(self.offset_ms.load(Ordering::SeqCst))
    }
    fn request_id(&self) -> String {
        SystemAuthority.request_id()
    }
    fn workspace_id(&self) -> String {
        SystemAuthority.workspace_id()
    }
    fn exec_id(&self) -> String {
        SystemAuthority.exec_id()
    }
    fn session_id(&self) -> String {
        SystemAuthority.session_id()
    }
    fn snapshot_id(&self) -> String {
        SystemAuthority.snapshot_id()
    }
    fn lease_clock(&self) -> Result<LeaseClock, String> {
        SystemAuthority.lease_clock()
    }
}

struct Harness {
    directory: TempDir,
    store: Arc<Store>,
    authority: Arc<MovableAuthority>,
    app: Arc<App>,
    /// The delegated cgroup this harness's driver owns, when it has one.
    cgroup: Option<PathBuf>,
}

impl Harness {
    fn build(directory: TempDir, cgroup: Option<PathBuf>) -> Self {
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let authority = Arc::new(MovableAuthority::default());
        let app = Self::app(&directory, &store, &authority, cgroup.as_deref());
        Self {
            directory,
            store,
            authority,
            app,
            cgroup,
        }
    }

    fn app(
        directory: &TempDir,
        store: &Arc<Store>,
        authority: &Arc<MovableAuthority>,
        cgroup: Option<&Path>,
    ) -> Arc<App> {
        let workspaces = directory.path().join("workspaces");
        std::fs::create_dir_all(&workspaces).expect("workspace root");
        let driver = HostDriver::open(HostConfig::minimum(workspaces)).expect("host driver");
        let mut app = App::with_authority(
            Arc::clone(store),
            driver,
            "dep_processes",
            Arc::clone(authority) as Arc<dyn Authority>,
        );
        let state = directory.path().join("state.processes");
        let supervisor = cgroup.map(|root| {
            HostProcessSupervisor::open(Some(root), &state)
                .expect("a delegated root serves host processes")
                as Arc<dyn ProcessSupervisor>
        });
        // As the composition root does: without a driver, the stored logs are still read.
        let logs = supervisor
            .is_none()
            .then(|| StoredProcessLogs::open(&state) as Arc<dyn ProcessLogs>);
        let configured = Arc::get_mut(&mut app).expect("unshared application");
        configured.host_process_uid = UID;
        configured.host_processes = supervisor;
        configured.host_process_logs = logs;
        app
    }

    /// A daemon that serves no host process: no delegated cgroup root.
    fn portable() -> Self {
        Self::build(tempfile::tempdir().expect("temporary harness"), None)
    }

    /// A daemon over a private child of the delegated subtree, or nothing: the case is absent.
    fn delegated(name: &str) -> Option<Self> {
        let root = PathBuf::from(std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")?);
        let path = root.join(format!("daemon-proc-{name}"));
        if path.is_dir() {
            let _replaced = std::fs::remove_dir(&path);
        }
        std::fs::create_dir(&path).ok()?;
        let controllers = "+cpu +memory +pids";
        std::fs::write(root.join("cgroup.subtree_control"), controllers).ok()?;
        std::fs::write(path.join("cgroup.subtree_control"), controllers).ok()?;
        Some(Self::build(
            tempfile::tempdir().expect("temporary harness"),
            Some(path),
        ))
    }

    /// The same durable state and cgroup tree under a fresh daemon run: a new driver that holds
    /// nothing in memory, and a restart cutoff of now.
    fn restart(&mut self) {
        self.app = Self::app(
            &self.directory,
            &self.store,
            &self.authority,
            self.cgroup.as_deref(),
        );
    }

    fn workdir(&self, name: &str) -> PathBuf {
        let path = self.directory.path().join("work").join(name);
        std::fs::create_dir_all(&path).expect("caller working directory");
        path.canonicalize().expect("canonical working directory")
    }

    fn log_dir(&self) -> PathBuf {
        self.directory.path().join("state.processes")
    }

    async fn call(
        &self,
        subject: &str,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-request-id", "req_processes")
            .body(body.map_or_else(Body::empty, |body| {
                Body::from(serde_json::to_vec(&body).expect("request JSON"))
            }))
            .expect("request");
        let identity = Identity {
            subject: subject.to_owned(),
            actor: "process-test".to_owned(),
            principal: None,
        };
        let response = router(Arc::clone(&self.app))
            .layer(Extension(identity))
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 16 * 1_048_576)
            .await
            .expect("response body");
        (
            status,
            serde_json::from_slice(&bytes).expect("JSON response"),
        )
    }

    async fn start(&self, operation: &str, input: Value) -> (StatusCode, Value) {
        self.call(
            SUBJECT,
            Method::POST,
            "/v1/processes",
            Some(json!({ "op": operation, "input": input })),
        )
        .await
    }

    async fn started(&self, operation: &str, input: Value) -> String {
        let (status, body) = self.start(operation, input).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        text(&body["result"]["id"]).to_owned()
    }

    async fn get(&self, id: &str) -> Value {
        let (status, body) = self
            .call(SUBJECT, Method::GET, &format!("/v1/processes/{id}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["result"].clone()
    }

    async fn list(&self, query: &str) -> Value {
        let (status, body) = self
            .call(SUBJECT, Method::GET, &format!("/v1/processes{query}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["result"].clone()
    }

    async fn stop(&self, operation: &str, id: &str) -> (StatusCode, Value) {
        self.call(
            SUBJECT,
            Method::POST,
            &format!("/v1/processes/{id}/stop"),
            Some(json!({ "op": operation, "input": {} })),
        )
        .await
    }

    async fn logs(&self, id: &str, query: &str) -> Value {
        let (status, body) = self
            .call(
                SUBJECT,
                Method::GET,
                &format!("/v1/processes/{id}/logs{query}"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["result"].clone()
    }

    async fn sweep(&self) {
        self.app.sweep_expired().await;
    }

    /// Waits for the record to end, running the maintenance sweep as the daemon does.
    async fn ended(&self, id: &str) -> Value {
        for _ in 0..400 {
            self.sweep().await;
            let record = self.get(id).await;
            if record["state"] == "ended" {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{id} did not end: {}", self.get(id).await);
    }

    /// Waits until the record's log holds a line containing `needle`, and returns that line.
    async fn log_line(&self, id: &str, needle: &str) -> String {
        for _ in 0..200 {
            let tail = self.logs(id, "").await;
            if let Some(line) = tail["lines"]
                .as_array()
                .expect("lines")
                .iter()
                .map(|line| text(&line["text"]).to_owned())
                .find(|line| line.contains(needle))
            {
                return line;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{id} never logged {needle:?}: {}", self.logs(id, "").await);
    }

    /// The operations the ledger holds for one kind, as `(actor, state, resource)`.
    fn operations(&self, kind: &str) -> Vec<(String, String, Option<String>)> {
        let connection =
            rusqlite::Connection::open(self.directory.path().join("state.db")).expect("ledger");
        let mut statement = connection
            .prepare(
                "SELECT actor, state, resource FROM operations WHERE operation_kind = ?1
                 ORDER BY accepted_at, operation",
            )
            .expect("ledger query");
        statement
            .query_map([kind], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("ledger rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("ledger rows")
    }

    /// Every byte of the durable state the daemon wrote: the database and its journals.
    fn durable_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for entry in std::fs::read_dir(self.directory.path()).expect("state directory") {
            let path = entry.expect("entry").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if name.starts_with("state.db") && path.is_file() {
                bytes.extend(std::fs::read(&path).expect("state bytes"));
            }
        }
        bytes
    }

    /// Ends everything this harness still runs and removes its cgroup.
    async fn finish(self) {
        let (status, _) = self
            .call(
                SUBJECT,
                Method::POST,
                "/v1/processes/stop",
                Some(json!({ "owner_session": "s-1" })),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        for session in ["s-2", "s-3"] {
            let _ = self
                .call(
                    SUBJECT,
                    Method::POST,
                    "/v1/processes/stop",
                    Some(json!({ "owner_session": session })),
                )
                .await;
        }
        // A stop answers the record `stopping`; the supervisor clock records each end.
        let stopping = self.list("?state=stopping").await;
        for item in stopping["items"].as_array().cloned().unwrap_or_default() {
            self.ended(text(&item["id"])).await;
        }
        if let Some(cgroup) = &self.cgroup {
            for _ in 0..100 {
                if std::fs::remove_dir(cgroup).is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
}

fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("JSON string: {value}"))
}

fn input(argv: &[&str], cwd: &Path, session: &str, name: &str) -> Value {
    json!({
        "profile": "host",
        "argv": argv,
        "cwd": cwd.to_str().expect("utf-8 cwd"),
        "env": { "PATH": "/usr/bin:/bin" },
        "owner": { "session": session, "name": name },
        "reason": "serve the preview",
        "lease": { "ttl_ms": 60_000 }
    })
}

fn with(mut base: Value, member: &str, value: Value) -> Value {
    base[member] = value;
    base
}

fn without(mut base: Value, member: &str) -> Value {
    base.as_object_mut().expect("input object").remove(member);
    base
}

fn instant(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn pid_gone(pid: u32) -> bool {
    let path = PathBuf::from(format!("/proc/{pid}/stat"));
    std::fs::read_to_string(path).map_or(true, |stat| {
        // A zombie awaiting its reaper runs nothing.
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| rest.starts_with('Z'))
    })
}

async fn eventually_gone(pid: u32) -> bool {
    for _ in 0..100 {
        if pid_gone(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

fn assert_refused(observed: &(StatusCode, Value), status: StatusCode, code: &str) {
    assert_eq!(observed.0, status, "{code}: {}", observed.1);
    assert_eq!(observed.1["error"]["code"], code, "{}", observed.1);
}

// ---------------------------------------------------------------------------------------------
// Portable: S2 admission and every named start refusal, on a daemon that serves no host process.
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn host_process_start_without_profile_is_refused() {
    let harness = Harness::portable();
    let cwd = harness.workdir("profile");
    let request = without(input(&["/bin/true"], &cwd, "s-1", "dev"), "profile");
    let observed = harness
        .start("op_profile_required_01", request.clone())
        .await;
    assert_refused(
        &observed,
        StatusCode::UNPROCESSABLE_ENTITY,
        "process.profile-required",
    );
    // The refusal is a durable ledger row: a retry replays it.
    let replayed = harness.start("op_profile_required_01", request).await;
    assert_eq!(replayed.1["error"], observed.1["error"]);
    assert_eq!(harness.list("").await["items"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_unknown_profile_is_refused() {
    let harness = Harness::portable();
    let cwd = harness.workdir("profile");
    for (index, profile) in [json!("workspace"), json!("HOST"), json!(5), json!(null)]
        .into_iter()
        .enumerate()
    {
        let request = with(
            input(&["/bin/true"], &cwd, "s-1", "dev"),
            "profile",
            profile,
        );
        let observed = harness
            .start(&format!("op_profile_unknown_{index:02}"), request)
            .await;
        assert_refused(
            &observed,
            StatusCode::UNPROCESSABLE_ENTITY,
            "process.profile-unknown",
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_invalid_argv_cwd_env_owner_are_refused() {
    let harness = Harness::portable();
    let cwd = harness.workdir("lexical");
    let base = input(&["/bin/true"], &cwd, "s-1", "dev");
    let link = harness.directory.path().join("link");
    std::os::unix::fs::symlink(&cwd, &link).expect("symlink");
    let too_many: Vec<String> = (0..=substrate_wire::MAX_PROCESS_ARGV_ITEMS)
        .map(|_| "x".to_owned())
        .collect();
    let cases = [
        (without(base.clone(), "argv"), "process.argv-invalid"),
        (
            with(base.clone(), "argv", json!([])),
            "process.argv-invalid",
        ),
        (
            with(base.clone(), "argv", json!([""])),
            "process.argv-invalid",
        ),
        (
            with(base.clone(), "argv", json!(["/bin/echo", "a\u{0}b"])),
            "process.argv-invalid",
        ),
        (
            with(base.clone(), "argv", json!(too_many)),
            "process.argv-invalid",
        ),
        (
            with(base.clone(), "argv", json!("/bin/true")),
            "process.argv-invalid",
        ),
        (without(base.clone(), "cwd"), "process.cwd-invalid"),
        (
            with(base.clone(), "cwd", json!("relative/dir")),
            "process.cwd-invalid",
        ),
        (
            with(base.clone(), "cwd", json!("/nonexistent-substrate-cwd")),
            "process.cwd-invalid",
        ),
        (
            with(base.clone(), "cwd", json!(link.to_str().expect("utf-8"))),
            "process.cwd-invalid",
        ),
        (
            with(base.clone(), "cwd", json!(format!("{}/.", cwd.display()))),
            "process.cwd-invalid",
        ),
        (without(base.clone(), "env"), "process.env-invalid"),
        (
            with(base.clone(), "env", json!({ "1BAD": "x" })),
            "process.env-invalid",
        ),
        (
            with(base.clone(), "env", json!({ "A=B": "x" })),
            "process.env-invalid",
        ),
        (
            with(base.clone(), "env", json!({ "GOOD": 5 })),
            "process.env-invalid",
        ),
        (
            with(base.clone(), "env", json!({ "GOOD": "a\u{0}b" })),
            "process.env-invalid",
        ),
        (without(base.clone(), "owner"), "process.owner-invalid"),
        (
            with(
                base.clone(),
                "owner",
                json!({ "session": "", "name": "dev" }),
            ),
            "process.owner-invalid",
        ),
        (
            with(base.clone(), "owner", json!({ "session": "s-1" })),
            "process.owner-invalid",
        ),
        (
            with(
                base.clone(),
                "owner",
                json!({ "session": "s".repeat(substrate_wire::MAX_PROCESS_OWNER_BYTES + 1), "name": "dev" }),
            ),
            "process.owner-invalid",
        ),
    ];
    for (index, (request, code)) in cases.into_iter().enumerate() {
        let observed = harness
            .start(&format!("op_lexical_refusal_{index:03}"), request)
            .await;
        assert_refused(&observed, StatusCode::UNPROCESSABLE_ENTITY, code);
    }
    assert_eq!(harness.list("").await["items"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_start_without_reason_is_refused() {
    let harness = Harness::portable();
    let cwd = harness.workdir("reason");
    let base = input(&["/bin/true"], &cwd, "s-1", "dev");
    for (index, request) in [
        without(base.clone(), "reason"),
        with(base.clone(), "reason", json!("")),
        with(base.clone(), "reason", json!("   ")),
        with(
            base.clone(),
            "reason",
            json!("r".repeat(substrate_wire::MAX_PROCESS_REASON_BYTES + 1)),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let observed = harness
            .start(&format!("op_reason_required_{index:02}"), request)
            .await;
        assert_refused(
            &observed,
            StatusCode::UNPROCESSABLE_ENTITY,
            "process.reason-required",
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_keep_without_reason_is_refused() {
    let harness = Harness::portable();
    let cwd = harness.workdir("keep");
    let base = input(&["/bin/true"], &cwd, "s-1", "dev");
    for (index, lease) in [
        json!({ "keep": true }),
        json!({ "keep": true, "keep_reason": "" }),
    ]
    .into_iter()
    .enumerate()
    {
        let observed = harness
            .start(
                &format!("op_keep_reason_{index:04}"),
                with(base.clone(), "lease", lease),
            )
            .await;
        assert_refused(
            &observed,
            StatusCode::UNPROCESSABLE_ENTITY,
            "process.keep-reason-required",
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_ttl_out_of_bounds_is_refused() {
    let harness = Harness::portable();
    let cwd = harness.workdir("ttl");
    let base = input(&["/bin/true"], &cwd, "s-1", "dev");
    for (index, lease) in [
        json!({ "ttl_ms": substrate_wire::MIN_LEASE_TTL_MS - 1 }),
        json!({ "ttl_ms": substrate_wire::MAX_LEASE_TTL_MS + 1 }),
        json!({ "ttl_ms": 1500.5 }),
        json!({ "ttl_ms": -1 }),
        json!({ "ttl_ms": 60_000, "keep": true, "keep_reason": "both" }),
        json!({ "ttl_ms": 60_000, "keep_reason": "a reason with no keep" }),
        json!({}),
        json!("60000"),
    ]
    .into_iter()
    .enumerate()
    {
        let observed = harness
            .start(
                &format!("op_ttl_bounds_{index:06}"),
                with(base.clone(), "lease", lease),
            )
            .await;
        assert_refused(
            &observed,
            StatusCode::UNPROCESSABLE_ENTITY,
            "process.lease-invalid",
        );
    }
    let observed = harness
        .start("op_ttl_bounds_absent", without(base, "lease"))
        .await;
    assert_refused(
        &observed,
        StatusCode::UNPROCESSABLE_ENTITY,
        "process.lease-invalid",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_invalid_timeout_is_refused() {
    let harness = Harness::portable();
    let cwd = harness.workdir("timeout");
    let base = input(&["/bin/true"], &cwd, "s-1", "dev");
    for (index, timeout) in [
        json!(0),
        json!(substrate_wire::MAX_PROCESS_TIMEOUT_MS + 1),
        json!(1.5),
        json!(-5),
        json!("1000"),
    ]
    .into_iter()
    .enumerate()
    {
        let observed = harness
            .start(
                &format!("op_timeout_invalid_{index:02}"),
                with(base.clone(), "timeout_ms", timeout),
            )
            .await;
        assert_refused(
            &observed,
            StatusCode::UNPROCESSABLE_ENTITY,
            "process.timeout-invalid",
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_invalid_expires_at_is_refused() {
    let harness = Harness::portable();
    let cwd = harness.workdir("expires");
    let base = input(&["/bin/true"], &cwd, "s-1", "dev");
    let now = harness.authority.now();
    let max = i64::try_from(substrate_wire::MAX_PROCESS_TIMEOUT_MS).expect("bound");
    for (index, expires_at) in [
        json!("tomorrow"),
        json!(5),
        // Not UTC.
        json!(
            (now + chrono::Duration::minutes(5))
                .with_timezone(&chrono::FixedOffset::east_opt(3_600).expect("offset"))
                .to_rfc3339()
        ),
        // Nearer than the minimum lease TTL.
        json!(instant(now + chrono::Duration::milliseconds(100))),
        // In the past.
        json!(instant(now - chrono::Duration::minutes(1))),
        // Beyond the process timeout bound.
        json!(instant(now + chrono::Duration::milliseconds(max + 60_000))),
    ]
    .into_iter()
    .enumerate()
    {
        let observed = harness
            .start(
                &format!("op_expires_invalid_{index:02}"),
                with(base.clone(), "expires_at", expires_at),
            )
            .await;
        assert_refused(
            &observed,
            StatusCode::UNPROCESSABLE_ENTITY,
            "process.expires-at-invalid",
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_refused_for_a_foreign_peer_uid() {
    let harness = Harness::portable();
    let cwd = harness.workdir("foreign");
    let request = json!({
        "op": "op_foreign_peer_start",
        "input": input(&["/bin/true"], &cwd, "s-1", "dev")
    });
    let observed = harness
        .call(FOREIGN, Method::POST, "/v1/processes", Some(request))
        .await;
    assert_refused(
        &observed,
        StatusCode::FORBIDDEN,
        "process.host-subject-mismatch",
    );
    for (method, uri, body) in [
        (Method::GET, "/v1/processes", None),
        (Method::GET, "/v1/processes/proc_any", None),
        (Method::GET, "/v1/processes/proc_any/logs", None),
        (
            Method::POST,
            "/v1/processes/proc_any/stop",
            Some(json!({ "op": "op_foreign_peer_stop", "input": {} })),
        ),
        (
            Method::POST,
            "/v1/processes/stop",
            Some(json!({ "owner_session": "s-1" })),
        ),
        (
            Method::POST,
            "/v1/processes/proc_any/lease/renew",
            Some(json!({ "op": "op_foreign_peer_renew", "input": { "ttl_ms": 60_000 } })),
        ),
    ] {
        let observed = harness.call(FOREIGN, method.clone(), uri, body).await;
        assert_refused(
            &observed,
            StatusCode::FORBIDDEN,
            "process.host-subject-mismatch",
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_unserved_without_delegated_cgroup() {
    let harness = Harness::portable();
    let cwd = harness.workdir("unserved");
    let observed = harness
        .start(
            "op_unserved_start_01",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    assert_refused(
        &observed,
        StatusCode::NOT_IMPLEMENTED,
        "process.host-unserved",
    );
    assert_eq!(harness.list("").await["items"], json!([]));
    // Nothing ran: the refusal is one refused operation naming no resource, and no record or log
    // exists.
    assert_eq!(
        harness.operations("process.start"),
        [("process-test".to_owned(), "refused".to_owned(), None)]
    );
    assert!(!harness.log_dir().exists());
    let (status, machine) = harness
        .call(SUBJECT, Method::GET, "/v1/machine", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        machine["result"]["facts"].get("process.host").is_none(),
        "an unserved daemon publishes no process.host: {machine}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn exec_sandbox_unavailable_never_becomes_a_host_process() {
    let harness = Harness::portable();
    let (_, machine) = harness
        .call(SUBJECT, Method::GET, "/v1/machine", None)
        .await;
    let snapshot = text(&machine["result"]["snapshot"]).to_owned();
    let (status, created) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/workspaces",
            Some(json!({
                "op": "op_sandbox_workspace_1",
                "input": { "source": "empty", "labels": {} }
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let workspace = text(&created["result"]["id"]).to_owned();
    let observed = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/execs",
            Some(json!({
                "op": "op_sandbox_unavailable_1",
                "input": {
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
                }
            })),
        )
        .await;
    assert_refused(
        &observed,
        StatusCode::NOT_IMPLEMENTED,
        "exec.sandbox-unavailable",
    );
    // A confined request that cannot be confined is refused, never run as a host process.
    assert_eq!(harness.list("").await["items"], json!([]));
    assert!(harness.operations("process.start").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stop_of_unknown_id_is_refused() {
    // A daemon that serves host processes: on one that does not, every stop is unserved
    // (host_process_stop_and_renew_without_process_host_are_unserved).
    let (harness, _fake) = Harness::faked();
    let observed = harness.stop("op_unknown_stop_0001", "proc_absent").await;
    assert_refused(&observed, StatusCode::NOT_FOUND, "process.not-found");
    let (status, body) = harness
        .call(SUBJECT, Method::GET, "/v1/processes/proc_absent", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "process.not-found");
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stop_by_owner_without_process_host_is_unserved() {
    // Bundle 0.20.0: process.stop-by-owner requires process.host and declares
    // process.host-unserved (501); a daemon that serves no host process never answers it with an
    // empty list. Its body is its input, so it has no operation to record.
    let harness = Harness::portable();
    let observed = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/processes/stop",
            Some(json!({ "owner_session": "s-1" })),
        )
        .await;
    assert_refused(
        &observed,
        StatusCode::NOT_IMPLEMENTED,
        "process.host-unserved",
    );
    assert!(harness.operations("process.stop").is_empty());
    assert!(harness.operations("process.stop-by-owner").is_empty());
}

// ---------------------------------------------------------------------------------------------
// Portable, behind the port: bundle 0.20.0's route shapes, on a driver that runs nothing.
// ---------------------------------------------------------------------------------------------

/// A host-process driver behind [`ProcessSupervisor`] that runs nothing: it records what the
/// daemon dispatched and which stops it began, and publishes a stop's end once one was begun.
/// The daemon cannot tell it from the cgroup driver, which is what the port promises.
#[derive(Default)]
struct FakeSupervisor {
    started: parking_lot::Mutex<Vec<String>>,
    stops: parking_lot::Mutex<Vec<(String, StopCause)>>,
}

impl FakeSupervisor {
    fn stops(&self) -> Vec<(String, StopCause)> {
        self.stops.lock().clone()
    }
}

impl ProcessLogs for FakeSupervisor {
    fn log_tail(
        &self,
        _id: &str,
        _selection: LogStreamSelection,
        _max_bytes: u64,
    ) -> Result<LogTail, DriverError> {
        Ok(LogTail {
            lines: Vec::new(),
            discarded_bytes: 0,
            retained_bytes: 0,
            profile: HostProfile::Host,
            confinement: Confinement::None,
        })
    }
}

impl ProcessSupervisor for FakeSupervisor {
    fn start(
        self: Arc<Self>,
        id: &str,
        spec: &HostProcessSpec,
    ) -> Result<HostProcessStarted, DriverError> {
        self.started.lock().push(id.to_owned());
        Ok(HostProcessStarted {
            id: id.to_owned(),
            pid: 1,
            cgroup: format!("fake-{id}"),
            started_at: Utc::now(),
            deadline: spec.deadline,
            profile: HostProfile::Host,
            confinement: Confinement::None,
        })
    }

    fn reattach(self: Arc<Self>, _id: &str) -> Result<Reattached, DriverError> {
        Ok(Reattached::Live)
    }

    fn begin_stop(&self, id: &str, cause: StopCause) -> Result<(), DriverError> {
        self.stops.lock().push((id.to_owned(), cause));
        Ok(())
    }

    fn ended(&self, id: &str) -> Result<Option<HostProcessEnd>, DriverError> {
        Ok(self
            .stops
            .lock()
            .iter()
            .any(|(stopped, _)| stopped == id)
            .then(|| HostProcessEnd {
                ended_by: EndedBy::Stop,
                exit: Some(ProcessExit::Signal(15)),
                stop_step: Some(StopStep::Term),
                ended_at: Utc::now(),
            }))
    }

    fn observe(&self, _id: &str) -> Result<HostProcessObservation, DriverError> {
        Ok(HostProcessObservation {
            observed_at: Utc::now(),
            cpu_usec: None,
            memory_bytes: None,
            listening: None,
            profile: HostProfile::Host,
            confinement: Confinement::None,
        })
    }

    fn forget(&self, _id: &str) -> Result<(), DriverError> {
        Ok(())
    }
}

impl Harness {
    /// A daemon whose host-process driver is `FakeSupervisor`, behind the port.
    fn faked() -> (Self, Arc<FakeSupervisor>) {
        let mut harness = Self::portable();
        let fake = Arc::new(FakeSupervisor::default());
        Arc::get_mut(&mut harness.app)
            .expect("unshared application")
            .host_processes = Some(Arc::clone(&fake) as Arc<dyn ProcessSupervisor>);
        (harness, fake)
    }

    /// The same durable state under a daemon run that serves no host process: `process.host` is
    /// false, and the records an earlier run left are still known.
    fn unserve(&mut self) {
        self.app = Self::app(&self.directory, &self.store, &self.authority, None);
    }

    async fn renew(&self, operation: &str, id: &str, input: Value) -> (StatusCode, Value) {
        self.call(
            SUBJECT,
            Method::POST,
            &format!("/v1/processes/{id}/lease/renew"),
            Some(json!({ "op": operation, "input": input })),
        )
        .await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_logs_of_a_known_record_need_no_process_host() {
    let (mut harness, _fake) = Harness::faked();
    let cwd = harness.workdir("logs-unserved");
    let id = harness
        .started(
            "op_logs_unserved_st01",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    harness.unserve();
    // Bundle 0.20.0 routes.json: process.logs.get has no capability predicate, so the stored log
    // of a known record is served whether or not this run serves host processes.
    let (status, body) = harness
        .call(
            SUBJECT,
            Method::GET,
            &format!("/v1/processes/{id}/logs"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["process"], id.as_str());
    assert_eq!(body["result"]["lines"], json!([]));
    assert_eq!(body["result"]["profile"], "host");
    assert_eq!(body["result"]["confinement"], "none");
    // An unknown record is still not found, never unserved.
    let observed = harness
        .call(SUBJECT, Method::GET, "/v1/processes/proc_absent/logs", None)
        .await;
    assert_refused(&observed, StatusCode::NOT_FOUND, "process.not-found");
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stop_and_renew_without_process_host_are_unserved() {
    let (mut harness, _fake) = Harness::faked();
    let cwd = harness.workdir("stop-unserved");
    let id = harness
        .started(
            "op_stop_unserved_st01",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let before = harness.get(&id).await;
    harness.unserve();
    // Bundle 0.20.0 routes.json: process.stop and process.lease.renew require process.host. Their
    // input schemas declare no refusal for it, so they answer the one process.start declares for
    // the same unmet predicate.
    for (operation, target) in [
        ("op_stop_unserved_0001", id.as_str()),
        ("op_stop_unserved_0002", "proc_absent"),
    ] {
        let observed = harness.stop(operation, target).await;
        assert_refused(
            &observed,
            StatusCode::NOT_IMPLEMENTED,
            "process.host-unserved",
        );
    }
    for (operation, target) in [
        ("op_renew_unserved_001", id.as_str()),
        ("op_renew_unserved_002", "proc_absent"),
    ] {
        let observed = harness
            .renew(operation, target, json!({ "ttl_ms": 120_000 }))
            .await;
        assert_refused(
            &observed,
            StatusCode::NOT_IMPLEMENTED,
            "process.host-unserved",
        );
    }
    // Nothing moved: the record is as it was, and every request is one refused operation.
    let after = harness.get(&id).await;
    assert_eq!(after["state"], before["state"]);
    assert_eq!(after["lease"], before["lease"]);
    for kind in ["process.stop", "process.lease.renew"] {
        let states: Vec<String> = harness
            .operations(kind)
            .into_iter()
            .map(|(_, state, _)| state)
            .collect();
        assert_eq!(states, ["refused", "refused"], "{kind}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_renew_out_of_schema_ttl_is_schema_invalid() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("renew-schema");
    let id = harness
        .started(
            "op_renew_schema_st001",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    // Bundle 0.20.0 schemas/inputs/process-lease-renew.json is closed, with ttl_ms an integer in
    // 1000..=86400000, and declares no process.lease-invalid: anything else is the strict schema's
    // refusal.
    for (operation, renewal) in [
        ("op_renew_schema_00001", json!({ "ttl_ms": 999 })),
        ("op_renew_schema_00002", json!({ "ttl_ms": 86_400_001 })),
        ("op_renew_schema_00003", json!({ "ttl_ms": "60000" })),
        ("op_renew_schema_00004", json!({})),
        (
            "op_renew_schema_00005",
            json!({ "ttl_ms": 60_000, "keep": true }),
        ),
    ] {
        let observed = harness.renew(operation, &id, renewal.clone()).await;
        assert_refused(
            &observed,
            StatusCode::UNPROCESSABLE_ENTITY,
            "request.schema-invalid",
        );
    }
    let (status, body) = harness
        .renew("op_renew_schema_00006", &id, json!({ "ttl_ms": 120_000 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["lease"]["ttl_ms"], 120_000);
}

/// `^proc_[A-Za-z0-9]+$`, bundle 0.20.0 `common.json#/$defs/process-id`.
fn is_process_id(id: &str) -> bool {
    id.strip_prefix("proc_").is_some_and(|rest| {
        !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_ids_are_proc_and_ascii_alphanumerics() {
    for _ in 0..16 {
        let minted = SystemAuthority.process_id();
        assert!(is_process_id(&minted), "{minted}");
    }
    let (harness, fake) = Harness::faked();
    let cwd = harness.workdir("ids");
    let id = harness
        .started(
            "op_proc_id_start_0001",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    assert!(is_process_id(&id), "{id}");
    assert_eq!(*fake.started.lock(), vec![id]);
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_start_answers_200_with_the_record() {
    let (harness, _fake) = Harness::faked();
    let cwd = harness.workdir("start");
    let (status, body) = harness
        .start(
            "op_start_status_00001",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    // Bundle 0.20.0 vectors/http/process-start.json: 200, the operation and the running record.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["operation"], "op_start_status_00001");
    assert_eq!(body["result"]["state"], "running");
    assert_eq!(body["result"]["kind"], "process");
    assert_eq!(body["result"]["profile"], "host");
    assert_eq!(body["result"]["confinement"], "none");
    // A replay answers the same status and record.
    let (replayed, again) = harness
        .start(
            "op_start_status_00001",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    assert_eq!(replayed, StatusCode::OK, "{again}");
    assert_eq!(again["result"]["id"], body["result"]["id"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stop_answers_the_stopping_record() {
    let (harness, fake) = Harness::faked();
    let cwd = harness.workdir("stop");
    let id = harness
        .started(
            "op_stop_answer_start1",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let (status, body) = harness.stop("op_stop_answer_stop01", &id).await;
    // Bundle 0.20.0 vectors/http/process-stop.json: 200 with the record in `stopping`; the end
    // is the supervisor's to observe, not the request's to wait for.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["operation"], "op_stop_answer_stop01");
    assert_eq!(body["result"]["id"], id.as_str());
    assert_eq!(body["result"]["state"], "stopping", "{body}");
    assert!(body["result"].get("ended_by").is_none(), "{body}");
    assert_eq!(fake.stops(), vec![(id.clone(), StopCause::Stop)]);
    // The replay answers the same record.
    let (_, replayed) = harness.stop("op_stop_answer_stop01", &id).await;
    assert_eq!(replayed["result"], body["result"]);
    // The supervisor clock records the end the driver published.
    let ended = harness.ended(&id).await;
    assert_eq!(ended["ended_by"], "stop");
    // Stopping an ended record answers it unchanged.
    let (status, again) = harness.stop("op_stop_answer_stop02", &id).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["result"]["state"], "ended");
    assert_eq!(fake.stops().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stop_by_owner_takes_its_input_as_the_body() {
    let (harness, fake) = Harness::faked();
    let cwd = harness.workdir("owner");
    let first = harness
        .started(
            "op_owner_body_start01",
            input(&["/bin/true"], &cwd, "s-1", "a"),
        )
        .await;
    let second = harness
        .started(
            "op_owner_body_start02",
            input(&["/bin/true"], &cwd, "s-1", "b"),
        )
        .await;
    let other = harness
        .started(
            "op_owner_body_start03",
            input(&["/bin/true"], &cwd, "s-2", "a"),
        )
        .await;
    // The keyed envelope is not this route's input: bundle 0.20.0 binds the whole body.
    let (status, refused) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/processes/stop",
            Some(json!({ "op": "op_owner_body_keyed1", "input": { "owner_session": "s-1" } })),
        )
        .await;
    // request.schema-invalid is 422 on every route of this daemon (app/responses.rs schema_invalid).
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "request.schema-invalid");
    assert!(fake.stops().is_empty());
    // Bundle 0.20.0 vectors/http/process-stop-by-owner.json: 200, no operation of its own, and
    // one process.stop operation per record.
    let (status, body) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/processes/stop",
            Some(json!({ "owner_session": "s-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("operation").is_none(), "{body}");
    assert_eq!(body["result"]["owner_session"], "s-1");
    let stopped = body["result"]["stopped"].as_array().expect("stopped");
    let mut processes: Vec<&str> = stopped
        .iter()
        .map(|entry| text(&entry["process"]))
        .collect();
    processes.sort_unstable();
    let mut expected = vec![first.as_str(), second.as_str()];
    expected.sort_unstable();
    assert_eq!(processes, expected);
    for entry in stopped {
        let operation = text(&entry["operation"]);
        assert!(
            (16..=128).contains(&operation.len())
                && operation
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
            "{operation}"
        );
        assert_eq!(
            harness.get(text(&entry["process"])).await["state"],
            "stopping"
        );
    }
    assert_eq!(harness.operations("process.stop").len(), 2);
    assert!(harness.operations("process.stop-by-owner").is_empty());
    assert_eq!(harness.get(&other).await["state"], "running");
    // An owner session that labels nothing is an empty list, never a refusal.
    for session in ["", "s-unknown"] {
        let (status, body) = harness
            .call(
                SUBJECT,
                Method::POST,
                "/v1/processes/stop",
                Some(json!({ "owner_session": session, "except_keep": true })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{session:?}: {body}");
        assert_eq!(body["result"]["stopped"], json!([]), "{session:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// Delegated: absent, never passed, without SUBSTRATE_VECTORS_CGROUP_ROOT.
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One start, read back every way it is observable.
async fn host_process_start_records_owner_reason_and_lease() {
    let Some(harness) = Harness::delegated("start") else {
        return;
    };
    let cwd = harness.workdir("start");
    let request = json!({
        "profile": "host",
        "argv": ["/bin/sh", "-c", "pwd; env | sort; cat /proc/self/cgroup; echo to-stderr >&2; exec /bin/sleep 300"],
        "cwd": cwd.to_str().expect("utf-8"),
        "env": { "PATH": "/usr/bin:/bin", "SUBSTRATE_TEST_SECRET": SECRET },
        "owner": { "session": "s-1", "name": "dev" },
        "reason": "serve the preview",
        "lease": { "ttl_ms": 60_000 }
    });
    let (status, started) = harness.start("op_start_journey_01", request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let id = text(&started["result"]["id"]).to_owned();
    let record = &started["result"];
    // host-process-start-records-owner-reason-and-lease
    assert_eq!(record["owner"], json!({ "session": "s-1", "name": "dev" }));
    assert_eq!(record["reason"], "serve the preview");
    assert_eq!(record["lease"]["keep"], false);
    assert_eq!(record["lease"]["ttl_ms"], 60_000);
    assert_eq!(record["lease"]["state"], "active");
    assert!(record["lease"]["renew_by"].is_string());
    assert_eq!(record["state"], "running");
    assert_eq!(record["kind"], "process");
    // host-process-record-reports-confinement-none
    assert_eq!(record["profile"], "host");
    assert_eq!(record["confinement"], "none");
    // A replay answers the same record and starts nothing.
    let (status, replayed) = harness.start("op_start_journey_01", request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["result"]["id"], id);
    assert_eq!(
        harness.list("").await["items"].as_array().map(Vec::len),
        Some(1)
    );

    // host-process-start-runs-in-caller-cwd-with-exact-env
    harness.log_line(&id, "to-stderr").await;
    let tail = harness.logs(&id, "?stream=stdout").await;
    let stdout: Vec<String> = tail["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|line| text(&line["text"]).to_owned())
        .collect();
    assert_eq!(stdout.first().map(String::as_str), cwd.to_str());
    let env_lines: Vec<&String> = stdout
        .iter()
        .filter(|line| line.contains('=') && !line.starts_with('0'))
        .collect();
    let names: Vec<&str> = env_lines
        .iter()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .filter(|name| !matches!(*name, "PWD" | "SHLVL" | "_" | "OLDPWD"))
        .collect();
    assert_eq!(
        names,
        ["PATH", "SUBSTRATE_TEST_SECRET"],
        "the environment is exactly the request's: {stdout:?}"
    );
    // host-process-child-is-born-in-its-cgroup
    assert!(
        stdout
            .iter()
            .any(|line| line.ends_with(&format!("/substrate-hp_{id}"))),
        "{stdout:?}"
    );
    // host-process-log-lines-name-their-stream
    assert_eq!(tail["profile"], "host");
    assert_eq!(tail["confinement"], "none");
    assert!(
        tail["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .all(|line| line["stream"] == "stdout")
    );
    let stderr = harness.logs(&id, "?stream=stderr").await;
    assert_eq!(stderr["lines"][0]["stream"], "stderr");
    assert_eq!(stderr["lines"][0]["text"], "to-stderr");

    // host-process-env-values-are-never-persisted, and never listed
    let read = harness.get(&id).await;
    assert_eq!(read["env_names"], json!(["PATH", "SUBSTRATE_TEST_SECRET"]));
    assert!(text(&read["env_digest"]).starts_with("sha256:"));
    assert!(!read.to_string().contains(SECRET));
    assert!(!harness.list("").await.to_string().contains(SECRET));
    let durable = harness.durable_bytes();
    assert!(
        !durable
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes()),
        "an environment value reached the durable state"
    );

    // host-process-output-is-logged-under-daemon-state and
    // host-process-nothing-is-written-to-the-caller-cwd
    assert!(harness.log_dir().join(format!("{id}.log")).is_file());
    assert_eq!(
        std::fs::read_dir(&cwd).expect("cwd").count(),
        0,
        "nothing is written to the caller's working directory"
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_duplicate_owner_name_is_refused() {
    let Some(harness) = Harness::delegated("duplicate") else {
        return;
    };
    let cwd = harness.workdir("duplicate");
    let first = harness
        .started(
            "op_duplicate_first_1",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        )
        .await;
    let observed = harness
        .start(
            "op_duplicate_second",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        )
        .await;
    assert_refused(&observed, StatusCode::CONFLICT, "process.name-conflict");
    // Another owner session may use the name; an ended record frees it.
    harness
        .started(
            "op_duplicate_other_s",
            input(&["/bin/sleep", "300"], &cwd, "s-2", "dev"),
        )
        .await;
    let (status, _) = harness.stop("op_duplicate_stop_one", &first).await;
    assert_eq!(status, StatusCode::OK);
    harness.ended(&first).await;
    harness
        .started(
            "op_duplicate_after_e",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        )
        .await;
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stop_kills_a_daemonized_grandchild() {
    let Some(harness) = Harness::delegated("grandchild") else {
        return;
    };
    let cwd = harness.workdir("grandchild");
    let id = harness
        .started(
            "op_grandchild_start1",
            input(
                &[
                    "/bin/sh",
                    "-c",
                    "setsid /bin/sleep 300 </dev/null >/dev/null 2>&1 & echo grandchild $!; exec /bin/sleep 300",
                ],
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    let line = harness.log_line(&id, "grandchild ").await;
    let pid: u32 = line
        .trim_start_matches("grandchild ")
        .trim()
        .parse()
        .expect("grandchild pid");
    assert!(
        !pid_gone(pid),
        "the detached grandchild runs before the stop"
    );
    let (status, stopped) = harness.stop("op_grandchild_stop_01", &id).await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["result"]["state"], "stopping");
    assert_eq!(harness.ended(&id).await["ended_by"], "stop");
    assert!(
        eventually_gone(pid).await,
        "the detached grandchild died with its record"
    );
    // host-process-unobservable-measure-is-absent-not-zero: an ended record has no cgroup.
    let read = harness.get(&id).await;
    for measure in ["cpu", "memory", "listening"] {
        assert!(read.get(measure).is_none(), "{measure}: {read}");
    }
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_main_exit_stops_remaining_members() {
    let Some(harness) = Harness::delegated("main-exit") else {
        return;
    };
    let cwd = harness.workdir("main-exit");
    let id = harness
        .started(
            "op_main_exit_start_1",
            input(
                &["/bin/sh", "-c", "/bin/sleep 300 & echo member $!; exit 3"],
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    let line = harness.log_line(&id, "member ").await;
    let pid: u32 = line
        .trim_start_matches("member ")
        .trim()
        .parse()
        .expect("member pid");
    let ended = harness.ended(&id).await;
    // host-process-exit-reports-code-or-signal
    assert_eq!(ended["ended_by"], "exit");
    assert_eq!(ended["exit"], json!({ "code": 3 }));
    assert!(
        eventually_gone(pid).await,
        "the remaining member was stopped"
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_exit_reports_code_or_signal() {
    let Some(harness) = Harness::delegated("signal") else {
        return;
    };
    let cwd = harness.workdir("signal");
    let id = harness
        .started(
            "op_signal_exit_start",
            input(&["/bin/sh", "-c", "kill -TERM $$"], &cwd, "s-1", "dev"),
        )
        .await;
    let ended = harness.ended(&id).await;
    assert_eq!(ended["ended_by"], "exit");
    assert_eq!(ended["exit"], json!({ "signal": "SIGTERM" }));
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_is_never_restarted() {
    let Some(harness) = Harness::delegated("never-restarted") else {
        return;
    };
    let cwd = harness.workdir("never-restarted");
    let id = harness
        .started(
            "op_never_restart_one",
            input(&["/bin/true"], &cwd, "s-1", "dev"),
        )
        .await;
    let ended = harness.ended(&id).await;
    assert_eq!(ended["exit"], json!({ "code": 0 }));
    for _ in 0..5 {
        harness.sweep().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let listed = harness.list("").await;
    assert_eq!(listed["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(listed["items"][0]["state"], "ended");
    assert!(
        !PathBuf::from(format!(
            "{}/substrate-hp_{id}",
            harness.cgroup.as_ref().expect("cgroup").display()
        ))
        .exists()
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_lease_expiry_stops_the_tree() {
    let Some(harness) = Harness::delegated("lease-expiry") else {
        return;
    };
    let cwd = harness.workdir("lease-expiry");
    let request = with(
        input(
            &[
                "/bin/sh",
                "-c",
                "/bin/sleep 300 & echo member $!; exec /bin/sleep 300",
            ],
            &cwd,
            "s-1",
            "dev",
        ),
        "lease",
        json!({ "ttl_ms": 1_000 }),
    );
    let id = harness.started("op_lease_expiry_one1", request).await;
    let line = harness.log_line(&id, "member ").await;
    let pid: u32 = line
        .trim_start_matches("member ")
        .trim()
        .parse()
        .expect("pid");
    harness.sweep().await;
    assert_eq!(harness.get(&id).await["state"], "running", "not due yet");
    harness.authority.advance(2_000);
    let ended = harness.ended(&id).await;
    assert_eq!(ended["ended_by"], "lease-expiry");
    assert_eq!(ended["lease"]["state"], "expired");
    assert!(eventually_gone(pid).await, "the whole tree was stopped");
    // The expiry is an operation the daemon originated under its own actor, completed.
    let stops = harness.operations("process.stop");
    assert_eq!(
        stops,
        [(
            "process-supervisor".to_owned(),
            "terminal".to_owned(),
            Some(id.clone())
        )]
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_lease_renewal_extends_renew_by() {
    let Some(harness) = Harness::delegated("renewal") else {
        return;
    };
    let cwd = harness.workdir("renewal");
    let request = with(
        with(
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
            "lease",
            json!({ "ttl_ms": 2_000 }),
        ),
        "timeout_ms",
        json!(10_000),
    );
    let id = harness.started("op_renewal_start_one", request).await;
    let before = harness.get(&id).await;
    harness.authority.advance(1_500);
    let (status, renewed) = harness
        .call(
            SUBJECT,
            Method::POST,
            &format!("/v1/processes/{id}/lease/renew"),
            Some(json!({ "op": "op_renewal_renew_one", "input": { "ttl_ms": 60_000 } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    let renew_by = |record: &Value| {
        DateTime::parse_from_rfc3339(text(&record["lease"]["renew_by"])).expect("renew_by")
    };
    assert!(renew_by(&renewed["result"]) > renew_by(&before));
    assert_eq!(renewed["result"]["lease"]["ttl_ms"], 60_000);
    // host-process-deadline-is-not-extended-by-lease-renewal
    assert_eq!(renewed["result"]["deadline"], before["deadline"]);
    harness.authority.advance(1_000);
    harness.sweep().await;
    assert_eq!(
        harness.get(&id).await["state"],
        "running",
        "the renewed lease holds"
    );
    harness.authority.advance(8_000);
    let ended = harness.ended(&id).await;
    assert_eq!(ended["ended_by"], "timeout", "the deadline was not moved");
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_keep_has_no_ttl_and_records_its_reason() {
    let Some(harness) = Harness::delegated("keep") else {
        return;
    };
    let cwd = harness.workdir("keep");
    let request = with(
        input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        "lease",
        json!({ "keep": true, "keep_reason": "the shared cluster outlives this session" }),
    );
    let id = harness.started("op_keep_start_one_01", request).await;
    let record = harness.get(&id).await;
    assert_eq!(record["lease"]["keep"], true);
    assert_eq!(
        record["lease"]["keep_reason"],
        "the shared cluster outlives this session"
    );
    for absent in ["ttl_ms", "renew_by", "state"] {
        assert!(record["lease"].get(absent).is_none(), "{absent}: {record}");
    }
    harness
        .authority
        .advance(i64::try_from(substrate_wire::MAX_LEASE_TTL_MS).expect("bound"));
    harness.sweep().await;
    assert_eq!(
        harness.get(&id).await["state"],
        "running",
        "a keep never expires"
    );
    // host-process-lease-cannot-become-keep-after-start: a TTL may not become a keep either.
    let ttl = harness
        .started(
            "op_keep_ttl_start_01",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "ttl"),
        )
        .await;
    let observed = harness
        .call(
            SUBJECT,
            Method::POST,
            &format!("/v1/processes/{ttl}/lease/renew"),
            Some(json!({
                "op": "op_keep_after_start1",
                "input": { "keep": true, "keep_reason": "too late" }
            })),
        )
        .await;
    assert_refused(
        &observed,
        StatusCode::UNPROCESSABLE_ENTITY,
        "process.lease-invalid",
    );
    assert_eq!(harness.get(&ttl).await["lease"]["keep"], false);
    // A keep may still become a TTL.
    let (status, changed) = harness
        .call(
            SUBJECT,
            Method::POST,
            &format!("/v1/processes/{id}/lease/renew"),
            Some(json!({ "op": "op_keep_becomes_ttl1", "input": { "ttl_ms": 60_000 } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["result"]["lease"]["keep"], false);
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_keep_lease_still_ends_at_its_timeout() {
    let Some(harness) = Harness::delegated("keep-timeout") else {
        return;
    };
    let cwd = harness.workdir("keep-timeout");
    let request = with(
        with(
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
            "lease",
            json!({ "keep": true, "keep_reason": "kept, but bounded" }),
        ),
        "timeout_ms",
        json!(2_000),
    );
    let id = harness.started("op_keep_timeout_one1", request).await;
    harness.authority.advance(3_000);
    let ended = harness.ended(&id).await;
    assert_eq!(ended["ended_by"], "timeout");
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_timeout_kills_at_the_deadline() {
    let Some(harness) = Harness::delegated("timeout") else {
        return;
    };
    let cwd = harness.workdir("timeout");
    let request = with(
        input(
            &[
                "/bin/sh",
                "-c",
                "/bin/sleep 300 & echo member $!; exec /bin/sleep 300",
            ],
            &cwd,
            "s-1",
            "dev",
        ),
        "timeout_ms",
        json!(2_000),
    );
    let id = harness.started("op_timeout_kills_one", request).await;
    let record = harness.get(&id).await;
    assert_eq!(record["timeout_ms"], 2_000);
    assert_eq!(record["deadline"]["set_by"], "timeout");
    let line = harness.log_line(&id, "member ").await;
    let pid: u32 = line
        .trim_start_matches("member ")
        .trim()
        .parse()
        .expect("pid");
    harness.sweep().await;
    assert_eq!(harness.get(&id).await["state"], "running", "not due yet");
    harness.authority.advance(2_500);
    let ended = harness.ended(&id).await;
    assert_eq!(ended["ended_by"], "timeout");
    assert!(eventually_gone(pid).await);
    assert_eq!(
        harness.operations("process.stop")[0].0,
        "process-supervisor",
        "a deadline stop is the daemon's own operation"
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_expires_at_kills_at_the_instant() {
    let Some(harness) = Harness::delegated("expires-at") else {
        return;
    };
    let cwd = harness.workdir("expires-at");
    let at = harness.authority.now() + chrono::Duration::seconds(5);
    let request = with(
        input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        "expires_at",
        json!(instant(at)),
    );
    let id = harness.started("op_expires_at_kills1", request).await;
    let record = harness.get(&id).await;
    assert_eq!(record["deadline"]["set_by"], "expires-at");
    harness.authority.advance(6_000);
    let ended = harness.ended(&id).await;
    assert_eq!(ended["ended_by"], "expires-at");
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_earlier_of_timeout_and_expires_at_wins() {
    let Some(harness) = Harness::delegated("earlier") else {
        return;
    };
    let cwd = harness.workdir("earlier");
    let now = harness.authority.now();
    let by_timeout = harness
        .started(
            "op_earlier_timeout_1",
            with(
                with(
                    input(&["/bin/sleep", "300"], &cwd, "s-1", "timeout"),
                    "timeout_ms",
                    json!(3_000),
                ),
                "expires_at",
                json!(instant(now + chrono::Duration::minutes(10))),
            ),
        )
        .await;
    let by_instant = harness
        .started(
            "op_earlier_instant_1",
            with(
                with(
                    input(&["/bin/sleep", "300"], &cwd, "s-1", "instant"),
                    "timeout_ms",
                    json!(600_000),
                ),
                "expires_at",
                json!(instant(now + chrono::Duration::seconds(3))),
            ),
        )
        .await;
    assert_eq!(
        harness.get(&by_timeout).await["deadline"]["set_by"],
        "timeout"
    );
    assert_eq!(
        harness.get(&by_instant).await["deadline"]["set_by"],
        "expires-at"
    );
    harness.authority.advance(5_000);
    assert_eq!(harness.ended(&by_timeout).await["ended_by"], "timeout");
    assert_eq!(harness.ended(&by_instant).await["ended_by"], "expires-at");
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_ended_by_names_the_first_cause_only() {
    let Some(harness) = Harness::delegated("first-cause") else {
        return;
    };
    let cwd = harness.workdir("first-cause");
    let request = with(
        input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        "timeout_ms",
        json!(1_000),
    );
    let id = harness.started("op_first_cause_start", request).await;
    harness.authority.advance(2_000);
    assert_eq!(harness.ended(&id).await["ended_by"], "timeout");
    // A later stop, lease expiry or renewal does not overwrite it.
    let (status, stopped) = harness.stop("op_first_cause_stop_1", &id).await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["result"]["ended_by"], "timeout");
    harness.authority.advance(120_000);
    harness.sweep().await;
    assert_eq!(harness.get(&id).await["ended_by"], "timeout");
    let observed = harness
        .call(
            SUBJECT,
            Method::POST,
            &format!("/v1/processes/{id}/lease/renew"),
            Some(json!({ "op": "op_first_cause_renew", "input": { "ttl_ms": 60_000 } })),
        )
        .await;
    assert_refused(&observed, StatusCode::CONFLICT, "lease.expired");
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stop_by_owner_stops_every_record_of_that_session() {
    let Some(harness) = Harness::delegated("owner-stop") else {
        return;
    };
    let cwd = harness.workdir("owner-stop");
    let a = harness
        .started(
            "op_owner_a_start_001",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "a"),
        )
        .await;
    let b = harness
        .started(
            "op_owner_b_start_001",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "b"),
        )
        .await;
    let kept = harness
        .started(
            "op_owner_keep_start1",
            with(
                input(&["/bin/sleep", "300"], &cwd, "s-1", "kept"),
                "lease",
                json!({ "keep": true, "keep_reason": "outlives the session" }),
            ),
        )
        .await;
    let other = harness
        .started(
            "op_owner_other_start",
            input(&["/bin/sleep", "300"], &cwd, "s-2", "a"),
        )
        .await;
    // host-process-stop-by-owner-except-keep-spares-keep-leases
    let (status, body) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/processes/stop",
            Some(json!({ "owner_session": "s-1", "except_keep": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stopped: Vec<&str> = body["result"]["stopped"]
        .as_array()
        .expect("stopped")
        .iter()
        .map(|item| text(&item["process"]))
        .collect();
    let mut expected = vec![a.as_str(), b.as_str()];
    expected.sort_unstable();
    assert_eq!(stopped, expected);
    for id in [&a, &b] {
        assert_eq!(harness.ended(id).await["ended_by"], "stop");
    }
    assert_eq!(harness.get(&kept).await["state"], "running");
    assert_eq!(
        harness.get(&other).await["state"],
        "running",
        "another owner session"
    );
    // host-process-stop-by-owner-records-one-operation-each
    let operations: Vec<String> = body["result"]["stopped"]
        .as_array()
        .expect("stopped")
        .iter()
        .map(|item| text(&item["operation"]).to_owned())
        .collect();
    assert_eq!(operations.len(), 2);
    assert_ne!(operations[0], operations[1]);
    for operation in &operations {
        let (status, recorded) = harness
            .call(SUBJECT, Method::GET, &format!("/v1/ops/{operation}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{recorded}");
        assert_eq!(recorded["result"]["state"], "terminal");
    }
    // Without except_keep the keep record is stopped too.
    let (status, body) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/processes/stop",
            Some(json!({ "owner_session": "s-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["stopped"][0]["process"], kept.as_str());
    assert_eq!(harness.get(&other).await["state"], "running");
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_stop_by_owner_never_reaches_another_subject() {
    let Some(mut harness) = Harness::delegated("owner-subject") else {
        return;
    };
    let cwd = harness.workdir("owner-subject");
    let id = harness
        .started(
            "op_subject_start_001",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        )
        .await;
    // The same durable state served to another subject, as a daemon of another uid would.
    let mine = Arc::clone(&harness.app);
    harness.restart();
    let other_app = Arc::get_mut(&mut harness.app).expect("unshared");
    other_app.host_process_uid = 4_243;
    let (status, body) = harness
        .call(
            FOREIGN,
            Method::POST,
            "/v1/processes/stop",
            Some(json!({ "owner_session": "s-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["stopped"], json!([]));
    // host-process-list-is-scoped-to-the-subject
    let (status, listed) = harness
        .call(FOREIGN, Method::GET, "/v1/processes", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["result"]["items"], json!([]));
    let (status, _) = harness
        .call(FOREIGN, Method::GET, &format!("/v1/processes/{id}"), None)
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another subject's record answers as absent"
    );
    harness.app = mine;
    assert_eq!(harness.get(&id).await["state"], "running");
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_list_filters_by_owner_name_and_state() {
    let Some(harness) = Harness::delegated("list") else {
        return;
    };
    let cwd = harness.workdir("list");
    let first = harness
        .started(
            "op_list_start_one_01",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        )
        .await;
    harness
        .started(
            "op_list_start_two_01",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "db"),
        )
        .await;
    harness
        .started(
            "op_list_start_three1",
            input(&["/bin/sleep", "300"], &cwd, "s-2", "dev"),
        )
        .await;
    let count = |page: &Value| page["items"].as_array().map_or(0, Vec::len);
    assert_eq!(count(&harness.list("?owner_session=s-1").await), 2);
    assert_eq!(count(&harness.list("?name=dev").await), 2);
    assert_eq!(count(&harness.list("?owner_session=s-2&name=dev").await), 1);
    assert_eq!(count(&harness.list("?state=running").await), 3);
    let (status, _) = harness.stop("op_list_stop_first_1", &first).await;
    assert_eq!(status, StatusCode::OK);
    harness.ended(&first).await;
    assert_eq!(count(&harness.list("?state=ended").await), 1);
    assert_eq!(count(&harness.list("?state=running").await), 2);
    // A bounded page with a cursor.
    let page = harness.list("?limit=2").await;
    assert_eq!(count(&page), 2);
    let cursor = text(&page["next_cursor"]).to_owned();
    let rest = harness.list(&format!("?limit=2&cursor={cursor}")).await;
    assert_eq!(count(&rest), 1);
    assert!(rest.get("next_cursor").is_none());
    for query in ["?limit=0", "?limit=100000", "?unknown=1", "?state=sleeping"] {
        let (status, body) = harness
            .call(SUBJECT, Method::GET, &format!("/v1/processes{query}"), None)
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}: {body}");
    }
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_list_shows_owner_cwd_argv_lease_and_state() {
    let Some(harness) = Harness::delegated("list-fields") else {
        return;
    };
    let cwd = harness.workdir("list-fields");
    let request = with(
        with(
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
            "env",
            json!({ "PATH": "/usr/bin:/bin", "TOKEN": SECRET }),
        ),
        "timeout_ms",
        json!(600_000),
    );
    let id = harness.started("op_list_fields_start", request).await;
    let item = harness.list("").await["items"][0].clone();
    assert_eq!(item["id"], id.as_str());
    assert_eq!(item["owner"], json!({ "session": "s-1", "name": "dev" }));
    assert_eq!(item["reason"], "serve the preview");
    assert_eq!(item["profile"], "host");
    assert_eq!(item["confinement"], "none");
    assert_eq!(item["argv"], json!(["/bin/sleep", "300"]));
    assert_eq!(item["cwd"], cwd.to_str().expect("utf-8"));
    assert!(item["started_at"].is_string());
    assert_eq!(item["lease"]["ttl_ms"], 60_000);
    assert_eq!(item["deadline"]["set_by"], "timeout");
    assert_eq!(item["state"], "running");
    // host-process-list-never-shows-env-values
    assert_eq!(item["env_names"], json!(["PATH", "TOKEN"]));
    assert!(!item.to_string().contains(SECRET));
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_list_reports_observed_cpu_and_memory() {
    let Some(harness) = Harness::delegated("cpu-memory") else {
        return;
    };
    let cwd = harness.workdir("cpu-memory");
    let id = harness
        .started(
            "op_cpu_memory_start1",
            input(
                &[
                    "/bin/sh",
                    "-c",
                    "i=0; while [ $i -lt 200000 ]; do i=$((i+1)); done; echo spun; exec /bin/sleep 300",
                ],
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    harness.log_line(&id, "spun").await;
    let item = harness.list("").await["items"][0].clone();
    assert!(
        item["cpu"]["usage_usec"].as_u64().expect("cpu observed") > 0,
        "{item}"
    );
    assert!(item["cpu"]["observed_at"].is_string());
    assert!(
        item["memory"]["memory_bytes"]
            .as_u64()
            .expect("memory observed")
            > 0,
        "{item}"
    );
    assert!(item["memory"]["observed_at"].is_string());
    harness.finish().await;
}

const LISTENING_HELPER_ENV: &str = "SUBSTRATE_DAEMON_PROCESS_LISTENING_HELPER";

/// Not a case: the program `host_process_list_reports_an_observed_listening_port` runs as a host
/// process. Ignored so that no lane counts it as a pass; it does nothing unless that case's
/// environment variable is set.
#[test]
#[ignore = "the listening case's child program, never a check of its own"]
fn listening_helper() {
    if std::env::var_os(LISTENING_HELPER_ENV).is_none() {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    println!(
        "LISTENING {}",
        listener.local_addr().expect("address").port()
    );
    std::thread::sleep(Duration::from_mins(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_list_reports_an_observed_listening_port() {
    let Some(harness) = Harness::delegated("listening") else {
        return;
    };
    let cwd = harness.workdir("listening");
    let exe = std::env::current_exe().expect("test binary");
    let exe = exe.to_str().expect("utf-8 test binary path");
    let request = with(
        input(
            &[
                exe,
                "--exact",
                "listening_helper",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ],
            &cwd,
            "s-1",
            "dev",
        ),
        "env",
        json!({ "PATH": "/usr/bin:/bin", LISTENING_HELPER_ENV: "1" }),
    );
    let id = harness.started("op_listening_start_1", request).await;
    let line = harness.log_line(&id, "LISTENING ").await;
    let port: u64 = line
        .split_once("LISTENING ")
        .expect("marker")
        .1
        .trim()
        .parse()
        .expect("port");
    let item = harness.get(&id).await;
    let sockets = item["listening"]["sockets"]
        .as_array()
        .expect("listening observed");
    assert!(
        sockets
            .iter()
            .any(|socket| socket["port"] == port && socket["protocol"] == "tcp"),
        "{item}"
    );
    assert!(item["listening"]["observed_at"].is_string());
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_logs_route_returns_a_bounded_tail() {
    let Some(harness) = Harness::delegated("logs") else {
        return;
    };
    let cwd = harness.workdir("logs");
    let id = harness
        .started(
            "op_logs_tail_start_1",
            input(
                &[
                    "/bin/sh",
                    "-c",
                    "for i in 1 2 3 4 5; do echo out$i; echo err$i >&2; done",
                ],
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    harness.ended(&id).await;
    let both = harness.logs(&id, "").await;
    assert_eq!(both["stream"], "both");
    assert_eq!(both["lines"].as_array().map(Vec::len), Some(10));
    let stdout = harness.logs(&id, "?stream=stdout").await;
    let texts: Vec<&str> = stdout["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|line| text(&line["text"]))
        .collect();
    assert_eq!(texts, ["out1", "out2", "out3", "out4", "out5"]);
    let small = harness.logs(&id, "?stream=stderr&max_bytes=12").await;
    let lines = small["lines"].as_array().expect("lines");
    assert!(!lines.is_empty() && lines.len() < 5, "{small}");
    assert_eq!(lines.last().map(|line| text(&line["text"])), Some("err5"));
    assert_eq!(small["discarded_bytes"], 0);
    for query in [
        "?max_bytes=0",
        "?max_bytes=99999999",
        "?stream=all",
        "?other=1",
    ] {
        let (status, _) = harness
            .call(
                SUBJECT,
                Method::GET,
                &format!("/v1/processes/{id}/logs{query}"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
    }
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_log_bound_discards_oldest_and_counts() {
    let Some(harness) = Harness::delegated("log-bound") else {
        return;
    };
    let cwd = harness.workdir("log-bound");
    // About 12 MiB of lines, past the 8 MiB bound.
    let id = harness
        .started(
            "op_log_bound_start_1",
            input(
                &[
                    "/bin/sh",
                    "-c",
                    "i=0; while [ $i -lt 4 ]; do /usr/bin/yes 0123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789 | /usr/bin/head -n 30000; i=$((i+1)); done; echo last-line",
                ],
                &cwd,
                "s-1",
                "dev",
            ),
        )
        .await;
    harness.ended(&id).await;
    let tail = harness.logs(&id, "?max_bytes=4096").await;
    assert!(
        tail["discarded_bytes"].as_u64().expect("discarded") > 0,
        "{tail}"
    );
    assert!(
        tail["retained_bytes"].as_u64().expect("retained") <= substrate_wire::MAX_PROCESS_LOG_BYTES
    );
    assert_eq!(
        tail["lines"]
            .as_array()
            .and_then(|lines| lines.last())
            .map(|line| text(&line["text"])),
        Some("last-line")
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_log_is_removed_with_its_record() {
    let Some(harness) = Harness::delegated("log-removed") else {
        return;
    };
    let cwd = harness.workdir("log-removed");
    let id = harness
        .started(
            "op_log_removed_start",
            input(&["/bin/echo", "hello"], &cwd, "s-1", "dev"),
        )
        .await;
    harness.ended(&id).await;
    let log = harness.log_dir().join(format!("{id}.log"));
    assert!(log.is_file());
    harness
        .authority
        .advance(i64::try_from(substrate_wire::PROCESS_RETAIN_ENDED_MS).expect("bound") + 60_000);
    harness.sweep().await;
    assert!(!log.exists(), "the log went with its record");
    let (status, _) = harness
        .call(SUBJECT, Method::GET, &format!("/v1/processes/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    harness.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Delegated: restart reconciliation.
// ---------------------------------------------------------------------------------------------

/// Records a `starting` record and its accepted start operation the way a daemon does before it
/// dispatches, and nothing else: what a crash between the two leaves behind.
fn reserve_start_only(
    harness: &Harness,
    operation: &str,
    id: &str,
    cwd: &Path,
) -> substrate_store::Scope {
    let scope = substrate_store::Scope {
        deployment: "dep_processes".to_owned(),
        subject: SUBJECT.to_owned(),
    };
    let new = substrate_store::NewOperation {
        scope: scope.clone(),
        operation: operation.to_owned(),
        operation_kind: substrate_store::HOST_PROCESS_START_KIND.to_owned(),
        request_hash: format!("sha256:{}", "a".repeat(64)),
        accepted_at: harness.authority.now().to_rfc3339(),
        capability_snapshot: None,
        actor: "process-test".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(id.to_owned()),
    };
    let process = substrate_store::NewHostProcess {
        id: id.to_owned(),
        owner_session: "s-1".to_owned(),
        owner_name: id.to_owned(),
        reason: "crashed".to_owned(),
        argv: vec!["/bin/sleep".to_owned(), "300".to_owned()],
        cwd: cwd.to_str().expect("utf-8").to_owned(),
        env_names: Vec::new(),
        env_digest: format!("sha256:{}", "b".repeat(64)),
        lease: substrate_store::HostProcessLeaseRequest::Ttl { ttl_ms: 60_000 },
        timeout_ms: None,
        expires_at: None,
    };
    assert!(matches!(
        harness
            .store
            .reserve_host_process_start(
                &new,
                &process,
                &substrate_store::HostProcessLimits::default()
            )
            .expect("reserve"),
        substrate_store::HostProcessStartReservation::Accepted(_)
    ));
    scope
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_start_interrupted_before_dispatch_is_abandoned() {
    let Some(mut harness) = Harness::delegated("abandoned") else {
        return;
    };
    let cwd = harness.workdir("abandoned");
    reserve_start_only(&harness, "op_abandoned_start_1", "proc_abandoned", &cwd);
    tokio::time::sleep(Duration::from_millis(5)).await;
    harness.restart();
    harness.sweep().await;
    let (status, operation) = harness
        .call(SUBJECT, Method::GET, "/v1/ops/op_abandoned_start_1", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(operation["result"]["state"], "terminal");
    assert_eq!(
        operation["result"]["outcome"]["error"]["code"], "process.start-abandoned",
        "{operation}"
    );
    let (status, _) = harness
        .call(SUBJECT, Method::GET, "/v1/processes/proc_abandoned", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "never re-dispatched");
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_restart_marks_a_missing_cgroup_vanished() {
    let Some(mut harness) = Harness::delegated("vanished") else {
        return;
    };
    let cwd = harness.workdir("vanished");
    let scope = reserve_start_only(&harness, "op_vanished_start_01", "proc_vanished", &cwd);
    harness
        .store
        .mark_host_process_running(&scope, "proc_vanished", harness.authority.now())
        .expect("running");
    tokio::time::sleep(Duration::from_millis(5)).await;
    harness.restart();
    harness.sweep().await;
    let record = harness.get("proc_vanished").await;
    assert_eq!(record["state"], "ended");
    assert_eq!(record["ended_by"], "vanished");
    assert!(
        record.get("exit").is_none(),
        "no exit status nobody observed"
    );
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_restart_reobserves_a_live_cgroup() {
    let Some(mut harness) = Harness::delegated("reobserve") else {
        return;
    };
    let cwd = harness.workdir("reobserve");
    let id = harness
        .started(
            "op_reobserve_start_1",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    harness.restart();
    harness.sweep().await;
    let record = harness.get(&id).await;
    assert_eq!(record["state"], "running", "{record}");
    assert!(
        record["memory"]["memory_bytes"].as_u64().is_some(),
        "observed again: {record}"
    );
    let (status, stopped) = harness.stop("op_reobserve_stop_01", &id).await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["result"]["state"], "stopping");
    assert_eq!(harness.ended(&id).await["ended_by"], "stop");
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_expiry_interrupted_by_crash_completes_on_restart() {
    let Some(mut harness) = Harness::delegated("expiry-crash") else {
        return;
    };
    let cwd = harness.workdir("expiry-crash");
    let scope = reserve_start_only(&harness, "op_expiry_crash_start", "proc_expirycrash", &cwd);
    harness
        .store
        .mark_host_process_running(&scope, "proc_expirycrash", harness.authority.now())
        .expect("running");
    harness.authority.advance(61_000);
    // The supervisor recorded the expiry and crashed before the driver acted.
    let stop = substrate_store::NewOperation {
        scope: scope.clone(),
        operation: "op_supervisor_crashed_1".to_owned(),
        operation_kind: substrate_store::HOST_PROCESS_STOP_KIND.to_owned(),
        request_hash: format!("sha256:{}", "c".repeat(64)),
        accepted_at: harness.authority.now().to_rfc3339(),
        capability_snapshot: None,
        actor: substrate_store::HOST_PROCESS_SUPERVISOR_ACTOR.to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some("proc_expirycrash".to_owned()),
    };
    assert!(matches!(
        harness
            .store
            .reserve_host_process_stop(
                &stop,
                "proc_expirycrash",
                substrate_store::HostProcessEndedBy::LeaseExpiry
            )
            .expect("reserve stop"),
        substrate_store::HostProcessStopReservation::Accepted(_)
    ));
    tokio::time::sleep(Duration::from_millis(5)).await;
    harness.restart();
    let record = harness.ended("proc_expirycrash").await;
    assert_eq!(record["ended_by"], "lease-expiry");
    let (status, operation) = harness
        .call(
            SUBJECT,
            Method::GET,
            "/v1/ops/op_supervisor_crashed_1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(operation["result"]["state"], "terminal", "{operation}");
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_process_deadline_passed_during_downtime_stops_on_restart() {
    let Some(mut harness) = Harness::delegated("downtime") else {
        return;
    };
    let cwd = harness.workdir("downtime");
    let id = harness
        .started(
            "op_downtime_start_01",
            with(
                input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
                "timeout_ms",
                json!(30_000),
            ),
        )
        .await;
    // The daemon is down while the deadline passes.
    harness.authority.advance(45_000);
    harness.restart();
    harness.sweep().await;
    let record = harness.ended(&id).await;
    assert_eq!(record["ended_by"], "timeout");
    harness.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Portable, behind the port: integrity of the record against what the driver still runs.
// ---------------------------------------------------------------------------------------------

/// A driver that answers as the cgroup driver does for an id it does not hold
/// (`process.not-found`), whose `start` can be held at its entry so a test can order another
/// request between the durable `starting` record and the dispatch, and whose `reattach` can be
/// made to fail as the cgroup driver's does on an unreadable cgroup or log.
struct StrictSupervisor {
    running: parking_lot::Mutex<std::collections::BTreeSet<String>>,
    stops: parking_lot::Mutex<Vec<String>>,
    reattach_calls: std::sync::atomic::AtomicUsize,
    reattach_fails: bool,
    /// How many of the next `begin_stop` calls fail as a cgroup write the kernel refused does.
    begin_stop_failures: std::sync::atomic::AtomicUsize,
    gate: Option<StartGate>,
}

/// Where a held `start` reports its entry, and what releases it.
type StartGate = (
    parking_lot::Mutex<std::sync::mpsc::Sender<()>>,
    parking_lot::Mutex<std::sync::mpsc::Receiver<()>>,
);

impl StrictSupervisor {
    fn new(
        reattach_fails: bool,
        gate: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            running: parking_lot::Mutex::default(),
            stops: parking_lot::Mutex::default(),
            reattach_calls: std::sync::atomic::AtomicUsize::new(0),
            reattach_fails,
            begin_stop_failures: std::sync::atomic::AtomicUsize::new(0),
            gate: gate.map(|(entered, proceed)| {
                (
                    parking_lot::Mutex::new(entered),
                    parking_lot::Mutex::new(proceed),
                )
            }),
        })
    }

    fn runs(&self, id: &str) -> bool {
        self.running.lock().contains(id) && !self.stops.lock().iter().any(|stop| stop == id)
    }
}

impl ProcessLogs for StrictSupervisor {
    fn log_tail(
        &self,
        _id: &str,
        _selection: LogStreamSelection,
        _max_bytes: u64,
    ) -> Result<LogTail, DriverError> {
        Err(DriverError::not_found())
    }
}

impl ProcessSupervisor for StrictSupervisor {
    fn start(
        self: Arc<Self>,
        id: &str,
        spec: &HostProcessSpec,
    ) -> Result<HostProcessStarted, DriverError> {
        if let Some((entered, proceed)) = &self.gate {
            let _ = entered.lock().send(());
            let _ = proceed.lock().recv();
        }
        self.running.lock().insert(id.to_owned());
        Ok(HostProcessStarted {
            id: id.to_owned(),
            pid: 1,
            cgroup: format!("strict-{id}"),
            started_at: Utc::now(),
            deadline: spec.deadline,
            profile: HostProfile::Host,
            confinement: Confinement::None,
        })
    }

    fn reattach(self: Arc<Self>, _id: &str) -> Result<Reattached, DriverError> {
        self.reattach_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.reattach_fails {
            return Err(DriverError::failed(
                "process.log-failed",
                "the retained log could not be reopened",
            ));
        }
        Ok(Reattached::Live)
    }

    fn begin_stop(&self, id: &str, _cause: StopCause) -> Result<(), DriverError> {
        if !self.running.lock().contains(id) {
            return Err(DriverError::not_found());
        }
        if self
            .begin_stop_failures
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok()
        {
            return Err(DriverError::failed(
                "process.cgroup-failed",
                "the stop signal could not be delivered",
            ));
        }
        self.stops.lock().push(id.to_owned());
        Ok(())
    }

    fn ended(&self, id: &str) -> Result<Option<HostProcessEnd>, DriverError> {
        if !self.running.lock().contains(id) {
            return Err(DriverError::not_found());
        }
        Ok(self
            .stops
            .lock()
            .iter()
            .any(|stopped| stopped == id)
            .then(|| HostProcessEnd {
                ended_by: EndedBy::Stop,
                exit: Some(ProcessExit::Signal(15)),
                stop_step: Some(StopStep::Term),
                ended_at: Utc::now(),
            }))
    }

    fn observe(&self, _id: &str) -> Result<HostProcessObservation, DriverError> {
        Err(DriverError::not_found())
    }

    fn forget(&self, _id: &str) -> Result<(), DriverError> {
        Ok(())
    }
}

/// S3/S5 and invariant 5: a stop by owner that lands while a start of the same owner session is
/// between its durable `starting` record and its dispatch must not leave the record `ended` while
/// the program the start then dispatches keeps running. A session-end handler running while one
/// of its starts is in flight reaches exactly this order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_process_stop_during_start_never_orphans_the_dispatched_process() {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (proceed_tx, proceed_rx) = std::sync::mpsc::channel();
    let strict = StrictSupervisor::new(false, Some((entered_tx, proceed_rx)));
    let mut harness = Harness::portable();
    Arc::get_mut(&mut harness.app)
        .expect("unshared application")
        .host_processes = Some(Arc::clone(&strict) as Arc<dyn ProcessSupervisor>);
    let cwd = harness.workdir("race");
    let harness = Arc::new(harness);
    let racer = Arc::clone(&harness);
    let start_input = input(&["/bin/sleep", "300"], &cwd, "s-race", "dev");
    let start = tokio::spawn(async move { racer.start("op_race_start_000001", start_input).await });
    // The start is now durable as `starting` and held at the driver's entry.
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(10)))
        .await
        .expect("join")
        .expect("the start reached the driver");
    let (status, stopped) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/processes/stop",
            Some(json!({ "owner_session": "s-race" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    proceed_tx.send(()).expect("release the start");
    let (_, started) = start.await.expect("start task");
    let id = text(&started["result"]["id"]).to_owned();
    for _ in 0..5 {
        harness.sweep().await;
    }
    let record = harness.get(&id).await;
    assert!(
        !(record["state"] == "ended" && strict.runs(&id)),
        "the record of {id} is ended ({}) while the driver still runs the program the start \
         dispatched after the stop; nothing will ever stop it: start answered {started}, \
         stop by owner answered {stopped}",
        record["ended_by"]
    );
}

/// S3 restart reconciliation and S4: a record whose re-attachment failed was not shown gone, so
/// a later lease expiry must not record it ended (and complete its stop) while its cgroup may
/// still hold the program: the failed re-attachment is retried, or the record is kept from ending.
#[tokio::test(flavor = "multi_thread")]
async fn host_process_failed_reattach_is_retried_before_the_record_ends() {
    let (mut harness, _fake) = Harness::faked();
    let cwd = harness.workdir("reattach");
    let id = harness
        .started(
            "op_reattach_start_001",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        )
        .await;
    harness.authority.advance(1_000);
    // A fresh daemon run whose driver cannot re-attach this record.
    harness.unserve();
    let strict = StrictSupervisor::new(true, None);
    Arc::get_mut(&mut harness.app)
        .expect("unshared application")
        .host_processes = Some(Arc::clone(&strict) as Arc<dyn ProcessSupervisor>);
    harness.sweep().await;
    // The lease passes.
    harness.authority.advance(61_000);
    harness.sweep().await;
    harness.sweep().await;
    let record = harness.get(&id).await;
    let attempts = strict
        .reattach_calls
        .load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        attempts >= 2 || record["state"] != "ended",
        "{id} was recorded ended after {attempts} failed re-attachment(s) and no retry, although \
         nothing showed its cgroup empty: {record}"
    );
}

/// S3 and invariant 5: a stop whose first `begin_stop` the driver failed (an error other than
/// not-found) stays recorded `stopping`, and the supervisor clock begins it again, so the program
/// is stopped and the record ends with the cause its stop recorded.
#[tokio::test(flavor = "multi_thread")]
async fn host_process_stop_the_driver_failed_to_begin_is_retried() {
    let strict = StrictSupervisor::new(false, None);
    let mut harness = Harness::portable();
    Arc::get_mut(&mut harness.app)
        .expect("unshared application")
        .host_processes = Some(Arc::clone(&strict) as Arc<dyn ProcessSupervisor>);
    let cwd = harness.workdir("retry-stop");
    let id = harness
        .started(
            "op_retry_stop_start_01",
            input(&["/bin/sleep", "300"], &cwd, "s-1", "dev"),
        )
        .await;
    strict
        .begin_stop_failures
        .store(1, std::sync::atomic::Ordering::SeqCst);
    let (status, stopped) = harness.stop("op_retry_stop_stop_001", &id).await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["result"]["state"], "stopping", "{stopped}");
    for _ in 0..3 {
        harness.sweep().await;
    }
    let record = harness.get(&id).await;
    assert!(
        !strict.runs(&id),
        "the driver still runs {id}: the stop it failed to begin was never begun again: {record}"
    );
    assert_eq!(record["state"], "ended", "{record}");
    assert_eq!(record["ended_by"], "stop", "{record}");
}

// ---------------------------------------------------------------------------------------------
// Security re-review, pass 2: interleavings around `starting` and `stopping`.
// ---------------------------------------------------------------------------------------------

/// Invariant 5 and ADR 0036 § Consequences: a start whose record was stopped while it was in
/// flight, left `stopping` by a crash, must have its start operation concluded by the restarted
/// daemon's reconciliation, not left open forever beside an ended record.
#[tokio::test(flavor = "multi_thread")]
async fn host_process_restart_concludes_the_start_of_a_record_stopped_while_starting() {
    let strict = StrictSupervisor::new(false, None);
    let mut harness = Harness::portable();
    Arc::get_mut(&mut harness.app)
        .expect("unshared application")
        .host_processes = Some(Arc::clone(&strict) as Arc<dyn ProcessSupervisor>);
    let cwd = harness.workdir("stopped-starting");
    // The start is durable as `starting`; its dispatch has not happened when the stop lands.
    reserve_start_only(
        &harness,
        "op_stopped_starting_st",
        "proc_stoppedstarting",
        &cwd,
    );
    let (status, stopped) = harness
        .stop("op_stopped_starting_sp", "proc_stoppedstarting")
        .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["result"]["state"], "stopping", "{stopped}");
    // The daemon crashes before the start dispatches; a fresh run re-attaches a live cgroup.
    tokio::time::sleep(Duration::from_millis(5)).await;
    harness.restart();
    let fake = Arc::new(FakeSupervisor::default());
    Arc::get_mut(&mut harness.app)
        .expect("unshared application")
        .host_processes = Some(Arc::clone(&fake) as Arc<dyn ProcessSupervisor>);
    let record = harness.ended("proc_stoppedstarting").await;
    assert_eq!(record["ended_by"], "stop", "{record}");
    let (status, operation) = harness
        .call(SUBJECT, Method::GET, "/v1/ops/op_stopped_starting_st", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{operation}");
    assert_eq!(
        operation["result"]["state"], "terminal",
        "the start operation of an ended record is still open after reconciliation: {operation}"
    );
}

/// S4 and invariant 3: a `starting` record this run will never dispatch (its start handler was
/// dropped after the durable reservation, as a client disconnect or the connection lifetime bound
/// drops it) holds no process. Its lease expiry must still end it, rather than leave it
/// `stopping` for the rest of the run with its live slot, its owner name and its stop open.
#[tokio::test(flavor = "multi_thread")]
async fn host_process_lease_expiry_ends_a_start_this_run_never_dispatches() {
    let strict = StrictSupervisor::new(false, None);
    let mut harness = Harness::portable();
    Arc::get_mut(&mut harness.app)
        .expect("unshared application")
        .host_processes = Some(Arc::clone(&strict) as Arc<dyn ProcessSupervisor>);
    let cwd = harness.workdir("dropped-start");
    // Exactly what a start handler dropped after its reservation committed leaves behind.
    reserve_start_only(
        &harness,
        "op_dropped_start_00001",
        "proc_droppedstart",
        &cwd,
    );
    harness.authority.advance(61_000);
    for _ in 0..5 {
        harness.sweep().await;
    }
    let record = harness.get("proc_droppedstart").await;
    assert!(
        !strict.runs("proc_droppedstart"),
        "nothing was dispatched for this record"
    );
    assert_eq!(
        record["state"], "ended",
        "a lease-expired record the driver never held, and never will in this run, stays \
         stopping: {record}"
    );
}

/// A driver whose `start` is held at its entry and then refused, as a spawn the kernel refused.
struct FailingStart {
    entered: parking_lot::Mutex<std::sync::mpsc::Sender<()>>,
    proceed: parking_lot::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl ProcessLogs for FailingStart {
    fn log_tail(
        &self,
        _id: &str,
        _selection: LogStreamSelection,
        _max_bytes: u64,
    ) -> Result<LogTail, DriverError> {
        Err(DriverError::not_found())
    }
}

impl ProcessSupervisor for FailingStart {
    fn start(
        self: Arc<Self>,
        _id: &str,
        _spec: &HostProcessSpec,
    ) -> Result<HostProcessStarted, DriverError> {
        let _ = self.entered.lock().send(());
        let _ = self.proceed.lock().recv();
        Err(DriverError::failed(
            "process.spawn-failed",
            "the program could not be started",
        ))
    }

    fn reattach(self: Arc<Self>, _id: &str) -> Result<Reattached, DriverError> {
        Ok(Reattached::Gone)
    }

    fn begin_stop(&self, _id: &str, _cause: StopCause) -> Result<(), DriverError> {
        Err(DriverError::not_found())
    }

    fn ended(&self, _id: &str) -> Result<Option<HostProcessEnd>, DriverError> {
        Err(DriverError::not_found())
    }

    fn observe(&self, _id: &str) -> Result<HostProcessObservation, DriverError> {
        Err(DriverError::not_found())
    }

    fn forget(&self, _id: &str) -> Result<(), DriverError> {
        Ok(())
    }
}

/// S1 and S3: a stop accepted while a start is in flight, whose dispatch the driver then refuses,
/// ends the record (nothing ran) and concludes the start with the driver's refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_process_stop_during_a_start_the_driver_refuses_ends_the_record() {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (proceed_tx, proceed_rx) = std::sync::mpsc::channel();
    let failing = Arc::new(FailingStart {
        entered: parking_lot::Mutex::new(entered_tx),
        proceed: parking_lot::Mutex::new(proceed_rx),
    });
    let mut harness = Harness::portable();
    Arc::get_mut(&mut harness.app)
        .expect("unshared application")
        .host_processes = Some(Arc::clone(&failing) as Arc<dyn ProcessSupervisor>);
    let cwd = harness.workdir("refused-start");
    let harness = Arc::new(harness);
    let racer = Arc::clone(&harness);
    let start_input = input(&["/bin/sleep", "300"], &cwd, "s-refused", "dev");
    let start =
        tokio::spawn(async move { racer.start("op_refused_start_00001", start_input).await });
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(10)))
        .await
        .expect("join")
        .expect("the start reached the driver");
    let (status, stopped) = harness
        .call(
            SUBJECT,
            Method::POST,
            "/v1/processes/stop",
            Some(json!({ "owner_session": "s-refused" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    let id = text(&stopped["result"]["stopped"][0]["process"]).to_owned();
    proceed_tx.send(()).expect("release the start");
    let (status, started) = start.await.expect("start task");
    assert_ne!(status, StatusCode::OK, "{started}");
    for _ in 0..3 {
        harness.sweep().await;
    }
    let record = harness.get(&id).await;
    assert_eq!(record["state"], "ended", "{record}");
    assert_eq!(record["ended_by"], "stop", "{record}");
    let (_, operation) = harness
        .call(SUBJECT, Method::GET, "/v1/ops/op_refused_start_00001", None)
        .await;
    assert_eq!(operation["result"]["state"], "terminal", "{operation}");
}

/// A driver whose `reattach` is held at its entry and then answers that the cgroup is gone, so a
/// test can land a stop between reconciliation's read of a `starting` record and its abandonment.
struct GatedReattach {
    entered: parking_lot::Mutex<std::sync::mpsc::Sender<()>>,
    proceed: parking_lot::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl ProcessLogs for GatedReattach {
    fn log_tail(
        &self,
        _id: &str,
        _selection: LogStreamSelection,
        _max_bytes: u64,
    ) -> Result<LogTail, DriverError> {
        Err(DriverError::not_found())
    }
}

impl ProcessSupervisor for GatedReattach {
    fn start(
        self: Arc<Self>,
        _id: &str,
        _spec: &HostProcessSpec,
    ) -> Result<HostProcessStarted, DriverError> {
        Err(DriverError::failed(
            "process.spawn-failed",
            "the program could not be started",
        ))
    }

    fn reattach(self: Arc<Self>, _id: &str) -> Result<Reattached, DriverError> {
        let _ = self.entered.lock().send(());
        let _ = self.proceed.lock().recv();
        Ok(Reattached::Gone)
    }

    fn begin_stop(&self, _id: &str, _cause: StopCause) -> Result<(), DriverError> {
        Err(DriverError::not_found())
    }

    fn ended(&self, _id: &str) -> Result<Option<HostProcessEnd>, DriverError> {
        Err(DriverError::not_found())
    }

    fn observe(&self, _id: &str) -> Result<HostProcessObservation, DriverError> {
        Err(DriverError::not_found())
    }

    fn forget(&self, _id: &str) -> Result<(), DriverError> {
        Ok(())
    }
}

/// S3 and invariant 5: a stop that lands while reconciliation is deciding a `starting` record an
/// earlier run left (after its read, before its abandonment) is not lost. The cgroup is gone, so
/// the record ends under the stop's cause, and both the start and the stop are concluded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_process_stop_during_reconciliation_of_a_starting_record_ends_it() {
    let mut harness = Harness::portable();
    let cwd = harness.workdir("reconcile-race");
    reserve_start_only(
        &harness,
        "op_reconcile_race_st1",
        "proc_reconcilerace",
        &cwd,
    );
    tokio::time::sleep(Duration::from_millis(5)).await;
    harness.restart();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (proceed_tx, proceed_rx) = std::sync::mpsc::channel();
    let gated = Arc::new(GatedReattach {
        entered: parking_lot::Mutex::new(entered_tx),
        proceed: parking_lot::Mutex::new(proceed_rx),
    });
    Arc::get_mut(&mut harness.app)
        .expect("unshared application")
        .host_processes = Some(Arc::clone(&gated) as Arc<dyn ProcessSupervisor>);
    let harness = Arc::new(harness);
    let sweeper = Arc::clone(&harness);
    let sweep = tokio::spawn(async move { sweeper.sweep().await });
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(10)))
        .await
        .expect("join")
        .expect("reconciliation reached the driver");
    let (status, stopped) = harness
        .stop("op_reconcile_race_sp1", "proc_reconcilerace")
        .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["result"]["state"], "stopping", "{stopped}");
    proceed_tx.send(()).expect("release the re-attachment");
    sweep.await.expect("sweep task");
    for _ in 0..3 {
        harness.sweep().await;
    }
    let record = harness.get("proc_reconcilerace").await;
    assert_eq!(
        record["state"], "ended",
        "a stop landing during reconciliation leaves the record stopping: {record}"
    );
    assert_eq!(record["ended_by"], "stop", "{record}");
    for operation in ["op_reconcile_race_st1", "op_reconcile_race_sp1"] {
        let (_, answer) = harness
            .call(SUBJECT, Method::GET, &format!("/v1/ops/{operation}"), None)
            .await;
        assert_eq!(answer["result"]["state"], "terminal", "{answer}");
    }
}

/// Records, without any daemon, a start and a stop accepted while it was starting: what a daemon
/// that crashed before the start dispatched leaves behind.
fn reserve_stopped_while_starting(harness: &Harness, prefix: &str, id: &str, cwd: &Path) {
    let scope = reserve_start_only(harness, &format!("{prefix}_st"), id, cwd);
    let stop = substrate_store::NewOperation {
        scope,
        operation: format!("{prefix}_sp"),
        operation_kind: substrate_store::HOST_PROCESS_STOP_KIND.to_owned(),
        request_hash: format!("sha256:{}", "d".repeat(64)),
        accepted_at: harness.authority.now().to_rfc3339(),
        capability_snapshot: None,
        actor: "process-test".to_owned(),
        principal: None,
        grant_ref: None,
        platform_principal: None,
        resource: Some(id.to_owned()),
    };
    assert!(matches!(
        harness
            .store
            .reserve_host_process_stop(&stop, id, substrate_store::HostProcessEndedBy::Stop)
            .expect("reserve stop"),
        substrate_store::HostProcessStopReservation::Accepted(_)
    ));
}

/// Invariant 5: reconciliation of a record an earlier run left `stopping` before its start
/// concluded concludes that start too, whether its cgroup is found live or gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_process_restart_concludes_the_start_of_a_record_left_stopping() {
    for live in [true, false] {
        let mut harness = Harness::portable();
        let cwd = harness.workdir("left-stopping");
        let prefix = if live {
            "op_left_live_000"
        } else {
            "op_left_gone_000"
        };
        let id = if live {
            "proc_leftlive"
        } else {
            "proc_leftgone"
        };
        reserve_stopped_while_starting(&harness, prefix, id, &cwd);
        tokio::time::sleep(Duration::from_millis(5)).await;
        harness.restart();
        let supervisor: Arc<dyn ProcessSupervisor> = if live {
            Arc::new(FakeSupervisor::default())
        } else {
            // Both channel ends dropped: `reattach` answers `Gone` at once.
            let (entered, _) = std::sync::mpsc::channel();
            let (_, proceed) = std::sync::mpsc::channel();
            Arc::new(GatedReattach {
                entered: parking_lot::Mutex::new(entered),
                proceed: parking_lot::Mutex::new(proceed),
            })
        };
        Arc::get_mut(&mut harness.app)
            .expect("unshared application")
            .host_processes = Some(supervisor);
        let record = harness.ended(id).await;
        assert_eq!(record["ended_by"], "stop", "live {live}: {record}");
        for suffix in ["st", "sp"] {
            let (_, answer) = harness
                .call(
                    SUBJECT,
                    Method::GET,
                    &format!("/v1/ops/{prefix}_{suffix}"),
                    None,
                )
                .await;
            assert_eq!(
                answer["result"]["state"], "terminal",
                "live {live}: {answer}"
            );
        }
    }
}
