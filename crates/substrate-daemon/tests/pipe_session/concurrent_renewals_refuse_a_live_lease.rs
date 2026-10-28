//! Two renewals of one live session lease: the one that read its clock first and commits second
//! is refused `lease.expired`, although the lease it judges is active and not due.
//!
//! `pipe_session_lease_renew` (and `exec_lease_renew`) read the lease clock in `new_lease` before
//! any store call and take no workspace lock, so `ensure_lease_renewable` judges the stored lease
//! by a reading that can predate a renewal another request issued and committed in between. A
//! boot-relative time below the stored issue is a monotonic regression to `lease_due`, which
//! expires conservatively, so the earlier reading reads a live lease as expired. It is the same
//! pre-transaction reading the sweeper had before this change, on the renewal path.
//!
//! The case holds renewal A right after it reads its clock, runs renewal B to completion against a
//! clock one second later, then lets A go on. Both renewals were asked of an active lease, and the
//! lease reads active afterwards; A must not be told it expired.
//!
//! Portable lane, fixture driver, a clock the case holds and moves. It runs once.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc as std_mpsc;

use axum::Extension;
use axum::http::Request;
use chrono::DateTime;
use substrate_daemon::{Authority, SystemAuthority, router};
use substrate_store::LeaseClock;
use tokio::sync::Notify;
use tower::ServiceExt as _;

use super::{
    Arc, Body, DEPLOYMENT, Driver, Harness, Method, Mutex, PipeFixtureDriver, StatusCode,
    TestServer, Utc, Value, identity, json, mutation,
};

/// How far renewal B's clock runs ahead of the reading renewal A holds.
const B_AHEAD_MS: i64 = 1_000;

/// A system clock whose next lease reading, once armed, is held until the case releases it;
/// every reading after it runs [`B_AHEAD_MS`] ahead, wall and boot-relative together.
struct HeldRenewalAuthority {
    armed: AtomicBool,
    offset_ms: AtomicI64,
    taken: Notify,
    release: Mutex<Option<std_mpsc::Receiver<()>>>,
}

impl HeldRenewalAuthority {
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

impl Authority for HeldRenewalAuthority {
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
            // Renewal A has its reading; every later one is later. Hold A here, as a loaded host
            // deschedules a request between its clock read and its store calls.
            self.offset_ms.fetch_add(B_AHEAD_MS, Ordering::SeqCst);
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

async fn renew(
    app: Arc<substrate_daemon::App>,
    session_id: String,
    op: &str,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("/v1/sessions/{session_id}/lease/renew"))
        .header("content-type", "application/json")
        .body(mutation(op, json!({"ttl_ms": 60_000})))
        .expect("request");
    let response = router(app)
        .layer(Extension(identity()))
        .oneshot(request)
        .await
        .expect("router response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 2_097_152)
        .await
        .expect("response body");
    (
        status,
        serde_json::from_slice(&bytes).expect("JSON response"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renewal_whose_clock_predates_a_concurrent_renewal_is_not_refused_as_expired() {
    let (authority, release) = HeldRenewalAuthority::new();
    let directory = tempfile::tempdir().expect("temporary pipe harness");
    let store = Arc::new(
        substrate_store::Store::open(directory.path().join("state.db")).expect("state store"),
    );
    let driver = PipeFixtureDriver::with_terminals(&directory.path().join("workspaces"), false);
    let app = substrate_daemon::App::with_authority(
        store,
        Arc::clone(&driver) as Arc<dyn Driver>,
        DEPLOYMENT,
        Arc::clone(&authority) as Arc<dyn Authority>,
    );
    let server = TestServer::spawn(Arc::clone(&app)).await;
    let harness = Harness {
        driver,
        _directory: directory,
        app,
        server,
        _containment: None,
    };
    let (session_id, _exec_id) = harness.start_pipe().await;

    // Renewal A reads its clock, and is held there.
    authority.armed.store(true, Ordering::SeqCst);
    let renewal_a = tokio::spawn(renew(
        Arc::clone(&harness.app),
        session_id.clone(),
        "01JU3ADVRENEWALA00000001",
    ));
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        authority.taken.notified(),
    )
    .await
    .expect("renewal A reads its lease clock");

    // Renewal B, whole, against a clock later than A's reading.
    let (status, renewed_b) = renew(
        Arc::clone(&harness.app),
        session_id.clone(),
        "01JU3ADVRENEWALB00000001",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renewed_b}");

    // A goes on with the reading it took before B.
    release.send(()).expect("release the held reading");
    let (status_a, renewed_a) = tokio::time::timeout(std::time::Duration::from_secs(10), renewal_a)
        .await
        .expect("renewal A finishes")
        .expect("renewal A does not panic");

    let (status, session) = harness
        .call(
            Method::GET,
            &format!("/v1/sessions/{session_id}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(
        session["result"]["lease"]["state"], "active",
        "the lease both renewals were asked of is live: {session}"
    );
    assert_eq!(
        status_a,
        StatusCode::OK,
        "a renewal of a live lease is not refused as expired because a concurrent renewal \
         committed a later issue first: {renewed_a}"
    );
}
