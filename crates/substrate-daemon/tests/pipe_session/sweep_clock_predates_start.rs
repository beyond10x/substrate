//! A lease sweep whose clock was read before a session start never expires that start's lease.
//!
//! `story:pty-session-refuses-first-attach-under-load`: under host load a freshly started pty
//! session refused its first attach with `session.not-attachable`. A recorded read-back named all
//! three conditions — state `expired`, attachment `consumed`, lease `expired` — and an exec ended
//! by `KILL` whose expiry was observed one millisecond *before* its lease was issued. The
//! background sweeper had read its lease clock, the start had then issued and committed a lease
//! from a later reading, and the sweeper judged that lease by its older reading: a boot-relative
//! time below a lease's issue is a monotonic regression, which expires conservatively.
//!
//! This case holds the sweeper right after it reads its clock, runs a whole pty start against a
//! clock one second later, then lets the sweeper go on. The session the start returned must still
//! be ready, available and actively leased, and its first attach must be served. A sweep by a
//! reading past the lease's deadline then still expires it.
//!
//! Portable lane: the fixture driver, and a clock the case holds and moves instead of a loaded
//! host. It runs once.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc as std_mpsc;

use chrono::DateTime;
use substrate_daemon::{Authority, SystemAuthority};
use substrate_store::LeaseClock;
use tokio::sync::Notify;

use super::{
    Arc, Body, DEPLOYMENT, Driver, Handshake, Harness, Method, Mutex, PipeFixtureDriver,
    StatusCode, TestServer, Utc, Value, json, mutation, pty_start,
};

/// How far the start's clock runs ahead of the reading the sweeper holds.
const START_AHEAD_MS: i64 = 1_000;

/// A system clock whose next lease reading, once armed, is held until the case releases it, and
/// every reading after that one runs [`START_AHEAD_MS`] ahead, wall and boot-relative together.
struct HeldReadingAuthority {
    armed: AtomicBool,
    offset_ms: AtomicI64,
    taken: Notify,
    release: Mutex<Option<std_mpsc::Receiver<()>>>,
}

impl HeldReadingAuthority {
    fn new() -> (Arc<Self>, std_mpsc::Sender<()>) {
        let (release, held) = std_mpsc::channel();
        let authority = Arc::new(Self {
            armed: AtomicBool::new(false),
            offset_ms: AtomicI64::new(0),
            taken: Notify::new(),
            release: Mutex::new(Some(held)),
        });
        (authority, release)
    }

    fn offset(&self) -> chrono::Duration {
        chrono::Duration::milliseconds(self.offset_ms.load(Ordering::SeqCst))
    }
}

impl Authority for HeldReadingAuthority {
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
        if self.armed.swap(false, Ordering::SeqCst) {
            // This reading is taken; every later one is later. Hold it until the case says so,
            // as a loaded host holds a sweeper between its clock read and its store calls.
            self.offset_ms.fetch_add(START_AHEAD_MS, Ordering::SeqCst);
            let held = self
                .release
                .lock()
                .expect("release receiver")
                .take()
                .expect("one held reading");
            self.taken.notify_one();
            held.recv().expect("the case releases the held reading");
        }
        Ok(clock)
    }
}

impl Harness {
    async fn with_terminals_and_held_reading(authority: Arc<HeldReadingAuthority>) -> Self {
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

    async fn session_read_back(&self, session_id: &str) -> Value {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sweep_whose_clock_predates_a_pty_start_leaves_that_session_attachable() {
    let (authority, release) = HeldReadingAuthority::new();
    let harness = Harness::with_terminals_and_held_reading(Arc::clone(&authority)).await;
    let workspace = harness.create_workspace("01JU3SWEEPCLOCKWORKSPACE").await;

    // The sweeper reads its clock, and is held there.
    authority.armed.store(true, Ordering::SeqCst);
    let sweeper = {
        let app = Arc::clone(&harness.app);
        tokio::spawn(async move { app.sweep_expired().await })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        authority.taken.notified(),
    )
    .await
    .expect("the sweeper reads its lease clock");

    // A whole pty start, against a clock later than the held reading.
    let mut input = pty_start(&workspace, Some(json!({"columns": 80, "rows": 24})));
    input["exec"]["lease_ttl_ms"] = json!(30_000);
    let (status, started) = harness
        .call(
            Method::POST,
            "/v1/sessions",
            mutation("01JU3SWEEPCLOCKSTART0001", input),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let session_id = started["result"]["id"]
        .as_str()
        .expect("pty session id")
        .to_owned();
    assert_eq!(started["result"]["state"], "ready", "{started}");
    assert_eq!(started["result"]["lease"]["state"], "active", "{started}");

    // The sweeper goes on with the reading it took before the start.
    release.send(()).expect("release the held reading");
    tokio::time::timeout(std::time::Duration::from_secs(10), sweeper)
        .await
        .expect("the sweep finishes")
        .expect("the sweep does not panic");

    let session = harness.session_read_back(&session_id).await;
    assert_eq!(
        (
            &session["state"],
            &session["attachment"],
            &session["lease"]["state"],
            &session["exit"],
        ),
        (
            &json!("ready"),
            &json!("available"),
            &json!("active"),
            &Value::Null,
        ),
        "a sweep must not expire a lease by a clock read before that lease was issued: {session}"
    );
    let attach = Handshake::open(
        harness.server.address,
        &format!("/v1/sessions/{session_id}/attach"),
    )
    .await;
    assert_eq!(
        attach.status, 101,
        "the first attach of the session the start returned is served"
    );

    // The lease still expires when it is due by a reading taken now.
    authority.offset_ms.fetch_add(60_000, Ordering::SeqCst);
    harness.app.sweep_expired().await;
    let expired = harness.session_read_back(&session_id).await;
    assert_eq!(expired["lease"]["state"], "expired", "{expired}");
    drop(attach);
}
