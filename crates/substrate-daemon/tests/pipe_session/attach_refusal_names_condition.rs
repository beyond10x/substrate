//! A `session.not-attachable` refusal names the condition that failed, with the observed values.
//!
//! `story:pty-session-refuses-first-attach-under-load`: a delegated run once refused the first
//! attach of a freshly started pty session with "The pty session is not running under an active
//! lease.", which is the same sentence for three different conditions — the session's state is
//! not `ready`, its attachment is not `available`, or its lease is not `active` — so nobody could
//! say which one had failed. Each case here reaches one condition deterministically and asserts
//! that the refusal names it and states the same values a read-back of the session shows. Code,
//! status, class and `retriable` are unchanged; only the free-text `error.message` says more.
//!
//! Portable lane: the fixture driver, and a wall and boot-relative clock the case moves forward
//! instead of sleeping through a lease.

use std::sync::atomic::{AtomicI64, Ordering};

use chrono::DateTime;
use substrate_daemon::{Authority, SystemAuthority};
use substrate_store::LeaseClock;

use super::{
    Arc, Body, DEPLOYMENT, Driver, ExecState, Harness, Method, PipeFixtureDriver, StatusCode,
    TestServer, Utc, Value, json, mutation, pty_start, refused_upgrade,
};

/// A clock the case moves forward: the wall time and the boot-relative lease clock together.
#[derive(Default)]
struct MovableAuthority {
    offset_ms: AtomicI64,
}

impl MovableAuthority {
    fn advance(&self, ms: i64) {
        self.offset_ms.fetch_add(ms, Ordering::SeqCst);
    }

    fn offset(&self) -> chrono::Duration {
        chrono::Duration::milliseconds(self.offset_ms.load(Ordering::SeqCst))
    }
}

impl Authority for MovableAuthority {
    fn now(&self) -> DateTime<Utc> {
        Utc::now() + self.offset()
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
        let mut clock = SystemAuthority.lease_clock()?;
        let offset = self.offset_ms.load(Ordering::SeqCst);
        clock.wall += self.offset();
        clock.boottime_ms = clock
            .boottime_ms
            .saturating_add(u64::try_from(offset).unwrap_or(0));
        Ok(clock)
    }
}

impl Harness {
    /// A pty-serving harness whose daemon reads every instant from `authority`.
    async fn with_terminals_and_clock(authority: Arc<MovableAuthority>) -> Self {
        let directory = tempfile::tempdir().expect("temporary pipe harness");
        let store = Arc::new(
            substrate_store::Store::open(directory.path().join("state.db")).expect("state store"),
        );
        let driver = PipeFixtureDriver::with_terminals(&directory.path().join("workspaces"), true);
        let app = substrate_daemon::App::with_authority(
            store,
            Arc::clone(&driver) as Arc<dyn Driver>,
            DEPLOYMENT,
            authority,
        );
        let server = TestServer::spawn(Arc::clone(&app)).await;
        Self {
            driver,
            _directory: directory,
            app,
            server,
            _containment: None,
        }
    }

    async fn start_pty_with_lease(&self, lease_ttl_ms: u64) -> (String, String) {
        let workspace = self.create_workspace("01JU2ATTACHWORKSPACE0001").await;
        let mut input = pty_start(&workspace, Some(json!({"columns": 80, "rows": 24})));
        input["exec"]["lease_ttl_ms"] = json!(lease_ttl_ms);
        let (status, session) = self
            .call(
                Method::POST,
                "/v1/sessions",
                mutation("01JU2ATTACHSESSIONSTART1", input),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{session}");
        (
            session["result"]["id"]
                .as_str()
                .expect("pty session id")
                .to_owned(),
            session["result"]["exec"]
                .as_str()
                .expect("pty exec id")
                .to_owned(),
        )
    }

    async fn session(&self, session_id: &str) -> Value {
        let (status, session) = self
            .call(
                Method::GET,
                &format!("/v1/sessions/{session_id}"),
                Body::empty(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{session}");
        session["result"].clone()
    }
}

/// Refuses one attach and returns its message, after asserting the parts that must not change.
async fn not_attachable_message(harness: &Harness, session_id: &str) -> String {
    let (status, refusal) = refused_upgrade(
        harness.server.address,
        &format!("/v1/sessions/{session_id}/attach"),
    )
    .await;
    assert_eq!(status, 409, "{refusal}");
    assert_eq!(
        refusal["error"]["code"].as_str(),
        Some(substrate_wire::SESSION_NOT_ATTACHABLE),
        "{refusal}"
    );
    assert_eq!(refusal["error"]["class"], "conflict", "{refusal}");
    assert_eq!(refusal["error"]["retriable"], false, "{refusal}");
    refusal["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("a refusal carries a message: {refusal}"))
        .to_owned()
}

/// The observed values a refusal must state, read back from the session itself.
fn observed_values(session: &Value) -> String {
    format!(
        "state {}, attachment {}, lease {}",
        session["state"].as_str().expect("session state"),
        session["attachment"].as_str().expect("session attachment"),
        session["lease"]["state"].as_str().expect("lease state"),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pty_session_whose_child_exited_before_attach_is_refused_naming_exited() {
    let harness = Harness::with_terminals_and_clock(Arc::default()).await;
    let (session_id, exec_id) = harness.start_pty_with_lease(60_000).await;
    let started = harness.session(&session_id).await;
    assert_eq!(started["state"], "ready", "{started}");
    assert_eq!(started["attachment"], "available", "{started}");

    // The child ends before any client attaches: `completed_execs()` now reports it `exited`.
    harness.driver.terminal(&exec_id, ExecState::Exited, None);
    let message = not_attachable_message(&harness, &session_id).await;

    let ended = harness.session(&session_id).await;
    assert_eq!(ended["state"], "exited", "{ended}");
    assert!(
        message.starts_with("The pty session is not attachable: its state is not ready"),
        "the refusal names the failed condition: {message:?}"
    );
    assert!(
        message.contains(&observed_values(&ended)),
        "the refusal states the observed values a read-back shows ({}): {message:?}",
        observed_values(&ended)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pty_session_whose_lease_passed_its_deadline_is_refused_naming_the_lease() {
    let authority = Arc::new(MovableAuthority::default());
    let harness = Harness::with_terminals_and_clock(Arc::clone(&authority)).await;
    let (session_id, _exec_id) = harness
        .start_pty_with_lease(substrate_wire::MIN_LEASE_TTL_MS)
        .await;
    let started = harness.session(&session_id).await;
    assert_eq!(started["lease"]["state"], "active", "{started}");

    // Past the deadline and every clock tolerance; the attach's own sweep observes it.
    authority.advance(60_000);
    let message = not_attachable_message(&harness, &session_id).await;

    let ended = harness.session(&session_id).await;
    assert_ne!(ended["lease"]["state"], "active", "{ended}");
    assert!(
        message.contains("its lease is not active"),
        "the refusal names the lease: {message:?}"
    );
    assert!(
        message.contains(&observed_values(&ended)),
        "the refusal states the observed values a read-back shows ({}): {message:?}",
        observed_values(&ended)
    );
}
