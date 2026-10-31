//! Operational liveness, readiness and `OpenMetrics` telemetry (ADR 0045).
//!
//! These answer on a listener of their own, never on `router_for`: no route here is in the
//! operation registry, so no contract bundle describes or changes because of them. The catalogue
//! below is closed. Every label value is a variant of an enum in this module or a refusal code the
//! wire crate names as a constant, so no subject, tenant, uid, resource or operation id, argv
//! element, path, destination, aperture or source name, peer address, credential or error text can
//! reach a label or a value.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use parking_lot::Mutex;
use substrate_wire::{ErrorClass, ErrorDetail};
use tokio::sync::Semaphore;
use tokio::task::AbortHandle;

use super::{App, CONTRACT_BUNDLE};

/// How long the readiness store statement may take before `store` is reported failed.
const STORE_PROBE_DEADLINE: Duration = Duration::from_secs(1);
/// How recently each maintenance task must have completed a round: 20 of its 250 ms ticks.
const MAINTENANCE_FRESHNESS: Duration = Duration::from_secs(5);
/// Clock ticks per second in `/proc/self/stat`. Linux fixes `USER_HZ` at 100 for every userspace
/// interface, whatever the kernel's internal tick rate.
const USER_HZ: f64 = 100.0;

const OPENMETRICS: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";
const TEXT: &str = "text/plain; charset=utf-8";

/// The eleven histogram bounds every histogram here shares, 5 ms to 10 s.
const BUCKETS: [(f64, &str); 11] = [
    (0.005, "0.005"),
    (0.01, "0.01"),
    (0.025, "0.025"),
    (0.05, "0.05"),
    (0.1, "0.1"),
    (0.25, "0.25"),
    (0.5, "0.5"),
    (1.0, "1.0"),
    (2.5, "2.5"),
    (5.0, "5.0"),
    (10.0, "10.0"),
];

/// The control listener a request arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Listener {
    Unix,
    TcpDevelopment,
    Tls,
}

impl Listener {
    const ALL: [Self; 3] = [Self::Unix, Self::TcpDevelopment, Self::Tls];

    const fn label(self) -> &'static str {
        match self {
            Self::Unix => "unix",
            Self::TcpDevelopment => "tcp-development",
            Self::Tls => "tls",
        }
    }
}

/// A route family: the hosted admission scope its path requires, or `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Observe,
    Workspaces,
    Exec,
    Process,
    None,
}

impl Family {
    const ALL: [Self; 5] = [
        Self::Observe,
        Self::Workspaces,
        Self::Exec,
        Self::Process,
        Self::None,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Workspaces => "workspaces",
            Self::Exec => "exec",
            Self::Process => "process",
            Self::None => "none",
        }
    }

    fn of(path: &str) -> Self {
        match crate::hosted::required_scope(path) {
            Some("observe") => Self::Observe,
            Some("workspaces") => Self::Workspaces,
            Some("exec") => Self::Exec,
            Some("process") => Self::Process,
            _ => Self::None,
        }
    }
}

/// `ok`, or the class of the failure answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Ok,
    Failure(ErrorClass),
}

impl Outcome {
    const ALL: [Self; 6] = [
        Self::Ok,
        Self::Failure(ErrorClass::Refused),
        Self::Failure(ErrorClass::Conflict),
        Self::Failure(ErrorClass::Unserved),
        Self::Failure(ErrorClass::Exhausted),
        Self::Failure(ErrorClass::Failed),
    ];

    const fn index(self) -> usize {
        match self {
            Self::Ok => 0,
            Self::Failure(ErrorClass::Refused) => 1,
            Self::Failure(ErrorClass::Conflict) => 2,
            Self::Failure(ErrorClass::Unserved) => 3,
            Self::Failure(ErrorClass::Exhausted) => 4,
            Self::Failure(ErrorClass::Failed) => 5,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failure(ErrorClass::Refused) => "refused",
            Self::Failure(ErrorClass::Conflict) => "conflict",
            Self::Failure(ErrorClass::Unserved) => "unserved",
            Self::Failure(ErrorClass::Exhausted) => "exhausted",
            Self::Failure(ErrorClass::Failed) => "failed",
        }
    }

    /// The class a failure answer names; an answer that names none is read from its status.
    fn of(response: &Response) -> Self {
        if let Some(detail) = response.extensions().get::<ErrorDetail>() {
            return Self::Failure(detail.class);
        }
        let status = response.status();
        if status.is_server_error() {
            Self::Failure(ErrorClass::Failed)
        } else if status.is_client_error() {
            Self::Failure(ErrorClass::Refused)
        } else {
            Self::Ok
        }
    }
}

/// The two maintenance tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaintenanceTask {
    LeaseSweep,
    Containment,
}

impl MaintenanceTask {
    const ALL: [Self; 2] = [Self::LeaseSweep, Self::Containment];

    const fn label(self) -> &'static str {
        match self {
            Self::LeaseSweep => "lease-sweep",
            Self::Containment => "containment",
        }
    }
}

/// The result of one hosted admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    Admitted,
    CredentialAbsent,
    AuthorityInvalid,
    ScopeDenied,
    AuthorityUnavailable,
}

impl Admission {
    const ALL: [Self; 5] = [
        Self::Admitted,
        Self::CredentialAbsent,
        Self::AuthorityInvalid,
        Self::ScopeDenied,
        Self::AuthorityUnavailable,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::CredentialAbsent => "credential-absent",
            Self::AuthorityInvalid => "authority-invalid",
            Self::ScopeDenied => "scope-denied",
            Self::AuthorityUnavailable => "authority-unavailable",
        }
    }
}

/// The four readiness checks, in the order `/readyz` states them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    Store,
    Schema,
    Maintenance,
    Driver,
}

impl Check {
    const ALL: [Self; 4] = [Self::Store, Self::Schema, Self::Maintenance, Self::Driver];

    const fn label(self) -> &'static str {
        match self {
            Self::Store => "store",
            Self::Schema => "schema",
            Self::Maintenance => "maintenance",
            Self::Driver => "driver",
        }
    }
}

/// Every code `substrate_refusals_total` names: the codes the wire crate names as constants.
/// Any other code is counted as `other`. `tests::the_refusal_codes_are_the_wire_constants` holds
/// this list to the wire crate's source, so a constant added there fails until it is added here.
const REFUSAL_CODES: [&str; 112] = [
    substrate_wire::EXEC_METRICS_UNSERVED,
    substrate_wire::EXEC_SCRATCH_QUOTA_UNSERVED,
    substrate_wire::EXEC_SCRATCH_QUOTA_LIMIT,
    substrate_wire::EXEC_WORKSPACE_ACCESS_INVALID,
    substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED,
    substrate_wire::EXEC_WORKSPACE_SUBTREE_ABSENT,
    substrate_wire::EXEC_WORKSPACE_SUBTREE_SYMLINK,
    substrate_wire::EXEC_WORKSPACE_SUBTREE_NOT_DIRECTORY,
    substrate_wire::EXEC_WORKSPACE_SUBTREE_CHANGED,
    substrate_wire::EVENT_STREAM_CAPACITY,
    substrate_wire::METRICS_STREAM_CAPACITY,
    substrate_wire::EXEC_WORKDIR_INVALID,
    substrate_wire::EXEC_WORKDIR_OUTSIDE_MOUNTS,
    substrate_wire::EXEC_WORKDIR_MISSING,
    substrate_wire::EXEC_WORKDIR_UNSERVED,
    substrate_wire::EXEC_APERTURES_TOO_FEW,
    substrate_wire::EXEC_APERTURES_TOO_MANY,
    substrate_wire::EXEC_APERTURE_LISTED_TWICE,
    substrate_wire::EXEC_APERTURE_HOST_LISTED_TWICE,
    substrate_wire::READ_ONLY_ROOT_INVALID,
    substrate_wire::READ_ONLY_ROOT_ABSENT,
    substrate_wire::READ_ONLY_ROOT_NOT_A_DIRECTORY,
    substrate_wire::READ_ONLY_FILES_EXCEEDED,
    substrate_wire::READ_ONLY_FILE_PATH_INVALID,
    substrate_wire::READ_ONLY_FILE_MOUNT_RESERVED,
    substrate_wire::READ_ONLY_FILE_MOUNT_OVERLAP,
    substrate_wire::READ_ONLY_FILE_MOUNT_DUPLICATE,
    substrate_wire::READ_ONLY_FILES_UNSERVED,
    substrate_wire::READ_ONLY_FILE_ABSENT,
    substrate_wire::READ_ONLY_FILE_SYMLINK,
    substrate_wire::READ_ONLY_FILE_NOT_A_FILE,
    substrate_wire::READ_ONLY_FILE_PIN_FAILED,
    substrate_wire::EXEC_WRITABLE_ROOTS_EXCEEDED,
    substrate_wire::EXEC_WRITABLE_ROOT_NAME_INVALID,
    substrate_wire::EXEC_WRITABLE_ROOT_MOUNT_INVALID,
    substrate_wire::EXEC_WRITABLE_ROOT_MOUNT_RESERVED,
    substrate_wire::EXEC_WRITABLE_ROOT_MOUNT_DUPLICATE,
    substrate_wire::EXEC_WRITABLE_ROOT_MOUNT_OVERLAP,
    substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED,
    substrate_wire::EXEC_WRITABLE_ROOTS_GIT_UNSERVED,
    substrate_wire::EXEC_WRITABLE_ROOT_ABSENT,
    substrate_wire::EXEC_WRITABLE_ROOT_NOT_A_DIRECTORY,
    substrate_wire::EXEC_WRITABLE_ROOT_NOT_OWNED,
    substrate_wire::EXEC_WRITABLE_ROOT_NOT_GIT,
    substrate_wire::EXEC_WRITABLE_ROOT_OWN_WORKSPACE,
    substrate_wire::EXEC_WRITABLE_ROOT_WORKSPACE_DUPLICATE,
    substrate_wire::SESSION_WINDOW_INVALID,
    substrate_wire::SESSION_PTY_UNSERVED,
    substrate_wire::SESSION_PTY_EXHAUSTED,
    substrate_wire::SESSION_PTY_FAILED,
    substrate_wire::SESSION_RESIZE_INVALID,
    substrate_wire::SESSION_NOT_PTY,
    substrate_wire::SESSION_PTY_ENDED,
    substrate_wire::SESSION_RESIZE_FAILED,
    substrate_wire::SESSION_INPUT_CLOSE_UNSERVED,
    substrate_wire::SESSION_OUTPUT_LIMIT,
    substrate_wire::SESSION_OUTPUT_BACKPRESSURE,
    substrate_wire::AUTH_CREDENTIAL_ABSENT,
    substrate_wire::AUTH_AUTHORITY_INVALID,
    substrate_wire::AUTH_SCOPE_DENIED,
    substrate_wire::AUTH_AUTHORITY_UNAVAILABLE,
    substrate_wire::SESSION_AUTHORITY_ABSENT,
    substrate_wire::SESSION_AUTHORITY_EXPIRED,
    substrate_wire::SESSION_AUTHORITY_REDEEMED,
    substrate_wire::SESSION_AUTHORITY_UNBOUND,
    substrate_wire::SESSION_NOT_ATTACHABLE,
    substrate_wire::SESSION_ALREADY_ATTACHED,
    substrate_wire::SESSION_ATTACHMENT_CAPACITY,
    substrate_wire::SESSION_CONFINEMENT_UNAVAILABLE,
    substrate_wire::SESSION_LIMIT_UNSERVED,
    substrate_wire::SESSION_STDIN_MISSING,
    substrate_wire::SESSION_UNSERVED,
    substrate_wire::SESSION_WAIT_INVALID,
    substrate_wire::SESSION_CONTROL_RATE_EXCEEDED,
    substrate_wire::SESSION_DRIVER_REFUSED,
    substrate_wire::SESSION_FRAME_INVALID,
    substrate_wire::SESSION_SEQUENCE_INVALID,
    substrate_wire::SESSION_BASE64_INVALID,
    substrate_wire::SESSION_SIGNAL_INVALID,
    substrate_wire::SESSION_INPUT_CLOSED,
    substrate_wire::SESSION_NOT_PIPE,
    substrate_wire::SESSION_FRAME_LIMIT,
    substrate_wire::SESSION_INPUT_LIMIT,
    substrate_wire::SESSION_WRITE_FAILED,
    substrate_wire::SESSION_READ_TIMEOUT,
    substrate_wire::SESSION_TIMEOUT_INVALID,
    substrate_wire::SESSION_CAPTURE_DISALLOWED,
    substrate_wire::SESSION_CAPTURE_UNSERVED,
    substrate_wire::PROCESS_HOST_UNSERVED,
    substrate_wire::PROCESS_HOST_POSTURE_REFUSED,
    substrate_wire::PROCESS_HOST_SUBJECT_MISMATCH,
    substrate_wire::PROCESS_PROFILE_REQUIRED,
    substrate_wire::PROCESS_PROFILE_UNKNOWN,
    substrate_wire::PROCESS_ARGV_INVALID,
    substrate_wire::PROCESS_CWD_INVALID,
    substrate_wire::PROCESS_ENV_INVALID,
    substrate_wire::PROCESS_OWNER_INVALID,
    substrate_wire::PROCESS_REASON_REQUIRED,
    substrate_wire::PROCESS_NAME_CONFLICT,
    substrate_wire::PROCESS_LEASE_INVALID,
    substrate_wire::PROCESS_KEEP_REASON_REQUIRED,
    substrate_wire::PROCESS_LIMIT_EXCEEDED,
    substrate_wire::PROCESS_NOT_FOUND,
    substrate_wire::PROCESS_TIMEOUT_INVALID,
    substrate_wire::PROCESS_EXPIRES_AT_INVALID,
    substrate_wire::PROCESS_EVENTS_READERS_EXCEEDED,
    substrate_wire::LEASE_EXPIRED,
    substrate_wire::EXEC_INLINE_CAPSULE_UNSERVED,
    substrate_wire::EXEC_SIGNAL_UNSERVED,
    substrate_wire::WORKSPACE_ADOPT_UNSERVED,
    substrate_wire::CONFINEMENT_FAMILY_UNSERVED,
    substrate_wire::CONFINEMENT_FACT_UNSERVED,
];

/// One counter per label combination.
fn counters<const N: usize>() -> [AtomicU64; N] {
    std::array::from_fn(|_| AtomicU64::new(0))
}

/// A fixed-bucket histogram: per-bucket counts, the observations above the last bound and the
/// summed nanoseconds. The observation count is not stored: a render derives it from the buckets
/// it read, so `+Inf` and `_count` are never below a finite bucket, whatever a concurrent
/// observation is doing.
struct Histogram {
    buckets: [AtomicU64; 11],
    overflow: AtomicU64,
    sum_nanos: AtomicU64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            buckets: counters(),
            overflow: AtomicU64::new(0),
            sum_nanos: AtomicU64::new(0),
        }
    }
}

impl Histogram {
    fn observe(&self, elapsed: Duration) {
        let seconds = elapsed.as_secs_f64();
        match BUCKETS.iter().position(|(bound, _)| seconds <= *bound) {
            Some(bucket) => self.buckets[bucket].fetch_add(1, Ordering::Relaxed),
            None => self.overflow.fetch_add(1, Ordering::Relaxed),
        };
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        let _ = self
            .sum_nanos
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |sum| {
                Some(sum.saturating_add(nanos))
            });
    }
}

/// What a control listener's transport bound holds: its global semaphore and size, and how many
/// connections it turned away at capacity.
#[derive(Clone)]
pub(crate) struct ConnectionGauge {
    pub(crate) global: Arc<Semaphore>,
    pub(crate) limit: usize,
    pub(crate) rejected: Arc<AtomicU64>,
}

impl ConnectionGauge {
    pub(crate) fn open(&self) -> usize {
        self.limit.saturating_sub(self.global.available_permits())
    }
}

#[derive(Default)]
struct MaintenanceRecord {
    rounds: AtomicU64,
    duration: Histogram,
    /// The monotonic instant and the wall-clock second the last round completed.
    last: Mutex<Option<(Instant, f64)>>,
}

/// The daemon's static metric registry (ADR 0045 decision 7), held by [`App`].
pub(crate) struct Telemetry {
    requests: [AtomicU64; 3 * 5 * 6],
    request_duration: [Histogram; 3 * 5],
    refusals: [AtomicU64; REFUSAL_CODES.len() + 1],
    dispatch: [AtomicU64; 5 * 6],
    dispatch_duration: [Histogram; 5],
    connections: [Mutex<Option<ConnectionGauge>>; 3],
    maintenance: [MaintenanceRecord; 2],
    admissions: [AtomicU64; 5],
    resolution: Histogram,
    /// Whether a control listener is bound and not shutting down: readiness is `503` otherwise.
    accepting: AtomicBool,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            requests: counters(),
            request_duration: std::array::from_fn(|_| Histogram::default()),
            refusals: counters(),
            dispatch: counters(),
            dispatch_duration: std::array::from_fn(|_| Histogram::default()),
            connections: std::array::from_fn(|_| Mutex::new(None)),
            maintenance: std::array::from_fn(|_| MaintenanceRecord::default()),
            admissions: counters(),
            resolution: Histogram::default(),
            accepting: AtomicBool::new(false),
        }
    }
}

impl Telemetry {
    fn request(&self, listener: Listener, family: Family, outcome: Outcome, elapsed: Duration) {
        let route = listener as usize * Family::ALL.len() + family as usize;
        self.requests[route * Outcome::ALL.len() + outcome.index()].fetch_add(1, Ordering::Relaxed);
        self.request_duration[route].observe(elapsed);
    }

    fn refusal(&self, code: &str) {
        let index = REFUSAL_CODES
            .iter()
            .position(|known| *known == code)
            .unwrap_or(REFUSAL_CODES.len());
        self.refusals[index].fetch_add(1, Ordering::Relaxed);
    }

    /// A failure answer that names no code: counted under `other`.
    fn unnamed_refusal(&self) {
        self.refusals[REFUSAL_CODES.len()].fetch_add(1, Ordering::Relaxed);
    }

    fn dispatch(&self, family: Family, outcome: Outcome, elapsed: Duration) {
        self.dispatch[family as usize * Outcome::ALL.len() + outcome.index()]
            .fetch_add(1, Ordering::Relaxed);
        self.dispatch_duration[family as usize].observe(elapsed);
    }

    /// Publishes a control listener's transport bound, once it exists.
    pub(crate) fn register_listener(&self, listener: Listener, gauge: ConnectionGauge) {
        *self.connections[listener as usize].lock() = Some(gauge);
    }

    /// Records one completed maintenance round.
    pub(crate) fn maintenance_round(&self, task: MaintenanceTask, elapsed: Duration) {
        let record = &self.maintenance[task as usize];
        record.rounds.fetch_add(1, Ordering::Relaxed);
        record.duration.observe(elapsed);
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64());
        *record.last.lock() = Some((Instant::now(), wall));
    }

    fn maintenance_fresh(&self, task: MaintenanceTask) -> bool {
        self.maintenance[task as usize]
            .last
            .lock()
            .is_some_and(|(at, _)| at.elapsed() <= MAINTENANCE_FRESHNESS)
    }

    /// Records one hosted admission and, when Identity was asked, how long it took to answer.
    pub(crate) fn admission(&self, result: Admission, resolution: Option<Duration>) {
        self.admissions[result as usize].fetch_add(1, Ordering::Relaxed);
        if let Some(elapsed) = resolution {
            self.resolution.observe(elapsed);
        }
    }

    /// A control listener is bound: readiness may answer `200`.
    pub(crate) fn start_accepting(&self) {
        self.accepting.store(true, Ordering::Release);
    }

    /// Shutdown has begun: readiness answers `503` from now on, before any listener closes.
    pub(crate) fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::Release);
    }

    /// `collisions` is what the driver port answers (`Driver::backend_collisions`): a driver that
    /// creates no backend object answers `None` and its scrape carries no collision series.
    #[allow(clippy::too_many_lines)] // The closed catalogue, in the order ADR 0045 states it.
    fn render(
        &self,
        ready: Readiness,
        process: Option<&ProcessStats>,
        output_stalls: u64,
        collisions: Option<u64>,
    ) -> String {
        let mut out = Exposition::default();
        out.family("substrate_build_info", "gauge", "The running binary.");
        out.sample(
            "substrate_build_info",
            &[
                ("version", env!("CARGO_PKG_VERSION")),
                ("contract_bundle", CONTRACT_BUNDLE),
            ],
            1,
        );
        out.family(
            "substrate_ready",
            "gauge",
            "Whether each readiness check passes.",
        );
        for (check, passed) in Check::ALL.into_iter().zip(ready.checks) {
            out.sample(
                "substrate_ready",
                &[("check", check.label())],
                u8::from(passed),
            );
        }
        out.family(
            "substrate_http_requests",
            "counter",
            "Control requests answered, by listener, route family and outcome.",
        );
        for listener in Listener::ALL {
            for family in Family::ALL {
                for outcome in Outcome::ALL {
                    let route = listener as usize * Family::ALL.len() + family as usize;
                    out.sample(
                        "substrate_http_requests_total",
                        &[
                            ("listener", listener.label()),
                            ("family", family.label()),
                            ("outcome", outcome.label()),
                        ],
                        self.requests[route * Outcome::ALL.len() + outcome.index()]
                            .load(Ordering::Relaxed),
                    );
                }
            }
        }
        out.family(
            "substrate_http_request_duration_seconds",
            "histogram",
            "Control request latency, by listener and route family.",
        );
        for listener in Listener::ALL {
            for family in Family::ALL {
                out.histogram(
                    "substrate_http_request_duration_seconds",
                    &[("listener", listener.label()), ("family", family.label())],
                    &self.request_duration[listener as usize * Family::ALL.len() + family as usize],
                );
            }
        }
        out.family(
            "substrate_refusals",
            "counter",
            "Failure answers, by the code the wire crate names, otherwise other.",
        );
        for (index, code) in REFUSAL_CODES.iter().chain(["other"].iter()).enumerate() {
            out.sample(
                "substrate_refusals_total",
                &[("code", code)],
                self.refusals[index].load(Ordering::Relaxed),
            );
        }
        out.family(
            "substrate_dispatch",
            "counter",
            "Driver dispatches after durable admission, by route family and outcome.",
        );
        for family in Family::ALL {
            for outcome in Outcome::ALL {
                out.sample(
                    "substrate_dispatch_total",
                    &[("family", family.label()), ("outcome", outcome.label())],
                    self.dispatch[family as usize * Outcome::ALL.len() + outcome.index()]
                        .load(Ordering::Relaxed),
                );
            }
        }
        out.family(
            "substrate_dispatch_duration_seconds",
            "histogram",
            "Time from durable admission to the answer, by route family.",
        );
        for family in Family::ALL {
            out.histogram(
                "substrate_dispatch_duration_seconds",
                &[("family", family.label())],
                &self.dispatch_duration[family as usize],
            );
        }
        let gauges =
            Listener::ALL.map(|listener| self.connections[listener as usize].lock().clone());
        out.family(
            "substrate_connections_open",
            "gauge",
            "Connections holding a control listener's transport admission.",
        );
        for (listener, gauge) in Listener::ALL.into_iter().zip(&gauges) {
            out.sample(
                "substrate_connections_open",
                &[("listener", listener.label())],
                gauge.as_ref().map_or(0, ConnectionGauge::open),
            );
        }
        out.family(
            "substrate_connections_limit",
            "gauge",
            "A control listener's global connection bound; 0 when it is not bound.",
        );
        for (listener, gauge) in Listener::ALL.into_iter().zip(&gauges) {
            out.sample(
                "substrate_connections_limit",
                &[("listener", listener.label())],
                gauge.as_ref().map_or(0, |gauge| gauge.limit),
            );
        }
        out.family(
            "substrate_connections_rejected",
            "counter",
            "Connections a control listener turned away at its transport bound.",
        );
        for (listener, gauge) in Listener::ALL.into_iter().zip(&gauges) {
            out.sample(
                "substrate_connections_rejected_total",
                &[("listener", listener.label())],
                gauge
                    .as_ref()
                    .map_or(0, |gauge| gauge.rejected.load(Ordering::Relaxed)),
            );
        }
        out.family(
            "substrate_output_stalls",
            "counter",
            "Executions ended because live output was not drained within its stall deadline.",
        );
        out.sample("substrate_output_stalls_total", &[], output_stalls);
        out.family(
            "substrate_maintenance_rounds",
            "counter",
            "Completed maintenance rounds, by task.",
        );
        for task in MaintenanceTask::ALL {
            out.sample(
                "substrate_maintenance_rounds_total",
                &[("task", task.label())],
                self.maintenance[task as usize]
                    .rounds
                    .load(Ordering::Relaxed),
            );
        }
        out.family(
            "substrate_maintenance_round_duration_seconds",
            "histogram",
            "Maintenance round duration, by task.",
        );
        for task in MaintenanceTask::ALL {
            out.histogram(
                "substrate_maintenance_round_duration_seconds",
                &[("task", task.label())],
                &self.maintenance[task as usize].duration,
            );
        }
        out.family(
            "substrate_maintenance_last_round_timestamp_seconds",
            "gauge",
            "When each maintenance task last completed a round; 0 before its first.",
        );
        for task in MaintenanceTask::ALL {
            let last = self.maintenance[task as usize]
                .last
                .lock()
                .map_or(0.0, |(_, wall)| wall);
            out.sample(
                "substrate_maintenance_last_round_timestamp_seconds",
                &[("task", task.label())],
                last,
            );
        }
        out.family(
            "substrate_hosted_admissions",
            "counter",
            "Hosted Identity admissions, by result.",
        );
        for result in Admission::ALL {
            out.sample(
                "substrate_hosted_admissions_total",
                &[("result", result.label())],
                self.admissions[result as usize].load(Ordering::Relaxed),
            );
        }
        out.family(
            "substrate_hosted_resolution_duration_seconds",
            "histogram",
            "Time Identity took to resolve one presented credential.",
        );
        out.histogram(
            "substrate_hosted_resolution_duration_seconds",
            &[],
            &self.resolution,
        );
        // Design 34 § 4's addition to the catalogue: published only by a driver that counts.
        if let Some(collisions) = collisions {
            out.family(
                "substrate_kubernetes_collisions",
                "counter",
                "Objects the Kubernetes driver met under a name it would use and did not create, \
                 each left untouched.",
            );
            out.sample("substrate_kubernetes_collisions_total", &[], collisions);
        }
        if let Some(process) = process {
            out.family(
                "process_cpu_seconds",
                "counter",
                "User and system CPU time.",
            );
            out.sample("process_cpu_seconds_total", &[], process.cpu_seconds);
            out.family("process_resident_memory_bytes", "gauge", "Resident memory.");
            out.sample("process_resident_memory_bytes", &[], process.resident_bytes);
            out.family("process_open_fds", "gauge", "Open file descriptors.");
            out.sample("process_open_fds", &[], process.open_fds);
            out.family(
                "process_max_fds",
                "gauge",
                "The open file descriptor limit.",
            );
            out.sample("process_max_fds", &[], process.max_fds);
            out.family(
                "process_start_time_seconds",
                "gauge",
                "When the process started.",
            );
            out.sample(
                "process_start_time_seconds",
                &[],
                process.start_time_seconds,
            );
        }
        out.0.push_str("# EOF\n");
        out.0
    }
}

/// `OpenMetrics` text, written from the closed catalogue only.
#[derive(Default)]
struct Exposition(String);

impl Exposition {
    fn family(&mut self, name: &str, kind: &str, help: &str) {
        let _ = writeln!(self.0, "# TYPE {name} {kind}");
        let _ = writeln!(self.0, "# HELP {name} {help}");
    }

    fn sample(&mut self, name: &str, labels: &[(&str, &str)], value: impl std::fmt::Display) {
        self.0.push_str(name);
        if !labels.is_empty() {
            self.0.push('{');
            for (index, (label, value)) in labels.iter().enumerate() {
                if index > 0 {
                    self.0.push(',');
                }
                let _ = write!(self.0, "{label}=\"{value}\"");
            }
            self.0.push('}');
        }
        let _ = writeln!(self.0, " {value}");
    }

    fn histogram(&mut self, name: &str, labels: &[(&str, &str)], histogram: &Histogram) {
        let bucket = format!("{name}_bucket");
        let mut cumulative = 0_u64;
        for ((_, le), count) in BUCKETS.iter().zip(&histogram.buckets) {
            cumulative = cumulative.saturating_add(count.load(Ordering::Relaxed));
            let mut with_le = labels.to_vec();
            with_le.push(("le", le));
            self.sample(&bucket, &with_le, cumulative);
        }
        let count = cumulative.saturating_add(histogram.overflow.load(Ordering::Relaxed));
        let mut with_le = labels.to_vec();
        with_le.push(("le", "+Inf"));
        self.sample(&bucket, &with_le, count);
        #[allow(clippy::cast_precision_loss)] // A sum of nanoseconds, published as seconds.
        let sum = histogram.sum_nanos.load(Ordering::Relaxed) as f64 / 1e9;
        self.sample(&format!("{name}_sum"), labels, sum);
        self.sample(&format!("{name}_count"), labels, count);
    }
}

tokio::task_local! {
    /// When the request being answered passed durable admission, if it has.
    static DISPATCH: Arc<Mutex<Option<Instant>>>;
}

/// Marks the request being answered as durably admitted: its answer is a driver dispatch.
///
/// Called where a reservation is accepted. Outside a request the control listeners instrument
/// (a maintenance round, a test driving a handler directly) it records nothing.
pub(super) fn dispatch_admitted() {
    let _ = DISPATCH.try_with(|started| {
        started.lock().get_or_insert_with(Instant::now);
    });
}

/// The state of the request-outcome layer every control listener wraps its service in.
#[derive(Clone)]
pub(crate) struct RequestMetrics {
    telemetry: Arc<Telemetry>,
    listener: Listener,
}

impl RequestMetrics {
    pub(crate) fn new(app: &App, listener: Listener) -> Self {
        Self {
            telemetry: Arc::clone(&app.telemetry),
            listener,
        }
    }
}

/// Counts one control request: its outcome, latency, refusal code and, when it was durably
/// admitted, its dispatch. It reads the path only to choose a family and keeps nothing of it.
pub(crate) async fn record_request(
    State(metrics): State<RequestMetrics>,
    request: Request,
    next: Next,
) -> Response {
    let family = Family::of(request.uri().path());
    let started = Instant::now();
    let admitted = Arc::new(Mutex::new(None));
    let response = DISPATCH
        .scope(Arc::clone(&admitted), next.run(request))
        .await;
    let outcome = Outcome::of(&response);
    metrics
        .telemetry
        .request(metrics.listener, family, outcome, started.elapsed());
    // Every failure answer is counted once among the refusals: by its named code, or `other`
    // when it carries none (the router's `405`, the development listener's `401`).
    if let Some(detail) = response.extensions().get::<ErrorDetail>() {
        metrics.telemetry.refusal(&detail.code);
    } else if outcome != Outcome::Ok {
        metrics.telemetry.unnamed_refusal();
    }
    if let Some(at) = *admitted.lock() {
        metrics.telemetry.dispatch(family, outcome, at.elapsed());
    }
    response
}

/// The result of one readiness evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Readiness {
    checks: [bool; 4],
    accepting: bool,
}

impl Readiness {
    fn ready(self) -> bool {
        self.accepting && self.checks.iter().all(|passed| *passed)
    }

    fn body(self) -> String {
        let mut body = String::new();
        for (check, passed) in Check::ALL.into_iter().zip(self.checks) {
            let verdict = if passed { "ok" } else { "failed" };
            let _ = writeln!(body, "{} {verdict}", check.label());
        }
        body
    }
}

/// Reads the schema version, waiting at most the given time for the ledger connection; `None`
/// when it was not free in time.
type StoreProbe = Arc<dyn Fn(Duration) -> Result<Option<i64>, ()> + Send + Sync>;

/// The facts of the startup capability snapshot readiness reads (D5 A). The snapshot is taken
/// once, when the driver's open sequence has finished or failed (design 34 § 6.1); until then
/// the `driver` check fails. A host driver's open finishes before the daemon serves, so its
/// snapshot is taken as the operational listener starts.
#[derive(Clone, Default)]
pub(crate) struct StartupFacts(Arc<std::sync::OnceLock<BTreeSet<String>>>);

impl StartupFacts {
    /// Takes the snapshot `facts` publish, the first time only: a later call changes nothing,
    /// because readiness does not re-read the driver (D5 B is deferred).
    pub(crate) fn take(&self, facts: &substrate_wire::CapabilityFacts) {
        self.0.get_or_init(|| present_facts(facts));
    }
}

/// What the operational listener answers from: the application, the maintenance tasks it watches,
/// and the facts readiness requires of the startup capability snapshot (D5 A).
pub(crate) struct Operations {
    app: Arc<App>,
    maintenance: Vec<AbortHandle>,
    required_facts: Vec<String>,
    startup: StartupFacts,
    store_probe: StoreProbe,
    output_stalls: fn() -> u64,
}

impl Operations {
    pub(crate) fn new(
        app: Arc<App>,
        maintenance: Vec<AbortHandle>,
        required_facts: Vec<String>,
        startup: StartupFacts,
        output_stalls: fn() -> u64,
    ) -> Arc<Self> {
        let store = Arc::clone(&app.store);
        Self::with_store_probe(
            app,
            maintenance,
            required_facts,
            startup,
            Arc::new(move |wait| store.schema_version(wait).map_err(|_| ())),
            output_stalls,
        )
    }

    fn with_store_probe(
        app: Arc<App>,
        maintenance: Vec<AbortHandle>,
        required_facts: Vec<String>,
        startup: StartupFacts,
        store_probe: StoreProbe,
        output_stalls: fn() -> u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            app,
            maintenance,
            required_facts,
            startup,
            store_probe,
            output_stalls,
        })
    }

    fn live(&self) -> bool {
        !self.maintenance.iter().any(AbortHandle::is_finished)
    }

    /// One read-only statement on the ledger connection: whether it answered within its deadline,
    /// and whether the schema version it read is the one this binary opens. It waits for the
    /// connection at most the deadline and starts no task, so nothing outlives the answer.
    fn probe_store(&self) -> (bool, bool) {
        let started = Instant::now();
        let version = tokio::task::block_in_place(|| (self.store_probe)(STORE_PROBE_DEADLINE));
        match version {
            Ok(Some(version)) if started.elapsed() <= STORE_PROBE_DEADLINE => {
                (true, version == substrate_store::SCHEMA_VERSION)
            }
            _ => (false, false),
        }
    }

    fn readiness(&self) -> Readiness {
        let (store, schema) = self.probe_store();
        let maintenance = self.live()
            && MaintenanceTask::ALL
                .into_iter()
                .all(|task| self.app.telemetry.maintenance_fresh(task));
        let driver = self.startup.0.get().is_some_and(|present| {
            self.required_facts
                .iter()
                .all(|fact| present.contains(fact))
        });
        Readiness {
            checks: [store, schema, maintenance, driver],
            accepting: self.app.telemetry.accepting.load(Ordering::Acquire),
        }
    }

    /// Answers one request on the operational listener. `GET` on `/livez`, `/readyz` or
    /// `/metrics`, with no body and no upgrade; everything else is `404` with an empty body. No
    /// header is read but the method, path and the three that would carry a body or an upgrade.
    ///
    /// Readiness and metrics wait on the ledger connection in place, so this runs on a
    /// multi-threaded runtime.
    pub(crate) fn answer<B>(&self, request: &axum::http::Request<B>) -> Response {
        let headers = request.headers();
        let body_declared = headers.contains_key(header::TRANSFER_ENCODING)
            || headers
                .get(header::CONTENT_LENGTH)
                .is_some_and(|length| length.as_bytes() != b"0");
        if request.method() != Method::GET || body_declared || headers.contains_key(header::UPGRADE)
        {
            return plain(StatusCode::NOT_FOUND, None, Vec::new());
        }
        match request.uri().path() {
            "/livez" => {
                if self.live() {
                    plain(StatusCode::OK, Some(TEXT), b"ok\n".to_vec())
                } else {
                    plain(
                        StatusCode::SERVICE_UNAVAILABLE,
                        Some(TEXT),
                        b"failed\n".to_vec(),
                    )
                }
            }
            "/readyz" => {
                let readiness = self.readiness();
                let status = if readiness.ready() {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                };
                plain(status, Some(TEXT), readiness.body().into_bytes())
            }
            "/metrics" => {
                let readiness = self.readiness();
                let exposition = self.app.telemetry.render(
                    readiness,
                    process_stats().as_ref(),
                    (self.output_stalls)(),
                    self.app.driver.backend_collisions(),
                );
                plain(StatusCode::OK, Some(OPENMETRICS), exposition.into_bytes())
            }
            _ => plain(StatusCode::NOT_FOUND, None, Vec::new()),
        }
    }
}

fn plain(status: StatusCode, content_type: Option<&'static str>, body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    if let Some(content_type) = content_type {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    }
    response
}

/// The fact names a snapshot publishes: a key whose value is neither absent nor `false`.
fn present_facts(facts: &substrate_wire::CapabilityFacts) -> BTreeSet<String> {
    match serde_json::to_value(facts) {
        Ok(serde_json::Value::Object(map)) => map
            .into_iter()
            .filter(|(_, value)| {
                !matches!(
                    value,
                    serde_json::Value::Null | serde_json::Value::Bool(false)
                )
            })
            .map(|(name, _)| name)
            .collect(),
        _ => BTreeSet::new(),
    }
}

/// Whether `name` is one of the closed set of capability fact names; startup refuses any other,
/// since a name outside the set is never published and would leave the daemon unready for life.
pub(crate) fn is_capability_fact_name(name: &str) -> bool {
    capability_fact_names().contains(&name)
}

/// The capability fact names, read from `CapabilityFacts`' own serde declaration, so a fact the
/// wire crate adds is accepted without a second list to keep in step.
fn capability_fact_names() -> &'static [&'static str] {
    use serde::Deserialize as _;
    use serde::de::{self, Visitor};

    /// A deserializer that records the field names a struct asks for and deserializes nothing.
    struct FieldNames<'a>(&'a mut &'static [&'static str]);

    impl<'de> de::Deserializer<'de> for FieldNames<'_> {
        type Error = de::value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
            Err(de::Error::custom("field names only"))
        }

        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, Self::Error> {
            *self.0 = fields;
            Err(de::Error::custom("field names only"))
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
            option unit unit_struct newtype_struct seq tuple tuple_struct map enum identifier
            ignored_any
        }
    }

    static NAMES: std::sync::OnceLock<&'static [&'static str]> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let mut fields: &'static [&'static str] = &[];
        let _ = substrate_wire::CapabilityFacts::deserialize(FieldNames(&mut fields));
        fields
    })
}

/// The conventional process metrics, read from `/proc/self`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ProcessStats {
    cpu_seconds: f64,
    resident_bytes: u64,
    open_fds: u64,
    max_fds: u64,
    start_time_seconds: f64,
}

/// `None` when any of them cannot be read, and then none of them is published.
#[allow(clippy::cast_precision_loss)] // Clock ticks and boot seconds are far below 2^52.
fn process_stats() -> Option<ProcessStats> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields = stat
        .get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .collect::<Vec<_>>();
    // Fields 14, 15 and 22 of proc(5), counted from the state field, which is field 3.
    let ticks = |field: usize| fields.get(field - 3)?.parse::<u64>().ok();
    let cpu_seconds = (ticks(14)?.saturating_add(ticks(15)?)) as f64 / USER_HZ;
    let started_ticks = ticks(22)?;
    let boot = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime ")?.trim().parse::<u64>().ok())?;
    let resident_kib = std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")?
                .trim()
                .strip_suffix("kB")?
                .trim()
                .parse::<u64>()
                .ok()
        })?;
    let max_fds = std::fs::read_to_string("/proc/self/limits")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("Max open files")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })?;
    let open_fds = u64::try_from(std::fs::read_dir("/proc/self/fd").ok()?.count()).ok()?;
    Some(ProcessStats {
        cpu_seconds,
        resident_bytes: resident_kib.saturating_mul(1024),
        open_fds,
        max_fds,
        start_time_seconds: boot as f64 + started_ticks as f64 / USER_HZ,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::http::{Request, StatusCode};
    use substrate_wire::{ErrorClass, ErrorDetail};

    use super::{
        Check, MaintenanceTask, Operations, ProcessStats, REFUSAL_CODES, Readiness, StartupFacts,
        StoreProbe, Telemetry,
    };
    use crate::app::App;

    /// ADR 0045 decision 7: 471 samples without `substrate_refusals_total`, under 500, and one
    /// more per refusal code the wire crate names plus `other`.
    const SERIES_WITHOUT_REFUSALS: usize = 471;
    const SERIES_MAXIMUM: usize = SERIES_WITHOUT_REFUSALS + REFUSAL_CODES.len() + 1;
    /// A daemon whose driver counts collisions (design 34 § 4) carries one series more.
    const KUBERNETES_SERIES_MAXIMUM: usize = SERIES_MAXIMUM + 1;

    fn every_check_passes() -> Readiness {
        Readiness {
            checks: [true; 4],
            accepting: true,
        }
    }

    fn process() -> ProcessStats {
        ProcessStats {
            cpu_seconds: 1.5,
            resident_bytes: 4096,
            open_fds: 9,
            max_fds: 1024,
            start_time_seconds: 1_700_000_000.0,
        }
    }

    fn samples(exposition: &str) -> Vec<&str> {
        exposition
            .lines()
            .filter(|line| !line.starts_with('#'))
            .collect()
    }

    /// A fresh registry's scrape with every check passing, process metrics and no stall, for a
    /// driver whose collision count is `collisions`.
    fn scrape(collisions: Option<u64>) -> String {
        Telemetry::default().render(every_check_passes(), Some(&process()), 0, collisions)
    }

    #[test]
    fn the_series_count_is_pinned_at_its_maximum() {
        let telemetry = Telemetry::default();
        let exposition = telemetry.render(every_check_passes(), Some(&process()), 0, None);
        assert_eq!(SERIES_MAXIMUM, 584);
        assert_eq!(samples(&exposition).len(), SERIES_MAXIMUM);
        let refusals = samples(&exposition)
            .iter()
            .filter(|line| line.starts_with("substrate_refusals_total"))
            .count();
        assert_eq!(refusals, REFUSAL_CODES.len() + 1);
        assert!(SERIES_MAXIMUM - refusals < 500);
        // Recording into every family adds no series: every combination is already published.
        telemetry.refusal("an.unnamed-code-with-a-path-/srv/secret");
        telemetry.admission(
            super::Admission::AuthorityUnavailable,
            Some(Duration::from_millis(3)),
        );
        telemetry.maintenance_round(MaintenanceTask::Containment, Duration::from_secs(20));
        let recorded = telemetry.render(every_check_passes(), Some(&process()), 3, None);
        assert_eq!(samples(&recorded).len(), SERIES_MAXIMUM);
        assert!(!recorded.contains("/srv/secret"));
        assert!(recorded.contains("substrate_refusals_total{code=\"other\"} 1\n"));
        assert!(recorded.contains("substrate_output_stalls_total 3\n"));
    }

    /// The metric names, types and label sets are the ADR's catalogue exactly. Changing one
    /// changes this test, the deployment guide and the changelog together.
    #[test]
    fn the_metric_names_and_label_sets_are_the_catalogue() {
        let exposition = scrape(None);
        let families = exposition
            .lines()
            .filter_map(|line| line.strip_prefix("# TYPE "))
            .collect::<Vec<_>>();
        assert_eq!(
            families,
            [
                "substrate_build_info gauge",
                "substrate_ready gauge",
                "substrate_http_requests counter",
                "substrate_http_request_duration_seconds histogram",
                "substrate_refusals counter",
                "substrate_dispatch counter",
                "substrate_dispatch_duration_seconds histogram",
                "substrate_connections_open gauge",
                "substrate_connections_limit gauge",
                "substrate_connections_rejected counter",
                "substrate_output_stalls counter",
                "substrate_maintenance_rounds counter",
                "substrate_maintenance_round_duration_seconds histogram",
                "substrate_maintenance_last_round_timestamp_seconds gauge",
                "substrate_hosted_admissions counter",
                "substrate_hosted_resolution_duration_seconds histogram",
                "process_cpu_seconds counter",
                "process_resident_memory_bytes gauge",
                "process_open_fds gauge",
                "process_max_fds gauge",
                "process_start_time_seconds gauge",
            ]
        );
        let mut label_sets = BTreeSet::new();
        for sample in samples(&exposition) {
            let name = sample.split(['{', ' ']).next().expect("sample name");
            let labels = sample
                .split_once('{')
                .and_then(|(_, rest)| rest.split_once('}'))
                .map(|(labels, _)| {
                    labels
                        .split(',')
                        .map(|pair| pair.split_once('=').expect("label pair").0)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            label_sets.insert(format!("{name}{{{labels}}}"));
        }
        let expected = [
            "process_cpu_seconds_total{}",
            "process_max_fds{}",
            "process_open_fds{}",
            "process_resident_memory_bytes{}",
            "process_start_time_seconds{}",
            "substrate_build_info{version,contract_bundle}",
            "substrate_connections_limit{listener}",
            "substrate_connections_open{listener}",
            "substrate_connections_rejected_total{listener}",
            "substrate_dispatch_duration_seconds_bucket{family,le}",
            "substrate_dispatch_duration_seconds_count{family}",
            "substrate_dispatch_duration_seconds_sum{family}",
            "substrate_dispatch_total{family,outcome}",
            "substrate_hosted_admissions_total{result}",
            "substrate_hosted_resolution_duration_seconds_bucket{le}",
            "substrate_hosted_resolution_duration_seconds_count{}",
            "substrate_hosted_resolution_duration_seconds_sum{}",
            "substrate_http_request_duration_seconds_bucket{listener,family,le}",
            "substrate_http_request_duration_seconds_count{listener,family}",
            "substrate_http_request_duration_seconds_sum{listener,family}",
            "substrate_http_requests_total{listener,family,outcome}",
            "substrate_maintenance_last_round_timestamp_seconds{task}",
            "substrate_maintenance_round_duration_seconds_bucket{task,le}",
            "substrate_maintenance_round_duration_seconds_count{task}",
            "substrate_maintenance_round_duration_seconds_sum{task}",
            "substrate_maintenance_rounds_total{task}",
            "substrate_output_stalls_total{}",
            "substrate_ready{check}",
            "substrate_refusals_total{code}",
        ];
        assert_eq!(
            label_sets.iter().map(String::as_str).collect::<Vec<_>>(),
            expected
        );
        assert!(exposition.ends_with("# EOF\n"));
        // Every histogram has the same eleven bounds and +Inf.
        let bounds = exposition
            .lines()
            .filter(|line| line.starts_with("substrate_hosted_resolution_duration_seconds_bucket"))
            .map(|line| {
                line.split("le=\"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .expect("le")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            bounds,
            [
                "0.005", "0.01", "0.025", "0.05", "0.1", "0.25", "0.5", "1.0", "2.5", "5.0",
                "10.0", "+Inf"
            ]
        );
    }

    /// Design 34 § 4's one addition to the catalogue, on a daemon whose driver answers
    /// `Driver::backend_collisions`: its name, type, help text and empty label set, after the
    /// hosted families and before the process ones. Every other line of the scrape is the host
    /// scrape's, byte for byte, and a host driver's scrape carries no collision line at all.
    #[test]
    fn the_kubernetes_collisions_series_is_the_one_addition_a_namespace_driver_makes() {
        let host = scrape(None);
        let namespace = scrape(Some(0));
        let golden = [
            "# TYPE substrate_kubernetes_collisions counter",
            "# HELP substrate_kubernetes_collisions Objects the Kubernetes driver met under a name \
             it would use and did not create, each left untouched.",
            "substrate_kubernetes_collisions_total 0",
        ];
        let added = namespace
            .lines()
            .filter(|line| !host.lines().any(|kept| kept == *line))
            .collect::<Vec<_>>();
        assert_eq!(added, golden);
        let without = namespace
            .lines()
            .filter(|line| !golden.contains(line))
            .collect::<Vec<_>>();
        assert_eq!(without, host.lines().collect::<Vec<_>>());
        assert!(!host.contains("substrate_kubernetes"), "{host}");
        let families = namespace
            .lines()
            .filter_map(|line| line.strip_prefix("# TYPE "))
            .collect::<Vec<_>>();
        let at = families
            .iter()
            .position(|family| *family == "substrate_kubernetes_collisions counter")
            .expect("the collision family");
        assert_eq!(
            (families[at - 1], families[at + 1]),
            (
                "substrate_hosted_resolution_duration_seconds histogram",
                "process_cpu_seconds counter"
            )
        );
        assert_eq!(samples(&namespace).len(), KUBERNETES_SERIES_MAXIMUM);
        assert_eq!(KUBERNETES_SERIES_MAXIMUM, 585);
    }

    /// The scrape states the driver's count as it is, so one more collision is one more on the
    /// scrape, and nothing else on it moves.
    #[test]
    fn the_collision_sample_moves_by_exactly_what_the_driver_counted() {
        let before = scrape(Some(4));
        let after = scrape(Some(5));
        let moved = before
            .lines()
            .zip(after.lines())
            .filter(|(was, is)| was != is)
            .collect::<Vec<_>>();
        assert_eq!(
            moved,
            [(
                "substrate_kubernetes_collisions_total 4",
                "substrate_kubernetes_collisions_total 5"
            )]
        );
    }

    /// The closed code set is the wire crate's named code constants, no more and no fewer: read
    /// from its source, so a constant added there fails here until it is counted by name.
    #[test]
    fn the_refusal_codes_are_the_wire_constants() {
        let source = include_str!("../../../substrate-wire/src/lib.rs");
        let not_codes = [
            "APERTURE_LOOPBACK_ADDRESS",
            "EXECUTION_CAPSULE_HASH_DOMAIN",
            "SESSION_AUTHORITY_TRANSCRIPT_DOMAIN",
        ];
        let named = source
            .lines()
            .filter_map(|line| {
                let (name, rest) = line.strip_prefix("pub const ")?.split_once(": &str = \"")?;
                let value = rest.strip_suffix("\";")?;
                let dotted = value.contains('.')
                    && value.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'.' | b'-')
                    });
                (dotted && !name.ends_with("_FACT") && !not_codes.contains(&name))
                    .then(|| value.to_owned())
            })
            .collect::<Vec<_>>();
        assert_eq!(named, REFUSAL_CODES.map(str::to_owned).to_vec());
        let distinct = REFUSAL_CODES.iter().collect::<BTreeSet<_>>();
        assert_eq!(
            distinct.len(),
            REFUSAL_CODES.len(),
            "a code is listed twice"
        );
    }

    fn app() -> (tempfile::TempDir, Arc<App>) {
        crate::app::tests::host_app("dep_health")
    }

    fn get(path: &str) -> Request<()> {
        Request::get(path).body(()).expect("request")
    }

    async fn answer(operations: &Operations, path: &str) -> (StatusCode, String) {
        let response = operations.answer(&get(path));
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        (status, String::from_utf8(body.to_vec()).expect("text"))
    }

    fn running() -> Vec<tokio::task::AbortHandle> {
        MaintenanceTask::ALL
            .iter()
            .map(|_| tokio::spawn(std::future::pending::<()>()).abort_handle())
            .collect()
    }

    fn fresh(app: &App) {
        for task in MaintenanceTask::ALL {
            app.telemetry
                .maintenance_round(task, Duration::from_millis(1));
        }
        app.telemetry.start_accepting();
    }

    fn no_stalls() -> u64 {
        0
    }

    /// The startup snapshot of `app`'s driver, taken now, as for a host daemon.
    fn taken(app: &App) -> StartupFacts {
        let startup = StartupFacts::default();
        startup.take(&app.driver.machine().facts);
        startup
    }

    /// D5 A and design 34 § 6.1: until the driver's open sequence has settled no snapshot is
    /// taken, so `driver` fails even when no fact is required. The snapshot taken then is the one
    /// readiness reads, with the facts that arrived before it, and it is taken once: a fact that
    /// arrives later is not seen, and one lost later is not missed (D5 B stays deferred).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn readiness_reads_the_one_snapshot_taken_once_the_open_sequence_settled() {
        let (_directory, app) = app();
        fresh(&app);
        let pending = "store ok\nschema ok\nmaintenance ok\ndriver failed\n";
        let ready = "store ok\nschema ok\nmaintenance ok\ndriver ok\n";
        let required = vec!["events.pull".to_owned(), "exec.argv-only".to_owned()];
        let startup = StartupFacts::default();
        let probed = Operations::new(
            Arc::clone(&app),
            running(),
            required,
            startup.clone(),
            no_stalls,
        );
        let unrequired = Operations::new(
            Arc::clone(&app),
            running(),
            Vec::new(),
            startup.clone(),
            no_stalls,
        );
        for operations in [&probed, &unrequired] {
            assert_eq!(
                answer(operations, "/readyz").await,
                (StatusCode::SERVICE_UNAVAILABLE, pending.to_owned())
            );
        }

        // The open settled after its probe proved `exec.argv-only`.
        startup.take(&substrate_wire::CapabilityFacts {
            events_pull: Some(true),
            exec_argv_only: Some(true),
            ..substrate_wire::CapabilityFacts::default()
        });
        for operations in [&probed, &unrequired] {
            assert_eq!(
                answer(operations, "/readyz").await,
                (StatusCode::OK, ready.to_owned())
            );
        }

        // Taken once: a later snapshot without the fact changes nothing.
        startup.take(&substrate_wire::CapabilityFacts::default());
        assert_eq!(answer(&probed, "/readyz").await.0, StatusCode::OK);

        // An open that failed: the snapshot lacks the probed fact, and stays without it.
        let failed = StartupFacts::default();
        failed.take(&substrate_wire::CapabilityFacts {
            events_pull: Some(true),
            ..substrate_wire::CapabilityFacts::default()
        });
        let unready = Operations::new(
            Arc::clone(&app),
            running(),
            vec!["exec.argv-only".to_owned()],
            failed.clone(),
            no_stalls,
        );
        failed.take(&substrate_wire::CapabilityFacts {
            exec_argv_only: Some(true),
            ..substrate_wire::CapabilityFacts::default()
        });
        assert_eq!(
            answer(&unready, "/readyz").await,
            (StatusCode::SERVICE_UNAVAILABLE, pending.to_owned())
        );
    }

    /// Readiness reads the store, its schema version, both maintenance tasks and the required
    /// facts, and turns `503` naming the one that failed; liveness turns only when a maintenance
    /// task has ended.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn readiness_names_each_failed_check_and_liveness_only_a_stopped_task() {
        let (_directory, app) = app();
        fresh(&app);
        let ready = Operations::new(
            Arc::clone(&app),
            running(),
            Vec::new(),
            taken(&app),
            no_stalls,
        );
        assert_eq!(
            answer(&ready, "/readyz").await,
            (
                StatusCode::OK,
                "store ok\nschema ok\nmaintenance ok\ndriver ok\n".to_owned()
            )
        );
        assert_eq!(
            answer(&ready, "/livez").await,
            (StatusCode::OK, "ok\n".to_owned())
        );

        // A required fact the startup snapshot does not publish.
        let unmet = Operations::new(
            Arc::clone(&app),
            running(),
            vec!["exec.never-published".to_owned(), "events.pull".to_owned()],
            taken(&app),
            no_stalls,
        );
        assert_eq!(
            answer(&unmet, "/readyz").await,
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "store ok\nschema ok\nmaintenance ok\ndriver failed\n".to_owned()
            )
        );
        let met = Operations::new(
            Arc::clone(&app),
            running(),
            vec!["events.pull".to_owned()],
            taken(&app),
            no_stalls,
        );
        assert_eq!(answer(&met, "/readyz").await.0, StatusCode::OK);

        // A stopped maintenance task: liveness fails, and so does readiness.
        let stopped = running();
        stopped[1].abort();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !stopped[1].is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("aborted task ends");
        let halted = Operations::new(
            Arc::clone(&app),
            stopped,
            Vec::new(),
            taken(&app),
            no_stalls,
        );
        assert_eq!(
            answer(&halted, "/livez").await,
            (StatusCode::SERVICE_UNAVAILABLE, "failed\n".to_owned())
        );
        assert_eq!(
            answer(&halted, "/readyz").await.1,
            "store ok\nschema ok\nmaintenance failed\ndriver ok\n"
        );

        // Shutdown has begun: not ready, whatever the checks say.
        app.telemetry.stop_accepting();
        assert_eq!(
            answer(&ready, "/readyz").await,
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "store ok\nschema ok\nmaintenance ok\ndriver ok\n".to_owned()
            )
        );
    }

    /// Each way the store can fail fails `store` or `schema` and nothing else.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn readiness_names_each_way_the_store_fails() {
        let (_directory, app) = app();
        fresh(&app);
        // An unreadable store: the statement fails, so neither the store nor its schema passes.
        let failing: StoreProbe = Arc::new(|_| Err(()));
        let unreadable = Operations::with_store_probe(
            Arc::clone(&app),
            running(),
            Vec::new(),
            taken(&app),
            failing,
            no_stalls,
        );
        assert_eq!(
            answer(&unreadable, "/readyz").await,
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "store failed\nschema failed\nmaintenance ok\ndriver ok\n".to_owned()
            )
        );
        assert_eq!(answer(&unreadable, "/livez").await.0, StatusCode::OK);

        // The ledger connection not free within the deadline, and a statement that answers only
        // after it: both fail the store, and the wait is the deadline the probe was given.
        let busy: StoreProbe = Arc::new(|wait| {
            assert_eq!(wait, Duration::from_secs(1));
            Ok(None)
        });
        let late: StoreProbe = Arc::new(|_| {
            std::thread::sleep(Duration::from_millis(1_100));
            Ok(Some(substrate_store::SCHEMA_VERSION))
        });
        for probe in [busy, late] {
            let slow = Operations::with_store_probe(
                Arc::clone(&app),
                running(),
                Vec::new(),
                taken(&app),
                probe,
                no_stalls,
            );
            assert_eq!(
                answer(&slow, "/readyz").await,
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "store failed\nschema failed\nmaintenance ok\ndriver ok\n".to_owned()
                )
            );
        }

        // A database another binary stamped: the store answers, the schema does not match.
        let foreign: StoreProbe = Arc::new(|_| Ok(Some(999)));
        let migrated = Operations::with_store_probe(
            Arc::clone(&app),
            running(),
            Vec::new(),
            taken(&app),
            foreign,
            no_stalls,
        );
        assert_eq!(
            answer(&migrated, "/readyz").await.1,
            "store ok\nschema failed\nmaintenance ok\ndriver ok\n"
        );
    }

    /// A maintenance task that has not completed a round within five seconds is not ready.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_maintenance_task_without_a_recent_round_is_not_ready() {
        let (_directory, app) = app();
        app.telemetry.start_accepting();
        app.telemetry
            .maintenance_round(MaintenanceTask::LeaseSweep, Duration::from_millis(1));
        let operations = Operations::new(
            Arc::clone(&app),
            running(),
            Vec::new(),
            taken(&app),
            no_stalls,
        );
        assert_eq!(
            answer(&operations, "/readyz").await.1,
            "store ok\nschema ok\nmaintenance failed\ndriver ok\n"
        );
        *app.telemetry.maintenance[MaintenanceTask::Containment as usize]
            .last
            .lock() = Some((
            std::time::Instant::now()
                .checked_sub(Duration::from_secs(6))
                .expect("six seconds ago"),
            0.0,
        ));
        assert_eq!(
            answer(&operations, "/readyz").await.1,
            "store ok\nschema ok\nmaintenance failed\ndriver ok\n"
        );
        app.telemetry
            .maintenance_round(MaintenanceTask::Containment, Duration::from_millis(1));
        assert_eq!(answer(&operations, "/readyz").await.0, StatusCode::OK);
    }

    #[test]
    fn a_failure_answer_counts_its_class_and_a_named_code() {
        let telemetry = Telemetry::default();
        let mut response = axum::response::Response::new(axum::body::Body::empty());
        *response.status_mut() = StatusCode::NOT_IMPLEMENTED;
        response.extensions_mut().insert(ErrorDetail {
            class: ErrorClass::Unserved,
            code: substrate_wire::EXEC_WORKDIR_UNSERVED.to_owned(),
            message: "unserved".to_owned(),
            retriable: false,
            address: None,
            operation: None,
        });
        assert_eq!(
            super::Outcome::of(&response),
            super::Outcome::Failure(ErrorClass::Unserved)
        );
        telemetry.refusal(substrate_wire::EXEC_WORKDIR_UNSERVED);
        let exposition = telemetry.render(every_check_passes(), None, 0, None);
        assert!(
            exposition.contains("substrate_refusals_total{code=\"exec.workdir-unserved\"} 1\n")
        );
        assert!(!exposition.contains("process_"));
        let check_order = Check::ALL.map(Check::label);
        assert_eq!(check_order, ["store", "schema", "maintenance", "driver"]);
    }

    /// A failure answer without a named code is counted once, under `other`, and adds no series.
    #[test]
    fn a_failure_answer_without_a_code_is_counted_as_other() {
        let telemetry = Telemetry::default();
        telemetry.unnamed_refusal();
        let exposition = telemetry.render(every_check_passes(), Some(&process()), 0, None);
        assert!(exposition.contains("substrate_refusals_total{code=\"other\"} 1\n"));
        assert_eq!(samples(&exposition).len(), SERIES_MAXIMUM);
    }

    /// The fact names `--ops-require-fact` accepts are exactly `CapabilityFacts`' keys: every
    /// key a host driver publishes is one, and a misspelling is not.
    #[test]
    fn the_capability_fact_names_are_the_closed_wire_set() {
        let names = super::capability_fact_names();
        assert!(names.len() >= 40, "{names:?}");
        for name in [
            "exec.no-egress",
            "process.host",
            "operation.ledger-subject-max-rows",
        ] {
            assert!(super::is_capability_fact_name(name), "{name}");
        }
        for name in ["exec.no-egres", "", "exec_no_egress", "Exec.no-egress"] {
            assert!(!super::is_capability_fact_name(name), "{name:?}");
        }
        let (_root, app) = app();
        for published in super::present_facts(&app.driver.machine().facts) {
            assert!(super::is_capability_fact_name(&published), "{published}");
        }
    }

    /// `+Inf` and `_count` are derived from the buckets a render read, overflow included, so a
    /// render cannot publish a finite bucket above them.
    #[test]
    fn a_histogram_count_is_its_buckets_plus_overflow() {
        let telemetry = Telemetry::default();
        telemetry.maintenance_round(MaintenanceTask::Containment, Duration::from_millis(1));
        telemetry.maintenance_round(MaintenanceTask::Containment, Duration::from_secs(20));
        let exposition = telemetry.render(every_check_passes(), None, 0, None);
        for expected in [
            "substrate_maintenance_round_duration_seconds_bucket{task=\"containment\",le=\"10.0\"} 1\n",
            "substrate_maintenance_round_duration_seconds_bucket{task=\"containment\",le=\"+Inf\"} 2\n",
            "substrate_maintenance_round_duration_seconds_count{task=\"containment\"} 2\n",
        ] {
            assert!(exposition.contains(expected), "{expected}{exposition}");
        }
    }
}
